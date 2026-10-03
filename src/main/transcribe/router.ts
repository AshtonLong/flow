/**
 * Picks the transcriber for a model id (the local engine or a cloud adapter),
 * splits long recordings at pauses, and falls back to the installed local
 * model when a cloud request fails.
 */
import { isCloudModelId } from '@shared/catalog';
import type { Config } from '@shared/config';
import type { Transcriber, TranscribeOptions, Transcript } from '@shared/types';
import { SAMPLE_RATE } from '@shared/types';
import { log } from '../log';
import type { ModelManager } from '../models';
import { createCloudTranscriber } from './cloud';
import type { EngineClient } from './engine-client';
import { TranscribeError } from './errors';
import { splitAtPauses } from './split';

/** Recordings longer than this are split at pauses. */
const MAX_PIECE_SECONDS = 30;

export interface RouterDeps {
  engine: EngineClient;
  models: ModelManager;
  getSecret(id: string): string | undefined;
}

export interface RouteResult {
  transcript: Transcript;
  cloud: boolean;
  /** The active model failed and the fallback produced the text. */
  fellBack: boolean;
  /** One line describing why the fallback was used. */
  fallbackReason?: string;
}

function isAbort(err: unknown): boolean {
  return (
    (err instanceof TranscribeError && err.code === 'aborted') ||
    (err as { name?: string })?.name === 'AbortError'
  );
}

/** Wraps any failure as a TranscribeError with a line fit for the overlay. */
export function toTranscribeError(err: unknown): TranscribeError {
  if (err instanceof TranscribeError) return err;
  if (isAbort(err)) return new TranscribeError('aborted', 'Cancelled', { cause: err });
  const message = err instanceof Error ? err.message : String(err);
  return new TranscribeError('engine', message, { cause: err });
}

export class TranscriberRouter {
  constructor(private readonly deps: RouterDeps) {}

  isCloud(modelId: string): boolean {
    return isCloudModelId(modelId);
  }

  /** Builds the transcriber for a model id. Throws TranscribeError if it cannot be used. */
  resolve(modelId: string, config: Config): Transcriber {
    if (isCloudModelId(modelId)) {
      return createCloudTranscriber(modelId, {
        catalog: this.deps.models.catalog(),
        config,
        getSecret: this.deps.getSecret,
      });
    }
    return this.local(modelId, config);
  }

  private local(modelId: string, config: Config): Transcriber {
    const { engine, models } = this.deps;
    const entry = models.localEntry(modelId);
    if (!entry) throw new TranscribeError('unknown-model', `Unknown model "${modelId}"`);
    const path = models.modelPath(modelId);
    if (!path) {
      throw new TranscribeError('not-installed', `${entry.name} is not downloaded`);
    }
    const ref = { modelId, path, device: config.model.device };
    return {
      id: modelId,
      cloud: false,
      async load(signal: AbortSignal) {
        await engine.load(ref.modelId, ref.path, ref.device, signal);
      },
      async transcribe(audio: Float32Array, opts: TranscribeOptions): Promise<Transcript> {
        const started = performance.now();
        const result = await engine.transcribe(ref, audio, opts.language, opts.hints, opts.signal);
        return {
          text: result.text,
          model: modelId,
          audioMs: (audio.length / SAMPLE_RATE) * 1000,
          elapsedMs: performance.now() - started,
        };
      },
      async unload() {
        await engine.unload();
      },
    };
  }

  /** Called at key-down so a local model loads while the user is speaking. */
  warmUp(config: Config): void {
    const id = config.model.active;
    const target = isCloudModelId(id) ? null : id;
    if (!target) return;
    try {
      const transcriber = this.local(target, config);
      transcriber.load?.(new AbortController().signal).catch((err) => {
        log.warn('router', `warm-up failed: ${String(err)}`);
      });
    } catch {
      // Not installed: the dictation itself reports it.
    }
  }

  private async run(
    transcriber: Transcriber,
    audio: Float32Array,
    opts: TranscribeOptions,
  ): Promise<Transcript> {
    const pieces =
      audio.length > MAX_PIECE_SECONDS * SAMPLE_RATE
        ? splitAtPauses(audio, { maxSeconds: MAX_PIECE_SECONDS, sampleRate: SAMPLE_RATE })
        : [audio];
    const started = performance.now();
    const texts: string[] = [];
    for (const piece of pieces) {
      if (opts.signal.aborted) throw new TranscribeError('aborted', 'Cancelled');
      // Pieces are views of one buffer; copy so only the piece crosses the process boundary.
      const part = await transcriber.transcribe(pieces.length > 1 ? piece.slice() : piece, opts);
      if (part.text) texts.push(part.text);
    }
    return {
      text: texts.join(' ').trim(),
      model: transcriber.id,
      audioMs: (audio.length / SAMPLE_RATE) * 1000,
      elapsedMs: performance.now() - started,
    };
  }

  /** Transcribes with one specific model and no fallback (test bench, retry on another model). */
  async transcribeWith(
    modelId: string,
    audio: Float32Array,
    config: Config,
    opts: TranscribeOptions,
  ): Promise<Transcript> {
    try {
      return await this.run(this.resolve(modelId, config), audio, opts);
    } catch (err) {
      throw toTranscribeError(err);
    }
  }

  /** Transcribes with the active model, falling back to the local fallback model on failure. */
  async transcribe(
    audio: Float32Array,
    config: Config,
    opts: TranscribeOptions,
  ): Promise<RouteResult> {
    const activeId = config.model.active;
    const cloud = isCloudModelId(activeId);
    try {
      const transcript = await this.run(this.resolve(activeId, config), audio, opts);
      return { transcript, cloud, fellBack: false };
    } catch (err) {
      const failure = toTranscribeError(err);
      if (failure.code === 'aborted') throw failure;
      const fallbackId = config.model.fallback;
      const canFallBack =
        fallbackId !== activeId &&
        !isCloudModelId(fallbackId) &&
        this.deps.models.isInstalled(fallbackId);
      if (!canFallBack) throw failure;
      log.warn('router', `${activeId} failed (${failure.code}); using ${fallbackId}`);
      try {
        const transcript = await this.run(this.local(fallbackId, config), audio, opts);
        return {
          transcript,
          cloud: false,
          fellBack: true,
          fallbackReason: failure.userMessage,
        };
      } catch (second) {
        if (isAbort(second)) throw toTranscribeError(second);
        // Report the original failure: it is the one the user can act on.
        throw failure;
      }
    }
  }
}
