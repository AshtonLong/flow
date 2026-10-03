/** The flood: the screen fills with the voice colour from the pill outwards while a key is held. */
import { $, gsap, root, still } from './env';

const el = $('.flood');

export const flood = {
  el,
  on(): void {
    root.classList.add('is-listening');
    gsap.to(el, { '--p': 1, duration: still ? 0 : 0.85, ease: 'power3.out', overwrite: true });
  },
  off(): void {
    root.classList.remove('is-listening');
    gsap.to(el, { '--p': 0, duration: still ? 0 : 0.6, ease: 'power3.inOut', overwrite: true });
  },
  /** Sets the level directly; used when scroll drives the flood. */
  set(p: number): void {
    gsap.set(el, { '--p': p, overwrite: true });
    root.classList.toggle('is-listening', p > 0.5);
  },
};
