/**
 * The replay: one dictation, pinned and scrubbed by scroll, from key down to
 * text in the window. Without motion the section stays a plain list of steps.
 */
import { $, $$, gsap, root, ScrollTrigger, still } from '../lib/env';
import { lock, pill } from '../lib/dictation';
import { flood } from '../lib/flood';
import type { PillState } from '../lib/pill';
import { hash } from '../lib/voice';

interface Token {
  text: string;
  /** Removed by cleanup; the value names the rule that removes it. */
  drop?: string;
  /** Rewritten by cleanup. */
  to?: string;
  /** Names the rule behind a rewrite. */
  tag?: string;
  /** A breath follows this word. */
  pause?: boolean;
  /** Starts a new line in the cleaned text. */
  br?: boolean;
}

/** What the model heard, word by word, and what cleanup does to each. */
const TOKENS: Token[] = [
  { text: 'Um,', drop: 'filler' },
  { text: 'hey', to: 'Hey' },
  { text: 'Pria,', to: 'Priya,', tag: 'dictionary' },
  { text: 'the' },
  { text: 'the', drop: 'repeat' },
  { text: 'draft' },
  { text: 'is' },
  { text: 'ready.', pause: true },
  { text: 'New line.', drop: 'new line' },
  { text: 'Can', br: true },
  { text: 'you' },
  { text: 'review' },
  { text: 'it' },
  { text: 'by' },
  { text: 'Friday?' },
];

/** Timeline seconds at which each of the six steps begins. */
const STEPS = [0, 1, 2.5, 3.7, 5, 6.4];
const LENGTH = 8;
const KEY_DOWN = 0.15;
const KEY_UP = 2.55;
const INSERTED = 7.3;
/** Scroll distance of the pinned scene, in viewport heights. */
const SCROLL_SCREENS = 4.4;

const BAR_PITCH = 7;
const BAR_HEIGHT = 96;

interface Bar {
  el: HTMLElement;
  /** Position in the recording, left to right. */
  order: number;
  amp: number;
}

interface VoicedBar extends Bar {
  /** Where the bar sits once silence is trimmed: under its word. */
  x: number;
  y: number;
  small: number;
}

/** Below this the scene cannot fit one screen, so the section stays a plain list. */
const MIN_HEIGHT = 600;

export function initReplay(): void {
  if (still || innerHeight < MIN_HEIGHT) return;

  const section = $('.replay');
  const stage = $('.replay__stage', section);
  const barsEl = $('.replay__bars', section);
  const raw = $('.replay__raw', section);
  const clean = $('.replay__clean', section);
  const caretA = $('.replay__caret-a', section);
  const steps = $$('.replay__steps li', section);
  const stops = $$('.track__stops li', section);
  const line = $('.track__line', section);
  const keys = $('.replay__keys', section);
  const budget = $('.track__budget', section);

  section.classList.add('is-live');
  stops.forEach((stop, i) => stop.style.setProperty('--at', String(STEPS[i]! / LENGTH)));
  budget.style.setProperty('--from', String(KEY_UP / LENGTH));
  budget.style.setProperty('--to', String(INSERTED / LENGTH));

  // The transcript as heard.
  const words = TOKENS.map((tok) => {
    const el = document.createElement('span');
    el.className = 'rw';
    const a = document.createElement('span');
    a.textContent = tok.text;
    el.append(a);
    let b: HTMLElement | undefined;
    if (tok.to) {
      b = document.createElement('span');
      b.className = 'rw__b';
      b.textContent = tok.to;
      el.append(b);
    }
    let strike: HTMLElement | undefined;
    if (tok.drop) {
      strike = document.createElement('i');
      strike.className = 'rw__strike';
      el.append(strike);
    }
    let tag: HTMLElement | undefined;
    const reason = tok.drop ?? tok.tag;
    if (reason) {
      tag = document.createElement('em');
      tag.className = 'rw__tag';
      tag.textContent = reason;
      el.append(tag);
    }
    raw.append(el, ' ');
    return { tok, el, a, b, strike, tag };
  });

  // The cleaned text, laid out in the window but unseen: it marks where each kept word lands.
  clean.textContent = '';
  const targets = new Map<number, HTMLElement>();
  TOKENS.forEach((tok, i) => {
    if (tok.drop) return;
    if (tok.br) clean.append(document.createElement('br'));
    const el = document.createElement('span');
    el.textContent = tok.to ?? tok.text;
    clean.append(el, ' ');
    targets.set(i, el);
  });
  const caretB = document.createElement('span');
  caretB.className = 'caret replay__caret-b';
  clean.append(caretB);

  const rise = gsap.parseEase('power2.out');
  const fall = gsap.parseEase('power2.inOut');
  let timeline: gsap.core.Timeline | undefined;
  let trigger: ScrollTrigger | undefined;
  let shown: PillState = 'hidden';

  function showPill(state: PillState): void {
    if (state === shown) return;
    shown = state;
    pill.set(state);
  }

  /** Everything that follows the playhead without being a tween: keys, flood, pill, captions. */
  function sync(time: number): void {
    const step = Math.max(
      0,
      STEPS.findLastIndex((at) => time >= at),
    );
    steps.forEach((li, i) => li.classList.toggle('is-on', i === step));
    stops.forEach((li, i) => li.classList.toggle('is-on', i === step));
    line.style.setProperty('--prog', (time / LENGTH).toFixed(4));
    keys.classList.toggle('is-down', time >= KEY_DOWN && time < KEY_UP);

    // The flood and the pill are shared with the other demos, so only touch them while pinned.
    if (!trigger?.isActive) return;
    let level = 0;
    if (time >= KEY_UP) level = 1 - fall(Math.min(1, (time - KEY_UP) / 0.6));
    else if (time >= KEY_DOWN) level = rise(Math.min(1, (time - KEY_DOWN) / 0.8));
    flood.el.style.setProperty('--p', level.toFixed(4));
    root.classList.toggle('is-listening', level > 0.5);

    if (time < 0.3) showPill('hidden');
    else if (time < KEY_UP + 0.05) showPill('listening');
    else if (time < INSERTED) showPill('transcribing');
    else showPill('done');
  }

  function layout(): void {
    trigger?.kill(true);
    trigger = undefined;
    timeline?.kill();
    barsEl.replaceChildren();
    const parts = words.flatMap((w) =>
      [w.el, w.a, w.b, w.strike, w.tag].filter((p) => p !== undefined),
    );
    gsap.set([...parts, caretA, caretB], { clearProps: 'all' });

    // Fix each word's width, so restyling it later cannot reflow the line.
    words.forEach((w) => {
      w.el.style.width = `${Math.ceil(Math.max(w.a.offsetWidth, w.b?.offsetWidth ?? 0))}px`;
    });

    const origin = stage.getBoundingClientRect();
    const place = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { x: r.left - origin.left, y: r.top - origin.top, w: r.width, h: r.height };
    };
    const from = words.map((w) => place(w.el));
    const laneTop = Math.min(...from.map((r) => r.y));
    const laneBottom = Math.max(...from.map((r) => r.y + r.h));
    const laneY = (laneTop + laneBottom) / 2;

    // One run of bars per word, with silence before, after and at the breath.
    const counts = from.map((r) => Math.max(2, Math.round(r.w / BAR_PITCH)));
    const voicedTotal = counts.reduce((sum, n) => sum + n, 0);
    const lead = Math.round(voicedTotal * 0.09);
    const breath = Math.round(voicedTotal * 0.06);
    const trail = Math.round(voicedTotal * 0.1);
    const total = voicedTotal + lead + breath + trail;
    const pitch = origin.width / total;
    const thin = Math.min(1, Math.max(0.3, (pitch - 1.5) / 4));
    barsEl.style.setProperty('--bh', `${BAR_HEIGHT}px`);

    const silent: Bar[] = [];
    const groups: VoicedBar[][] = [];
    let order = 0;
    const make = (): HTMLElement => {
      const el = document.createElement('i');
      barsEl.append(el);
      gsap.set(el, { x: (order + 0.5) * pitch, y: laneY, scaleX: thin, scaleY: 0 });
      return el;
    };
    const hush = (n: number) => {
      for (let k = 0; k < n; k++) {
        silent.push({ el: make(), order, amp: 0.035 });
        order++;
      }
    };

    hush(lead);
    from.forEach((r, i) => {
      const n = counts[i]!;
      const fit = Math.min(1, (r.h * 0.8) / BAR_HEIGHT);
      const group: VoicedBar[] = [];
      for (let k = 0; k < n; k++) {
        const hump = Math.sin(((k + 0.5) / n) * Math.PI);
        const amp = 0.2 + 0.8 * hump * (0.4 + 0.6 * hash(order));
        group.push({
          el: make(),
          order,
          amp,
          x: r.x + (k + 0.5) * (r.w / n),
          y: r.y + r.h / 2,
          small: amp * fit,
        });
        order++;
      }
      groups.push(group);
      if (TOKENS[i]!.pause) hush(breath);
    });
    hush(trail);

    gsap.set(
      words.map((w) => w.el),
      { opacity: 0 },
    );
    const tl = gsap.timeline({
      paused: true,
      defaults: { ease: 'none' },
      onUpdate: () => sync(tl.time()),
    });

    // 2. You talk: the recording draws in, left to right.
    for (const bar of [...silent, ...groups.flat()]) {
      tl.to(
        bar.el,
        { scaleY: bar.amp, duration: 0.22, ease: 'power2.out' },
        STEPS[1]! + (bar.order / total) * 1.3,
      );
    }

    // 3. Key up: silence is cut and what is left closes up, word by word.
    tl.to(
      silent.map((b) => b.el),
      { opacity: 0, scaleY: 0, duration: 0.3 },
      KEY_UP + 0.3,
    );
    groups.forEach((group, i) => {
      for (const bar of group) {
        tl.to(
          bar.el,
          {
            x: bar.x,
            y: bar.y,
            scaleX: 1,
            scaleY: bar.small,
            duration: 0.65,
            ease: 'power3.inOut',
          },
          KEY_UP + 0.35 + i * 0.008,
        );
      }
    });

    // 4. Transcribe: each run of bars becomes its word.
    words.forEach((w, i) => {
      const at = STEPS[3]! + 0.1 + (i / words.length) * 1.0;
      tl.to(
        groups[i]!.map((b) => b.el),
        { scaleY: 0, opacity: 0, duration: 0.22, ease: 'power2.in' },
        at,
      );
      tl.fromTo(
        w.el,
        { opacity: 0, y: 6 },
        { opacity: 1, y: 0, duration: 0.3, ease: 'power2.out' },
        at + 0.08,
      );
    });

    // 5. Clean up: rules mark their words, then the transcript settles from speech into type.
    const tidy = STEPS[4]!;
    words.forEach((w) => {
      if (w.tag) {
        tl.fromTo(
          w.tag,
          { opacity: 0, y: 5 },
          { opacity: 1, y: 0, duration: 0.25, ease: 'power2.out' },
          tidy,
        );
      }
      if (w.strike)
        tl.to(w.strike, { scaleX: 1, duration: 0.3, ease: 'power2.inOut' }, tidy + 0.15);
      if (w.tok.drop) {
        tl.to(w.el, { opacity: 0, duration: 0.3 }, tidy + 0.6);
        return;
      }
      if (w.b) {
        tl.to(w.a, { opacity: 0, duration: 0.25 }, tidy + 0.35);
        tl.to(w.b, { opacity: 1, duration: 0.25 }, tidy + 0.4);
      }
      if (w.tag) tl.to(w.tag, { opacity: 0, duration: 0.25 }, tidy + 0.95);
      tl.to(
        w.el,
        {
          '--hl': 0,
          '--casl': 0,
          '--slnt': 0,
          fontWeight: 460,
          duration: 0.5,
          ease: 'power2.inOut',
        },
        tidy + 0.85,
      );
    });

    // 6. Insert: the kept words drop into the window at the cursor.
    const drop = STEPS[5]!;
    tl.to(caretA, { opacity: 0, duration: 0.04 }, drop);
    let n = 0;
    words.forEach((w, i) => {
      const target = targets.get(i);
      if (!target) return;
      const to = place(target);
      const at = from[i]!;
      tl.to(
        w.el,
        { x: to.x - at.x, y: to.y - at.y, duration: 0.75, ease: 'power3.inOut' },
        drop + 0.02 + n * 0.014,
      );
      n++;
    });
    tl.fromTo(caretB, { opacity: 0 }, { opacity: 1, duration: 0.04 }, INSERTED);
    tl.set({}, {}, LENGTH);

    timeline = tl;
    trigger = ScrollTrigger.create({
      trigger: section,
      start: 'top top',
      end: () => `+=${Math.round(innerHeight * SCROLL_SCREENS)}`,
      pin: true,
      scrub: 0.6,
      animation: tl,
      // Refreshed before triggers further down the page, which sit below this pin's spacing.
      refreshPriority: 1,
      onToggle: (self) => {
        lock(self.isActive);
        if (self.isActive) return;
        showPill('hidden');
        keys.classList.remove('is-down');
        flood.el.style.setProperty('--p', '0');
        root.classList.remove('is-listening');
      },
    });
    sync(tl.time());
  }

  layout();
  ScrollTrigger.refresh();

  let width = innerWidth;
  let timer: number | undefined;
  addEventListener('resize', () => {
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      if (innerWidth === width) return;
      width = innerWidth;
      layout();
      ScrollTrigger.refresh();
    }, 200);
  });
}
