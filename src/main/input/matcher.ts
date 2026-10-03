/**
 * The hotkey state machine. It is fed physical key-downs and key-ups and emits
 * dictation events. Pure: no timers, no native calls, and no memory of keys
 * beyond the set currently held.
 */
import type { Config } from '@shared/config';
import {
  type Binding,
  type KeyId,
  bindingSatisfied,
  formatBinding,
  genericOf,
  keyMatchesToken,
  parseBinding,
} from '@shared/hotkeys';

export type HotkeyEvent =
  | { type: 'hold-start' }
  /** `aborted`: a foreign key was pressed during the hold, so it was a different shortcut. */
  | { type: 'hold-end'; aborted: boolean }
  /** Double-tap of the hold binding: start hands-free. */
  | { type: 'lock' }
  | { type: 'toggle' }
  | { type: 'cancel' }
  | { type: 'paste-last' };

export interface MatcherOptions {
  bindings: Config['hotkeys'];
  emit(event: HotkeyEvent): void;
  /** Milliseconds, injectable for tests. */
  now?: () => number;
  /** A second press this soon after a tap's release is a double-tap. Default 350. */
  doubleTapMs?: number;
  /** A press shorter than this counts as a tap. Default 300. */
  tapMaxMs?: number;
  /**
   * Asks the OS whether a key is down (null: cannot tell). Used by
   * `verifyHeld` to notice releases the hook never delivered.
   */
  isKeyDown?: (key: KeyId) => boolean | null;
}

type EdgeEvent = { type: 'toggle' } | { type: 'cancel' } | { type: 'paste-last' };

function matchesBinding(key: KeyId, binding: Binding): boolean {
  return binding.tokens.some((token) => keyMatchesToken(key, token));
}

function sameBinding(a: Binding | null, b: Binding | null): boolean {
  if (!a || !b) return a === b;
  return formatBinding(a) === formatBinding(b);
}

export class HotkeyMatcher {
  private readonly emit: (event: HotkeyEvent) => void;
  private readonly now: () => number;
  private readonly doubleTapMs: number;
  private readonly tapMaxMs: number;
  private readonly isKeyDown?: (key: KeyId) => boolean | null;

  private hold: Binding | null = null;
  private toggle: Binding | null = null;
  private cancel: Binding | null = null;
  private pasteLast: Binding | null = null;
  private doubleTapLock = true;
  private cancelArmed = false;

  private readonly down = new Set<KeyId>();
  /** Held keys the OS agreed were down, so a later "up" from it can be trusted. */
  private readonly confirmed = new Set<KeyId>();
  /** Held keys the OS reported up at the last `verifyHeld`. */
  private readonly suspects = new Set<KeyId>();

  /** `spent`: the hold keys are still down but this press no longer counts (locked or aborted). */
  private holdState: 'idle' | 'active' | 'spent' = 'idle';
  private holdStartedAt = 0;
  private lastTapAt: number | null = null;

  constructor(options: MatcherOptions) {
    this.emit = options.emit;
    this.now = options.now ?? (() => performance.now());
    this.doubleTapMs = options.doubleTapMs ?? 350;
    this.tapMaxMs = options.tapMaxMs ?? 300;
    this.isKeyDown = options.isKeyDown;
    this.setBindings(options.bindings);
  }

  /** An unparsable or empty binding disables that action only. */
  setBindings(bindings: Config['hotkeys']): void {
    const hold = parseBinding(bindings.hold_to_talk);
    const holdChanged = !sameBinding(hold, this.hold);
    this.hold = hold;
    this.toggle = parseBinding(bindings.toggle);
    this.cancel = parseBinding(bindings.cancel);
    this.pasteLast = parseBinding(bindings.paste_last);
    this.doubleTapLock = bindings.double_tap_lock;
    if (!holdChanged) return;
    // The keys being held no longer mean "hold to talk".
    const wasActive = this.holdState === 'active';
    this.holdState = 'idle';
    this.lastTapAt = null;
    if (wasActive) this.emit({ type: 'hold-end', aborted: true });
  }

  /** `cancel` only fires while armed (the session is recording or transcribing). */
  setCancelArmed(armed: boolean): void {
    this.cancelArmed = armed;
  }

  /** Auto-repeat downs for a key already held are ignored. */
  keyDown(key: KeyId): void {
    if (this.down.has(key)) return;
    this.down.add(key);
    if (this.isKeyDown?.(key) === true) this.confirmed.add(key);

    const inHold = this.hold !== null && matchesBinding(key, this.hold);

    if (this.holdState === 'active') {
      if (inHold) return;
      // A key outside the hold binding either completes a longer chord
      // (Ctrl+Win, then Space for toggle) or means the user is doing some
      // other shortcut (Ctrl+Win+Left) and is not dictating.
      const event = this.edgeEvent(key, true);
      if (event) {
        this.emit(event);
        return;
      }
      this.holdState = 'spent';
      this.lastTapAt = null;
      this.emit({ type: 'hold-end', aborted: true });
      return;
    }

    if (!inHold) this.lastTapAt = null;

    const event = this.edgeEvent(key, false);
    if (event) {
      this.emit(event);
      return;
    }

    // Hold starts only on the key-down that completes it, and only when
    // nothing else is down: releasing a foreign key must not start a recording.
    if (inHold && this.holdState === 'idle' && this.exactly(this.hold!)) {
      const t = this.now();
      const doubleTap =
        this.doubleTapLock && this.lastTapAt !== null && t - this.lastTapAt <= this.doubleTapMs;
      this.lastTapAt = null;
      if (doubleTap) {
        this.holdState = 'spent';
        this.emit({ type: 'lock' });
      } else {
        this.holdState = 'active';
        this.holdStartedAt = t;
        this.emit({ type: 'hold-start' });
      }
    }
  }

  keyUp(key: KeyId): void {
    this.confirmed.delete(key);
    this.suspects.delete(key);
    if (!this.down.delete(key)) return;
    if (this.holdState === 'idle' || !this.hold) return;
    // With a generic modifier the other side may still satisfy the binding.
    if (bindingSatisfied(this.hold, this.down)) return;
    const wasActive = this.holdState === 'active';
    this.holdState = 'idle';
    if (!wasActive) return;
    const t = this.now();
    const tapped = this.doubleTapLock && t - this.holdStartedAt < this.tapMaxMs;
    this.lastTapAt = tapped ? t : null;
    this.emit({ type: 'hold-end', aborted: false });
  }

  /**
   * Releases held keys the OS says are up. The hook misses key-ups that go to
   * an elevated window or the lock screen, which would otherwise leave a key
   * "held" forever. A key is only dropped when the OS saw it down earlier and
   * reports it up on two calls in a row, so a key-up still queued behind this
   * call, or a key another tool hides from the OS, is never dropped by mistake.
   */
  verifyHeld(): void {
    if (!this.isKeyDown) return;
    for (const key of [...this.down]) {
      const state = this.isKeyDown(key);
      if (state === true) {
        this.confirmed.add(key);
        this.suspects.delete(key);
      } else if (state === false && this.confirmed.has(key)) {
        if (this.suspects.has(key)) this.keyUp(key);
        else this.suspects.add(key);
      }
    }
  }

  /** Forget all held keys (after sleep/resume, or when the hook restarts). Ends an active hold as aborted. */
  reset(): void {
    const wasActive = this.holdState === 'active';
    this.down.clear();
    this.confirmed.clear();
    this.suspects.clear();
    this.holdState = 'idle';
    this.lastTapAt = null;
    if (wasActive) this.emit({ type: 'hold-end', aborted: true });
  }

  /** True while any binding's keys are partially or fully held (used to delay paste until released). */
  get anyHeld(): boolean {
    const bindings = [this.hold, this.toggle, this.cancel, this.pasteLast];
    for (const key of this.down) {
      if (bindings.some((b) => b !== null && matchesBinding(key, b))) return true;
    }
    return false;
  }

  /** How many keys are currently down. */
  get heldCount(): number {
    return this.down.size;
  }

  /** The edge-triggered action this key-down completes, if any. */
  private edgeEvent(key: KeyId, duringHold: boolean): EdgeEvent | null {
    const candidates: [Binding | null, EdgeEvent][] = [
      [this.cancelArmed ? this.cancel : null, { type: 'cancel' }],
      [this.toggle, { type: 'toggle' }],
      [this.pasteLast, { type: 'paste-last' }],
    ];
    for (const [binding, event] of candidates) {
      if (!binding || !matchesBinding(key, binding)) continue;
      if (!bindingSatisfied(binding, this.down)) continue;
      if (this.modifiersExact(binding, null)) return event;
      // The hold's own keys are not "extra" modifiers: Esc still cancels while Ctrl+Win is held.
      if (duringHold && this.modifiersExact(binding, this.hold)) return event;
    }
    return null;
  }

  /** No modifier is held beyond those the binding (or `allow`) names: Esc is not Ctrl+Shift+Esc. */
  private modifiersExact(binding: Binding, allow: Binding | null): boolean {
    for (const key of this.down) {
      if (genericOf(key) === null) continue;
      if (matchesBinding(key, binding)) continue;
      if (allow && matchesBinding(key, allow)) continue;
      return false;
    }
    return true;
  }

  /** Every token is down and no other key is. */
  private exactly(binding: Binding): boolean {
    if (!bindingSatisfied(binding, this.down)) return false;
    for (const key of this.down) if (!matchesBinding(key, binding)) return false;
    return true;
  }
}
