/** Types shared across the main process, the engine, preloads and renderers. */

/** Sample rate of all audio inside Flow: 16 kHz mono float32. */
export const SAMPLE_RATE = 16000;

export interface TranscribeOptions {
  /** `"en"` in v1. */
  language: string;
  /** Dictionary terms, used where the model supports biasing. */
  hints: string[];
  /** Cancels the request (Esc). */
  signal: AbortSignal;
}

export interface Transcript {
  text: string;
  /** Model id that produced the text. */
  model: string;
  audioMs: number;
  elapsedMs: number;
}

/** Every model, local or cloud, is reached through this interface. */
export interface Transcriber {
  readonly id: string;
  /** True when audio leaves the machine. */
  readonly cloud: boolean;
  /** Local models load here; cloud adapters omit it. */
  load?(signal: AbortSignal): Promise<void>;
  transcribe(audio: Float32Array, opts: TranscribeOptions): Promise<Transcript>;
  unload?(): Promise<void>;
}

/** The focused window, used for per-app profiles and for insertion targeting. */
export interface FocusedApp {
  /** Native window handle as a decimal string (HWND on Windows). */
  windowId: string;
  /** Executable name, e.g. `slack.exe`. */
  processName: string;
  title: string;
  pid: number;
  /** True when the window belongs to an elevated (administrator) process. */
  elevated?: boolean;
}

export type SessionState = 'idle' | 'recording' | 'transcribing' | 'inserting';

export type OverlayPhase = 'hidden' | 'listening' | 'transcribing' | 'done' | 'error' | 'notice';

export interface OverlayState {
  phase: OverlayPhase;
  /** Audio is being sent to a provider. */
  cloud: boolean;
  /** Recording is locked hands-free (double-tap or toggle). */
  locked?: boolean;
  /** One line shown for `error` and `notice`. */
  message?: string;
  /** Number of earlier dictations still being transcribed. */
  queued?: number;
}

export interface OverlayAppearance {
  style: 'pill' | 'minimal' | 'none';
  size: 'small' | 'medium' | 'large';
  showWaveform: boolean;
  sounds: boolean;
  theme: ThemeInfo;
}

export interface ThemeInfo {
  dark: boolean;
  /** System accent colour as `#rrggbb`. */
  accent: string;
  highContrast: boolean;
  reducedMotion: boolean;
}

export type CaptureErrorReason = 'device-missing' | 'permission' | 'busy' | 'unknown';

export interface CaptureStart {
  deviceId: string;
  /** Leave the microphone stream open after stopping, for a faster next start. */
  keepWarm: boolean;
}

export type InsertOutcome = 'pasted' | 'typed' | 'clipboard';

export interface InsertResult {
  outcome: InsertOutcome;
  /** Why the text went to the clipboard instead of the cursor. */
  reason?: 'no-target' | 'target-closed' | 'elevated' | 'failed';
}

/** One finished (or failed) dictation, broadcast to open settings windows. */
export interface DictationEvent {
  id: number;
  ok: boolean;
  text: string;
  rawText: string;
  model: string;
  cloud: boolean;
  audioMs: number;
  /** Key release to inserted text. */
  elapsedMs: number;
  outcome?: InsertOutcome;
  /** Set when the active model failed and the fallback produced the text. */
  fellBack?: boolean;
  error?: string;
}

export interface HistoryEntry {
  id: number;
  /** Unix epoch milliseconds. */
  createdAt: number;
  text: string;
  rawText: string;
  model: string;
  app: string;
  audioMs: number;
  elapsedMs: number;
}

export interface BackendDevice {
  name: string;
  kind: string;
  deviceType: string;
  description: string;
  memoryTotal: number;
}

export interface AppInfo {
  version: string;
  electron: string;
  platform: string;
  configPath: string;
  logsPath: string;
  modelsPath: string;
  /** Compute devices the local engine can use; empty until first probed. */
  devices: BackendDevice[];
  /** Native modules that failed to load, with the feature each one disables. */
  degraded: { module: string; effect: string }[];
  paused: boolean;
}
