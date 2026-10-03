/** Any app: pick the focused app, dictate into it, and see its profile change the result. */
import { $, $$, gsap, onceVisible, still } from '../lib/env';
import { abort, play, ready, register, type Zone } from '../lib/dictation';

export function initApps(): void {
  const section = $('.apps');
  const tabs = $$<HTMLButtonElement>('[role="tab"]', section);
  const panels = $$('.app', section);
  const profiles = $$('.profile', section);
  const said = $('.said__text', section);
  const hold = $('.desk__hold .keys', section);
  let current = panels[0]!;

  // The markup carries finished text for readers without scripts; the demo starts empty.
  for (const slot of $$('[data-slot]', section)) slot.textContent = '';

  const slot = () => current.querySelector<HTMLElement>('[data-slot]');
  const spoken = () => current.dataset.said ?? '';
  /** Long enough to say the sentence at a natural pace. */
  const speakingMs = () => spoken().split(' ').length * 105 + 350;

  function speak(text: string): void {
    gsap.killTweensOf(said);
    gsap.set(said, { opacity: 1 });
    const spans = text.split(' ').map((word, i) => {
      const span = document.createElement('span');
      span.textContent = (i ? ' ' : '') + word;
      return span;
    });
    said.replaceChildren(...spans);
    if (still) return;
    gsap.fromTo(spans, { opacity: 0 }, { opacity: 1, duration: 0.1, stagger: 0.1 });
  }

  function hush(): void {
    gsap.killTweensOf(said.children);
    gsap.set(said.children, { opacity: 1 });
    gsap.to(said, {
      opacity: 0,
      duration: still ? 0 : 0.3,
      delay: still ? 0 : 0.5,
      onComplete: () => {
        said.replaceChildren();
        gsap.set(said, { opacity: 1 });
      },
    });
  }

  const zone: Zone = {
    el: section,
    cloud: () => 'cloud' in current.dataset,
    blocked() {
      if (!('off' in current.dataset)) return false;
      // The profile says no: the keys go down, and nothing else happens.
      hold.classList.add('is-down');
      setTimeout(() => hold.classList.remove('is-down'), 180);
      const notice = current.querySelector('.vault__off');
      if (notice && !still) {
        gsap.fromTo(
          notice,
          { backgroundColor: 'rgb(255 90 31)' },
          {
            backgroundColor: 'rgb(233 236 240)',
            duration: 0.9,
            ease: 'power2.out',
            clearProps: 'backgroundColor',
          },
        );
      }
      return true;
    },
    down() {
      const target = slot();
      if (target) target.textContent = '';
      speak(spoken());
    },
    land() {
      hush();
      const target = slot();
      if (!target) return;
      // Pasted, not typed: the whole text arrives at once.
      target.textContent = current.dataset.typed ?? '';
      if (still) return;
      gsap.fromTo(
        target,
        { backgroundColor: 'rgb(255 90 31)' },
        {
          backgroundColor: 'rgb(255 90 31 / 0)',
          duration: 1.1,
          ease: 'power2.out',
          clearProps: 'backgroundColor',
        },
      );
    },
    cancel: hush,
  };
  register(zone);

  function select(tab: HTMLButtonElement): void {
    const panel = panels.find((p) => p.id === tab.getAttribute('aria-controls'));
    if (!panel || panel === current) return;
    abort();
    for (const t of tabs) {
      t.setAttribute('aria-selected', String(t === tab));
      t.tabIndex = t === tab ? 0 : -1;
    }
    for (const p of panels) p.hidden = p !== panel;
    const name = panel.id.replace('app-', '');
    for (const p of profiles) p.hidden = p.dataset.for !== name;
    current = panel;
    said.replaceChildren();
    if (!still) {
      gsap.fromTo(
        panel,
        { opacity: 0, y: 12, scale: 0.985 },
        { opacity: 1, y: 0, scale: 1, duration: 0.5, ease: 'power3.out', clearProps: 'all' },
      );
    }
    void play(zone, speakingMs());
  }

  tabs.forEach((tab, i) => {
    tab.addEventListener('click', () => select(tab));
    tab.addEventListener('keydown', (e) => {
      const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      const next = tabs[(i + step + tabs.length) % tabs.length]!;
      next.focus();
      select(next);
    });
  });

  // Show it once, unprompted, when the section arrives.
  onceVisible(
    $('.desk__stack', section),
    () => {
      if (!ready()) return false;
      void play(zone, speakingMs());
    },
    0.6,
  );
}
