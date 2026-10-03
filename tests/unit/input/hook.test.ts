import { EventEmitter } from 'node:events';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_CONFIG } from '@shared/config';
import type { KeyId } from '@shared/hotkeys';
import { InputHook, type UiohookLike } from '../../../src/main/input/hook';
import { keycodeFromKeyId } from '../../../src/main/input/keymap';
import type { HotkeyEvent } from '../../../src/main/input/matcher';
import { InjectedKeys } from '../../../src/main/platform/injected';

class FakeUiohook extends EventEmitter implements UiohookLike {
  running = false;
  taps: { key: number; modifiers: number[] }[] = [];
  start(): void {
    this.running = true;
  }
  stop(): void {
    this.running = false;
  }
  keyTap(key: number, modifiers: number[] = []): void {
    this.taps.push({ key, modifiers });
  }
  press(key: KeyId): void {
    this.emit('keydown', { keycode: keycodeFromKeyId(key) });
  }
  release(key: KeyId): void {
    this.emit('keyup', { keycode: keycodeFromKeyId(key) });
  }
}

function setup(deps: ConstructorParameters<typeof InputHook>[0] = {}) {
  const uiohook = new FakeUiohook();
  const injected = new InjectedKeys();
  const hook = new InputHook({ injected, loadUiohook: () => uiohook, ...deps });
  const events: HotkeyEvent[] = [];
  hook.on('hotkey', (e) => events.push(e));
  return { uiohook, injected, hook, events, take: () => events.splice(0) };
}

const START = { type: 'hold-start' };
const END = { type: 'hold-end', aborted: false };
const ABORT = { type: 'hold-end', aborted: true };

describe('InputHook', () => {
  it('starts the hook and emits hotkey events with the default bindings', () => {
    const t = setup();
    expect(t.hook.start()).toBe(true);
    expect(t.uiohook.running).toBe(true);
    t.uiohook.press('LCtrl');
    t.uiohook.press('LWin');
    expect(t.hook.anyHeld).toBe(true);
    t.uiohook.release('LWin');
    t.uiohook.release('LCtrl');
    expect(t.take()).toEqual([START, END]);
    expect(t.hook.anyHeld).toBe(false);
  });

  it('start is idempotent and stop detaches', () => {
    const t = setup();
    expect(t.hook.start()).toBe(true);
    expect(t.hook.start()).toBe(true);
    expect(t.uiohook.listenerCount('keydown')).toBe(1);
    t.hook.stop();
    expect(t.uiohook.running).toBe(false);
    expect(t.uiohook.listenerCount('keydown')).toBe(0);
    expect(t.uiohook.listenerCount('mouseup')).toBe(0);
    t.hook.stop();
  });

  it('returns false and stays inert when uiohook fails to load', async () => {
    const hook = new InputHook({
      loadUiohook: () => {
        throw new Error('no prebuilt binary');
      },
    });
    expect(hook.start()).toBe(false);
    expect(hook.loadError).toBe('no prebuilt binary');
    expect(hook.anyHeld).toBe(false);
    expect(hook.tapShortcut('Ctrl+V')).toBe(false);
    await expect(hook.captureBinding()).resolves.toBeNull();
    hook.setBindings(DEFAULT_CONFIG.hotkeys);
    hook.setSuspended(true);
    hook.reset();
    hook.stop();
  });

  it('returns false when the hook thread cannot start', () => {
    const uiohook = new FakeUiohook();
    uiohook.start = () => {
      throw new Error('hook refused');
    };
    const hook = new InputHook({ loadUiohook: () => uiohook });
    expect(hook.start()).toBe(false);
    expect(hook.loadError).toBe('hook refused');
    expect(uiohook.listenerCount('keydown')).toBe(0);
  });

  it('maps mouse buttons and ignores left/right click and unknown keys', () => {
    const t = setup();
    t.hook.setBindings({ ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'Mouse4' });
    t.hook.start();
    t.uiohook.emit('mousedown', { button: 4 });
    // Clicking, a media key and an id-less key during the hold do not abort it.
    t.uiohook.emit('mousedown', { button: 1 });
    t.uiohook.emit('mouseup', { button: 1 });
    t.uiohook.emit('keydown', { keycode: 0xe020 });
    t.uiohook.emit('keydown', { keycode: 0 });
    t.uiohook.emit('keyup', { keycode: 0 });
    t.uiohook.emit('mouseup', { button: 4 });
    expect(t.take()).toEqual([START, END]);
  });

  it('setBindings and setCancelArmed reach the matcher', () => {
    const t = setup();
    t.hook.start();
    t.hook.setBindings({ ...DEFAULT_CONFIG.hotkeys, hold_to_talk: 'F9' });
    t.uiohook.press('Esc');
    t.uiohook.release('Esc');
    t.hook.setCancelArmed(true);
    t.uiohook.press('Esc');
    t.uiohook.release('Esc');
    t.uiohook.press('F9');
    expect(t.take()).toEqual([{ type: 'cancel' }, START]);
  });

  describe('suspension', () => {
    it('emits nothing while suspended and resumes cleanly', () => {
      const t = setup();
      t.hook.start();
      t.hook.setSuspended(true);
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.uiohook.release('LWin');
      t.uiohook.release('LCtrl');
      expect(t.take()).toEqual([]);
      expect(t.hook.anyHeld).toBe(false);
      t.hook.setSuspended(false);
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      expect(t.take()).toEqual([START]);
    });

    it('ends a hold in progress as aborted when suspended', () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.hook.setSuspended(true);
      t.uiohook.release('LWin');
      expect(t.take()).toEqual([START, ABORT]);
    });

    it('reset ends a hold in progress as aborted', () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.hook.reset();
      expect(t.take()).toEqual([START, ABORT]);
      expect(t.hook.anyHeld).toBe(false);
    });
  });

  describe('Start-menu masking', () => {
    it('masks when a binding containing Win is engaged', () => {
      const maskStartMenu = vi.fn();
      const t = setup({ maskStartMenu });
      t.hook.start();
      t.uiohook.press('LCtrl');
      expect(maskStartMenu).not.toHaveBeenCalled();
      t.uiohook.press('LWin');
      expect(maskStartMenu).toHaveBeenCalledTimes(1);
      t.uiohook.press('Space'); // toggle: Ctrl+Win+Space
      expect(maskStartMenu).toHaveBeenCalledTimes(2);
      t.uiohook.release('Space');
      t.uiohook.release('LWin');
      expect(maskStartMenu).toHaveBeenCalledTimes(2);
    });

    it('masks lone Alt bindings and sided Win, but not bindings without them', () => {
      const maskStartMenu = vi.fn();
      const t = setup({ maskStartMenu });
      t.hook.setBindings({
        hold_to_talk: 'RAlt',
        toggle: 'RWin',
        cancel: 'Esc',
        paste_last: 'Ctrl+Shift+V',
        double_tap_lock: false,
      });
      t.hook.setCancelArmed(true);
      t.hook.start();
      for (const key of ['Esc', 'LCtrl', 'LShift', 'V']) t.uiohook.press(key);
      for (const key of ['V', 'LShift', 'LCtrl', 'Esc']) t.uiohook.release(key);
      expect(t.take()).toEqual([{ type: 'cancel' }, { type: 'paste-last' }]);
      expect(maskStartMenu).not.toHaveBeenCalled();
      t.uiohook.press('RAlt');
      t.uiohook.release('RAlt');
      t.uiohook.press('RWin');
      expect(maskStartMenu).toHaveBeenCalledTimes(2);
    });

    it('a failing mask does not stop the event', () => {
      const t = setup({
        maskStartMenu: () => {
          throw new Error('SendInput failed');
        },
      });
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      expect(t.take()).toEqual([START]);
    });
  });

  describe('injected keys', () => {
    it('ignores the echo of keys this process injected', () => {
      const t = setup();
      t.hook.setBindings({ ...DEFAULT_CONFIG.hotkeys, toggle: 'Ctrl+V' });
      t.hook.start();
      for (const down of [true, false]) {
        t.injected.expect('LCtrl', down);
        t.injected.expect('V', down);
      }
      t.uiohook.press('LCtrl');
      t.uiohook.press('V');
      t.uiohook.release('V');
      t.uiohook.release('LCtrl');
      expect(t.take()).toEqual([]);
      expect(t.hook.anyHeld).toBe(false);
      // The same keys pressed for real do fire.
      t.uiohook.press('LCtrl');
      t.uiohook.press('V');
      expect(t.take()).toEqual([{ type: 'toggle' }]);
    });

    it('an injected Enter while typing does not abort a hold', () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.injected.expect('Enter', true);
      t.injected.expect('Enter', false);
      t.uiohook.press('Enter');
      t.uiohook.release('Enter');
      t.uiohook.release('LWin');
      expect(t.take()).toEqual([START, END]);
    });

    it('an injected release does not end a hold the user is physically keeping', () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.injected.expect('LCtrl', true);
      t.injected.expect('LCtrl', false);
      t.uiohook.press('LCtrl');
      t.uiohook.release('LCtrl');
      expect(t.take()).toEqual([START]);
    });
  });

  describe('tapShortcut', () => {
    it('taps the key with its modifiers held and expects the echo', () => {
      const t = setup();
      expect(t.hook.tapShortcut('Ctrl+Shift+V')).toBe(true);
      expect(t.uiohook.taps).toEqual([
        {
          key: keycodeFromKeyId('V'),
          modifiers: [keycodeFromKeyId('LCtrl'), keycodeFromKeyId('LShift')],
        },
      ]);
      expect(t.injected.size).toBe(6);
      expect(t.injected.consume('LShift', true)).toBe(true);
      expect(t.injected.consume('V', false)).toBe(true);
    });

    it('works without the hook running, and for a single key', () => {
      const t = setup();
      expect(t.hook.tapShortcut('Enter')).toBe(true);
      expect(t.uiohook.taps).toEqual([{ key: keycodeFromKeyId('Enter'), modifiers: [] }]);
    });

    it('refuses bindings it cannot send', () => {
      const t = setup();
      expect(t.hook.tapShortcut('')).toBe(false);
      expect(t.hook.tapShortcut('Ctrl+Nope')).toBe(false);
      expect(t.hook.tapShortcut('Ctrl+Mouse4')).toBe(false);
      expect(t.uiohook.taps).toEqual([]);
      expect(t.injected.size).toBe(0);
    });

    it('returns false when keyTap throws', () => {
      const t = setup();
      t.uiohook.keyTap = () => {
        throw new Error('nope');
      };
      expect(t.hook.tapShortcut('Ctrl+V')).toBe(false);
    });
  });

  describe('captureBinding', () => {
    it('reports progress and resolves with the largest chord once all keys are up', async () => {
      const t = setup();
      t.hook.start();
      const progress: string[] = [];
      const result = t.hook.captureBinding((p) => progress.push(p));
      t.uiohook.press('LCtrl');
      t.uiohook.press('LCtrl'); // auto-repeat
      t.uiohook.press('LShift');
      t.uiohook.press('D');
      t.uiohook.release('D');
      t.uiohook.release('LShift');
      t.uiohook.release('LCtrl');
      await expect(result).resolves.toBe('Ctrl+Shift+D');
      expect(progress).toEqual(['LCtrl', 'Ctrl+Shift', 'Ctrl+Shift+D']);
    });

    it('suspends dictation events while recording and resumes after', async () => {
      const t = setup();
      t.hook.start();
      const result = t.hook.captureBinding();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      t.uiohook.release('LWin');
      t.uiohook.release('LCtrl');
      await expect(result).resolves.toBe('Ctrl+Win');
      expect(t.take()).toEqual([]);
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      expect(t.take()).toEqual([START]);
    });

    it('records a lone modifier sided and a mouse button by name', async () => {
      const t = setup();
      t.hook.start();
      let result = t.hook.captureBinding();
      t.uiohook.press('RCtrl');
      t.uiohook.release('RCtrl');
      await expect(result).resolves.toBe('RCtrl');

      result = t.hook.captureBinding();
      t.uiohook.emit('mousedown', { button: 4 });
      t.uiohook.emit('mouseup', { button: 4 });
      await expect(result).resolves.toBe('Mouse4');

      result = t.hook.captureBinding();
      t.uiohook.press('Esc');
      t.uiohook.release('Esc');
      await expect(result).resolves.toBe('Esc');
    });

    it('keeps the largest chord when keys are released one by one or re-pressed', async () => {
      const t = setup();
      t.hook.start();
      const result = t.hook.captureBinding();
      t.uiohook.press('LAlt');
      t.uiohook.press('LShift');
      t.uiohook.press('V');
      t.uiohook.release('V');
      t.uiohook.press('B'); // same size: the latest chord wins
      t.uiohook.release('LAlt');
      t.uiohook.release('B');
      t.uiohook.release('LShift');
      await expect(result).resolves.toBe('Alt+Shift+B');
    });

    it('ignores the release of a key that was down before recording began, and clicks', async () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('Enter');
      const result = t.hook.captureBinding();
      t.uiohook.release('Enter');
      t.uiohook.emit('mousedown', { button: 1 });
      t.uiohook.emit('mouseup', { button: 1 });
      t.uiohook.press('F9');
      t.uiohook.release('F9');
      await expect(result).resolves.toBe('F9');
    });

    it('resolves null on cancelCapture, and a new capture cancels the previous one', async () => {
      const t = setup();
      t.hook.start();
      const first = t.hook.captureBinding();
      const second = t.hook.captureBinding();
      await expect(first).resolves.toBeNull();
      t.uiohook.press('F9');
      t.hook.cancelCapture();
      await expect(second).resolves.toBeNull();
      t.hook.cancelCapture();
      // Dictation is live again, with no keys remembered from the capture.
      t.uiohook.release('F9');
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      expect(t.take()).toEqual([START]);
    });

    it('resolves null when the hook stops mid-capture', async () => {
      const t = setup();
      t.hook.start();
      const result = t.hook.captureBinding();
      t.hook.stop();
      await expect(result).resolves.toBeNull();
    });

    it('ends a hold in progress when recording begins', () => {
      const t = setup();
      t.hook.start();
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      void t.hook.captureBinding();
      expect(t.take()).toEqual([START, ABORT]);
      t.hook.cancelCapture();
    });
  });

  describe('missed key releases', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('ends a hold whose key-up never arrived, then stops checking', () => {
      const os = new Map<KeyId, boolean>();
      const isKeyDown = vi.fn((key: KeyId) => os.get(key) ?? false);
      const t = setup({ isKeyDown });
      t.hook.start();
      os.set('LCtrl', true).set('LWin', true);
      t.uiohook.press('LCtrl');
      t.uiohook.press('LWin');
      vi.advanceTimersByTime(1000);
      expect(t.take()).toEqual([START]);
      // Released while an elevated window had focus: the hook sees nothing.
      os.clear();
      vi.advanceTimersByTime(600);
      expect(t.take()).toEqual([END]);
      expect(t.hook.anyHeld).toBe(false);
      const calls = isKeyDown.mock.calls.length;
      vi.advanceTimersByTime(5000);
      expect(isKeyDown.mock.calls.length).toBe(calls);
    });

    it('does not poll while no key is held', () => {
      const isKeyDown = vi.fn(() => true);
      const t = setup({ isKeyDown });
      t.hook.start();
      vi.advanceTimersByTime(5000);
      expect(isKeyDown).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
      t.uiohook.press('A');
      t.uiohook.release('A');
      vi.advanceTimersByTime(5000);
      expect(vi.getTimerCount()).toBe(0);
    });

    it('stop clears the timer', () => {
      const t = setup({ isKeyDown: () => true });
      t.hook.start();
      t.uiohook.press('A');
      expect(vi.getTimerCount()).toBe(1);
      t.hook.stop();
      expect(vi.getTimerCount()).toBe(0);
    });
  });
});
