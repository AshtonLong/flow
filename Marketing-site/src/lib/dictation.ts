/**
 * Drives a demo dictation along the path the app walks: key down, record, key
 * up, transcribe, insert. The speech is scripted; nothing here opens a microphone.
 */
import { $, $$, still, wait } from './env';
import { flood } from './flood';
import { createPill } from './pill';

export interface Zone {
  /** The section this zone covers. A keyboard hold goes to whichever zone fills most of the screen. */
  el: HTMLElement;
  /** Recording started. */
  down(): void;
  /** Transcription finished: put the text at the cursor. */
  land(): void;
  /** The recording was dropped, by Esc or by another key turning the hold into a shortcut. */
  cancel(): void;
  /** True when the focused app's profile uses a cloud model. */
  cloud?(): boolean;
  /** True when the focused app's profile switches dictation off. The zone shows why. */
  blocked?(): boolean;
}

export const pill = createPill($('.pill--global'));

/** A keyboard hold shorter than this is an accidental tap, as in the app. */
const TAP_MS = 300;
/** How long a scripted dictation "speaks" when the visitor clicks instead of holding. */
const SCRIPT_MS = 1300;
const TRANSCRIBE_MS = 520;

type Phase = 'idle' | 'recording' | 'finishing';

const zones: Zone[] = [];
const holdButtons = $$<HTMLButtonElement>('button[data-hold]');
let phase: Phase = 'idle';
let active: Zone | null = null;
let startedAt = 0;
/** Bumped on every start and abort, so a stale async step can tell it has been superseded. */
let run = 0;
let locked = false;

function setKeys(down: boolean): void {
  for (const button of holdButtons) button.classList.toggle('is-down', down);
}

export function register(zone: Zone): void {
  zones.push(zone);
}

/** While locked, holds are ignored. The pinned replay locks because scroll drives the pill there. */
export function lock(on: boolean): void {
  locked = on;
  if (on) abort();
}

/** True when a new dictation could start right now. */
export function ready(): boolean {
  return phase === 'idle' && !locked;
}

function begin(zone: Zone): boolean {
  if (locked || phase !== 'idle') return false;
  if (zone.blocked?.()) return false;
  run++;
  phase = 'recording';
  active = zone;
  startedAt = performance.now();
  setKeys(true);
  flood.on();
  pill.set('listening', { cloud: zone.cloud?.() });
  zone.down();
  return true;
}

async function finish(): Promise<void> {
  if (phase !== 'recording' || !active) return;
  const zone = active;
  const id = run;
  const cloud = zone.cloud?.();
  phase = 'finishing';
  setKeys(false);
  flood.off();
  pill.set('transcribing', { cloud });
  await wait(still ? 0 : TRANSCRIBE_MS);
  if (id !== run) return;
  zone.land();
  pill.set('done', { cloud });
  phase = 'idle';
  active = null;
  await wait(1200);
  if (id === run && phase === 'idle') pill.set('hidden');
}

/** Drops the dictation in progress. Nothing is typed. */
export function abort(): void {
  if (phase === 'idle') return;
  run++;
  const zone = active;
  phase = 'idle';
  active = null;
  setKeys(false);
  flood.off();
  pill.set('hidden');
  zone?.cancel();
}

/** Runs one scripted dictation in `zone`, holding for `holdMs` and at least until `ready` settles. */
export async function play(zone: Zone, holdMs = SCRIPT_MS, ready?: Promise<void>): Promise<void> {
  if (!begin(zone)) return;
  const id = run;
  await Promise.all([wait(still ? 0 : holdMs), ready]);
  if (id === run && phase === 'recording') await finish();
}

function release(): void {
  if (phase !== 'recording') return;
  const held = performance.now() - startedAt;
  if (held >= TAP_MS) {
    void finish();
    return;
  }
  // A click, not a hold: let the scripted sentence play out.
  const id = run;
  void wait(SCRIPT_MS - held).then(() => {
    if (id === run) void finish();
  });
}

/** The registered zone that fills most of the viewport, if any fills enough of it. */
function frontZone(): Zone | undefined {
  let best: Zone | undefined;
  let most = innerHeight * 0.3;
  for (const zone of zones) {
    const r = zone.el.getBoundingClientRect();
    const seen = Math.min(r.bottom, innerHeight) - Math.max(r.top, 0);
    if (seen > most) {
      most = seen;
      best = zone;
    }
  }
  return best;
}

for (const button of holdButtons) {
  const zone = () => zones.find((z) => z.el.contains(button));
  button.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    const z = zone();
    if (!z) return;
    // Keep receiving the release even if the pointer leaves the keys while held.
    if (button.hasPointerCapture(e.pointerId) === false) {
      try {
        button.setPointerCapture(e.pointerId);
      } catch {
        // Not every pointer can be captured; the hold still works without it.
      }
    }
    begin(z);
  });
  button.addEventListener('pointerup', release);
  button.addEventListener('pointercancel', release);
  button.addEventListener('contextmenu', (e) => e.preventDefault());
  button.addEventListener('keydown', (e) => {
    if ((e.key !== ' ' && e.key !== 'Enter') || e.repeat) return;
    e.preventDefault();
    const z = zone();
    if (z) begin(z);
  });
  button.addEventListener('keyup', (e) => {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    release();
  });
}

// Holding Ctrl on the keyboard works like the app's lone-modifier binding.
let armed: number | undefined;
let byKey = false;

function disarm(): void {
  clearTimeout(armed);
  armed = undefined;
}

addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    disarm();
    byKey = false;
    abort();
    return;
  }
  if (e.key === 'Control') {
    if (e.repeat || armed !== undefined || phase !== 'idle') return;
    const zone = frontZone();
    if (!zone) return;
    // Wait a beat first, so Ctrl+C and other shortcuts never start a recording.
    armed = window.setTimeout(() => {
      armed = undefined;
      byKey = begin(zone);
    }, TAP_MS);
    return;
  }
  // Any other key means Ctrl was part of a shortcut, not a hold.
  disarm();
  if (byKey) {
    byKey = false;
    abort();
  }
});

addEventListener('keyup', (e) => {
  if (e.key !== 'Control') return;
  disarm();
  if (byKey) {
    byKey = false;
    void finish();
  }
});

addEventListener('blur', () => {
  disarm();
  if (byKey) {
    byKey = false;
    abort();
  }
});

addEventListener('wheel', disarm, { passive: true });
addEventListener('pointerdown', disarm);
