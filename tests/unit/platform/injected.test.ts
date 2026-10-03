import { describe, expect, it } from 'vitest';
import { InjectedKeys, shortcutKeys } from '../../../src/main/platform/injected';

describe('InjectedKeys', () => {
  it('consumes each expected event once', () => {
    const keys = new InjectedKeys();
    keys.expect('LCtrl', true);
    keys.expect('V', true);
    keys.expect('V', false);
    keys.expect('LCtrl', false);
    expect(keys.consume('LCtrl', true)).toBe(true);
    expect(keys.consume('LCtrl', true)).toBe(false);
    expect(keys.consume('V', true)).toBe(true);
    expect(keys.consume('V', false)).toBe(true);
    expect(keys.consume('LCtrl', false)).toBe(true);
    expect(keys.size).toBe(0);
  });

  it('tells down from up and one key from another', () => {
    const keys = new InjectedKeys();
    keys.expect('Enter', true);
    expect(keys.consume('Enter', false)).toBe(false);
    expect(keys.consume('Tab', true)).toBe(false);
    expect(keys.consume('Enter', true)).toBe(true);
  });

  it('matches out of order and handles repeats of the same key', () => {
    const keys = new InjectedKeys();
    keys.expect('Enter', true);
    keys.expect('Enter', false);
    keys.expect('Enter', true);
    keys.expect('Enter', false);
    expect(keys.consume('Enter', false)).toBe(true);
    expect(keys.consume('Enter', true)).toBe(true);
    expect(keys.consume('Enter', true)).toBe(true);
    expect(keys.consume('Enter', false)).toBe(true);
    expect(keys.consume('Enter', false)).toBe(false);
  });

  it('forgets an echo that never arrived, so a later real key is not swallowed', () => {
    let time = 0;
    const keys = new InjectedKeys(() => time, 1000);
    keys.expect('V', true);
    time = 999;
    expect(keys.consume('V', true)).toBe(true);
    keys.expect('V', true);
    time = 2000;
    expect(keys.consume('V', true)).toBe(false);
    // Expired entries are dropped the next time something is expected.
    keys.expect('A', true);
    expect(keys.size).toBe(1);
  });

  it('clear drops everything', () => {
    const keys = new InjectedKeys();
    keys.expect('V', true);
    keys.clear();
    expect(keys.consume('V', true)).toBe(false);
  });
});

describe('shortcutKeys', () => {
  it('orders modifiers first and uses the left key for generic modifiers', () => {
    expect(shortcutKeys('Ctrl+V')).toEqual(['LCtrl', 'V']);
    expect(shortcutKeys('v + shift + ctrl')).toEqual(['LCtrl', 'LShift', 'V']);
    expect(shortcutKeys('Shift+Insert')).toEqual(['LShift', 'Insert']);
    expect(shortcutKeys('Alt+Win+V')).toEqual(['LAlt', 'LWin', 'V']);
  });

  it('keeps sided modifiers and lone keys', () => {
    expect(shortcutKeys('RCtrl+V')).toEqual(['RCtrl', 'V']);
    expect(shortcutKeys('Enter')).toEqual(['Enter']);
  });

  it('rejects unparsable bindings and mouse buttons', () => {
    expect(shortcutKeys('')).toBeNull();
    expect(shortcutKeys('Ctrl+Nope')).toBeNull();
    expect(shortcutKeys('Ctrl+Mouse4')).toBeNull();
  });
});
