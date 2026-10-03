/**
 * A ledger of key events this process is about to inject (the paste shortcut,
 * Enter while typing). uiohook does not expose the "injected" flag, so the
 * input hook asks this ledger instead and drops the echo of our own keys
 * before they reach the hotkey matcher.
 */
import { type KeyId, isModifierToken, parseBinding } from '@shared/hotkeys';

interface Expected {
  key: KeyId;
  down: boolean;
  expires: number;
}

export class InjectedKeys {
  private pending: Expected[] = [];

  constructor(
    private readonly now: () => number = () => performance.now(),
    /** An echo that has not arrived by then never will (the hook is stopped). */
    private readonly ttlMs = 1000,
  ) {}

  /** Call just before injecting a key event. */
  expect(key: KeyId, down: boolean): void {
    const t = this.now();
    if (this.pending.length > 0 && this.pending[0]!.expires <= t) {
      this.pending = this.pending.filter((e) => e.expires > t);
    }
    this.pending.push({ key, down, expires: t + this.ttlMs });
  }

  /** True when the event is the echo of one we injected; the entry is used up. */
  consume(key: KeyId, down: boolean): boolean {
    if (this.pending.length === 0) return false;
    const t = this.now();
    const index = this.pending.findIndex((e) => e.key === key && e.down === down && e.expires > t);
    if (index < 0) return false;
    this.pending.splice(index, 1);
    return true;
  }

  clear(): void {
    this.pending = [];
  }

  get size(): number {
    return this.pending.length;
  }
}

/** Shared by the platform (which injects) and the input hook (which filters). */
export const injectedKeys = new InjectedKeys();

const LEFT_OF: Record<string, KeyId> = { Ctrl: 'LCtrl', Alt: 'LAlt', Shift: 'LShift', Win: 'LWin' };

/**
 * The physical keys to press for a shortcut such as `Ctrl+Shift+V`, in press
 * order (modifiers first; generic modifiers use the left key). Null when the
 * binding does not parse or contains a mouse button.
 */
export function shortcutKeys(binding: string): KeyId[] | null {
  const parsed = parseBinding(binding);
  if (!parsed) return null;
  const keys = parsed.tokens.map((t) => (isModifierToken(t) ? (LEFT_OF[t] ?? t) : t));
  if (keys.some((k) => k.startsWith('Mouse'))) return null;
  return keys;
}
