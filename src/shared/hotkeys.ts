/**
 * Hotkey binding grammar, shared by the input hook (main) and the hotkey
 * recorder (settings). A binding is `+`-joined tokens, e.g. `Ctrl+Win`,
 * `RCtrl`, `Alt+Shift+V`, `Mouse4`.
 */

/** Generic modifiers match either side; `L`/`R` variants match one side only. */
export const GENERIC_MODIFIERS = ['Ctrl', 'Alt', 'Shift', 'Win'] as const;
export type GenericModifier = (typeof GENERIC_MODIFIERS)[number];

export const SIDED_MODIFIERS = [
  'LCtrl',
  'RCtrl',
  'LAlt',
  'RAlt',
  'LShift',
  'RShift',
  'LWin',
  'RWin',
] as const;
export type SidedModifier = (typeof SIDED_MODIFIERS)[number];

export const MOUSE_BUTTONS = ['Mouse3', 'Mouse4', 'Mouse5'] as const;

const NAMED_KEYS = [
  'Space',
  'Esc',
  'Enter',
  'Tab',
  'Backspace',
  'Delete',
  'Insert',
  'Home',
  'End',
  'PageUp',
  'PageDown',
  'Up',
  'Down',
  'Left',
  'Right',
  'CapsLock',
  'ScrollLock',
  'NumLock',
  'PrintScreen',
  'Pause',
  'Minus',
  'Equal',
  'BracketLeft',
  'BracketRight',
  'Backslash',
  'Semicolon',
  'Quote',
  'Comma',
  'Period',
  'Slash',
  'Backquote',
] as const;

const LETTERS = Array.from({ length: 26 }, (_, i) => String.fromCharCode(65 + i));
const DIGITS = Array.from({ length: 10 }, (_, i) => String(i));
const FUNCTION_KEYS = Array.from({ length: 24 }, (_, i) => `F${i + 1}`);
const NUMPAD_KEYS = Array.from({ length: 10 }, (_, i) => `Numpad${i}`);

/** Every physical key id the hook can report. Sided modifiers, never generic ones. */
export const KEY_IDS: readonly string[] = [
  ...SIDED_MODIFIERS,
  ...LETTERS,
  ...DIGITS,
  ...FUNCTION_KEYS,
  ...NUMPAD_KEYS,
  ...NAMED_KEYS,
  ...MOUSE_BUTTONS,
];

/** A physical key as reported by the hook, e.g. `LCtrl`, `A`, `Mouse4`. */
export type KeyId = string;

const ALIASES: Record<string, string> = {
  control: 'Ctrl',
  ctl: 'Ctrl',
  option: 'Alt',
  meta: 'Win',
  super: 'Win',
  windows: 'Win',
  cmd: 'Win',
  command: 'Win',
  escape: 'Esc',
  return: 'Enter',
  del: 'Delete',
  ins: 'Insert',
  pgup: 'PageUp',
  pgdn: 'PageDown',
  pagedown: 'PageDown',
  pageup: 'PageUp',
  spacebar: 'Space',
  rightctrl: 'RCtrl',
  leftctrl: 'LCtrl',
  rightalt: 'RAlt',
  leftalt: 'LAlt',
  rightshift: 'RShift',
  leftshift: 'LShift',
  rightwin: 'RWin',
  leftwin: 'LWin',
  mousemiddle: 'Mouse3',
  mouseback: 'Mouse4',
  mouseforward: 'Mouse5',
};

const CANONICAL = new Map<string, string>(
  [...GENERIC_MODIFIERS, ...KEY_IDS].map((t) => [t.toLowerCase(), t]),
);

const MODIFIER_ORDER = [
  'Ctrl',
  'LCtrl',
  'RCtrl',
  'Alt',
  'LAlt',
  'RAlt',
  'Shift',
  'LShift',
  'RShift',
  'Win',
  'LWin',
  'RWin',
];

export interface Binding {
  /** Canonical tokens: modifiers first in a fixed order, then keys. */
  tokens: string[];
}

export function isModifierToken(token: string): boolean {
  return MODIFIER_ORDER.includes(token);
}

/** The generic modifier a sided key belongs to: `LCtrl` → `Ctrl`. */
export function genericOf(key: KeyId): GenericModifier | null {
  if ((SIDED_MODIFIERS as readonly string[]).includes(key)) return key.slice(1) as GenericModifier;
  return null;
}

function canonicalToken(raw: string): string | null {
  const t = raw.trim().toLowerCase().replace(/\s+/g, '');
  if (!t) return null;
  const aliased = ALIASES[t];
  if (aliased) return aliased;
  return CANONICAL.get(t) ?? null;
}

function sortTokens(tokens: string[]): string[] {
  const mods = tokens
    .filter(isModifierToken)
    .sort((a, b) => MODIFIER_ORDER.indexOf(a) - MODIFIER_ORDER.indexOf(b));
  const keys = tokens.filter((t) => !isModifierToken(t));
  return [...new Set([...mods, ...keys])];
}

/** Parses a binding string. Returns null when empty or when any token is unknown. */
export function parseBinding(text: string): Binding | null {
  if (!text || !text.trim()) return null;
  const tokens: string[] = [];
  for (const part of text.split('+')) {
    const token = canonicalToken(part);
    if (!token) return null;
    tokens.push(token);
  }
  if (tokens.length === 0) return null;
  return { tokens: sortTokens(tokens) };
}

export function formatBinding(binding: Binding): string {
  return binding.tokens.join('+');
}

/** Canonical text for a binding string, or null if it does not parse. */
export function normalizeBinding(text: string): string | null {
  const b = parseBinding(text);
  return b ? formatBinding(b) : null;
}

/** Whether a physical key satisfies a binding token (`LCtrl` satisfies `Ctrl`). */
export function keyMatchesToken(key: KeyId, token: string): boolean {
  return key === token || genericOf(key) === token;
}

/** True when every token of the binding is satisfied by a key that is down. */
export function bindingSatisfied(binding: Binding, down: ReadonlySet<KeyId>): boolean {
  return binding.tokens.every((token) => {
    for (const key of down) if (keyMatchesToken(key, token)) return true;
    return false;
  });
}

/** Builds a binding from the set of keys currently held (used by the recorder). */
export function bindingFromKeys(keys: Iterable<KeyId>, preferGeneric = true): Binding {
  const tokens = [...keys].map((k) => (preferGeneric ? (genericOf(k) ?? k) : k));
  return { tokens: sortTokens(tokens) };
}

/** True when `a` contains every token of `b` and more (`Ctrl+Win+Space` ⊃ `Ctrl+Win`). */
export function isSuperset(a: Binding, b: Binding): boolean {
  return (
    a.tokens.length > b.tokens.length &&
    b.tokens.every((t) => a.tokens.some((x) => x === t || genericOf(x) === t))
  );
}

export interface BindingConflict {
  action: string;
  other: string;
  /** `same`: identical bindings. `invalid`: does not parse. */
  kind: 'same' | 'invalid';
}

/**
 * Flags bindings that cannot be told apart. A superset (`Ctrl+Win+Space` over
 * `Ctrl+Win`) is not a conflict: the longer chord wins when it completes.
 */
export function findConflicts(bindings: Record<string, string>): BindingConflict[] {
  const conflicts: BindingConflict[] = [];
  const entries = Object.entries(bindings);
  for (const [action, text] of entries) {
    if (!parseBinding(text)) conflicts.push({ action, other: action, kind: 'invalid' });
  }
  for (let i = 0; i < entries.length; i++) {
    for (let j = i + 1; j < entries.length; j++) {
      const a = normalizeBinding(entries[i]![1]);
      const b = normalizeBinding(entries[j]![1]);
      if (a && b && a === b) {
        conflicts.push({ action: entries[i]![0], other: entries[j]![0], kind: 'same' });
        conflicts.push({ action: entries[j]![0], other: entries[i]![0], kind: 'same' });
      }
    }
  }
  return conflicts;
}

const DISPLAY: Record<string, string> = {
  LCtrl: 'Left Ctrl',
  RCtrl: 'Right Ctrl',
  LAlt: 'Left Alt',
  RAlt: 'Right Alt',
  LShift: 'Left Shift',
  RShift: 'Right Shift',
  LWin: 'Left Win',
  RWin: 'Right Win',
  Mouse3: 'Middle click',
  Mouse4: 'Mouse back',
  Mouse5: 'Mouse forward',
  PageUp: 'Page Up',
  PageDown: 'Page Down',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→',
};

/** Human-readable key caps for a binding string, e.g. `["Ctrl", "Win"]`. */
export function displayTokens(text: string): string[] {
  const b = parseBinding(text);
  if (!b) return text ? [text] : [];
  return b.tokens.map((t) => DISPLAY[t] ?? t);
}
