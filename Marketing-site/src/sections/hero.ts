/** The hero. Its headline can stand as type or as a waveform shaped like its own letters. */
import { $, $$, fontsReady, gsap, root, still } from '../lib/env';
import { play, register, type Zone } from '../lib/dictation';
import { flood } from '../lib/flood';
import { level } from '../lib/voice';

interface Char {
  glyph: HTMLElement;
  bar: HTMLElement;
  /** 0 hides the bar, 1 lets it move at full height. */
  gain: { v: number };
}

function split(title: HTMLElement): Char[] {
  const chars: Char[] = [];
  const lines = $$('.line', title);
  title.setAttribute('aria-label', lines.map((l) => (l.textContent ?? '').trim()).join(' '));
  lines.forEach((line, n) => {
    const words = (line.textContent ?? '').trim().split(' ');
    line.textContent = '';
    line.setAttribute('aria-hidden', 'true');
    words.forEach((word, w) => {
      if (w > 0) line.append(' ');
      for (const letter of word) {
        const ch = document.createElement('span');
        ch.className = 'ch';
        const glyph = document.createElement('span');
        glyph.className = 'g';
        glyph.textContent = letter;
        const bar = document.createElement('i');
        bar.className = 'b';
        ch.append(glyph, bar);
        line.append(ch);
        chars.push({ glyph, bar, gain: { v: 0 } });
      }
    });
    if (n === lines.length - 1) {
      const caret = document.createElement('span');
      caret.className = 'caret';
      line.append(caret);
    }
  });
  title.classList.add('is-split');
  return chars;
}

export function initHero(): void {
  const section = $('.hero');
  const title = $('.hero__title', section);
  const chars = split(title);
  const glyphs = chars.map((c) => c.glyph);
  const gains = chars.map((c) => c.gain);

  const tick = () => {
    const t = performance.now() / 1000;
    chars.forEach((c, i) => {
      c.bar.style.transform = `scaleY(${(c.gain.v * level(t, i)).toFixed(3)})`;
    });
  };

  let landing: gsap.core.Timeline | undefined;

  function toBars(): void {
    landing?.kill();
    title.classList.add('is-bars');
    gsap.killTweensOf([...glyphs, ...gains]);
    if (still) {
      gsap.set(glyphs, { opacity: 0.25 });
      return;
    }
    gsap.to(glyphs, { opacity: 0, duration: 0.12 });
    gsap.to(gains, { v: 1, duration: 0.4, ease: 'power2.out', stagger: 0.008 });
    gsap.ticker.add(tick);
  }

  function toText(): void {
    title.classList.remove('is-bars');
    gsap.killTweensOf([...glyphs, ...gains]);
    if (still) {
      gsap.set(glyphs, { opacity: 1 });
      return;
    }
    // Left to right, each bar thins away and its letter comes up from a hairline to full weight.
    const tl = gsap.timeline({ onComplete: () => gsap.ticker.remove(tick) });
    chars.forEach((c, i) => {
      const at = i * 0.018;
      tl.to(c.gain, { v: 0, duration: 0.2, ease: 'power2.in' }, at);
      tl.fromTo(
        c.glyph,
        { opacity: 0, fontWeight: 300, '--casl': 1, '--slnt': -14 },
        { opacity: 1, fontWeight: 860, '--casl': 0, '--slnt': 0, duration: 0.7, ease: 'expo.out' },
        at + 0.1,
      );
    });
    landing = tl;
  }

  const zone: Zone = { el: section, down: toBars, land: toText, cancel: toText };
  register(zone);

  // The page opens mid-recording: flooded, keys down, the headline still a waveform.
  if (root.classList.contains('intro')) {
    flood.set(1);
    root.classList.remove('intro');
    void play(zone, 1350, fontsReady(2500));
  }
}
