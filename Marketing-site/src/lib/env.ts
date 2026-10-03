/** Shared plumbing: GSAP, the reduced-motion flag and small DOM helpers. */
import gsap from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';

gsap.registerPlugin(ScrollTrigger);

export { gsap, ScrollTrigger };

/** True when the visitor asked for reduced motion. Demos still run; they just don't animate. */
export const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

export const root = document.documentElement;

export function $<T extends Element = HTMLElement>(
  selector: string,
  scope: ParentNode = document,
): T {
  const el = scope.querySelector<T>(selector);
  if (!el) throw new Error(`Missing element: ${selector}`);
  return el;
}

export function $$<T extends Element = HTMLElement>(
  selector: string,
  scope: ParentNode = document,
): T[] {
  return [...scope.querySelectorAll<T>(selector)];
}

export const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Resolves when the page's font is usable, or after `capMs` if it is slow. */
export function fontsReady(capMs: number): Promise<void> {
  const loaded = Promise.all([
    document.fonts.load('860 1em "Recursive Variable"'),
    document.fonts.load('420 1em "Recursive Variable"'),
  ]).then(() => undefined);
  return Promise.race([loaded, wait(capMs)]).catch(() => undefined);
}

/**
 * Runs `fn` the first time `el` is at least `ratio` visible. If `fn` returns
 * false it was not ready, and it is tried again the next time `el` comes into view.
 */
export function onceVisible(el: Element, fn: () => boolean | void, ratio = 0.35): void {
  const io = new IntersectionObserver(
    (entries) => {
      if (entries.some((e) => e.isIntersecting) && fn() !== false) io.disconnect();
    },
    { threshold: ratio },
  );
  io.observe(el);
}
