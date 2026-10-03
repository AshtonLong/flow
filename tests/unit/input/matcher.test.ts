import { describe, expect, it } from 'vitest';
import { type Config, DEFAULT_CONFIG } from '@shared/config';
import type { KeyId } from '@shared/hotkeys';
import { type HotkeyEvent, HotkeyMatcher } from '../../../src/main/input/matcher';

type Bindings = Config['hotkeys'];

const START: HotkeyEvent = { type: 'hold-start' };
const END: HotkeyEvent = { type: 'hold-end', aborted: false };
const ABORT: HotkeyEvent = { type: 'hold-end', aborted: true };
const LOCK: HotkeyEvent = { type: 'lock' };
const TOGGLE: HotkeyEvent = { type: 'toggle' };
const CANCEL: HotkeyEvent = { type: 'cancel' };
const PASTE: HotkeyEvent = { type: 'paste-last' };

function setup(overrides: Partial<Bindings> = {}, osKeys?: Map<KeyId, boolean | null>) {
  const events: HotkeyEvent[] = [];
  let time = 1000;
  const matcher = new HotkeyMatcher({
    bindings: { ...DEFAULT_CONFIG.hotkeys, ...overrides },
    emit: (e) => events.push(e),
    now: () => time,
    isKeyDown: osKeys ? (key) => osKeys.get(key) ?? null : undefined,
  });
  return {
    matcher,
    events,
    /** Returns the events emitted since the last call. */
    take: () => events.splice(0),
    advance: (ms: number) => {
      time += ms;
    },
    down: (...keys: KeyId[]) => keys.forEach((k) => matcher.keyDown(k)),
    up: (...keys: KeyId[]) => keys.forEach((k) => matcher.keyUp(k)),
  };
}

describe('hold to talk', () => {
  it('starts when the binding is exactly satisfied and ends on release', () => {
    const t = setup();
    t.down('LCtrl');
    expect(t.take()).toEqual([]);
    t.down('LWin');
    expect(t.take()).toEqual([START]);
    t.advance(2000);
    t.up('LWin');
    expect(t.take()).toEqual([END]);
    t.up('LCtrl');
    expect(t.take()).toEqual([]);
  });

  it('works in either press order and with either side of a generic modifier', () => {
    const t = setup();
    t.down('RWin', 'RCtrl');
    expect(t.take()).toEqual([START]);
    t.up('RCtrl');
    expect(t.take()).toEqual([END]);
  });

  it('ends when either key is released, once', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.advance(1000);
    t.up('LCtrl');
    expect(t.take()).toEqual([START, END]);
    t.up('LWin');
    expect(t.take()).toEqual([]);
  });

  it('ignores auto-repeat', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'LWin', 'LWin', 'LCtrl');
    expect(t.take()).toEqual([START]);
  });

  it('does not start when another key is already down', () => {
    const t = setup();
    t.down('A', 'LCtrl', 'LWin');
    expect(t.take()).toEqual([]);
    t.up('A');
    // Releasing the foreign key must not start a recording either.
    expect(t.take()).toEqual([]);
    t.up('LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
  });

  it('does not start with an extra modifier held', () => {
    const t = setup();
    t.down('LShift', 'LCtrl', 'LWin');
    expect(t.take()).toEqual([]);
  });

  it('keeps going while the other side of a generic modifier is pressed and released', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.advance(1000);
    t.down('RCtrl');
    t.up('LCtrl');
    expect(t.take()).toEqual([START]);
    t.up('RCtrl');
    expect(t.take()).toEqual([END]);
  });

  it('can restart by re-pressing one key while the other stays down', () => {
    const t = setup({ double_tap_lock: false });
    t.down('LCtrl', 'LWin');
    t.up('LWin');
    t.down('LWin');
    t.up('LWin');
    expect(t.take()).toEqual([START, END, START, END]);
  });

  it('a sided binding only matches that side', () => {
    const t = setup({ hold_to_talk: 'RCtrl' });
    t.down('LCtrl');
    t.up('LCtrl');
    expect(t.take()).toEqual([]);
    t.down('RCtrl');
    t.advance(1000);
    t.up('RCtrl');
    expect(t.take()).toEqual([START, END]);
  });

  it('a lone-modifier hold does not start when part of a larger chord', () => {
    const t = setup({ hold_to_talk: 'RCtrl' });
    t.down('LShift', 'RCtrl');
    expect(t.take()).toEqual([]);
  });

  it('works with a mouse button', () => {
    const t = setup({ hold_to_talk: 'Mouse4' });
    t.down('Mouse4');
    t.advance(900);
    t.up('Mouse4');
    expect(t.take()).toEqual([START, END]);
  });

  it('works with a modifier plus a mouse button', () => {
    const t = setup({ hold_to_talk: 'Ctrl+Mouse5' });
    t.down('Mouse5');
    t.up('Mouse5');
    expect(t.take()).toEqual([]);
    t.down('LCtrl', 'Mouse5');
    t.advance(900);
    t.up('Mouse5');
    expect(t.take()).toEqual([START, END]);
  });

  it('works with a plain key', () => {
    const t = setup({ hold_to_talk: 'F9' });
    t.down('F9');
    t.advance(900);
    t.up('F9');
    expect(t.take()).toEqual([START, END]);
  });
});

describe('foreign keys during a hold', () => {
  it('aborts the hold and emits no second hold-end on release', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.down('Left');
    expect(t.take()).toEqual([START, ABORT]);
    t.up('Left', 'LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
  });

  it('does not restart while the hold keys stay down after an abort', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Left');
    t.up('Left');
    t.down('Left');
    t.up('Left');
    t.down('RCtrl');
    t.up('RCtrl');
    expect(t.take()).toEqual([START, ABORT]);
    // Fully releasing and pressing again starts a fresh hold.
    t.up('LWin', 'LCtrl');
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START]);
  });

  it('an extra modifier is a foreign key too', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'LShift');
    expect(t.take()).toEqual([START, ABORT]);
  });

  it('a mouse button is a foreign key too', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Mouse4');
    expect(t.take()).toEqual([START, ABORT]);
  });

  it('a key that completes another binding fires it and keeps the hold going', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.down('Space');
    expect(t.take()).toEqual([START, TOGGLE]);
    t.up('Space');
    expect(t.take()).toEqual([]);
    t.advance(1000);
    t.up('LWin');
    expect(t.take()).toEqual([END]);
  });

  it('a foreign key after a completed chord still aborts', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Space', 'A');
    expect(t.take()).toEqual([START, TOGGLE, ABORT]);
  });

  it('the cancel key cancels during a hold even though the hold modifiers are down', () => {
    const t = setup();
    t.matcher.setCancelArmed(true);
    t.down('LCtrl', 'LWin', 'Esc');
    expect(t.take()).toEqual([START, CANCEL]);
    t.advance(1000);
    t.up('Esc', 'LWin');
    expect(t.take()).toEqual([END]);
  });

  it('the cancel key is a foreign key when cancel is not armed', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Esc');
    expect(t.take()).toEqual([START, ABORT]);
  });
});

describe('double-tap lock', () => {
  const tap = (t: ReturnType<typeof setup>, heldMs = 80) => {
    t.down('LCtrl', 'LWin');
    t.advance(heldMs);
    t.up('LWin', 'LCtrl');
  };

  it('emits lock instead of hold-start for the second press, and no hold-end for it', () => {
    const t = setup();
    tap(t);
    expect(t.take()).toEqual([START, END]);
    t.advance(150);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([LOCK]);
    t.advance(80);
    t.up('LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
  });

  it('works when only one key of the chord is tapped twice', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.advance(60);
    t.up('LWin');
    t.advance(100);
    t.down('LWin');
    expect(t.take()).toEqual([START, END, LOCK]);
  });

  it('a second press after the window is an ordinary hold', () => {
    const t = setup();
    tap(t);
    t.advance(351);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START, END, START]);
  });

  it('a second press exactly at the window still locks', () => {
    const t = setup();
    tap(t);
    t.advance(350);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START, END, LOCK]);
  });

  it('a long first press is not a tap', () => {
    const t = setup();
    tap(t, 300);
    t.advance(50);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START, END, START]);
  });

  it('is disabled by double_tap_lock = false', () => {
    const t = setup({ double_tap_lock: false });
    tap(t);
    t.advance(100);
    tap(t);
    expect(t.take()).toEqual([START, END, START, END]);
  });

  it('a key pressed between the taps breaks the double-tap', () => {
    const t = setup();
    tap(t);
    t.advance(50);
    t.down('A');
    t.up('A');
    t.advance(50);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START, END, START]);
  });

  it('an aborted hold is not a tap', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Left');
    t.up('Left', 'LWin', 'LCtrl');
    t.advance(50);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START, ABORT, START]);
  });

  it('a third tap after a lock is a plain tap, and a fourth locks again', () => {
    const t = setup();
    tap(t);
    t.advance(100);
    tap(t);
    expect(t.take()).toEqual([START, END, LOCK]);
    t.advance(100);
    tap(t);
    expect(t.take()).toEqual([START, END]);
    t.advance(100);
    tap(t);
    expect(t.take()).toEqual([LOCK]);
  });

  it('honours custom timings', () => {
    const events: HotkeyEvent[] = [];
    let time = 0;
    const matcher = new HotkeyMatcher({
      bindings: { ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'F9' },
      emit: (e) => events.push(e),
      now: () => time,
      doubleTapMs: 100,
      tapMaxMs: 50,
    });
    matcher.keyDown('F9');
    time += 40;
    matcher.keyUp('F9');
    time += 101;
    matcher.keyDown('F9');
    time += 60;
    matcher.keyUp('F9');
    time += 10;
    matcher.keyDown('F9');
    expect(events).toEqual([START, END, START, END, START]);
  });

  it('works with a mouse button', () => {
    const t = setup({ hold_to_talk: 'Mouse4' });
    t.down('Mouse4');
    t.advance(50);
    t.up('Mouse4');
    t.advance(50);
    t.down('Mouse4');
    expect(t.take()).toEqual([START, END, LOCK]);
  });
});

describe('edge-triggered bindings', () => {
  it('toggle fires on the key-down that completes the chord, once', () => {
    const t = setup({ hold_to_talk: 'F9', toggle: 'Ctrl+Shift+D' });
    t.down('LCtrl', 'LShift');
    expect(t.take()).toEqual([]);
    t.down('D', 'D', 'D');
    expect(t.take()).toEqual([TOGGLE]);
    t.up('D');
    t.down('D');
    expect(t.take()).toEqual([TOGGLE]);
  });

  it('fires whichever key completes the chord last', () => {
    const t = setup({ hold_to_talk: 'F9', toggle: 'Ctrl+Shift+D' });
    t.down('D', 'LShift', 'RCtrl');
    expect(t.take()).toEqual([TOGGLE]);
  });

  it('paste-last requires exactly its modifiers', () => {
    const t = setup();
    t.down('LAlt', 'LShift', 'V');
    expect(t.take()).toEqual([PASTE]);
    t.up('V', 'LShift', 'LAlt');

    t.down('LCtrl', 'LAlt', 'LShift', 'V');
    expect(t.take()).toEqual([]);
    t.up('V', 'LShift', 'LAlt', 'LCtrl');

    t.down('LAlt', 'V');
    expect(t.take()).toEqual([]);
  });

  it('cancel fires only while armed', () => {
    const t = setup();
    t.down('Esc');
    t.up('Esc');
    expect(t.take()).toEqual([]);
    t.matcher.setCancelArmed(true);
    t.down('Esc');
    t.up('Esc');
    expect(t.take()).toEqual([CANCEL]);
    t.matcher.setCancelArmed(false);
    t.down('Esc');
    expect(t.take()).toEqual([]);
  });

  it('Esc does not cancel on Ctrl+Shift+Esc', () => {
    const t = setup();
    t.matcher.setCancelArmed(true);
    t.down('LCtrl', 'LShift', 'Esc');
    expect(t.take()).toEqual([]);
  });

  it('a non-modifier key still down from typing does not block a chord', () => {
    const t = setup();
    t.matcher.setCancelArmed(true);
    t.down('A', 'Esc');
    expect(t.take()).toEqual([CANCEL]);
  });

  it('generic Ctrl matches either side; RCtrl only the right key', () => {
    const generic = setup({ hold_to_talk: 'F9', toggle: 'Ctrl+D' });
    generic.down('RCtrl', 'D');
    expect(generic.take()).toEqual([TOGGLE]);

    const sided = setup({ hold_to_talk: 'F9', toggle: 'RCtrl+D' });
    sided.down('LCtrl', 'D');
    expect(sided.take()).toEqual([]);
    sided.up('D', 'LCtrl');
    sided.down('RCtrl', 'D');
    expect(sided.take()).toEqual([TOGGLE]);
  });

  it('lone-modifier and mouse bindings work for every action', () => {
    const t = setup({ hold_to_talk: 'F9', toggle: 'RCtrl', cancel: 'Mouse3', paste_last: 'RAlt' });
    t.matcher.setCancelArmed(true);
    t.down('RCtrl');
    t.up('RCtrl');
    t.down('Mouse3');
    t.up('Mouse3');
    t.down('RAlt');
    t.up('RAlt');
    expect(t.take()).toEqual([TOGGLE, CANCEL, PASTE]);
    // The other side, or an extra modifier, does not count.
    t.down('LCtrl');
    t.up('LCtrl');
    t.down('LShift', 'RCtrl');
    expect(t.take()).toEqual([]);
  });

  it('a modifier-plus-mouse chord needs the modifier', () => {
    const t = setup({ toggle: 'Ctrl+Mouse4' });
    t.down('Mouse4');
    t.up('Mouse4');
    expect(t.take()).toEqual([]);
    t.down('RCtrl', 'Mouse4');
    expect(t.take()).toEqual([TOGGLE]);
  });

  it('toggle through the default chord passes through hold-start first', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Space');
    t.advance(120);
    t.up('Space', 'LWin', 'LCtrl');
    expect(t.take()).toEqual([START, TOGGLE, END]);
  });

  it('toggle fires even when the shorter hold chord was aborted', () => {
    const t = setup();
    t.down('LCtrl', 'LWin', 'Left');
    t.up('Left');
    t.down('Space');
    expect(t.take()).toEqual([START, ABORT, TOGGLE]);
  });
});

describe('bindings', () => {
  it('an unparsable or empty binding disables only that action', () => {
    const t = setup({ hold_to_talk: 'Ctrl+Nope', toggle: '', paste_last: 'Alt+Shift+V' });
    t.down('LCtrl', 'LWin');
    t.up('LWin', 'LCtrl');
    t.down('LCtrl', 'LWin', 'Space');
    t.up('Space', 'LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
    t.down('LAlt', 'LShift', 'V');
    expect(t.take()).toEqual([PASTE]);
  });

  it('a disabled hold leaves the other actions working', () => {
    const t = setup({ hold_to_talk: '' });
    t.down('LCtrl', 'LWin', 'Space');
    expect(t.take()).toEqual([TOGGLE]);
  });

  it('setBindings takes effect immediately', () => {
    const t = setup();
    t.matcher.setBindings({ ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'F9', toggle: 'F10' });
    t.down('LCtrl', 'LWin');
    t.up('LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
    t.down('F9');
    t.advance(500);
    t.up('F9');
    t.down('F10');
    expect(t.take()).toEqual([START, END, TOGGLE]);
  });

  it('changing the hold binding during a hold ends it as aborted', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.matcher.setBindings({ ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'F9' });
    expect(t.take()).toEqual([START, ABORT]);
    t.up('LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
  });

  it('changing another binding during a hold leaves the hold alone', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    // Same hold binding, written differently.
    t.matcher.setBindings({ ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'win + ctrl', toggle: 'F10' });
    t.advance(1000);
    t.up('LWin');
    expect(t.take()).toEqual([START, END]);
  });

  it('accepts aliases and any token order', () => {
    const t = setup({ hold_to_talk: 'super+control', paste_last: 'v+shift+option' });
    t.down('LCtrl', 'LWin');
    t.advance(1000);
    t.up('LWin', 'LCtrl');
    t.down('RAlt', 'RShift', 'V');
    expect(t.take()).toEqual([START, END, PASTE]);
  });
});

describe('reset and anyHeld', () => {
  it('reset forgets held keys and ends an active hold as aborted', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.matcher.reset();
    expect(t.take()).toEqual([START, ABORT]);
    expect(t.matcher.heldCount).toBe(0);
    // The stale key-ups that follow are harmless.
    t.up('LWin', 'LCtrl');
    expect(t.take()).toEqual([]);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START]);
  });

  it('reset with nothing active emits nothing, and clears a pending double-tap', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.advance(50);
    t.up('LWin', 'LCtrl');
    t.take();
    t.matcher.reset();
    t.advance(50);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START]);
  });

  it('a key-up for a key that was never down is ignored', () => {
    const t = setup();
    t.up('LWin', 'A');
    expect(t.take()).toEqual([]);
    expect(t.matcher.heldCount).toBe(0);
  });

  it('anyHeld is true while any binding key is down', () => {
    const t = setup();
    expect(t.matcher.anyHeld).toBe(false);
    t.down('A');
    expect(t.matcher.anyHeld).toBe(false);
    t.down('LShift');
    // Shift is part of Alt+Shift+V.
    expect(t.matcher.anyHeld).toBe(true);
    t.up('LShift', 'A');
    t.down('LCtrl', 'LWin');
    expect(t.matcher.anyHeld).toBe(true);
    t.up('LWin');
    expect(t.matcher.anyHeld).toBe(true);
    t.up('LCtrl');
    expect(t.matcher.anyHeld).toBe(false);
  });

  it('anyHeld ignores keys of disabled bindings', () => {
    const t = setup({ hold_to_talk: 'F9', toggle: '', cancel: '', paste_last: 'not a key' });
    t.down('LCtrl', 'LShift', 'Esc');
    expect(t.matcher.anyHeld).toBe(false);
    t.down('F9');
    expect(t.matcher.anyHeld).toBe(true);
  });

  it('keeps nothing but the keys currently held', () => {
    const t = setup();
    for (const key of ['A', 'B', 'LShift', 'Enter', 'Mouse4']) {
      t.down(key);
      t.up(key);
    }
    expect(t.matcher.heldCount).toBe(0);
    t.down('A', 'B');
    expect(t.matcher.heldCount).toBe(2);
  });
});

describe('verifyHeld (releases the hook never delivered)', () => {
  it('ends a hold whose release was missed, after two checks', () => {
    const os = new Map<KeyId, boolean | null>([
      ['LCtrl', true],
      ['LWin', true],
    ]);
    const t = setup({}, os);
    t.down('LCtrl', 'LWin');
    t.advance(1000);
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([START]);
    // The user let go while an elevated window had focus: no key-up arrives.
    os.set('LWin', false);
    os.set('LCtrl', false);
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([]);
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([END]);
    expect(t.matcher.heldCount).toBe(0);
  });

  it('clears a stuck foreign key so the hold works again', () => {
    const os = new Map<KeyId, boolean | null>([['Esc', true]]);
    const t = setup({}, os);
    t.down('Esc');
    os.set('Esc', false);
    t.matcher.verifyHeld();
    t.matcher.verifyHeld();
    os.set('LCtrl', true);
    os.set('LWin', true);
    t.down('LCtrl', 'LWin');
    expect(t.take()).toEqual([START]);
  });

  it('a key that reads down again between checks is kept', () => {
    const os = new Map<KeyId, boolean | null>([['F9', true]]);
    const t = setup({ hold_to_talk: 'F9' }, os);
    t.down('F9');
    os.set('F9', false);
    t.matcher.verifyHeld();
    os.set('F9', true);
    t.matcher.verifyHeld();
    os.set('F9', false);
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([START]);
    expect(t.matcher.heldCount).toBe(1);
  });

  it('never drops a key the OS did not see go down (hidden by another tool)', () => {
    const os = new Map<KeyId, boolean | null>([['CapsLock', false]]);
    const t = setup({ hold_to_talk: 'CapsLock' }, os);
    t.down('CapsLock');
    t.matcher.verifyHeld();
    t.matcher.verifyHeld();
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([START]);
    t.advance(1000);
    t.up('CapsLock');
    expect(t.take()).toEqual([END]);
  });

  it('never drops a key the OS cannot report on', () => {
    const os = new Map<KeyId, boolean | null>();
    const t = setup({ hold_to_talk: 'F9' }, os);
    t.down('F9');
    t.matcher.verifyHeld();
    t.matcher.verifyHeld();
    expect(t.matcher.heldCount).toBe(1);
  });

  it('does nothing without an isKeyDown probe', () => {
    const t = setup();
    t.down('LCtrl', 'LWin');
    t.matcher.verifyHeld();
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([START]);
  });

  it('a real key-up between checks is not double-counted', () => {
    const os = new Map<KeyId, boolean | null>([['F9', true]]);
    const t = setup({ hold_to_talk: 'F9' }, os);
    t.down('F9');
    t.advance(1000);
    os.set('F9', false);
    t.matcher.verifyHeld();
    t.up('F9');
    t.matcher.verifyHeld();
    expect(t.take()).toEqual([START, END]);
  });
});
