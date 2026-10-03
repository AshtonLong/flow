/** The recording pill. One instance floats over the page; others sit inline as specimens. */
import { gsap, still } from './env';
import { level } from './voice';

export type PillState = 'hidden' | 'listening' | 'transcribing' | 'done' | 'error' | 'note';

export interface PillOptions {
  label?: string;
  /** Audio is leaving the PC, so the cloud icon shows. */
  cloud?: boolean;
}

export interface Pill {
  el: HTMLElement;
  set(state: PillState, opts?: PillOptions): void;
  state(): PillState;
}

const LABELS: Record<PillState, string> = {
  hidden: '',
  listening: 'Listening',
  transcribing: 'Transcribing',
  done: 'Typed',
  error: '',
  note: '',
};

const BARS = 13;

const MARKUP = `
  <svg class="pill__cloud" viewBox="0 0 24 24"><path d="M7 18a4 4 0 0 1-.6-7.96A5.5 5.5 0 0 1 17 9.5a4.25 4.25 0 0 1 .25 8.5z"/></svg>
  <span class="pill__wave">${'<i></i>'.repeat(BARS)}</span>
  <span class="pill__dots"><i></i><i></i><i></i></span>
  <svg class="pill__ok" viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>
  <svg class="pill__bang" viewBox="0 0 24 24"><path d="M12 6v7M12 17.5v.5"/></svg>
  <span class="pill__label"></span>`;

export function createPill(el: HTMLElement): Pill {
  el.innerHTML = MARKUP;
  el.dataset.state = 'hidden';
  // The floating pill stays centred as its width changes. GSAP owns the transform, so it centres it.
  if (!el.classList.contains('pill--inline')) gsap.set(el, { xPercent: -50 });
  const label = el.querySelector<HTMLElement>('.pill__label')!;
  const bars = [...el.querySelectorAll<HTMLElement>('.pill__wave i')];
  let current: PillState = 'hidden';

  const wave = () => {
    const t = performance.now() / 1000;
    bars.forEach((bar, i) => {
      bar.style.transform = `scaleY(${level(t, i).toFixed(3)})`;
    });
  };

  function set(next: PillState, opts: PillOptions = {}): void {
    const was = current;
    const from = el.offsetWidth;
    current = next;

    if (next === 'hidden') {
      gsap.ticker.remove(wave);
      if (was === 'hidden') return;
      gsap.to(el, {
        autoAlpha: 0,
        y: 12,
        scale: 0.92,
        duration: still ? 0 : 0.28,
        ease: 'power2.in',
        overwrite: true,
        onComplete: () => {
          el.dataset.state = 'hidden';
        },
      });
      return;
    }

    el.dataset.state = next;
    el.classList.toggle('is-cloud', Boolean(opts.cloud));
    label.textContent = opts.label ?? LABELS[next];

    if (next === 'listening' && !still) gsap.ticker.add(wave);
    else gsap.ticker.remove(wave);

    if (was === 'hidden') {
      gsap.fromTo(
        el,
        { autoAlpha: 0, y: 14, scale: 0.9, width: 'auto' },
        {
          autoAlpha: 1,
          y: 0,
          scale: 1,
          duration: still ? 0 : 0.5,
          ease: 'back.out(1.8)',
          overwrite: true,
        },
      );
      return;
    }

    // Already showing: grow or shrink to fit the new contents.
    gsap.set(el, { width: 'auto', autoAlpha: 1, y: 0, scale: 1, overwrite: true });
    if (still) return;
    const to = el.offsetWidth;
    gsap.fromTo(
      el,
      { width: from },
      { width: to, duration: 0.4, ease: 'power3.out', clearProps: 'width' },
    );
  }

  return { el, set, state: () => current };
}
