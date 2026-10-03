/**
 * Microphone capture shared by the overlay (dictation) and the settings window
 * (level meter, test bench). Chromium resamples to 16 kHz mono for us because
 * the AudioContext is created at that rate.
 */
import workletUrl from './capture-worklet.js?url';
import type { CaptureErrorReason } from '@shared/types';

export const CAPTURE_SAMPLE_RATE = 16000;

export class CaptureError extends Error {
  constructor(
    readonly reason: CaptureErrorReason,
    message: string,
  ) {
    super(message);
    this.name = 'CaptureError';
  }
}

export interface Microphone {
  /** Stored in `audio.input_device`. Labels are stable across windows; device ids are not. */
  label: string;
  deviceId: string;
}

export interface CaptureOptions {
  /** A microphone label, or `"default"`. */
  device: string;
  onChunk: (pcm: Float32Array) => void;
  /** Called about 60 times a second with the RMS level, 0..1. */
  onLevel?: (level: number) => void;
}

export interface CaptureHandle {
  /** For drawing a waveform. */
  readonly analyser: AnalyserNode;
  /** Flushes the last partial chunk, then releases the microphone unless `keepWarm`. */
  stop(keepWarm?: boolean): Promise<void>;
  /** Resumes a handle that was stopped with `keepWarm`. */
  resume(onChunk: (pcm: Float32Array) => void, onLevel?: (level: number) => void): void;
  /** Releases the microphone and audio context. */
  dispose(): void;
  readonly disposed: boolean;
  /** Fires if the device is unplugged while capturing. */
  onEnded: (() => void) | null;
}

export function toCaptureError(err: unknown): CaptureError {
  if (err instanceof CaptureError) return err;
  const name = (err as { name?: string })?.name ?? '';
  const message = (err as { message?: string })?.message ?? String(err);
  if (name === 'NotFoundError' || name === 'OverconstrainedError') {
    return new CaptureError('device-missing', 'Microphone not found');
  }
  if (name === 'NotAllowedError' || name === 'SecurityError') {
    return new CaptureError(
      'permission',
      'Microphone access is blocked in Windows privacy settings',
    );
  }
  if (name === 'NotReadableError' || name === 'AbortError') {
    return new CaptureError('busy', 'Microphone is in use by another app');
  }
  return new CaptureError('unknown', message);
}

/** Lists microphones. Labels are empty until the first successful capture grants access. */
export async function listMicrophones(): Promise<Microphone[]> {
  const devices = await navigator.mediaDevices.enumerateDevices();
  return devices
    .filter(
      (d) => d.kind === 'audioinput' && d.deviceId !== 'default' && d.deviceId !== 'communications',
    )
    .map((d) => ({ label: d.label, deviceId: d.deviceId }))
    .filter((d) => d.label);
}

async function resolveDeviceId(device: string): Promise<string | undefined> {
  if (!device || device === 'default') return undefined;
  const mics = await listMicrophones();
  const match = mics.find((m) => m.label === device);
  if (!match) throw new CaptureError('device-missing', `Microphone "${device}" is not connected`);
  return match.deviceId;
}

export async function startCapture(options: CaptureOptions): Promise<CaptureHandle> {
  let stream: MediaStream;
  let context: AudioContext;
  try {
    const deviceId = await resolveDeviceId(options.device);
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: deviceId ? { exact: deviceId } : undefined,
        channelCount: 1,
        echoCancellation: false,
        noiseSuppression: true,
        autoGainControl: true,
      },
      video: false,
    });
  } catch (err) {
    throw toCaptureError(err);
  }
  try {
    context = new AudioContext({ sampleRate: CAPTURE_SAMPLE_RATE, latencyHint: 'interactive' });
    await context.audioWorklet.addModule(workletUrl);
  } catch (err) {
    stream.getTracks().forEach((t) => t.stop());
    throw toCaptureError(err);
  }

  const source = context.createMediaStreamSource(stream);
  const analyser = context.createAnalyser();
  analyser.fftSize = 512;
  analyser.smoothingTimeConstant = 0.6;
  const node = new AudioWorkletNode(context, 'flow-capture', {
    numberOfInputs: 1,
    numberOfOutputs: 0,
    channelCount: 1,
    channelCountMode: 'explicit',
  });
  source.connect(analyser);
  source.connect(node);

  let onChunk: ((pcm: Float32Array) => void) | null = options.onChunk;
  let onLevel = options.onLevel;
  let flushed: (() => void) | null = null;
  let disposed = false;
  let raf = 0;

  node.port.onmessage = (event: MessageEvent<{ type: string; pcm?: Float32Array }>) => {
    if (event.data.type === 'chunk' && event.data.pcm) onChunk?.(event.data.pcm);
    else if (event.data.type === 'flushed') flushed?.();
  };

  const levelBuffer = new Float32Array(analyser.fftSize);
  const tick = () => {
    if (disposed || !onLevel) return;
    analyser.getFloatTimeDomainData(levelBuffer);
    let sum = 0;
    for (const v of levelBuffer) sum += v * v;
    onLevel(Math.min(1, Math.sqrt(sum / levelBuffer.length) * 4));
    raf = requestAnimationFrame(tick);
  };
  if (onLevel) raf = requestAnimationFrame(tick);

  const handle: CaptureHandle = {
    analyser,
    onEnded: null,
    get disposed() {
      return disposed;
    },
    async stop(keepWarm = false) {
      if (disposed) return;
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, 250);
        flushed = () => {
          clearTimeout(timer);
          resolve();
        };
        node.port.postMessage('flush');
      });
      flushed = null;
      onChunk = null;
      cancelAnimationFrame(raf);
      onLevel = undefined;
      if (!keepWarm) handle.dispose();
    },
    resume(nextChunk, nextLevel) {
      if (disposed) return;
      onChunk = nextChunk;
      onLevel = nextLevel;
      node.port.postMessage('resume');
      if (onLevel) raf = requestAnimationFrame(tick);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      cancelAnimationFrame(raf);
      onChunk = null;
      node.port.onmessage = null;
      try {
        source.disconnect();
        node.disconnect();
      } catch {
        // already disconnected
      }
      stream.getTracks().forEach((t) => t.stop());
      void context.close();
    },
  };

  for (const track of stream.getAudioTracks()) {
    track.addEventListener('ended', () => {
      if (!disposed) handle.onEnded?.();
    });
  }

  return handle;
}

/** Records until `stop()` is called and returns the whole clip. Used by the test bench. */
export async function recordClip(
  device: string,
  onLevel?: (level: number) => void,
): Promise<{ stop(): Promise<Float32Array>; cancel(): void; analyser: AnalyserNode }> {
  const chunks: Float32Array[] = [];
  const handle = await startCapture({ device, onChunk: (c) => chunks.push(c), onLevel });
  return {
    analyser: handle.analyser,
    async stop() {
      await handle.stop();
      const total = chunks.reduce((n, c) => n + c.length, 0);
      const out = new Float32Array(total);
      let offset = 0;
      for (const c of chunks) {
        out.set(c, offset);
        offset += c.length;
      }
      return out;
    },
    cancel() {
      handle.dispose();
    },
  };
}
