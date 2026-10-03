import type { KeyId } from '@shared/hotkeys';
import type { FocusedApp } from '@shared/types';

/** Everything OS-specific the dictation path needs: the focused window, simulated input, the clipboard. */
export interface Platform {
  /** False when koffi failed to load or the OS is unsupported: paste still works, typing and profiles are disabled. */
  readonly available: boolean;
  /** Whether this process is elevated. */
  readonly selfElevated: boolean;
  /** Foreground window, executable name, title, pid and whether it is elevated. */
  getFocusedApp(): FocusedApp | null;
  isWindowAlive(windowId: string): boolean;
  /** Best-effort SetForegroundWindow. True when the window ends up in the foreground. */
  focusWindow(windowId: string): boolean;
  /** False for the desktop, taskbar/shell windows or no foreground window: there is nowhere to type. */
  isTextTarget(app: FocusedApp | null): boolean;
  /**
   * Types text with SendInput KEYEVENTF_UNICODE (newlines as Enter), in chunks.
   * Stops early and resolves when the signal aborts; rejects if input is blocked.
   */
  typeText(text: string, signal?: AbortSignal): Promise<void>;
  /** Presses and releases a shortcut such as `Ctrl+V` or `Ctrl+Shift+V` with SendInput. */
  sendShortcut(binding: string): boolean;
  /** Resolves once Ctrl/Alt/Shift/Win are all physically up (GetAsyncKeyState), after timeoutMs, or on abort. */
  waitForModifiersUp(timeoutMs?: number, signal?: AbortSignal): Promise<void>;
  /** Whether a key or mouse button is down right now. Null when it cannot be told. */
  isKeyDown(key: KeyId): boolean | null;
  /** Injects a no-op key (VK 0xE8, as AutoHotkey does) so releasing Win does not open the Start menu. */
  maskStartMenu(): void;
  /**
   * Puts text on the clipboard flagged to stay out of clipboard history and cloud clipboard
   * (ExcludeClipboardContentFromMonitorProcessing, CanIncludeInClipboardHistory=0, CanUploadToCloudClipboard=0).
   * Returns false if it could not, so the caller falls back to a plain clipboard write.
   */
  writeClipboardExcluded(text: string): boolean;
}
