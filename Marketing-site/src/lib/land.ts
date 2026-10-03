/**
 * Section headings arrive the way dictated text does: loose and slanted as
 * spoken, then settling into type. Recursive keeps glyph widths fixed across
 * these axes, so nothing reflows while they settle.
 */
import { $$, gsap, onceVisible, still } from './env';

export function initLanding(): void {
  if (still) return;
  for (const heading of $$('[data-land]')) {
    // Wrap each word, keeping any line breaks the heading was written with.
    const words: HTMLElement[] = [];
    for (const node of [...heading.childNodes]) {
      if (node.nodeType !== Node.TEXT_NODE) continue;
      const parts: (HTMLElement | string)[] = [];
      for (const word of (node.textContent ?? '').trim().split(/\s+/).filter(Boolean)) {
        const span = document.createElement('span');
        span.className = 'lw';
        span.setAttribute('aria-hidden', 'true');
        span.textContent = word;
        if (parts.length) parts.push(' ');
        parts.push(span);
        words.push(span);
      }
      node.replaceWith(...parts);
    }
    heading.setAttribute('aria-label', words.map((w) => w.textContent).join(' '));
    gsap.set(words, { opacity: 0, y: '0.2em', '--casl': 1, '--slnt': -13 });
    onceVisible(
      heading,
      () => {
        gsap.to(words, {
          opacity: 1,
          y: 0,
          '--casl': 0,
          '--slnt': 0,
          duration: 0.9,
          ease: 'expo.out',
          stagger: 0.05,
        });
      },
      0.6,
    );
  }
}
