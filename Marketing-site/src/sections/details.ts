/** The smaller interactive sections: privacy route, model costs, the config twin and questions. */
import { $, $$, gsap, ScrollTrigger, still } from '../lib/env';
import { createPill } from '../lib/pill';

const CAPTIONS: Record<string, string> = {
  local:
    'Audio goes from the microphone to the model to your cursor, and never touches the network.',
  cloud:
    'The clip goes straight from your PC to the provider, and the text comes straight back. The pill shows a cloud while it does.',
};

export function initPrivacy(): void {
  const route = $('.route');
  const caption = $('.route__caption', route);
  for (const input of $$<HTMLInputElement>('input[name="route"]', route)) {
    input.addEventListener('change', () => {
      route.dataset.mode = input.value;
      caption.textContent = CAPTIONS[input.value] ?? '';
    });
  }
}

const usd = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' });
const SUBSCRIPTION = 15;

function spoken(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  const hours = h ? `${h} ${h === 1 ? 'hour' : 'hours'}` : '';
  const mins = m ? `${m} minutes` : '';
  return [hours, mins].filter(Boolean).join(' ');
}

/** Keeps a range input's filled track in step with its value. */
function fill(range: HTMLInputElement): void {
  const min = Number(range.min);
  const max = Number(range.max);
  range.style.setProperty('--fill', `${((Number(range.value) - min) / (max - min)) * 100}%`);
}

export function initModels(): void {
  const section = $('.models');
  const picks = $$<HTMLButtonElement>('.mlist--pick [role="radio"]', section);
  const range = $<HTMLInputElement>('#cost-min', section);
  const minutes = $('.cost__min', section);
  const name = $('[data-cost-name]', section);
  const amount = $('[data-cost-cloud]', section);
  const bar = $('.cost__row--cloud i', section);
  let picked = picks[0]!;

  function render(): void {
    const perDay = Number(range.value);
    const cost = (perDay / 60) * 30 * Number(picked.dataset.price);
    minutes.textContent = spoken(perDay);
    name.textContent = picked.dataset.name ?? '';
    amount.textContent = usd.format(cost);
    bar.style.setProperty('--w', String(Math.min(1, cost / SUBSCRIPTION)));
    fill(range);
  }

  function pick(button: HTMLButtonElement): void {
    picked = button;
    for (const b of picks) {
      b.setAttribute('aria-checked', String(b === button));
      b.tabIndex = b === button ? 0 : -1;
    }
    render();
  }

  picks.forEach((button, i) => {
    button.addEventListener('click', () => pick(button));
    button.addEventListener('keydown', (e) => {
      const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      const next = picks[(i + step + picks.length) % picks.length]!;
      next.focus();
      pick(next);
    });
  });
  range.addEventListener('input', render);
  render();
}

type Setting = 'hotkey' | 'filler' | 'keep' | 'method' | 'style';

const HOTKEYS = ['Ctrl+Win', 'RCtrl', 'Mouse4', 'F13'];
const METHODS = ['paste', 'type'];
const STYLES = ['pill', 'minimal', 'none'];
const KEEP_STEPS = [0, 5, 10, 15, 20, 25, 30];

const after = <T>(list: T[], value: T): T => list[(list.indexOf(value) + 1) % list.length]!;

export function initConfig(): void {
  const section = $('.config');
  const state = { hotkey: 'Ctrl+Win', filler: true, keep: 10, method: 'paste', style: 'pill' };
  const preview = createPill($('.twin__pill', section));

  const hotkey = $<HTMLButtonElement>('[data-bind="hotkey"]', section);
  const filler = $<HTMLInputElement>('[data-bind="filler"]', section);
  const keep = $<HTMLInputElement>('[data-bind="keep"]', section);
  const keepOut = $('.set__range output', section);
  const radios = $$<HTMLInputElement>('input[type="radio"][data-bind]', section);
  const value = (key: Setting) => $(`[data-edit="${key}"]`, section);

  const quoted = (s: string) => `"${s}"`;
  const inFile: Record<Setting, () => string> = {
    hotkey: () => quoted(state.hotkey),
    filler: () => String(state.filler),
    keep: () => String(state.keep),
    method: () => quoted(state.method),
    style: () => quoted(state.style),
  };

  function flash(el: Element): void {
    if (still) return;
    el.classList.remove('is-flash');
    void (el as HTMLElement).offsetWidth;
    el.classList.add('is-flash');
  }

  /** Writes the state to both sides. `from` is the side the change came from; the other one flashes. */
  function render(changed?: Setting, from?: 'window' | 'file'): void {
    hotkey.textContent = state.hotkey;
    filler.checked = state.filler;
    keep.value = String(state.keep);
    keepOut.textContent = String(state.keep);
    fill(keep);
    for (const radio of radios) {
      const key = radio.dataset.bind as 'method' | 'style';
      radio.checked = radio.value === state[key];
    }
    for (const key of Object.keys(inFile) as Setting[]) value(key).textContent = inFile[key]();

    preview.el.dataset.style = state.style;
    preview.set(state.style === 'none' ? 'hidden' : 'listening');

    if (!changed) return;
    if (from === 'window') flash(value(changed));
    else flash($(`[data-bind="${changed}"]`, section).closest('.set')!);
  }

  hotkey.addEventListener('click', () => {
    state.hotkey = after(HOTKEYS, state.hotkey);
    render('hotkey', 'window');
  });
  filler.addEventListener('change', () => {
    state.filler = filler.checked;
    render('filler', 'window');
  });
  keep.addEventListener('input', () => {
    state.keep = Number(keep.value);
    render('keep', 'window');
  });
  for (const radio of radios) {
    radio.addEventListener('change', () => {
      const key = radio.dataset.bind as 'method' | 'style';
      state[key] = radio.value;
      render(key, 'window');
    });
  }

  // The other direction: edit the file, and the window follows.
  const edits: Record<Setting, () => void> = {
    hotkey: () => (state.hotkey = after(HOTKEYS, state.hotkey)),
    filler: () => (state.filler = !state.filler),
    keep: () => (state.keep = after(KEEP_STEPS, state.keep)),
    method: () => (state.method = after(METHODS, state.method)),
    style: () => (state.style = after(STYLES, state.style)),
  };
  for (const key of Object.keys(edits) as Setting[]) {
    value(key).addEventListener('click', () => {
      edits[key]();
      render(key, 'file');
    });
  }

  render();
}

export function initFaq(): void {
  for (const item of $$<HTMLDetailsElement>('.faq details')) {
    const summary = $('summary', item);
    const body = $('.faq__a', item);
    summary.addEventListener('click', (e) => {
      if (still) return;
      e.preventDefault();
      gsap.killTweensOf(body);
      const closing = item.open && !('closing' in item.dataset);
      if (closing) {
        item.dataset.closing = '';
        gsap.to(body, {
          height: 0,
          duration: 0.35,
          ease: 'power3.inOut',
          onComplete: () => {
            item.open = false;
            delete item.dataset.closing;
            gsap.set(body, { clearProps: 'height' });
            ScrollTrigger.refresh();
          },
        });
        return;
      }
      delete item.dataset.closing;
      item.open = true;
      gsap.fromTo(
        body,
        { height: body.offsetHeight === body.scrollHeight ? 0 : body.offsetHeight },
        {
          height: 'auto',
          duration: 0.5,
          ease: 'power3.out',
          clearProps: 'height',
          onComplete: () => ScrollTrigger.refresh(),
        },
      );
    });
  }
}

/** The last section fills with the voice colour as it arrives, spreading out from the download key. */
export function initDownload(): void {
  if (still) return;
  const section = $('.cta');
  const fillEl = $('.cta__fill', section);
  const key = $('.key--giant', section);
  const origin = () => {
    const s = section.getBoundingClientRect();
    const k = key.getBoundingClientRect();
    fillEl.style.setProperty('--cx', `${k.left + k.width / 2 - s.left}px`);
    fillEl.style.setProperty('--cy', `${k.top + k.height / 2 - s.top}px`);
  };
  gsap.fromTo(
    fillEl,
    { '--p': 0 },
    {
      '--p': 1,
      ease: 'power1.in',
      scrollTrigger: {
        trigger: section,
        start: 'top 80%',
        end: 'top 15%',
        scrub: 0.5,
        onRefresh: origin,
      },
    },
  );
}
