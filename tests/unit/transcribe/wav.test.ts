import { describe, expect, it } from 'vitest';
import { encodeWav } from '../../../src/main/transcribe/wav';

const ascii = (bytes: Uint8Array, start: number, length: number): string =>
  String.fromCharCode(...bytes.subarray(start, start + length));

describe('encodeWav', () => {
  it('writes a 16-bit PCM mono header at 16 kHz by default', () => {
    const wav = encodeWav(new Float32Array(160));
    const view = new DataView(wav.buffer, wav.byteOffset, wav.byteLength);
    expect(wav.byteLength).toBe(44 + 320);
    expect(ascii(wav, 0, 4)).toBe('RIFF');
    expect(view.getUint32(4, true)).toBe(36 + 320);
    expect(ascii(wav, 8, 4)).toBe('WAVE');
    expect(ascii(wav, 12, 4)).toBe('fmt ');
    expect(view.getUint32(16, true)).toBe(16);
    expect(view.getUint16(20, true)).toBe(1); // PCM
    expect(view.getUint16(22, true)).toBe(1); // mono
    expect(view.getUint32(24, true)).toBe(16000);
    expect(view.getUint32(28, true)).toBe(32000);
    expect(view.getUint16(32, true)).toBe(2);
    expect(view.getUint16(34, true)).toBe(16);
    expect(ascii(wav, 36, 4)).toBe('data');
    expect(view.getUint32(40, true)).toBe(320);
  });

  it('honours another sample rate', () => {
    const wav = encodeWav(new Float32Array(10), 48000);
    const view = new DataView(wav.buffer);
    expect(view.getUint32(24, true)).toBe(48000);
    expect(view.getUint32(28, true)).toBe(96000);
  });

  it('scales, clamps and silences bad samples', () => {
    const wav = encodeWav(new Float32Array([0, 1, -1, 0.5, -0.5, 2, -2, NaN, Infinity]));
    const view = new DataView(wav.buffer);
    const samples = Array.from({ length: 9 }, (_, i) => view.getInt16(44 + i * 2, true));
    expect(samples).toEqual([0, 32767, -32768, 16384, -16384, 32767, -32768, 0, 32767]);
  });

  it('encodes empty audio as a bare header', () => {
    const wav = encodeWav(new Float32Array(0));
    expect(wav.byteLength).toBe(44);
    expect(new DataView(wav.buffer).getUint32(40, true)).toBe(0);
  });
});
