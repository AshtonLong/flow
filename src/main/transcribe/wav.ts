/** WAV encoding for cloud uploads. */
import { SAMPLE_RATE } from '@shared/types';

const HEADER_BYTES = 44;

/** 16-bit PCM mono WAV. */
export function encodeWav(
  pcm: Float32Array,
  sampleRate: number = SAMPLE_RATE,
): Uint8Array<ArrayBuffer> {
  const dataBytes = pcm.length * 2;
  const buffer = new ArrayBuffer(HEADER_BYTES + dataBytes);
  const view = new DataView(buffer);
  const ascii = (offset: number, text: string): void => {
    for (let i = 0; i < text.length; i++) view.setUint8(offset + i, text.charCodeAt(i));
  };

  ascii(0, 'RIFF');
  view.setUint32(4, 36 + dataBytes, true);
  ascii(8, 'WAVE');
  ascii(12, 'fmt ');
  view.setUint32(16, 16, true); // fmt chunk size
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  ascii(36, 'data');
  view.setUint32(40, dataBytes, true);

  for (let i = 0; i < pcm.length; i++) {
    const sample = pcm[i]!;
    // NaN fails both comparisons and becomes silence.
    const clamped = sample >= 1 ? 1 : sample <= -1 ? -1 : sample > -1 ? sample : 0;
    view.setInt16(
      HEADER_BYTES + i * 2,
      Math.round(clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff),
      true,
    );
  }
  return new Uint8Array(buffer);
}
