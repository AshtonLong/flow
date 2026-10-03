import type { Platform } from './types';

/**
 * Used when koffi cannot load or the OS has no implementation yet. Nothing can
 * be learnt about windows or keys, so the inserter pastes blind through uiohook.
 */
export class NullPlatform implements Platform {
  readonly available = false;
  readonly selfElevated = false;

  /** Why the real implementation is not in use, for the "degraded" list. */
  constructor(readonly reason: string = 'unsupported platform') {}

  getFocusedApp(): null {
    return null;
  }
  isWindowAlive(): boolean {
    return true;
  }
  focusWindow(): boolean {
    return false;
  }
  isTextTarget(): boolean {
    return true;
  }
  typeText(): Promise<void> {
    return Promise.reject(new Error('typing is unavailable on this platform'));
  }
  sendShortcut(): boolean {
    return false;
  }
  waitForModifiersUp(): Promise<void> {
    return Promise.resolve();
  }
  isKeyDown(): null {
    return null;
  }
  maskStartMenu(): void {}
  writeClipboardExcluded(): boolean {
    return false;
  }
}
