/** Nothing is lost: each failure the app handles, played back in a specimen pill. */
import { $, $$, gsap, onceVisible, still, wait } from '../lib/env';
import { createPill, type PillOptions, type PillState } from '../lib/pill';

const TEXT = 'Order the long HDMI cable before Thursday.';

/**
 * A pause in milliseconds, a pill state, or a named stage action. Each case
 * rests on its last state, so the outcome stays readable.
 */
type Step = number | [PillState, PillOptions?] | 'type' | 'unfocus' | 'esc';

const CASES: Record<string, Step[]> = {
  cloud: [
    ['listening', { cloud: true }],
    1100,
    ['transcribing', { cloud: true }],
    1000,
    ['note', { label: 'Groq didn’t answer. Used Parakeet.' }],
    'type',
  ],
  focus: [
    'unfocus',
    ['listening'],
    1100,
    ['transcribing'],
    600,
    ['note', { label: 'No text field. Copied to clipboard.' }],
  ],
  mic: [['error', { label: 'No microphone found' }]],
  esc: [['listening'], 1300, 'esc', ['hidden']],
  retry: [
    ['listening'],
    1100,
    ['transcribing'],
    800,
    ['error', { label: 'Couldn’t transcribe. Retry is ready.' }],
    2000,
    ['transcribing'],
    700,
    ['done'],
    'type',
  ],
};

export function initSafety(): void {
  const section = $('.safety');
  const buttons = $$<HTMLButtonElement>('.case', section);
  const stage = $('.cases__stage', section);
  const text = $('.cases__text', section);
  const caret = $('.cases__body .caret', section);
  const esc = $('.cases__esc', section);
  const pill = createPill($('.cases__pill', section));
  let run = 0;

  async function playCase(name: string): Promise<void> {
    const steps = CASES[name];
    if (!steps) return;
    const id = ++run;
    text.textContent = '';
    caret.style.visibility = '';
    gsap.set(esc, { opacity: 0 });
    pill.set('hidden');
    await wait(still ? 0 : 350);

    for (const step of steps) {
      if (id !== run) return;
      if (typeof step === 'number') {
        await wait(step);
      } else if (step === 'type') {
        text.textContent = TEXT;
        if (!still) {
          gsap.fromTo(
            text,
            { backgroundColor: 'rgb(255 90 31)' },
            {
              backgroundColor: 'rgb(255 90 31 / 0)',
              duration: 1.1,
              ease: 'power2.out',
              clearProps: 'backgroundColor',
            },
          );
        }
      } else if (step === 'unfocus') {
        caret.style.visibility = 'hidden';
      } else if (step === 'esc') {
        gsap.fromTo(esc, { opacity: 0, y: 8 }, { opacity: 1, y: 0, duration: still ? 0 : 0.25 });
        await wait(450);
      } else {
        pill.set(step[0], step[1]);
      }
    }
  }

  for (const button of buttons) {
    button.addEventListener('click', () => {
      for (const b of buttons) b.setAttribute('aria-pressed', String(b === button));
      void playCase(button.dataset.case ?? '');
    });
  }

  // Play the first case once on arrival, unless the visitor has already picked one.
  onceVisible(
    stage,
    () => {
      if (run === 0) void playCase('cloud');
    },
    0.7,
  );
}
