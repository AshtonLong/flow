/**
 * The global input hook: uiohook-napi key and mouse events, mapped to key ids
 * and fed to the hotkey matcher. Events are compared against the bindings and
 * discarded; nothing is stored or logged.
 */
import { EventEmitter } from 'node:events';
import { createRequire } from 'node:module';
import { type Config, DEFAULT_CONFIG, type HotkeyAction } from '@shared/config';
import {
  type KeyId,
  bindingFromKeys,
  formatBinding,
  isModifierToken,
  parseBinding,
} from '@shared/hotkeys';
import { type InjectedKeys, injectedKeys, shortcutKeys } from '../platform/injected';
import { keyIdFromKeycode, keyIdFromMouseButton, keycodeFromKeyId } from './keymap';
import { type HotkeyEvent, HotkeyMatcher } from './matcher';

/** The part of uiohook-napi's `uIOhook` this wrapper uses. */
export interface UiohookLike {
  on(event: string, listener: (event: { keycode?: number; button?: unknown }) => void): unknown;
  off(event: string, listener: (event: { keycode?: number; button?: unknown }) => void): unknown;
  start(): void;
  stop(): void;
  keyTap(key: number, modifiers?: number[]): void;
}

export interface InputHookDeps {
  /** Called when a binding containing Win or Alt is engaged, while the modifier is still down. */
  maskStartMenu?: () => void;
  /** `Platform.isKeyDown`: lets the hook notice key releases it was never sent. */
  isKeyDown?: (key: KeyId) => boolean | null;
  /** Keys this process injects, to be ignored. Defaults to the ledger the platform writes to. */
  injected?: InjectedKeys;
  /** Test seam: supplies the uiohook instance instead of loading the native module. */
  loadUiohook?: () => UiohookLike;
}

interface Capture {
  held: Set<KeyId>;
  best: KeyId[];
  onProgress?: (partial: string) => void;
  resolve(binding: string | null): void;
}

/** While keys are held, how often they are checked against the OS for missed releases. */
const VERIFY_INTERVAL_MS = 250;

const ACTION_OF: Record<HotkeyEvent['type'], HotkeyAction | null> = {
  'hold-start': 'hold_to_talk',
  'hold-end': null,
  lock: 'hold_to_talk',
  toggle: 'toggle',
  cancel: 'cancel',
  'paste-last': 'paste_last',
};

/** A lone modifier is recorded sided (`RCtrl`): a generic lone `Ctrl` would fire on every Ctrl+C. */
function chordText(keys: KeyId[]): string {
  return formatBinding(bindingFromKeys(keys, keys.length > 1));
}

function loadNativeUiohook(): UiohookLike {
  const require = createRequire(import.meta.url);
  return (require('uiohook-napi') as { uIOhook: UiohookLike }).uIOhook;
}

/** Emits `hotkey` with a `HotkeyEvent`. */
export class InputHook extends EventEmitter<{ hotkey: [HotkeyEvent] }> {
  private readonly deps: InputHookDeps;
  private readonly injected: InjectedKeys;
  private readonly matcher: HotkeyMatcher;
  private bindings: Config['hotkeys'] = DEFAULT_CONFIG.hotkeys;
  private uiohook: UiohookLike | null = null;
  private started = false;
  private suspended = false;
  private capture: Capture | null = null;
  private verifyTimer: NodeJS.Timeout | null = null;
  private loadFailure: string | null = null;

  private readonly onKeyDown = (e: { keycode?: number }) =>
    this.handle(keyIdFromKeycode(e.keycode ?? 0), true);
  private readonly onKeyUp = (e: { keycode?: number }) =>
    this.handle(keyIdFromKeycode(e.keycode ?? 0), false);
  private readonly onMouseDown = (e: { button?: unknown }) =>
    this.handle(keyIdFromMouseButton(e.button), true);
  private readonly onMouseUp = (e: { button?: unknown }) =>
    this.handle(keyIdFromMouseButton(e.button), false);

  constructor(deps: InputHookDeps = {}) {
    super();
    this.deps = deps;
    this.injected = deps.injected ?? injectedKeys;
    this.matcher = new HotkeyMatcher({
      bindings: this.bindings,
      emit: (event) => this.dispatch(event),
      isKeyDown: deps.isKeyDown,
    });
  }

  /** Starts the global hook. Returns false (and stays inert) if uiohook-napi fails to load. */
  start(): boolean {
    if (this.started) return true;
    const uiohook = this.load();
    if (!uiohook) return false;
    try {
      uiohook.on('keydown', this.onKeyDown);
      uiohook.on('keyup', this.onKeyUp);
      uiohook.on('mousedown', this.onMouseDown);
      uiohook.on('mouseup', this.onMouseUp);
      uiohook.start();
    } catch (error) {
      this.loadFailure = error instanceof Error ? error.message : String(error);
      this.detach(uiohook);
      return false;
    }
    this.started = true;
    return true;
  }

  stop(): void {
    this.cancelCapture();
    this.stopVerifying();
    if (this.started && this.uiohook) {
      this.started = false;
      this.detach(this.uiohook);
      try {
        this.uiohook.stop();
      } catch {
        // The hook thread is already gone.
      }
    }
    this.matcher.reset();
  }

  /** Why `start()` returned false, for the "degraded" list. */
  get loadError(): string | null {
    return this.loadFailure;
  }

  setBindings(bindings: Config['hotkeys']): void {
    this.bindings = bindings;
    this.matcher.setBindings(bindings);
  }

  setCancelArmed(armed: boolean): void {
    this.matcher.setCancelArmed(armed);
  }

  /**
   * Paused: no events are emitted (tray "Pause dictation", and while recording
   * a binding). A hold in progress is ended first, as aborted.
   */
  setSuspended(suspended: boolean): void {
    if (suspended && !this.suspended) this.matcher.reset();
    this.suspended = suspended;
  }

  /** Forget all held keys (after sleep/resume). A hold in progress ends as aborted. */
  reset(): void {
    this.matcher.reset();
  }

  /** True while any binding's keys are partially or fully held. */
  get anyHeld(): boolean {
    return this.matcher.anyHeld;
  }

  /**
   * Records the next chord for the settings hotkey recorder: tracks keys as
   * they go down, reports the partial chord through onProgress, and resolves
   * with the largest chord held once every key is released. Dictation events
   * are suspended meanwhile. Resolves null if cancelCapture() is called or the
   * hook is not running.
   */
  captureBinding(onProgress?: (partial: string) => void): Promise<string | null> {
    this.cancelCapture();
    if (!this.started) return Promise.resolve(null);
    this.matcher.reset();
    return new Promise((resolve) => {
      this.capture = { held: new Set(), best: [], onProgress, resolve };
    });
  }

  cancelCapture(): void {
    this.finishCapture(null);
  }

  /** Simulates a shortcut such as `Ctrl+V` with uiohook's keyTap (fallback when koffi is unavailable). */
  tapShortcut(binding: string): boolean {
    const keys = shortcutKeys(binding);
    const uiohook = this.load();
    if (!keys || keys.length === 0 || !uiohook) return false;
    const codes: number[] = [];
    for (const key of keys) {
      const code = keycodeFromKeyId(key);
      if (code === null) return false;
      codes.push(code);
    }
    try {
      // Presses the modifiers, taps the key, then releases the modifiers in reverse.
      uiohook.keyTap(codes[codes.length - 1]!, codes.slice(0, -1));
    } catch {
      return false;
    }
    // The echo arrives on a later turn of the event loop, so this is early enough.
    for (const key of keys) this.injected.expect(key, true);
    for (const key of keys) this.injected.expect(key, false);
    return true;
  }

  private load(): UiohookLike | null {
    if (this.uiohook) return this.uiohook;
    try {
      this.uiohook = (this.deps.loadUiohook ?? loadNativeUiohook)();
      return this.uiohook;
    } catch (error) {
      this.loadFailure = error instanceof Error ? error.message : String(error);
      return null;
    }
  }

  private detach(uiohook: UiohookLike): void {
    uiohook.off('keydown', this.onKeyDown);
    uiohook.off('keyup', this.onKeyUp);
    uiohook.off('mousedown', this.onMouseDown);
    uiohook.off('mouseup', this.onMouseUp);
  }

  private handle(key: KeyId | null, down: boolean): void {
    // Keys without an id are ignored outright: they cannot be bound, and some
    // (IME keys) never send a release, which would leave them "held".
    if (key === null) return;
    if (this.injected.consume(key, down)) return;
    if (this.capture) {
      this.captureKey(this.capture, key, down);
      return;
    }
    if (this.suspended) return;
    if (down) {
      this.matcher.keyDown(key);
      this.startVerifying();
    } else {
      this.matcher.keyUp(key);
    }
  }

  private dispatch(event: HotkeyEvent): void {
    const action = ACTION_OF[event.type];
    if (action && this.needsMask(this.bindings[action])) {
      try {
        this.deps.maskStartMenu?.();
      } catch {
        // Masking is cosmetic; the hotkey still works.
      }
    }
    this.emit('hotkey', event);
  }

  /** Releasing Win alone opens the Start menu, and Alt alone focuses the menu bar, unless another key was pressed meanwhile. */
  private needsMask(binding: string): boolean {
    const tokens = parseBinding(binding)?.tokens ?? [];
    return tokens.some((t) => isModifierToken(t) && (t.endsWith('Win') || t.endsWith('Alt')));
  }

  private captureKey(capture: Capture, key: KeyId, down: boolean): void {
    if (down) {
      if (capture.held.has(key)) return;
      capture.held.add(key);
      if (capture.held.size >= capture.best.length) capture.best = [...capture.held];
      capture.onProgress?.(chordText([...capture.held]));
      return;
    }
    // Ignores the release of a key pressed before recording began (the Enter that clicked "Record").
    if (!capture.held.delete(key)) return;
    if (capture.held.size === 0) this.finishCapture(chordText(capture.best));
  }

  private finishCapture(result: string | null): void {
    const capture = this.capture;
    if (!capture) return;
    this.capture = null;
    capture.resolve(result);
  }

  /** Runs only while keys are held, so an idle keyboard costs nothing. */
  private startVerifying(): void {
    if (this.verifyTimer || !this.deps.isKeyDown) return;
    this.verifyTimer = setInterval(() => {
      this.matcher.verifyHeld();
      if (this.matcher.heldCount === 0) this.stopVerifying();
    }, VERIFY_INTERVAL_MS);
    this.verifyTimer.unref();
  }

  private stopVerifying(): void {
    if (this.verifyTimer) clearInterval(this.verifyTimer);
    this.verifyTimer = null;
  }
}
