/**
 * Main-process client for the engine utility process. Starts the engine on
 * demand, restarts it after a crash, and exits it after the idle timeout so a
 * loaded model's memory goes back to the system.
 */
import { EventEmitter } from 'node:events';
import { utilityProcess, type UtilityProcess } from 'electron';
import type {
  EngineDevice,
  EngineLoadResult,
  EngineProbeResult,
  EngineRequest,
  EngineResponse,
  EngineResult,
  EngineTranscribeResult,
  EngineTrimResult,
} from '@shared/engine-protocol';
import { ENGINE_ENV } from '@shared/engine-protocol';
import type { BackendDevice } from '@shared/types';
import { log } from '../log';

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type RequestBody = DistributiveOmit<EngineRequest, 'id'>;

interface Pending {
  resolve(result: EngineResult): void;
  reject(error: Error): void;
}

export interface EngineClientOptions {
  /** Path of the bundled engine entry (`out/main/engine.js`). */
  entry: string;
  vadModelPath: string;
  /** Explicit native library path for packaged builds, where it is unpacked from the archive. */
  libraryPath?: string;
  /** Directory holding the CUDA runtime, when the optional GPU pack is installed. */
  cudaRuntimeDir?: string;
  debug?: boolean;
}

export class EngineAbortError extends Error {
  constructor() {
    super('Aborted');
    this.name = 'AbortError';
  }
}

export class EngineClient extends EventEmitter {
  private child: UtilityProcess | null = null;
  private ready: Promise<void> | null = null;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private idleTimer: NodeJS.Timeout | null = null;
  private keepLoadedMinutes = 10;
  private loadedId: string | null = null;
  private devicesCache: BackendDevice[] = [];
  private vadKind: 'silero' | 'energy' | null = null;

  constructor(private readonly options: EngineClientOptions) {
    super();
  }

  /** Local model currently held in engine memory. */
  get loadedModelId(): string | null {
    return this.loadedId;
  }

  get running(): boolean {
    return this.child !== null;
  }

  /** Compute devices seen by the last probe. */
  get devices(): BackendDevice[] {
    return this.devicesCache;
  }

  get vad(): 'silero' | 'energy' | null {
    return this.vadKind;
  }

  /** `0` unloads right after each dictation, `-1` never unloads. */
  setKeepLoadedMinutes(minutes: number): void {
    this.keepLoadedMinutes = minutes;
    this.scheduleIdleExit();
  }

  private start(): Promise<void> {
    if (this.ready) return this.ready;
    const env: Record<string, string> = {
      ...(process.env as Record<string, string>),
      [ENGINE_ENV.vadModel]: this.options.vadModelPath,
    };
    if (this.options.debug) env[ENGINE_ENV.debug] = '1';
    if (this.options.libraryPath) env.TRANSCRIBE_LIBRARY = this.options.libraryPath;
    if (this.options.cudaRuntimeDir) env.TRANSCRIBE_CUDA_RUNTIME_DIR = this.options.cudaRuntimeDir;
    const child = utilityProcess.fork(this.options.entry, [], {
      serviceName: 'Flow Engine',
      stdio: 'pipe',
      env,
    });
    this.child = child;
    child.stderr?.on('data', (data: Buffer) => log.warn('engine', data.toString().trim()));
    child.stdout?.on('data', (data: Buffer) => log.debug('engine', data.toString().trim()));

    this.ready = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('The speech engine did not start')), 15000);
      child.on('message', (message: EngineResponse | { id: 0; hello: true }) => {
        if ('hello' in message) {
          clearTimeout(timer);
          resolve();
          return;
        }
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.ok) pending.resolve(message.result);
        else pending.reject(message.aborted ? new EngineAbortError() : new Error(message.error));
        this.scheduleIdleExit();
      });
      child.once('exit', (code) => {
        clearTimeout(timer);
        if (this.child === child) this.handleExit(code);
        reject(new Error(`The speech engine exited (code ${code})`));
      });
    });
    // A failed start must not poison later attempts.
    this.ready.catch(() => {
      if (this.child === child) this.handleExit(null);
    });
    return this.ready;
  }

  private handleExit(code: number | null): void {
    const crashed = this.pending.size > 0;
    this.child = null;
    this.ready = null;
    this.loadedId = null;
    if (this.idleTimer) clearTimeout(this.idleTimer);
    this.idleTimer = null;
    for (const [, pending] of this.pending) {
      pending.reject(new Error('The speech engine stopped unexpectedly'));
    }
    this.pending.clear();
    if (crashed) log.error('engine', `exited with code ${code} while busy`);
    this.emit('exit', code);
  }

  private scheduleIdleExit(): void {
    if (this.idleTimer) clearTimeout(this.idleTimer);
    this.idleTimer = null;
    if (!this.child || this.pending.size > 0 || this.keepLoadedMinutes < 0) return;
    const delay = this.keepLoadedMinutes === 0 ? 1500 : this.keepLoadedMinutes * 60_000;
    this.idleTimer = setTimeout(() => {
      if (this.pending.size === 0) this.shutdown();
    }, delay);
    this.idleTimer.unref();
  }

  /** Exits the engine process, releasing the model. */
  shutdown(): void {
    const child = this.child;
    if (!child) return;
    this.handleExit(0);
    child.kill();
  }

  private async request<T extends EngineResult>(
    body: RequestBody,
    signal?: AbortSignal,
  ): Promise<T> {
    if (signal?.aborted) throw new EngineAbortError();
    await this.start();
    const child = this.child;
    if (!child) throw new Error('The speech engine is not running');
    const id = this.nextId++;
    if (this.idleTimer) clearTimeout(this.idleTimer);
    this.idleTimer = null;
    return new Promise<T>((resolve, reject) => {
      const onAbort = () => child.postMessage({ id: this.nextId++, type: 'cancel', target: id });
      signal?.addEventListener('abort', onAbort, { once: true });
      this.pending.set(id, {
        resolve: (result) => {
          signal?.removeEventListener('abort', onAbort);
          resolve(result as T);
        },
        reject: (error) => {
          signal?.removeEventListener('abort', onAbort);
          reject(error);
        },
      });
      child.postMessage({ ...body, id });
    });
  }

  async probe(): Promise<EngineProbeResult> {
    const result = await this.request<EngineProbeResult>({ type: 'probe' });
    this.devicesCache = result.devices;
    this.vadKind = result.vad;
    return result;
  }

  async load(modelId: string, path: string, device: EngineDevice, signal?: AbortSignal) {
    const result = await this.request<EngineLoadResult>(
      { type: 'load', modelId, path, device },
      signal,
    );
    this.setLoaded(modelId);
    return result;
  }

  async transcribe(
    ref: { modelId: string; path: string; device: EngineDevice },
    audio: Float32Array,
    language: string,
    hints: string[],
    signal?: AbortSignal,
  ): Promise<EngineTranscribeResult> {
    const result = await this.request<EngineTranscribeResult>(
      { type: 'transcribe', ...ref, audio, language, hints },
      signal,
    );
    this.setLoaded(ref.modelId);
    return result;
  }

  trim(audio: Float32Array): Promise<EngineTrimResult> {
    return this.request<EngineTrimResult>({ type: 'trim', audio });
  }

  async unload(): Promise<void> {
    if (!this.child) return;
    await this.request<null>({ type: 'unload' });
    this.setLoaded(null);
  }

  private setLoaded(id: string | null): void {
    if (this.loadedId === id) return;
    this.loadedId = id;
    this.emit('loaded', id);
  }
}
