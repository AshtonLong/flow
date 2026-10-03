/** Windows virtual-key codes for every `KeyId`. Pure data: no native module is loaded here. */
import type { KeyId } from '@shared/hotkeys';

const NAMED: Record<string, number> = {
  LShift: 0xa0,
  RShift: 0xa1,
  LCtrl: 0xa2,
  RCtrl: 0xa3,
  LAlt: 0xa4,
  RAlt: 0xa5,
  LWin: 0x5b,
  RWin: 0x5c,
  Space: 0x20,
  Esc: 0x1b,
  Enter: 0x0d,
  Tab: 0x09,
  Backspace: 0x08,
  Delete: 0x2e,
  Insert: 0x2d,
  Home: 0x24,
  End: 0x23,
  PageUp: 0x21,
  PageDown: 0x22,
  Up: 0x26,
  Down: 0x28,
  Left: 0x25,
  Right: 0x27,
  CapsLock: 0x14,
  ScrollLock: 0x91,
  NumLock: 0x90,
  PrintScreen: 0x2c,
  Pause: 0x13,
  Minus: 0xbd,
  Equal: 0xbb,
  BracketLeft: 0xdb,
  BracketRight: 0xdd,
  Backslash: 0xdc,
  Semicolon: 0xba,
  Quote: 0xde,
  Comma: 0xbc,
  Period: 0xbe,
  Slash: 0xbf,
  Backquote: 0xc0,
  // Mouse buttons have virtual-key codes too; they are only valid for GetAsyncKeyState.
  Mouse3: 0x04,
  Mouse4: 0x05,
  Mouse5: 0x06,
};

function build(): Map<KeyId, number> {
  const map = new Map<KeyId, number>(Object.entries(NAMED));
  for (let i = 0; i < 26; i++) map.set(String.fromCharCode(65 + i), 0x41 + i);
  for (let i = 0; i < 10; i++) map.set(String(i), 0x30 + i);
  for (let i = 0; i < 24; i++) map.set(`F${i + 1}`, 0x70 + i);
  for (let i = 0; i < 10; i++) map.set(`Numpad${i}`, 0x60 + i);
  return map;
}

const VK = build();

/** The virtual-key code for a key id, or null when it has none. */
export function vkFromKeyId(key: KeyId): number | null {
  return VK.get(key) ?? null;
}

/**
 * Keys that need KEYEVENTF_EXTENDEDKEY when injected. Without it Windows reads
 * Insert/arrows/etc. as their numpad twins, which breaks e.g. `Shift+Insert`.
 */
const EXTENDED = new Set<KeyId>([
  'RCtrl',
  'RAlt',
  'LWin',
  'RWin',
  'Insert',
  'Delete',
  'Home',
  'End',
  'PageUp',
  'PageDown',
  'Up',
  'Down',
  'Left',
  'Right',
  'NumLock',
  'PrintScreen',
]);

export function isExtendedKey(key: KeyId): boolean {
  return EXTENDED.has(key);
}

export const VK_MASK_KEY = 0xe8;
export const MODIFIER_VKS = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c] as const;
