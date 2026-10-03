import { createRequire } from 'node:module';
import { describe, expect, it } from 'vitest';
import { KEY_IDS, MOUSE_BUTTONS, SIDED_MODIFIERS, parseBinding } from '@shared/hotkeys';
import {
  UNSUPPORTED_KEY_IDS,
  isMappedKeyId,
  keyIdFromKeycode,
  keyIdFromMouseButton,
  keycodeFromKeyId,
} from '../../../src/main/input/keymap';

/** uiohook-napi's own key table, when the native module loads in this environment. */
function loadUiohookKeys(): Record<string, number> | null {
  try {
    const require = createRequire(import.meta.url);
    return (require('uiohook-napi') as { UiohookKey: Record<string, number> }).UiohookKey;
  } catch {
    return null;
  }
}

describe('keymap', () => {
  it('maps every KEY_IDS entry, or lists it as unsupported', () => {
    const unaccounted = KEY_IDS.filter(
      (key) => !isMappedKeyId(key) && !UNSUPPORTED_KEY_IDS.includes(key),
    );
    expect(unaccounted).toEqual([]);
  });

  it('lists nothing as unsupported that is in fact mapped, and only real key ids', () => {
    for (const key of UNSUPPORTED_KEY_IDS) {
      expect(KEY_IDS).toContain(key);
      expect(isMappedKeyId(key)).toBe(false);
    }
  });

  it('round-trips every keyboard key through its keycode', () => {
    const keyboardKeys = KEY_IDS.filter(
      (key) => !(MOUSE_BUTTONS as readonly string[]).includes(key) && isMappedKeyId(key),
    );
    expect(keyboardKeys.length).toBeGreaterThan(100);
    for (const key of keyboardKeys) {
      const code = keycodeFromKeyId(key);
      expect(code, key).not.toBeNull();
      expect(keyIdFromKeycode(code!), key).toBe(key);
    }
  });

  it('gives every key a distinct keycode', () => {
    const codes = KEY_IDS.map(keycodeFromKeyId).filter((c): c is number => c !== null);
    expect(new Set(codes).size).toBe(codes.length);
  });

  it('only produces ids the binding grammar understands', () => {
    for (let code = 0; code <= 0xffff; code++) {
      const key = keyIdFromKeycode(code);
      if (key === null) continue;
      expect(KEY_IDS).toContain(key);
      expect(parseBinding(key)?.tokens).toEqual([key]);
    }
  });

  it('reports sided modifiers, never generic ones', () => {
    expect(keyIdFromKeycode(0x001d)).toBe('LCtrl');
    expect(keyIdFromKeycode(0x0e1d)).toBe('RCtrl');
    expect(keyIdFromKeycode(0x0038)).toBe('LAlt');
    expect(keyIdFromKeycode(0x0e38)).toBe('RAlt');
    expect(keyIdFromKeycode(0x002a)).toBe('LShift');
    expect(keyIdFromKeycode(0x0036)).toBe('RShift');
    expect(keyIdFromKeycode(0x0e5b)).toBe('LWin');
    expect(keyIdFromKeycode(0x0e5c)).toBe('RWin');
    for (const key of SIDED_MODIFIERS) expect(isMappedKeyId(key)).toBe(true);
  });

  it('maps mouse buttons 3/4/5 and ignores left and right click', () => {
    expect(keyIdFromMouseButton(1)).toBeNull();
    expect(keyIdFromMouseButton(2)).toBeNull();
    expect(keyIdFromMouseButton(3)).toBe('Mouse3');
    expect(keyIdFromMouseButton(4)).toBe('Mouse4');
    expect(keyIdFromMouseButton(5)).toBe('Mouse5');
    expect(keyIdFromMouseButton(6)).toBeNull();
    expect(keyIdFromMouseButton(undefined)).toBeNull();
    expect(keyIdFromMouseButton('4')).toBeNull();
    expect(keycodeFromKeyId('Mouse4')).toBeNull();
  });

  it('maps the numpad the same with NumLock on or off', () => {
    // NumLock off: the numpad reports navigation codes, 0xEE00 | the digit's code.
    expect(keyIdFromKeycode(0x004f)).toBe('Numpad1');
    expect(keyIdFromKeycode(0xee4f)).toBe('Numpad1');
    expect(keyIdFromKeycode(0x0052)).toBe('Numpad0');
    expect(keyIdFromKeycode(0xee52)).toBe('Numpad0');
    expect(keyIdFromKeycode(0xe04c)).toBe('Numpad5');
    // The dedicated navigation keys stay themselves.
    expect(keyIdFromKeycode(0x0e4f)).toBe('End');
    expect(keyIdFromKeycode(0xe048)).toBe('Up');
    expect(keyIdFromKeycode(0x0e1c)).toBe('Enter');
  });

  it('returns null for unknown codes, including 0 (injected Unicode and the mask key)', () => {
    expect(keyIdFromKeycode(0)).toBeNull();
    expect(keyIdFromKeycode(0x0e5d)).toBeNull(); // context menu
    expect(keyIdFromKeycode(0xe020)).toBeNull(); // volume mute
    expect(keyIdFromKeycode(-1)).toBeNull();
    expect(keycodeFromKeyId('toString')).toBeNull();
    expect(keycodeFromKeyId('Ctrl')).toBeNull();
    expect(isMappedKeyId('constructor')).toBe(false);
  });

  it('agrees with uiohook-napi’s own key table', (ctx) => {
    const UiohookKey = loadUiohookKeys();
    if (!UiohookKey) return ctx.skip();
    const names: Record<string, string> = {
      Ctrl: 'LCtrl',
      CtrlRight: 'RCtrl',
      Alt: 'LAlt',
      AltRight: 'RAlt',
      Shift: 'LShift',
      ShiftRight: 'RShift',
      Meta: 'LWin',
      MetaRight: 'RWin',
      Escape: 'Esc',
      ArrowLeft: 'Left',
      ArrowRight: 'Right',
      ArrowUp: 'Up',
      ArrowDown: 'Down',
      NumpadEnter: 'Enter',
      NumpadEnd: 'Numpad1',
      NumpadArrowDown: 'Numpad2',
      NumpadPageDown: 'Numpad3',
      NumpadArrowLeft: 'Numpad4',
      NumpadArrowRight: 'Numpad6',
      NumpadHome: 'Numpad7',
      NumpadArrowUp: 'Numpad8',
      NumpadPageUp: 'Numpad9',
      NumpadInsert: 'Numpad0',
    };
    // Keys uiohook knows that the binding grammar has no id for.
    const noId = new Set([
      'NumpadMultiply',
      'NumpadAdd',
      'NumpadSubtract',
      'NumpadDecimal',
      'NumpadDivide',
      'NumpadDelete',
    ]);
    let checked = 0;
    for (const [name, code] of Object.entries(UiohookKey)) {
      if (noId.has(name)) {
        expect(keyIdFromKeycode(code), name).toBeNull();
        continue;
      }
      expect(keyIdFromKeycode(code), name).toBe(names[name] ?? name);
      checked++;
    }
    expect(checked).toBeGreaterThan(100);
  });
});
