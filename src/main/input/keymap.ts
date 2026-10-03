/**
 * uiohook keycode ↔ `KeyId`. The codes are libuiohook's `VC_*` constants,
 * written out here so this table can be used without loading the native module.
 */
import type { KeyId } from '@shared/hotkeys';

const NAMED: Record<string, number> = {
  LCtrl: 0x001d,
  RCtrl: 0x0e1d,
  LAlt: 0x0038,
  RAlt: 0x0e38,
  LShift: 0x002a,
  RShift: 0x0036,
  LWin: 0x0e5b,
  RWin: 0x0e5c,

  Space: 0x0039,
  Esc: 0x0001,
  Enter: 0x001c,
  Tab: 0x000f,
  Backspace: 0x000e,
  Delete: 0x0e53,
  Insert: 0x0e52,
  Home: 0x0e47,
  End: 0x0e4f,
  PageUp: 0x0e49,
  PageDown: 0x0e51,
  Up: 0xe048,
  Down: 0xe050,
  Left: 0xe04b,
  Right: 0xe04d,
  CapsLock: 0x003a,
  ScrollLock: 0x0046,
  NumLock: 0x0045,
  PrintScreen: 0x0e37,
  Pause: 0x0e45,
  Minus: 0x000c,
  Equal: 0x000d,
  BracketLeft: 0x001a,
  BracketRight: 0x001b,
  Backslash: 0x002b,
  Semicolon: 0x0027,
  Quote: 0x0028,
  Comma: 0x0033,
  Period: 0x0034,
  Slash: 0x0035,
  Backquote: 0x0029,

  A: 0x001e,
  B: 0x0030,
  C: 0x002e,
  D: 0x0020,
  E: 0x0012,
  F: 0x0021,
  G: 0x0022,
  H: 0x0023,
  I: 0x0017,
  J: 0x0024,
  K: 0x0025,
  L: 0x0026,
  M: 0x0032,
  N: 0x0031,
  O: 0x0018,
  P: 0x0019,
  Q: 0x0010,
  R: 0x0013,
  S: 0x001f,
  T: 0x0014,
  U: 0x0016,
  V: 0x002f,
  W: 0x0011,
  X: 0x002d,
  Y: 0x0015,
  Z: 0x002c,

  '1': 0x0002,
  '2': 0x0003,
  '3': 0x0004,
  '4': 0x0005,
  '5': 0x0006,
  '6': 0x0007,
  '7': 0x0008,
  '8': 0x0009,
  '9': 0x000a,
  '0': 0x000b,

  F1: 0x003b,
  F2: 0x003c,
  F3: 0x003d,
  F4: 0x003e,
  F5: 0x003f,
  F6: 0x0040,
  F7: 0x0041,
  F8: 0x0042,
  F9: 0x0043,
  F10: 0x0044,
  F11: 0x0057,
  F12: 0x0058,
  F13: 0x005b,
  F14: 0x005c,
  F15: 0x005d,
  F16: 0x0063,
  F17: 0x0064,
  F18: 0x0065,
  F19: 0x0066,
  F20: 0x0067,
  F21: 0x0068,
  F22: 0x0069,
  F23: 0x006a,
  F24: 0x006b,

  Numpad0: 0x0052,
  Numpad1: 0x004f,
  Numpad2: 0x0050,
  Numpad3: 0x0051,
  Numpad4: 0x004b,
  Numpad5: 0x004c,
  Numpad6: 0x004d,
  Numpad7: 0x0047,
  Numpad8: 0x0048,
  Numpad9: 0x0049,
};

/**
 * Other codes for the same physical key. With NumLock off the numpad reports
 * navigation codes (`0xEE00 | digit code`; the 5 key reports "clear"), and the
 * numpad Enter has its own code.
 */
const ALTERNATES: Record<string, number[]> = {
  Numpad0: [0xee52],
  Numpad1: [0xee4f],
  Numpad2: [0xee50],
  Numpad3: [0xee51],
  Numpad4: [0xee4b],
  Numpad5: [0xee4c, 0xe04c],
  Numpad6: [0xee4d],
  Numpad7: [0xee47],
  Numpad8: [0xee48],
  Numpad9: [0xee49],
  Enter: [0x0e1c],
};

/** uiohook mouse button numbers. Left (1) and right (2) are not bindable. */
const MOUSE: Record<number, KeyId> = { 3: 'Mouse3', 4: 'Mouse4', 5: 'Mouse5' };

const BY_KEY = new Map<KeyId, number>(Object.entries(NAMED));
const BY_CODE = new Map<number, KeyId>();
for (const [key, code] of BY_KEY) BY_CODE.set(code, key);
for (const [key, codes] of Object.entries(ALTERNATES)) {
  for (const code of codes) BY_CODE.set(code, key);
}

/** `KEY_IDS` entries the hook can never report. Every other id must be in the tables above. */
export const UNSUPPORTED_KEY_IDS: readonly KeyId[] = [];

/**
 * The key for a uiohook keycode. Null for keys Flow has no id for — including
 * code 0, which is what injected Unicode characters and the Start-menu mask
 * key (VK 0xE8) arrive as.
 */
export function keyIdFromKeycode(keycode: number): KeyId | null {
  return BY_CODE.get(keycode) ?? null;
}

/** The key for a uiohook mouse button, or null for left/right click. */
export function keyIdFromMouseButton(button: unknown): KeyId | null {
  return typeof button === 'number' ? (MOUSE[button] ?? null) : null;
}

/** The uiohook keycode to simulate a key with. Null for mouse buttons and unknown ids. */
export function keycodeFromKeyId(key: KeyId): number | null {
  return BY_KEY.get(key) ?? null;
}

/** Whether the hook can report this key id at all. */
export function isMappedKeyId(key: KeyId): boolean {
  return BY_KEY.has(key) || Object.values(MOUSE).includes(key);
}
