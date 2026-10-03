/** Splits long recordings at pauses so each piece fits a model's window. */
import { SAMPLE_RATE } from '@shared/types';

/** Energy is measured in 10 ms frames. */
const FRAME_SECONDS = 0.01;
/** A cut goes in the middle of the quietest stretch this long. */
const PAUSE_SECONDS = 0.3;
/** Avoid leaving a final piece shorter than this. */
const MIN_TAIL_SECONDS = 1;

/**
 * Splits long audio at the quietest points so each piece is ≤ maxSeconds (default 30).
 * Pieces concatenate back to the input exactly; they are views of it, not copies.
 */
export function splitAtPauses(
  audio: Float32Array,
  opts: { maxSeconds?: number; sampleRate?: number } = {},
): Float32Array[] {
  const sampleRate = opts.sampleRate && opts.sampleRate > 0 ? opts.sampleRate : SAMPLE_RATE;
  const maxSeconds = opts.maxSeconds && opts.maxSeconds > 0 ? opts.maxSeconds : 30;
  const maxSamples = Math.max(1, Math.floor(maxSeconds * sampleRate));
  if (audio.length <= maxSamples) return [audio];

  const frame = Math.max(1, Math.round(FRAME_SECONDS * sampleRate));
  const frameCount = Math.ceil(audio.length / frame);
  // prefix[i] is the energy of frames 0..i-1, so any window sum is one subtraction.
  const prefix = new Float64Array(frameCount + 1);
  for (let f = 0; f < frameCount; f++) {
    const end = Math.min(audio.length, (f + 1) * frame);
    let energy = 0;
    for (let i = f * frame; i < end; i++) energy += audio[i]! * audio[i]!;
    prefix[f + 1] = prefix[f]! + energy;
  }
  const window = Math.max(1, Math.round(PAUSE_SECONDS / FRAME_SECONDS));
  const minTail = Math.min(Math.floor(MIN_TAIL_SECONDS * sampleRate), Math.floor(maxSamples / 2));

  const pieces: Float32Array[] = [];
  let start = 0;
  while (audio.length - start > maxSamples) {
    // Search the second half of the allowed span, so pieces stay reasonably long.
    const low = start + Math.ceil(maxSamples / 2);
    let high = start + maxSamples;
    if (audio.length - high < minTail) high = Math.max(low, audio.length - minTail);
    const cut = quietestPoint(prefix, frame, window, low, high);
    pieces.push(audio.subarray(start, cut));
    start = cut;
  }
  pieces.push(audio.subarray(start));
  return pieces;
}

/** Sample index in [low, high] at the centre of the quietest window; the latest one on a tie. */
function quietestPoint(
  prefix: Float64Array,
  frame: number,
  window: number,
  low: number,
  high: number,
): number {
  const frames = prefix.length - 1;
  const half = Math.floor(window / 2);
  const firstFrame = Math.ceil(low / frame);
  const lastFrame = Math.floor(high / frame);
  let best = -1;
  let bestEnergy = Infinity;
  for (let f = firstFrame; f <= lastFrame; f++) {
    const from = Math.max(0, f - half);
    const to = Math.min(frames, f + (window - half));
    const energy = (prefix[to]! - prefix[from]!) / (to - from);
    if (energy <= bestEnergy) {
      bestEnergy = energy;
      best = f;
    }
  }
  // No frame boundary inside the range (a tiny maxSeconds): cut at the limit.
  if (best < 0) return high;
  return Math.min(high, Math.max(low, best * frame));
}
