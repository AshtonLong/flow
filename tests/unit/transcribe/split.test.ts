import { describe, expect, it } from 'vitest';
import { splitAtPauses } from '../../../src/main/transcribe/split';

const RATE = 16000;

/** Loud noise with silent gaps at the given [startSeconds, endSeconds] ranges. */
function speech(seconds: number, pauses: [number, number][] = [], rate = RATE): Float32Array {
  const audio = new Float32Array(Math.round(seconds * rate));
  let seed = 1;
  for (let i = 0; i < audio.length; i++) {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    audio[i] = (seed / 0x7fffffff - 0.5) * 0.8;
  }
  for (const [from, to] of pauses) audio.fill(0, Math.round(from * rate), Math.round(to * rate));
  return audio;
}

/** True if the pieces, end to end, are exactly `audio`. (`toEqual` is too slow at this size.) */
function rebuilds(pieces: Float32Array[], audio: Float32Array): boolean {
  let offset = 0;
  for (const piece of pieces) {
    if (piece.byteOffset !== audio.byteOffset + offset * 4) return false;
    for (let i = 0; i < piece.length; i++) {
      if (piece[i] !== audio[offset + i]) return false;
    }
    offset += piece.length;
  }
  return offset === audio.length;
}

describe('splitAtPauses', () => {
  it('returns short audio as one piece', () => {
    const audio = speech(12);
    const pieces = splitAtPauses(audio);
    expect(pieces).toHaveLength(1);
    expect(pieces[0]).toBe(audio);
  });

  it('does not split audio exactly at the limit', () => {
    expect(splitAtPauses(speech(30))).toHaveLength(1);
  });

  it('returns empty audio as one empty piece', () => {
    const pieces = splitAtPauses(new Float32Array(0));
    expect(pieces).toHaveLength(1);
    expect(pieces[0]).toHaveLength(0);
  });

  it('cuts inside the pause', () => {
    const audio = speech(45, [[22, 22.6]]);
    const pieces = splitAtPauses(audio);
    expect(pieces).toHaveLength(2);
    const cut = pieces[0]!.length / RATE;
    expect(cut).toBeGreaterThan(22);
    expect(cut).toBeLessThan(22.6);
  });

  it('picks the quietest pause within reach, not the first', () => {
    const audio = speech(45, [[26, 26.5]]);
    // A quieter-but-not-silent stretch earlier on.
    for (let i = 18 * RATE; i < 18.5 * RATE; i++) audio[i] = audio[i]! * 0.3;
    const cut = splitAtPauses(audio)[0]!.length / RATE;
    expect(cut).toBeGreaterThan(26);
    expect(cut).toBeLessThan(26.5);
  });

  it.each([
    [
      'long speech with pauses',
      speech(100, [
        [20, 20.4],
        [47, 47.5],
        [70, 70.3],
        [95, 95.4],
      ]),
      30,
    ],
    ['speech with no pause at all', speech(95), 30],
    ['pure silence', new Float32Array(100 * RATE), 30],
    ['a shorter limit', speech(31, [[7, 7.5]]), 10],
    ['a length just over the limit', speech(30.01), 30],
  ])('keeps every piece within the limit and loses nothing: %s', (_name, audio, maxSeconds) => {
    const pieces = splitAtPauses(audio, { maxSeconds });
    expect(pieces.length).toBeGreaterThan(1);
    for (const piece of pieces) {
      expect(piece.length).toBeGreaterThan(0);
      expect(piece.length).toBeLessThanOrEqual(maxSeconds * RATE);
    }
    expect(rebuilds(pieces, audio)).toBe(true);
  });

  it('returns views of the input rather than copies', () => {
    const audio = speech(40, [[25, 25.5]]);
    for (const piece of splitAtPauses(audio)) expect(piece.buffer).toBe(audio.buffer);
  });

  it('avoids a sliver at the end', () => {
    const pieces = splitAtPauses(speech(30.2));
    expect(pieces).toHaveLength(2);
    expect(pieces[1]!.length).toBeGreaterThanOrEqual(RATE);
  });

  it('honours the sample rate', () => {
    const audio = speech(45, [[22, 22.6]], 8000);
    const pieces = splitAtPauses(audio, { sampleRate: 8000 });
    expect(pieces).toHaveLength(2);
    const cut = pieces[0]!.length / 8000;
    expect(cut).toBeGreaterThan(22);
    expect(cut).toBeLessThan(22.6);
  });

  it('falls back to defaults for nonsense options', () => {
    const pieces = splitAtPauses(speech(45), { maxSeconds: 0, sampleRate: -1 });
    for (const piece of pieces) expect(piece.length).toBeLessThanOrEqual(30 * RATE);
  });
});
