/**
 * Voice detection. Silero VAD scores 32 ms frames; the scores are turned into
 * speech segments, and the segments are stitched together so leading silence,
 * trailing silence and long pauses are dropped.
 */

export const VAD_FRAME = 512;
export const VAD_CONTEXT = 64;
const SAMPLE_RATE = 16000;

export interface SegmentOptions {
  /** Probability at which speech starts. */
  threshold: number;
  /** Probability below which speech may end (hysteresis). */
  negThreshold: number;
  /** Silence this long ends a segment. */
  minSilenceMs: number;
  /** Segments shorter than this are noise. */
  minSpeechMs: number;
  /** Audio kept on both sides of each segment. */
  padMs: number;
  frameMs: number;
}

export const DEFAULT_SEGMENT_OPTIONS: SegmentOptions = {
  threshold: 0.5,
  negThreshold: 0.35,
  minSilenceMs: 400,
  minSpeechMs: 90,
  padMs: 200,
  frameMs: (VAD_FRAME / SAMPLE_RATE) * 1000,
};

export interface Segment {
  startMs: number;
  endMs: number;
}

/** Turns per-frame speech probabilities into padded, merged speech segments. */
export function segmentsFromProbs(
  probs: ArrayLike<number>,
  totalMs: number,
  options: Partial<SegmentOptions> = {},
): Segment[] {
  const o = { ...DEFAULT_SEGMENT_OPTIONS, ...options };
  const raw: Segment[] = [];
  let start = -1;
  let silenceStart = -1;
  for (let i = 0; i < probs.length; i++) {
    const p = probs[i]!;
    const t = i * o.frameMs;
    if (start < 0) {
      if (p >= o.threshold) {
        start = t;
        silenceStart = -1;
      }
      continue;
    }
    if (p >= o.threshold) {
      silenceStart = -1;
    } else if (p < o.negThreshold) {
      if (silenceStart < 0) silenceStart = t;
      if (t + o.frameMs - silenceStart >= o.minSilenceMs) {
        raw.push({ startMs: start, endMs: silenceStart });
        start = -1;
        silenceStart = -1;
      }
    }
  }
  if (start >= 0) raw.push({ startMs: start, endMs: silenceStart >= 0 ? silenceStart : totalMs });

  const padded = raw
    .filter((s) => s.endMs - s.startMs >= o.minSpeechMs)
    .map((s) => ({
      startMs: Math.max(0, s.startMs - o.padMs),
      endMs: Math.min(totalMs, s.endMs + o.padMs),
    }));

  const merged: Segment[] = [];
  for (const seg of padded) {
    const last = merged[merged.length - 1];
    if (last && seg.startMs <= last.endMs) last.endMs = Math.max(last.endMs, seg.endMs);
    else merged.push({ ...seg });
  }
  return merged;
}

/** Concatenates the audio of the given segments. */
export function extractSegments(audio: Float32Array, segments: Segment[]): Float32Array {
  const ranges = segments.map((s) => {
    const from = Math.max(0, Math.round((s.startMs / 1000) * SAMPLE_RATE));
    const to = Math.min(audio.length, Math.round((s.endMs / 1000) * SAMPLE_RATE));
    return [from, Math.max(from, to)] as const;
  });
  const total = ranges.reduce((n, [from, to]) => n + (to - from), 0);
  const out = new Float32Array(total);
  let offset = 0;
  for (const [from, to] of ranges) {
    out.set(audio.subarray(from, to), offset);
    offset += to - from;
  }
  return out;
}

/**
 * Fallback detector for when the ONNX runtime cannot load: frame energy against
 * an adaptive noise floor. Returns pseudo-probabilities on the Silero frame grid.
 */
export function energyProbs(audio: Float32Array): Float32Array {
  const frames = Math.floor(audio.length / VAD_FRAME);
  const rms = new Float32Array(frames);
  for (let f = 0; f < frames; f++) {
    let sum = 0;
    const base = f * VAD_FRAME;
    for (let i = 0; i < VAD_FRAME; i++) {
      const v = audio[base + i]!;
      sum += v * v;
    }
    rms[f] = Math.sqrt(sum / VAD_FRAME);
  }
  if (frames === 0) return rms;
  // Noise floor: the 10th percentile frame, with an absolute minimum.
  const sorted = Float32Array.from(rms).sort();
  const floor = Math.max(sorted[Math.floor(frames * 0.1)] ?? 0, 0.0015);
  const speechLevel = Math.max(floor * 3.5, 0.008);
  const probs = new Float32Array(frames);
  for (let f = 0; f < frames; f++) probs[f] = Math.min(1, rms[f]! / (speechLevel * 2));
  return probs;
}

interface OrtTensor {
  data: Float32Array | BigInt64Array;
}
interface OrtSession {
  inputNames: readonly string[];
  outputNames: readonly string[];
  run(feeds: Record<string, unknown>): Promise<Record<string, OrtTensor>>;
}
interface OrtModule {
  InferenceSession: {
    create(path: string, options?: Record<string, unknown>): Promise<OrtSession>;
  };
  Tensor: new (type: string, data: unknown, dims: number[]) => unknown;
}

/** Silero VAD v5 through onnxruntime-node. */
export class SileroVad {
  private constructor(
    private readonly ort: OrtModule,
    private readonly session: OrtSession,
  ) {}

  static async create(modelPath: string): Promise<SileroVad> {
    const imported = (await import('onnxruntime-node')) as unknown as OrtModule & {
      default?: OrtModule;
    };
    const ort = imported.default ?? imported;
    const session = await ort.InferenceSession.create(modelPath, {
      intraOpNumThreads: 1,
      interOpNumThreads: 1,
      executionProviders: ['cpu'],
      logSeverityLevel: 3,
    });
    return new SileroVad(ort, session);
  }

  /** Speech probability for each 512-sample frame. */
  async probabilities(audio: Float32Array): Promise<Float32Array> {
    const frames = Math.floor(audio.length / VAD_FRAME);
    const probs = new Float32Array(frames);
    let state = new Float32Array(2 * 1 * 128);
    const sr = new this.ort.Tensor('int64', BigInt64Array.from([BigInt(SAMPLE_RATE)]), []);
    const input = new Float32Array(VAD_CONTEXT + VAD_FRAME);
    const [outName, stateName] = this.session.outputNames;
    for (let f = 0; f < frames; f++) {
      const base = f * VAD_FRAME;
      // Each frame is prefixed with the last 64 samples of the previous one.
      if (f === 0) input.fill(0, 0, VAD_CONTEXT);
      else input.set(audio.subarray(base - VAD_CONTEXT, base), 0);
      input.set(audio.subarray(base, base + VAD_FRAME), VAD_CONTEXT);
      const out = await this.session.run({
        input: new this.ort.Tensor('float32', input, [1, VAD_CONTEXT + VAD_FRAME]),
        state: new this.ort.Tensor('float32', state, [2, 1, 128]),
        sr,
      });
      probs[f] = (out[outName!]!.data as Float32Array)[0]!;
      state = Float32Array.from(out[stateName!]!.data as Float32Array);
    }
    return probs;
  }
}
