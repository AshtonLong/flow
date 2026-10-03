import { describe, expect, it } from 'vitest';
import {
  bindingFromKeys,
  bindingSatisfied,
  displayTokens,
  findConflicts,
  formatBinding,
  isSuperset,
  keyMatchesToken,
  normalizeBinding,
  parseBinding,
} from '@shared/hotkeys';

describe('parseBinding', () => {
  it.each([
    ['Ctrl+Win', 'Ctrl+Win'],
    ['win + ctrl', 'Ctrl+Win'],
    ['Ctrl+Win+Space', 'Ctrl+Win+Space'],
    ['alt+shift+v', 'Alt+Shift+V'],
    ['Esc', 'Esc'],
    ['escape', 'Esc'],
    ['RCtrl', 'RCtrl'],
    ['right ctrl', 'RCtrl'],
    ['Mouse4', 'Mouse4'],
    ['control+f9', 'Ctrl+F9'],
    ['Shift+Ctrl+Alt+K', 'Ctrl+Alt+Shift+K'],
  ])('normalises %s to %s', (input, expected) => {
    expect(normalizeBinding(input)).toBe(expected);
  });

  it.each(['', '   ', 'Ctrl+', 'Ctrl+Banana', '+'])('rejects %j', (input) => {
    expect(parseBinding(input)).toBeNull();
  });

  it('round-trips through formatBinding', () => {
    const binding = parseBinding('Ctrl+Win+Space')!;
    expect(parseBinding(formatBinding(binding))).toEqual(binding);
  });
});

describe('matching', () => {
  it('lets either side satisfy a generic modifier', () => {
    expect(keyMatchesToken('LCtrl', 'Ctrl')).toBe(true);
    expect(keyMatchesToken('RCtrl', 'Ctrl')).toBe(true);
    expect(keyMatchesToken('LCtrl', 'RCtrl')).toBe(false);
    expect(keyMatchesToken('A', 'A')).toBe(true);
  });

  it('is satisfied only when every token is held', () => {
    const binding = parseBinding('Ctrl+Win')!;
    expect(bindingSatisfied(binding, new Set(['LCtrl']))).toBe(false);
    expect(bindingSatisfied(binding, new Set(['LCtrl', 'LWin']))).toBe(true);
    expect(bindingSatisfied(binding, new Set(['RCtrl', 'RWin']))).toBe(true);
  });

  it('builds a binding from held keys', () => {
    expect(formatBinding(bindingFromKeys(['LWin', 'Space', 'LCtrl']))).toBe('Ctrl+Win+Space');
    expect(formatBinding(bindingFromKeys(['RCtrl'], false))).toBe('RCtrl');
  });

  it('recognises a longer chord as a superset', () => {
    const hold = parseBinding('Ctrl+Win')!;
    const toggle = parseBinding('Ctrl+Win+Space')!;
    expect(isSuperset(toggle, hold)).toBe(true);
    expect(isSuperset(hold, toggle)).toBe(false);
    expect(isSuperset(hold, hold)).toBe(false);
    expect(isSuperset(parseBinding('LCtrl+Win+Space')!, hold)).toBe(true);
  });
});

describe('findConflicts', () => {
  it('accepts the defaults', () => {
    expect(
      findConflicts({
        hold_to_talk: 'Ctrl+Win',
        toggle: 'Ctrl+Win+Space',
        cancel: 'Esc',
        paste_last: 'Alt+Shift+V',
      }),
    ).toEqual([]);
  });

  it('flags identical bindings on both actions', () => {
    const conflicts = findConflicts({ hold_to_talk: 'F9', toggle: 'f9', cancel: 'Esc' });
    expect(conflicts).toEqual([
      { action: 'hold_to_talk', other: 'toggle', kind: 'same' },
      { action: 'toggle', other: 'hold_to_talk', kind: 'same' },
    ]);
  });

  it('flags a binding that does not parse', () => {
    expect(findConflicts({ cancel: 'Nope' })).toEqual([
      { action: 'cancel', other: 'cancel', kind: 'invalid' },
    ]);
  });
});

describe('displayTokens', () => {
  it('spells out sided modifiers and mouse buttons', () => {
    expect(displayTokens('RCtrl')).toEqual(['Right Ctrl']);
    expect(displayTokens('Ctrl+Win')).toEqual(['Ctrl', 'Win']);
    expect(displayTokens('Mouse4')).toEqual(['Mouse back']);
  });
});
