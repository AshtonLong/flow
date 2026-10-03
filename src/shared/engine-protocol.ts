/** Messages between the main process and the engine utility process. */
import type { BackendDevice } from './types';

export type EngineDevice = 'auto' | 'cpu' | 'gpu';

export interface EngineModelRef {
  modelId: string;
  /** Absolute path of the GGUF file. */
  path: string;
  device: EngineDevice;
}

export type EngineRequest =
  | { id: number; type: 'probe' }
  | ({ id: number; type: 'load' } & EngineModelRef)
  | ({
      id: number;
      type: 'transcribe';
      audio: Float32Array;
      language: string;
      hints: string[];
    } & EngineModelRef)
  | { id: number; type: 'trim'; audio: Float32Array }
  | { id: number; type: 'unload' }
  /** Aborts the in-flight request with id `target`. No response is sent for the cancel itself. */
  | { id: number; type: 'cancel'; target: number };

export interface EngineProbeResult {
  version: string;
  devices: BackendDevice[];
  /** `silero` when the ONNX model loaded, `energy` for the fallback detector. */
  vad: 'silero' | 'energy';
}

export interface EngineLoadResult {
  modelId: string;
  /** Backend the model actually runs on, e.g. `CPU` or `Vulkan0`. */
  backend: string;
  loadMs: number;
}

export interface EngineTranscribeResult {
  text: string;
  backend: string;
  /** Milliseconds spent loading the model in this call (0 if already loaded). */
  loadMs: number;
  inferMs: number;
}

export interface EngineTrimResult {
  /** Speech with silence removed, or null when no speech was found. */
  audio: Float32Array | null;
  speechMs: number;
  detector: 'silero' | 'energy';
}

export type EngineResult =
  EngineProbeResult | EngineLoadResult | EngineTranscribeResult | EngineTrimResult | null;

export type EngineResponse =
  | { id: number; ok: true; result: EngineResult }
  | { id: number; ok: false; error: string; aborted?: boolean };

/** Sent unprompted by the engine once it is ready to take requests. */
export interface EngineHello {
  id: 0;
  hello: true;
}

/** Environment variables the main process sets when starting the engine. */
export const ENGINE_ENV = {
  vadModel: 'FLOW_VAD_MODEL',
  debug: 'FLOW_ENGINE_DEBUG',
} as const;
