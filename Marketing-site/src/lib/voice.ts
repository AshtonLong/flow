/**
 * A speech-shaped level in 0..1 for bar `i` at time `t` (seconds).
 * It is synthesised: the page never opens a microphone.
 */
export function level(t: number, i: number): number {
  const syllable = 0.5 + 0.5 * Math.sin(t * 9.5 + Math.sin(t * 2.1) * 1.8);
  const grain = 0.5 + 0.5 * Math.sin(t * 17.3 + i * 1.9) * Math.cos(t * 7.7 - i * 0.7);
  const drift = 0.5 + 0.5 * Math.sin(i * 0.55 + t * 3.1);
  return Math.min(1, 0.14 + 0.86 * syllable * (0.35 + 0.65 * grain) * (0.55 + 0.45 * drift));
}

/** A fixed pseudo-random value in 0..1 for index `i`, so layouts repeat exactly on rebuild. */
export function hash(i: number): number {
  const x = Math.sin(i * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}
