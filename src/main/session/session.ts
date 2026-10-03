/**
 * The dictation session: hotkey events in, text at the cursor out.
 *
 *   Idle → Recording → Transcribing → Inserting → Idle
 *
 * A new recording can start while earlier ones are still transcribing; results
 * are inserted in the order they were spoken. Every failure ends in typed text,
 * clipboard text or a visible error — a dictation is never silently lost.
 */
import { EventEmitter } from 'node:events';
import type { Catalog } from '@shared/catalog';
import type { Config } from '@shared/config';
import type {
  CaptureErrorReason,
  CaptureStart,
  DictationEvent,
  FocusedApp,
  HistoryEntry,
  InsertResult,
  OverlayState,
  SessionState,
  Transcript,
} from '@shared/types';
import { SAMPLE_RATE } from '@shared/types';
import { applyRules, dictionaryHints } from '../cleanup';
import { cleanupWithLlm } from '../cleanup/llm';
import type { HotkeyEvent } from '../input/matcher';
import type { InsertOptions } from '../insert/inserter';
import { log } from '../log';
import { resolveProfile } from '../profiles/resolve';
import type { EngineClient } from '../transcribe/engine-client';
import type { RouteResult, TranscriberRouter } from '../transcribe/router';
import { toTranscribeError } from '../transcribe/router';

/** Recordings shorter than this are accidental taps. */
export const MIN_RECORDING_MS = 300;
/** How long to wait for the overlay to flush its last audio chunk. */
const FLUSH_TIMEOUT_MS = 300;

const FLASH_MS = { done: 900, notice: 2600, error: 4200 } as const;

export interface SessionDeps {
  getConfig(): Config;
  getCatalog(): Catalog;
  getSecret(id: string): string | undefined;
  getFocusedApp(): FocusedApp | null;
  insert(text: string, options: InsertOptions): Promise<InsertResult>;
  copy(text: string): void;
  router: TranscriberRouter;
  engine: Pick<EngineClient, 'trim'>;
  overlay: {
    startCapture(options: CaptureStart): void;
    stopCapture(): void;
    setState(state: OverlayState): void;
  };
  /** `cancel` (Esc) is only listened for while there is something to cancel. */
  setCancelArmed(armed: boolean): void;
  /** Null when history is unavailable (the native module failed to load). */
  addHistory(entry: Omit<HistoryEntry, 'id'>): void;
  now?: () => number;
}

interface Recording {
  id: number;
  mode: 'hold' | 'locked';
  chunks: Float32Array[];
  samples: number;
  /** Effective config, with the focused app's profile applied at key-down. */
  config: Config;
  /** Audio is flowing from the microphone. */
  started: boolean;
  cancelled: boolean;
  maxTimer: NodeJS.Timeout | null;
}

interface Job {
  id: number;
  abort: AbortController;
  cloud: boolean;
}

interface LastDictation {
  audio: Float32Array;
  config: Config;
  target: FocusedApp | null;
  text: string | null;
}

type Outcome =
  | { kind: 'text'; text: string; rawText: string; route: RouteResult }
  | { kind: 'empty' }
  | { kind: 'cancelled' }
  | { kind: 'error'; message: string };

function concat(chunks: Float32Array[], total: number): Float32Array {
  const out = new Float32Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

export class DictationSession extends EventEmitter {
  private recording: Recording | null = null;
  private flushing: { rec: Recording; finish(): void } | null = null;
  private readonly jobs = new Set<Job>();
  private insertChain: Promise<void> = Promise.resolve();
  private last: LastDictation | null = null;
  private swallowToggle = false;
  private paused = false;
  private nextId = 1;
  private flash: OverlayState | null = null;
  private flashTimer: NodeJS.Timeout | null = null;
  private inserting = 0;
  private readonly now: () => number;

  constructor(private readonly deps: SessionDeps) {
    super();
    this.now = deps.now ?? (() => performance.now());
  }

  get state(): SessionState {
    if (this.recording) return 'recording';
    if (this.inserting > 0) return 'inserting';
    if (this.jobs.size > 0 || this.flushing) return 'transcribing';
    return 'idle';
  }

  get isPaused(): boolean {
    return this.paused;
  }

  get lastText(): string | null {
    return this.last?.text ?? null;
  }

  get canRetry(): boolean {
    return this.last !== null;
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    if (paused) this.cancel();
    this.emit('paused', paused);
  }

  // ── Hotkey events ────────────────────────────────────────────────────────

  handleHotkey(event: HotkeyEvent): void {
    if (this.paused && event.type !== 'cancel') return;
    switch (event.type) {
      case 'hold-start':
        if (this.recording?.mode === 'locked') {
          // Hands-free recording ends on the next tap of the hold key. The same
          // chord may go on to complete the toggle binding; that must not restart.
          this.swallowToggle = true;
          this.stop();
        } else if (!this.recording) {
          this.start('hold');
        }
        break;
      case 'hold-end':
        this.swallowToggle = false;
        if (this.recording?.mode === 'hold') {
          if (event.aborted) this.discardRecording();
          else this.stop();
        }
        break;
      case 'lock':
        if (!this.recording) this.start('locked');
        break;
      case 'toggle':
        if (this.swallowToggle) break;
        if (!this.recording) this.start('locked');
        else if (this.recording.mode === 'hold') {
          this.recording.mode = 'locked';
          this.refresh();
        } else this.stop();
        break;
      case 'cancel':
        this.cancel();
        break;
      case 'paste-last':
        void this.pasteLast();
        break;
    }
  }

  // ── Recording ────────────────────────────────────────────────────────────

  private start(mode: 'hold' | 'locked'): void {
    // A previous recording still waiting on its last chunk is finalised now.
    this.flushing?.finish();
    const app = this.deps.getFocusedApp();
    const resolved = resolveProfile(this.deps.getConfig(), app);
    if (resolved.disabled) {
      log.info('session', `dictation is disabled by profile "${resolved.profile}"`);
      return;
    }
    const config = resolved.config;
    const rec: Recording = {
      id: this.nextId++,
      mode,
      chunks: [],
      samples: 0,
      config,
      started: false,
      cancelled: false,
      maxTimer: null,
    };
    rec.maxTimer = setTimeout(() => {
      if (this.recording === rec) this.stop();
    }, config.audio.max_recording_seconds * 1000);
    this.recording = rec;
    this.clearFlash();
    this.deps.overlay.startCapture({
      deviceId: config.audio.input_device,
      keepWarm: config.audio.keep_mic_warm,
    });
    // The local model loads while the user is speaking.
    this.deps.router.warmUp(config);
    this.refresh();
  }

  private stop(): void {
    const rec = this.recording;
    if (!rec) return;
    this.recording = null;
    if (rec.maxTimer) clearTimeout(rec.maxTimer);
    const releasedAt = this.now();
    // The window focused at key-up receives the text, even if focus moves later.
    const target = this.deps.getFocusedApp();
    this.deps.overlay.stopCapture();
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      if (this.flushing?.rec === rec) this.flushing = null;
      this.finalize(rec, target, releasedAt);
    };
    const timer = setTimeout(finish, FLUSH_TIMEOUT_MS);
    this.flushing = { rec, finish };
    this.refresh();
  }

  private discardRecording(): void {
    const rec = this.recording;
    if (!rec) return;
    this.recording = null;
    rec.cancelled = true;
    if (rec.maxTimer) clearTimeout(rec.maxTimer);
    this.deps.overlay.stopCapture();
    this.refresh();
  }

  /** Esc: drops the recording and any dictation still being transcribed. Types nothing. */
  cancel(): void {
    this.discardRecording();
    if (this.flushing) {
      this.flushing.rec.cancelled = true;
      this.flushing.finish();
    }
    for (const job of this.jobs) job.abort.abort();
    this.clearFlash();
    this.refresh();
  }

  // ── Capture callbacks from the overlay window ────────────────────────────

  onCaptureStarted(): void {
    if (!this.recording) return;
    this.recording.started = true;
    this.refresh();
  }

  onCaptureChunk(pcm: Float32Array): void {
    const rec = this.flushing?.rec ?? this.recording;
    if (!rec) return;
    rec.chunks.push(pcm);
    rec.samples += pcm.length;
  }

  onCaptureStopped(): void {
    this.flushing?.finish();
  }

  onCaptureError(reason: CaptureErrorReason, message?: string): void {
    const rec = this.recording;
    if (!rec) return;
    this.recording = null;
    rec.cancelled = true;
    if (rec.maxTimer) clearTimeout(rec.maxTimer);
    const text =
      message ??
      {
        'device-missing': 'Microphone not found',
        permission: 'Microphone access is blocked',
        busy: 'Microphone is in use by another app',
        unknown: 'Microphone error',
      }[reason];
    log.error('session', `capture failed: ${reason}`);
    this.showFlash({ phase: 'error', cloud: false, message: text });
    this.emitEvent({ id: rec.id, ok: false, error: text });
  }

  // ── Transcribe, clean up, insert ─────────────────────────────────────────

  private finalize(rec: Recording, target: FocusedApp | null, releasedAt: number): void {
    if (rec.cancelled) {
      this.refresh();
      return;
    }
    const audioMs = (rec.samples / SAMPLE_RATE) * 1000;
    if (audioMs < MIN_RECORDING_MS) {
      this.refresh();
      return;
    }
    const audio = concat(rec.chunks, rec.samples);
    rec.chunks = [];
    // Kept in memory (never on disk) until the next dictation, for "retry last".
    this.last = { audio, config: rec.config, target, text: null };
    this.enqueue(rec.id, audio, rec.config, target, releasedAt);
  }

  private enqueue(
    id: number,
    audio: Float32Array,
    config: Config,
    target: FocusedApp | null,
    releasedAt: number,
  ): void {
    const job: Job = {
      id,
      abort: new AbortController(),
      cloud: this.deps.router.isCloud(config.model.active),
    };
    this.jobs.add(job);
    this.refresh();
    const work = this.process(job, audio, config);
    const previous = this.insertChain;
    this.insertChain = (async () => {
      const outcome = await work;
      await previous;
      await this.deliver(job, outcome, config, target, audio, releasedAt);
    })().catch((err) => {
      log.error('session', `delivery failed: ${String(err)}`);
      this.jobs.delete(job);
      this.refresh();
    });
  }

  private async trim(audio: Float32Array): Promise<Float32Array | null> {
    try {
      const result = await this.deps.engine.trim(audio);
      return result.audio;
    } catch (err) {
      // Voice detection is an optimisation; without it the raw audio is used.
      log.warn('session', `voice detection failed: ${String(err)}`);
      return audio;
    }
  }

  private async process(job: Job, audio: Float32Array, config: Config): Promise<Outcome> {
    const signal = job.abort.signal;
    try {
      const speech = await this.trim(audio);
      if (signal.aborted) return { kind: 'cancelled' };
      // No speech: nothing is sent or typed. This also prevents phantom text on silence.
      if (!speech || speech.length < SAMPLE_RATE * 0.1) return { kind: 'empty' };
      const route = await this.deps.router.transcribe(speech, config, {
        language: config.general.language,
        hints: dictionaryHints(config),
        signal,
      });
      if (signal.aborted) return { kind: 'cancelled' };
      const rawText = route.transcript.text;
      if (!rawText.trim()) return { kind: 'empty' };
      const text = await this.cleanUp(rawText, config, signal);
      if (signal.aborted) return { kind: 'cancelled' };
      if (!text.trim()) return { kind: 'empty' };
      return { kind: 'text', text, rawText, route };
    } catch (err) {
      const failure = toTranscribeError(err);
      if (failure.code === 'aborted' || signal.aborted) return { kind: 'cancelled' };
      log.error('session', `transcription failed: ${failure.code} ${failure.message}`);
      return { kind: 'error', message: failure.userMessage };
    }
  }

  /** Layer 1 rules, then the optional LLM pass. The LLM can never lose a dictation. */
  async cleanUp(rawText: string, config: Config, signal?: AbortSignal): Promise<string> {
    const ruled = applyRules(rawText, config);
    if (!config.cleanup.llm_enabled || !ruled.trim()) return ruled;
    try {
      const rewritten = await cleanupWithLlm(
        ruled.trim(),
        { catalog: this.deps.getCatalog(), config, getSecret: this.deps.getSecret },
        signal,
      );
      const trailing = config.insert.trailing_space && !rewritten.endsWith('\n') ? ' ' : '';
      return rewritten.trim() + trailing;
    } catch (err) {
      if (!signal?.aborted) log.warn('session', `LLM cleanup skipped: ${String(err)}`);
      return ruled;
    }
  }

  private async deliver(
    job: Job,
    outcome: Outcome,
    config: Config,
    target: FocusedApp | null,
    audio: Float32Array,
    releasedAt: number,
  ): Promise<void> {
    const audioMs = (audio.length / SAMPLE_RATE) * 1000;
    const finish = (flash?: OverlayState) => {
      this.jobs.delete(job);
      if (flash) this.showFlash(flash);
      else this.refresh();
    };

    if (outcome.kind === 'cancelled' || job.abort.signal.aborted) return finish();
    if (outcome.kind === 'empty') return finish();
    if (outcome.kind === 'error') {
      this.emitEvent({ id: job.id, ok: false, error: outcome.message, audioMs, cloud: job.cloud });
      return finish({ phase: 'error', cloud: false, message: outcome.message });
    }

    const { text, rawText, route } = outcome;
    this.inserting++;
    let result: InsertResult;
    try {
      result = await this.deps.insert(text, {
        method: config.insert.method,
        pasteShortcut: config.insert.paste_shortcut,
        restoreClipboard: config.insert.restore_clipboard,
        target,
      });
    } finally {
      this.inserting--;
    }
    const elapsedMs = this.now() - releasedAt;
    if (this.last?.audio === audio) this.last.text = text;
    log.info(
      'session',
      `dictation ${job.id}: ${Math.round(audioMs)} ms audio, ${route.transcript.model}, ` +
        `${Math.round(elapsedMs)} ms to ${result.outcome}`,
    );
    log.debug('session', `text: ${text}`);

    if (config.history.enabled) {
      try {
        this.deps.addHistory({
          createdAt: Date.now(),
          text,
          rawText,
          model: route.transcript.model,
          app: target?.processName ?? '',
          audioMs: Math.round(audioMs),
          elapsedMs: Math.round(elapsedMs),
        });
      } catch (err) {
        log.warn('session', `history write failed: ${String(err)}`);
      }
    }

    this.emitEvent({
      id: job.id,
      ok: true,
      text,
      rawText,
      model: route.transcript.model,
      cloud: route.cloud,
      audioMs,
      elapsedMs,
      outcome: result.outcome,
      fellBack: route.fellBack,
    });

    if (result.outcome === 'clipboard') {
      const why = {
        'no-target': 'No text field focused',
        'target-closed': 'The window was closed',
        elevated: 'Cannot type into an administrator window',
        failed: 'Could not insert the text',
      }[result.reason ?? 'failed'];
      return finish({ phase: 'notice', cloud: false, message: `${why} — copied to clipboard` });
    }
    if (route.fellBack) {
      return finish({
        phase: 'notice',
        cloud: false,
        message: `${route.fallbackReason ?? 'Cloud model failed'} — used the local model`,
      });
    }
    return finish({ phase: 'done', cloud: route.cloud });
  }

  // ── Tray and settings actions ────────────────────────────────────────────

  /** Re-inserts the previous transcript at the cursor. */
  async pasteLast(): Promise<void> {
    const text = this.last?.text;
    if (!text) return;
    const config = resolveProfile(this.deps.getConfig(), this.deps.getFocusedApp()).config;
    await this.deps.insert(text, {
      method: config.insert.method,
      pasteShortcut: config.insert.paste_shortcut,
      restoreClipboard: config.insert.restore_clipboard,
      target: this.deps.getFocusedApp(),
    });
  }

  copyLast(): boolean {
    const text = this.last?.text;
    if (!text) return false;
    this.deps.copy(text);
    return true;
  }

  /** Re-runs the last recording, with the current active model. */
  retryLast(): boolean {
    const last = this.last;
    if (!last) return false;
    const config = { ...last.config, model: this.deps.getConfig().model };
    this.enqueue(this.nextId++, last.audio, config, last.target, this.now());
    return true;
  }

  /** Transcribes audio recorded in the settings window; nothing is inserted. */
  async test(audio: Float32Array): Promise<Transcript> {
    const config = this.deps.getConfig();
    const speech = await this.trim(audio);
    const audioMs = (audio.length / SAMPLE_RATE) * 1000;
    if (!speech) return { text: '', model: config.model.active, audioMs, elapsedMs: 0 };
    const started = this.now();
    const route = await this.deps.router.transcribe(speech, config, {
      language: config.general.language,
      hints: dictionaryHints(config),
      signal: new AbortController().signal,
    });
    const text = await this.cleanUp(route.transcript.text, config);
    return { text, model: route.transcript.model, audioMs, elapsedMs: this.now() - started };
  }

  // ── Overlay ──────────────────────────────────────────────────────────────

  private emitEvent(event: Partial<DictationEvent> & { id: number; ok: boolean }): void {
    const full: DictationEvent = {
      text: '',
      rawText: '',
      model: '',
      cloud: false,
      audioMs: 0,
      elapsedMs: 0,
      ...event,
    };
    this.emit('dictation', full);
  }

  private clearFlash(): void {
    if (this.flashTimer) clearTimeout(this.flashTimer);
    this.flashTimer = null;
    this.flash = null;
  }

  private showFlash(state: OverlayState): void {
    this.clearFlash();
    this.flash = state;
    const ms = FLASH_MS[state.phase as keyof typeof FLASH_MS] ?? FLASH_MS.done;
    this.flashTimer = setTimeout(() => {
      this.flash = null;
      this.flashTimer = null;
      this.refresh();
    }, ms);
    this.refresh();
  }

  /** Recomputes what the overlay shows from the current recording, jobs and flash message. */
  private refresh(): void {
    this.deps.setCancelArmed(
      this.recording !== null || this.flushing !== null || this.jobs.size > 0,
    );
    this.emit('state', this.state);
    const rec = this.recording;
    if (rec?.started) {
      this.deps.overlay.setState({
        phase: 'listening',
        cloud: this.deps.router.isCloud(rec.config.model.active),
        locked: rec.mode === 'locked',
        queued: this.jobs.size,
      });
      return;
    }
    // A tap too short to keep must not flash the pill while its audio is flushed.
    const flushingReal =
      this.flushing !== null &&
      !this.flushing.rec.cancelled &&
      this.flushing.rec.samples >= (MIN_RECORDING_MS / 1000) * SAMPLE_RATE;
    if (this.jobs.size > 0 || flushingReal) {
      const cloud =
        [...this.jobs].some((j) => j.cloud) ||
        (flushingReal && this.deps.router.isCloud(this.flushing!.rec.config.model.active));
      this.deps.overlay.setState({ phase: 'transcribing', cloud, queued: this.jobs.size });
      return;
    }
    // The pill appears only once audio is flowing, so its appearance means "speak now".
    if (rec) return;
    this.deps.overlay.setState(this.flash ?? { phase: 'hidden', cloud: false });
  }
}
