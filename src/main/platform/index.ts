import type { InjectedKeys } from './injected';
import { NullPlatform } from './null';
import type { Platform } from './types';
import { createWin32Platform } from './win32';

export type { Platform } from './types';
export { NullPlatform } from './null';
export { InjectedKeys, injectedKeys, shortcutKeys } from './injected';

export interface PlatformOptions {
  /** The ledger of injected keys the input hook filters against. Defaults to the shared one. */
  injected?: InjectedKeys;
}

/**
 * The Win32 implementation via koffi, else a null implementation with
 * `available: false` (koffi failed to load, or the OS has no implementation).
 */
export function createPlatform(options: PlatformOptions = {}): Platform {
  if (process.platform !== 'win32')
    return new NullPlatform(`no implementation for ${process.platform}`);
  try {
    return createWin32Platform(options.injected);
  } catch (error) {
    return new NullPlatform(error instanceof Error ? error.message : String(error));
  }
}
