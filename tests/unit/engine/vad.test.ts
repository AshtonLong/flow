import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SEGMENT_OPTIONS,
  energyProbs,
  extractSegments,
  segmentsFromProbs,
  VAD_FRAME,
} from '../../../src/engine/vad';

const FRAME_MS = DEFAULT_SEGMENT_OPTIONS.frameMs;

/** Builds a probability track from `[durationMs, probability]` runs. */
function track(...runs: Array<[number, number]>): { probs: number[]; totalMs: number } {
  const probs: number[] = [];
  for (const [ms, p] of runs) {
    for (let i = 0; i < Math.round(ms / FRAME_MS); i++) probs.push(p);
  }
  return { probs, totalMs: probs.length * FRAME_MS };
}

describe('segmentsFromProbs', () => {
  it('returns nothing for silence', () => {
    const { probs, totalMs } = track([3000, 0.02]);
    expect(segmentsFromProbs(probs, totalMs)).toEqual([]);
  });

  it('trims leading and trailing silence, keeping padding', () => {
    const { probs, totalMs } = track([1000, 0.01], [2000, 0.95], [1500, 0.01]);
    const segments = segmentsFromProbs(probs, totalMs);
    expect(segments).toHaveLength(1);
    const [seg] = segments;
    expect(seg!.startMs).toBeGreaterThan(700);
    expect(seg!.startMs).toBeLessThanOrEqual(1000);
    expect(seg!.endMs).toBeGreaterThanOrEqual(3000);
    expect(seg!.endMs).toBeLessThan(3400);
  });

  it('drops a long pause between two utterances', () => {
    const { probs, totalMs } = track([1000, 0.9], [3000, 0.01], [1000, 0.9]);
    const segments = segmentsFromProbs(probs, totalMs);
    expect(segments).toHaveLength(2);
    const kept = segments.reduce((n, s) => n + (s.endMs - s.startMs), 0);
    expect(kept).toBeLessThan(3000);
    expect(kept).toBeGreaterThanOrEqual(2000);
  });

  it('keeps a short pause inside one segment', () => {
    const { probs, totalMs } = track([800, 0.9], [200, 0.05], [800, 0.9]);
    expect(segmentsFromProbs(probs, totalMs)).toHaveLength(1);
  });

  it('ignores a click shorter than the minimum speech length', () => {
    const { probs, totalMs } = track([1000, 0.01], [FRAME_MS, 0.9], [1000, 0.01]);
    expect(segmentsFromProbs(probs, totalMs)).toEqual([]);
  });

  it('keeps speech that runs to the end of the recording', () => {
    const { probs, totalMs } = track([500, 0.01], [1000, 0.9]);
    const segments = segmentsFromProbs(probs, totalMs);
    expect(segments).toHaveLength(1);
    expect(segments[0]!.endMs).toBe(totalMs);
  });

  it('uses hysteresis so a dip between thresholds does not end speech', () => {
    const { probs, totalMs } = track([600, 0.9], [600, 0.42], [600, 0.9]);
    expect(segmentsFromProbs(probs, totalMs)).toHaveLength(1);
  });
});

describe('extractSegments', () => {
  it('concatenates the audio of each segment', () => {
    const audio = new Float32Array(16000);
    for (let i = 0; i < audio.length; i++) audio[i] = i;
    const out = extractSegments(audio, [
      { startMs: 0, endMs: 100 },
      { startMs: 500, endMs: 600 },
    ]);
    expect(out.length).toBe(3200);
    expect(out[0]).toBe(0);
    expect(out[1600]).toBe(8000);
  });

  it('clamps segments to the audio length', () => {
    const audio = new Float32Array(1600);
    expect(extractSegments(audio, [{ startMs: 50, endMs: 5000 }]).length).toBe(800);
  });
});

describe('energyProbs', () => {
  it('scores loud frames above quiet ones', () => {
    const audio = new Float32Array(VAD_FRAME * 40);
    for (let i = 0; i < audio.length; i++) {
      const loud = i >= VAD_FRAME * 15 && i < VAD_FRAME * 25;
      audio[i] = Math.sin(i / 5) * (loud ? 0.3 : 0.001);
    }
    const probs = energyProbs(audio);
    expect(probs.length).toBe(40);
    expect(probs[20]).toBeGreaterThan(0.8);
    expect(probs[5]).toBeLessThan(0.3);
    const segments = segmentsFromProbs(probs, 40 * FRAME_MS);
    expect(segments).toHaveLength(1);
  });

  it('finds no speech in a silent recording', () => {
    const probs = energyProbs(new Float32Array(VAD_FRAME * 30));
    expect(segmentsFromProbs(probs, 30 * FRAME_MS)).toEqual([]);
  });
});
