import { describe, expect, it } from 'vitest';
import { KEY_IDS, SIDED_MODIFIERS } from '@shared/hotkeys';
import { NullPlatform, createPlatform } from '../../../src/main/platform';
import { InjectedKeys } from '../../../src/main/platform/injected';
import { MODIFIER_VKS, isExtendedKey, vkFromKeyId } from '../../../src/main/platform/vk';

describe('virtual-key table', () => {
  it('has a distinct code for every key id', () => {
    const codes = KEY_IDS.map((key) => [key, vkFromKeyId(key)] as const);
    expect(codes.filter(([, vk]) => vk === null)).toEqual([]);
    expect(new Set(codes.map(([, vk]) => vk)).size).toBe(KEY_IDS.length);
  });

  it('matches the Win32 constants', () => {
    expect(vkFromKeyId('A')).toBe(0x41);
    expect(vkFromKeyId('Z')).toBe(0x5a);
    expect(vkFromKeyId('0')).toBe(0x30);
    expect(vkFromKeyId('F1')).toBe(0x70);
    expect(vkFromKeyId('F24')).toBe(0x87);
    expect(vkFromKeyId('Numpad9')).toBe(0x69);
    expect(vkFromKeyId('V')).toBe(0x56);
    expect(vkFromKeyId('LCtrl')).toBe(0xa2);
    expect(vkFromKeyId('Enter')).toBe(0x0d);
    expect(vkFromKeyId('Insert')).toBe(0x2d);
    expect(vkFromKeyId('Mouse4')).toBe(0x05);
  });

  it('has no code for generic modifiers or unknown ids', () => {
    expect(vkFromKeyId('Ctrl')).toBeNull();
    expect(vkFromKeyId('toString')).toBeNull();
  });

  it('lists every sided modifier for the "modifiers up" check', () => {
    expect([...MODIFIER_VKS].sort()).toEqual(SIDED_MODIFIERS.map((k) => vkFromKeyId(k)).sort());
  });

  it('flags the keys that need KEYEVENTF_EXTENDEDKEY', () => {
    for (const key of ['Insert', 'Delete', 'Home', 'End', 'Up', 'RCtrl', 'RAlt', 'LWin']) {
      expect(isExtendedKey(key), key).toBe(true);
    }
    for (const key of ['LCtrl', 'LShift', 'RShift', 'LAlt', 'V', 'Enter', 'Numpad0']) {
      expect(isExtendedKey(key), key).toBe(false);
    }
  });
});

describe('NullPlatform', () => {
  it('reports itself unavailable and degrades every call', async () => {
    const platform = new NullPlatform('koffi failed to load');
    expect(platform.available).toBe(false);
    expect(platform.reason).toBe('koffi failed to load');
    expect(platform.selfElevated).toBe(false);
    expect(platform.getFocusedApp()).toBeNull();
    expect(platform.isWindowAlive()).toBe(true);
    expect(platform.focusWindow()).toBe(false);
    expect(platform.isTextTarget()).toBe(true);
    expect(platform.sendShortcut()).toBe(false);
    expect(platform.isKeyDown()).toBeNull();
    expect(platform.writeClipboardExcluded()).toBe(false);
    platform.maskStartMenu();
    await expect(platform.waitForModifiersUp()).resolves.toBeUndefined();
    await expect(platform.typeText()).rejects.toThrow(/unavailable/);
  });
});

describe('createPlatform', () => {
  it('never throws and returns a usable platform', () => {
    const platform = createPlatform({ injected: new InjectedKeys() });
    expect(typeof platform.available).toBe('boolean');
    if (process.platform !== 'win32') expect(platform.available).toBe(false);
  });
});

// Read-only Win32 calls against the real desktop. Nothing here sends input or
// touches the clipboard.
describe.runIf(process.platform === 'win32')('Win32Platform (read-only calls)', () => {
  const platform = createPlatform({ injected: new InjectedKeys() });

  it('loads koffi with a correctly sized INPUT struct', () => {
    // createPlatform falls back to NullPlatform when sizeof(INPUT) is not 40 (28 on ia32).
    expect(platform.available).toBe(true);
    expect(typeof platform.selfElevated).toBe('boolean');
  });

  it('describes the foreground window, when there is one', () => {
    const app = platform.getFocusedApp();
    if (!app) return; // a headless CI session has no foreground window
    expect(app.windowId).toMatch(/^\d+$/);
    expect(app.pid).toBeGreaterThan(0);
    expect(typeof app.title).toBe('string');
    expect(typeof app.elevated).toBe('boolean');
    if (!app.elevated) expect(app.processName).toMatch(/\.exe$/i);
    expect(platform.isWindowAlive(app.windowId)).toBe(true);
    expect(typeof platform.isTextTarget(app)).toBe('boolean');
  });

  it('rejects window ids that are not live windows', () => {
    for (const id of ['', '0', 'abc', '-5', '1e3']) {
      expect(platform.isWindowAlive(id), id).toBe(false);
      expect(platform.focusWindow(id), id).toBe(false);
    }
    expect(platform.isTextTarget(null)).toBe(false);
    expect(platform.isTextTarget({ windowId: 'abc', processName: '', title: '', pid: 0 })).toBe(
      false,
    );
  });

  it('reads key state and refuses shortcuts it cannot send', () => {
    expect(typeof platform.isKeyDown('F24')).toBe('boolean');
    expect(typeof platform.isKeyDown('Mouse5')).toBe('boolean');
    expect(platform.isKeyDown('NotAKey')).toBeNull();
    expect(platform.sendShortcut('')).toBe(false);
    expect(platform.sendShortcut('Ctrl+Nope')).toBe(false);
    expect(platform.sendShortcut('Ctrl+Mouse4')).toBe(false);
  });

  it('waitForModifiersUp honours its timeout and an abort', async () => {
    const started = performance.now();
    await platform.waitForModifiersUp(60);
    const aborted = new AbortController();
    aborted.abort();
    await platform.waitForModifiersUp(60, aborted.signal);
    expect(performance.now() - started).toBeLessThan(1000);
  });

  it('typing nothing sends nothing', async () => {
    await expect(platform.typeText('')).resolves.toBeUndefined();
  });
});
