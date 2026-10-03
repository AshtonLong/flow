import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_CONFIG, type Config } from '@shared/config';
import type { DictationEvent, FocusedApp, InsertResult, OverlayState } from '@shared/types';
import type { InsertOptions } from '../../../src/main/insert/inserter';
import { DictationSession, type SessionDeps } from '../../../src/main/session/session';
import { TranscribeError } from '../../../src/main/transcribe/errors';
import type { RouteResult } from '../../../src/main/transcribe/router';

const NOTEPAD: FocusedApp = { windowId: '100', processName: 'notepad.exe', title: 'Notes', pid: 1 };
const SLACK: FocusedApp = { windowId: '200', processName: 'slack.exe', title: 'Slack', pid: 2 };

function seconds(n: number): Float32Array {
  return new Float32Array(Math.round(16000 * n)).fill(0.1);
}

interface Harness {
  session: DictationSession;
  states: OverlayState[];
  inserted: Array<{ text: string; options: InsertOptions }>;
  events: DictationEvent[];
  captures: string[];
  copied: string[];
  armed: boolean[];
  focused: { app: FocusedApp | null };
  config: { value: Config };
  transcribe: ReturnType<typeof vi.fn>;
  insertResult: { value: InsertResult };
  trim: ReturnType<typeof vi.fn>;
  /** Hold the key, speak for `sec` seconds, release. */
  dictate(sec?: number): void;
}

function route(text: string, extra: Partial<RouteResult> = {}): RouteResult {
  return {
    transcript: { text, model: 'parakeet-tdt-0.6b-v2', audioMs: 1000, elapsedMs: 5 },
    cloud: false,
    fellBack: false,
    ...extra,
  };
}

function harness(): Harness {
  const states: OverlayState[] = [];
  const inserted: Harness['inserted'] = [];
  const events: DictationEvent[] = [];
  const captures: string[] = [];
  const copied: string[] = [];
  const armed: boolean[] = [];
  const focused = { app: NOTEPAD as FocusedApp | null };
  const config = { value: structuredClone(DEFAULT_CONFIG) };
  const insertResult = { value: { outcome: 'pasted' } as InsertResult };
  const transcribe = vi.fn(async () => route('Hello world'));
  const trim = vi.fn(async (audio: Float32Array) => ({
    audio,
    speechMs: 1000,
    detector: 'silero' as const,
  }));

  const deps: SessionDeps = {
    getConfig: () => config.value,
    getCatalog: () => ({
      version: 1,
      asOf: '2026-10-03',
      providers: [],
      local: [],
      cloud: [],
      cleanup: [],
    }),
    getSecret: () => undefined,
    getFocusedApp: () => focused.app,
    insert: async (text, options) => {
      inserted.push({ text, options });
      return insertResult.value;
    },
    copy: (text) => copied.push(text),
    router: {
      isCloud: (id: string) => id.includes('/'),
      warmUp: vi.fn(),
      transcribe,
    } as unknown as SessionDeps['router'],
    engine: { trim } as unknown as SessionDeps['engine'],
    overlay: {
      startCapture: () => captures.push('start'),
      stopCapture: () => captures.push('stop'),
      setState: (state) => states.push(state),
    },
    setCancelArmed: (value) => armed.push(value),
    addHistory: vi.fn(),
  };
  const session = new DictationSession(deps);
  session.on('dictation', (event: DictationEvent) => events.push(event));

  return {
    session,
    states,
    inserted,
    events,
    captures,
    copied,
    armed,
    focused,
    config,
    transcribe,
    insertResult,
    trim,
    dictate(sec = 1) {
      session.handleHotkey({ type: 'hold-start' });
      session.onCaptureStarted();
      session.onCaptureChunk(seconds(sec));
      session.handleHotkey({ type: 'hold-end', aborted: false });
      session.onCaptureStopped();
    },
  };
}

/** Overlay phases in order, with consecutive repeats collapsed (the real window de-duplicates). */
const phases = (h: Harness) =>
  h.states.map((s) => s.phase).filter((phase, i, all) => i === 0 || all[i - 1] !== phase);

describe('DictationSession', () => {
  let h: Harness;
  beforeEach(() => {
    h = harness();
  });

  it('types the transcript at the cursor after hold and release', async () => {
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.inserted[0]!.text).toBe('Hello world ');
    expect(h.inserted[0]!.options).toMatchObject({ method: 'paste', target: NOTEPAD });
    await vi.waitFor(() => expect(phases(h)).toContain('done'));
    expect(
      phases(h)
        .filter((p) => p !== 'hidden')
        .slice(0, 3),
    ).toEqual(['listening', 'transcribing', 'done']);
    expect(h.events[0]).toMatchObject({ ok: true, text: 'Hello world ', outcome: 'pasted' });
    expect(h.session.lastText).toBe('Hello world ');
  });

  it('shows the pill only once audio is flowing', () => {
    h.session.handleHotkey({ type: 'hold-start' });
    expect(phases(h)).not.toContain('listening');
    h.session.onCaptureStarted();
    expect(phases(h).at(-1)).toBe('listening');
  });

  it('discards recordings under 300 ms as accidental taps', async () => {
    h.dictate(0.2);
    await new Promise((r) => setTimeout(r, 20));
    expect(h.transcribe).not.toHaveBeenCalled();
    expect(h.inserted).toEqual([]);
    expect(phases(h)).not.toContain('transcribing');
  });

  it('types nothing when no speech is detected', async () => {
    h.trim.mockResolvedValueOnce({ audio: null, speechMs: 0, detector: 'silero' });
    h.dictate();
    await vi.waitFor(() => expect(phases(h).at(-1)).toBe('hidden'));
    expect(h.transcribe).not.toHaveBeenCalled();
    expect(h.inserted).toEqual([]);
  });

  it('still transcribes when voice detection fails', async () => {
    h.trim.mockRejectedValueOnce(new Error('engine down'));
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
  });

  it('cancels a recording with Esc and types nothing', async () => {
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureStarted();
    h.session.onCaptureChunk(seconds(1));
    h.session.handleHotkey({ type: 'cancel' });
    h.session.handleHotkey({ type: 'hold-end', aborted: false });
    await new Promise((r) => setTimeout(r, 20));
    expect(h.inserted).toEqual([]);
    expect(h.transcribe).not.toHaveBeenCalled();
    expect(h.session.state).toBe('idle');
  });

  it('cancels during transcription and types nothing', async () => {
    let release!: (value: RouteResult) => void;
    h.transcribe.mockImplementationOnce(
      (_audio: Float32Array, _config: Config, opts: { signal: AbortSignal }) =>
        new Promise<RouteResult>((resolve, reject) => {
          release = resolve;
          opts.signal.addEventListener('abort', () =>
            reject(new TranscribeError('aborted', 'Cancelled')),
          );
        }),
    );
    h.dictate();
    await vi.waitFor(() => expect(h.transcribe).toHaveBeenCalled());
    expect(h.session.state).toBe('transcribing');
    h.session.handleHotkey({ type: 'cancel' });
    release(route('too late'));
    await vi.waitFor(() => expect(h.session.state).toBe('idle'));
    expect(h.inserted).toEqual([]);
  });

  it('inserts back-to-back dictations in the order they were spoken', async () => {
    let finishFirst!: (value: RouteResult) => void;
    h.transcribe
      .mockImplementationOnce(() => new Promise<RouteResult>((r) => (finishFirst = r)))
      .mockImplementationOnce(async () => route('Second'));
    h.dictate();
    await vi.waitFor(() => expect(h.transcribe).toHaveBeenCalledTimes(1));
    h.dictate();
    await vi.waitFor(() => expect(h.transcribe).toHaveBeenCalledTimes(2));
    await new Promise((r) => setTimeout(r, 20));
    expect(h.inserted).toEqual([]);
    finishFirst(route('First'));
    await vi.waitFor(() => expect(h.inserted).toHaveLength(2));
    expect(h.inserted.map((i) => i.text.trim())).toEqual(['First', 'Second']);
  });

  it('targets the window focused at key-up', async () => {
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureStarted();
    h.session.onCaptureChunk(seconds(1));
    h.focused.app = SLACK;
    h.session.handleHotkey({ type: 'hold-end', aborted: false });
    h.focused.app = NOTEPAD;
    h.session.onCaptureStopped();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.inserted[0]!.options.target).toEqual(SLACK);
  });

  it('drops the recording when a foreign key aborts the hold', async () => {
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureStarted();
    h.session.onCaptureChunk(seconds(1));
    h.session.handleHotkey({ type: 'hold-end', aborted: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(h.transcribe).not.toHaveBeenCalled();
    expect(h.captures).toEqual(['start', 'stop']);
  });

  it('locks hands-free when the toggle chord completes during a hold', async () => {
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureStarted();
    h.session.handleHotkey({ type: 'toggle' });
    expect(h.states.at(-1)).toMatchObject({ phase: 'listening', locked: true });
    h.session.handleHotkey({ type: 'hold-end', aborted: false });
    expect(h.session.state).toBe('recording');
    h.session.onCaptureChunk(seconds(1));
    // Pressing the chord again: the hold part stops it, the toggle part must not restart.
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.handleHotkey({ type: 'toggle' });
    h.session.handleHotkey({ type: 'hold-end', aborted: false });
    h.session.onCaptureStopped();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.session.state).toBe('idle');
    expect(h.captures).toEqual(['start', 'stop']);
  });

  it('starts hands-free on a double tap and stops on the next tap', async () => {
    h.session.handleHotkey({ type: 'lock' });
    h.session.onCaptureStarted();
    expect(h.states.at(-1)).toMatchObject({ phase: 'listening', locked: true });
    h.session.onCaptureChunk(seconds(1));
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureStopped();
    h.session.handleHotkey({ type: 'hold-end', aborted: false });
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
  });

  it('toggles with a binding that is not a superset of hold', async () => {
    h.session.handleHotkey({ type: 'toggle' });
    h.session.onCaptureStarted();
    h.session.onCaptureChunk(seconds(1));
    h.session.handleHotkey({ type: 'toggle' });
    h.session.onCaptureStopped();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
  });

  it('does nothing in an app whose profile disables dictation', () => {
    h.config.value = {
      ...h.config.value,
      profiles: [
        { name: 'Vault', match_process: ['notepad.exe'], match_title: [], enabled: false },
      ],
    };
    h.session.handleHotkey({ type: 'hold-start' });
    expect(h.captures).toEqual([]);
    expect(h.session.state).toBe('idle');
  });

  it('applies the focused app profile to insertion', async () => {
    h.config.value = {
      ...h.config.value,
      profiles: [
        {
          name: 'Terminals',
          match_process: ['notepad.exe'],
          match_title: [],
          enabled: true,
          insert: { paste_shortcut: 'Ctrl+Shift+V' },
        },
      ],
    };
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.inserted[0]!.options.pasteShortcut).toBe('Ctrl+Shift+V');
  });

  it('says so when the text went to the clipboard instead', async () => {
    h.insertResult.value = { outcome: 'clipboard', reason: 'no-target' };
    h.dictate();
    await vi.waitFor(() => expect(phases(h)).toContain('notice'));
    const notice = h.states.find((s) => s.phase === 'notice');
    expect(notice?.message).toMatch(/clipboard/i);
  });

  it('says so when the local fallback produced the text', async () => {
    h.transcribe.mockResolvedValueOnce(
      route('offline text', { fellBack: true, fallbackReason: 'No connection' }),
    );
    h.dictate();
    await vi.waitFor(() => expect(phases(h)).toContain('notice'));
    expect(h.states.find((s) => s.phase === 'notice')?.message).toMatch(/local model/i);
    expect(h.events[0]).toMatchObject({ ok: true, fellBack: true });
  });

  it('shows a one-line error and keeps the audio for retry', async () => {
    h.transcribe.mockRejectedValueOnce(new TranscribeError('auth', 'Invalid API key'));
    h.dictate();
    await vi.waitFor(() => expect(phases(h)).toContain('error'));
    expect(h.inserted).toEqual([]);
    expect(h.events[0]).toMatchObject({ ok: false });
    expect(h.session.canRetry).toBe(true);
    expect(h.session.retryLast()).toBe(true);
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.inserted[0]!.options.target).toEqual(NOTEPAD);
  });

  it('reports a microphone error immediately at key-down', () => {
    h.session.handleHotkey({ type: 'hold-start' });
    h.session.onCaptureError('busy');
    expect(h.states.at(-1)).toMatchObject({ phase: 'error' });
    expect(h.states.at(-1)?.message).toMatch(/in use/i);
    expect(h.session.state).toBe('idle');
  });

  it('shows the cloud flag while a cloud model is active', async () => {
    h.config.value = {
      ...h.config.value,
      model: { ...h.config.value.model, active: 'groq/whisper-large-v3-turbo' },
    };
    h.transcribe.mockResolvedValueOnce(route('from the cloud', { cloud: true }));
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    expect(h.states.find((s) => s.phase === 'listening')?.cloud).toBe(true);
    expect(h.states.find((s) => s.phase === 'transcribing')?.cloud).toBe(true);
  });

  it('ignores hotkeys while paused', () => {
    h.session.setPaused(true);
    h.session.handleHotkey({ type: 'hold-start' });
    expect(h.captures).toEqual([]);
    h.session.setPaused(false);
    h.session.handleHotkey({ type: 'hold-start' });
    expect(h.captures).toEqual(['start']);
  });

  it('re-inserts the previous transcript on paste-last', async () => {
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    h.focused.app = SLACK;
    h.session.handleHotkey({ type: 'paste-last' });
    await vi.waitFor(() => expect(h.inserted).toHaveLength(2));
    expect(h.inserted[1]).toMatchObject({ text: 'Hello world ', options: { target: SLACK } });
  });

  it('arms Esc only while there is something to cancel', async () => {
    h.dictate();
    await vi.waitFor(() => expect(h.inserted).toHaveLength(1));
    await vi.waitFor(() => expect(h.armed.at(-1)).toBe(false));
    expect(h.armed).toContain(true);
  });
});
