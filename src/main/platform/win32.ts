/**
 * The Windows implementation of `Platform`: every Win32 call Flow makes goes
 * through koffi from this file. Handles (HWND, HANDLE, HGLOBAL) are BigInt
 * pointers; BOOL is declared as `int` because "nonzero" is not always 1.
 */
import { createRequire } from 'node:module';
import type { KeyId } from '@shared/hotkeys';
import type { FocusedApp } from '@shared/types';
import { type InjectedKeys, injectedKeys, shortcutKeys } from './injected';
import type { Platform } from './types';
import { MODIFIER_VKS, VK_MASK_KEY, isExtendedKey, vkFromKeyId } from './vk';

type Koffi = typeof import('koffi');
type TypeSpec = Parameters<Koffi['sizeof']>[0];
type Handle = bigint | null;

const INPUT_KEYBOARD = 1;
const KEYEVENTF_EXTENDEDKEY = 0x0001;
const KEYEVENTF_KEYUP = 0x0002;
const KEYEVENTF_UNICODE = 0x0004;
const MAPVK_VK_TO_VSC = 0;

const PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
const TOKEN_QUERY = 0x0008;
const TOKEN_ELEVATION_CLASS = 20;
const ERROR_ACCESS_DENIED = 5;

const SW_RESTORE = 9;
const CF_UNICODETEXT = 13;
const GMEM_MOVEABLE = 0x0002;

/** Tags our injected events (`dwExtraInfo`) so other tools can tell them from real keys: "FLOW". */
const INJECT_TAG = 0x464c4f57;

/**
 * Characters per SendInput call, and the pause between calls, when typing.
 * Windows timers tick every ~16 ms whatever delay is asked for, so the chunk
 * size sets the top speed (about 4,000 characters a second). In practice every
 * injected event also runs each keyboard hook installed on the machine.
 */
export const TYPE_CHUNK_CHARS = 64;
export const TYPE_CHUNK_DELAY_MS = 8;
const MODIFIER_POLL_MS = 15;
const CLIPBOARD_OPEN_TRIES = 6;
const CLIPBOARD_RETRY_MS = 4;

/** Foreground windows that have no text field: the desktop, the taskbar, the task switcher. */
const SHELL_CLASSES = new Set([
  'Progman',
  'WorkerW',
  'Shell_TrayWnd',
  'Shell_SecondaryTrayWnd',
  'NotifyIconOverflowWindow',
  'TopLevelWindowForOverflowXamlIsland',
  'MultitaskingViewFrame',
  'XamlExplorerHostIslandWindow',
  'ForegroundStaging',
]);

function bind(koffi: Koffi) {
  const user32 = koffi.load('user32.dll');
  const kernel32 = koffi.load('kernel32.dll');
  const advapi32 = koffi.load('advapi32.dll');

  // Anonymous types: koffi type names are process-global and cannot be redefined.
  const MOUSEINPUT = koffi.struct({
    dx: 'int32',
    dy: 'int32',
    mouseData: 'uint32',
    dwFlags: 'uint32',
    time: 'uint32',
    dwExtraInfo: 'uintptr_t',
  });
  const KEYBDINPUT = koffi.struct({
    wVk: 'uint16',
    wScan: 'uint16',
    dwFlags: 'uint32',
    time: 'uint32',
    dwExtraInfo: 'uintptr_t',
  });
  const HARDWAREINPUT = koffi.struct({ uMsg: 'uint32', wParamL: 'uint16', wParamH: 'uint16' });
  // The union must include MOUSEINPUT even though only `ki` is used: it sets
  // the size (40 bytes on x64) that SendInput checks against cbSize.
  const INPUT = koffi.struct({
    type: 'uint32',
    u: koffi.union({ mi: MOUSEINPUT, ki: KEYBDINPUT, hi: HARDWAREINPUT }),
  });

  const outU32 = koffi.out(koffi.pointer('uint32'));
  const f = (lib: ReturnType<Koffi['load']>, name: string, result: TypeSpec, args: TypeSpec[]) =>
    lib.func('__stdcall', name, result, args);

  return {
    inputSize: koffi.sizeof(INPUT),

    GetForegroundWindow: f(user32, 'GetForegroundWindow', 'void *', []),
    GetWindowTextW: f(user32, 'GetWindowTextW', 'int', ['void *', 'void *', 'int']),
    GetClassNameW: f(user32, 'GetClassNameW', 'int', ['void *', 'void *', 'int']),
    GetWindowThreadProcessId: f(user32, 'GetWindowThreadProcessId', 'uint32', ['void *', outU32]),
    IsWindow: f(user32, 'IsWindow', 'int', ['void *']),
    IsIconic: f(user32, 'IsIconic', 'int', ['void *']),
    ShowWindow: f(user32, 'ShowWindow', 'int', ['void *', 'int']),
    SetForegroundWindow: f(user32, 'SetForegroundWindow', 'int', ['void *']),
    BringWindowToTop: f(user32, 'BringWindowToTop', 'int', ['void *']),
    AttachThreadInput: f(user32, 'AttachThreadInput', 'int', ['uint32', 'uint32', 'int']),
    SendInput: f(user32, 'SendInput', 'uint32', ['uint32', koffi.pointer(INPUT), 'int']),
    GetAsyncKeyState: f(user32, 'GetAsyncKeyState', 'int16', ['int']),
    MapVirtualKeyW: f(user32, 'MapVirtualKeyW', 'uint32', ['uint32', 'uint32']),
    OpenClipboard: f(user32, 'OpenClipboard', 'int', ['void *']),
    EmptyClipboard: f(user32, 'EmptyClipboard', 'int', []),
    CloseClipboard: f(user32, 'CloseClipboard', 'int', []),
    RegisterClipboardFormatW: f(user32, 'RegisterClipboardFormatW', 'uint32', ['str16']),
    SetClipboardData: f(user32, 'SetClipboardData', 'void *', ['uint32', 'void *']),

    OpenProcess: f(kernel32, 'OpenProcess', 'void *', ['uint32', 'int', 'uint32']),
    CloseHandle: f(kernel32, 'CloseHandle', 'int', ['void *']),
    QueryFullProcessImageNameW: f(kernel32, 'QueryFullProcessImageNameW', 'int', [
      'void *',
      'uint32',
      'void *',
      koffi.inout(koffi.pointer('uint32')),
    ]),
    GetCurrentProcess: f(kernel32, 'GetCurrentProcess', 'void *', []),
    GetCurrentThreadId: f(kernel32, 'GetCurrentThreadId', 'uint32', []),
    GetLastError: f(kernel32, 'GetLastError', 'uint32', []),
    GlobalAlloc: f(kernel32, 'GlobalAlloc', 'void *', ['uint32', 'uintptr_t']),
    GlobalLock: f(kernel32, 'GlobalLock', 'void *', ['void *']),
    GlobalUnlock: f(kernel32, 'GlobalUnlock', 'int', ['void *']),
    GlobalFree: f(kernel32, 'GlobalFree', 'void *', ['void *']),
    RtlMoveMemory: f(kernel32, 'RtlMoveMemory', 'void', ['void *', 'void *', 'uintptr_t']),

    OpenProcessToken: f(advapi32, 'OpenProcessToken', 'int', [
      'void *',
      'uint32',
      koffi.out(koffi.pointer('void *')),
    ]),
    GetTokenInformation: f(advapi32, 'GetTokenInformation', 'int', [
      'void *',
      'int',
      'void *',
      'uint32',
      outU32,
    ]),
  };
}

type Win32Api = ReturnType<typeof bind>;

let cachedApi: Win32Api | null = null;

/** Loads koffi and binds the Win32 functions. Throws if koffi or a symbol is missing. */
function loadApi(): Win32Api {
  if (cachedApi) return cachedApi;
  const require = createRequire(import.meta.url);
  const api = bind(require('koffi') as Koffi);
  // The classic SendInput trap: a wrong INPUT layout makes every call fail silently.
  const expected = process.arch === 'ia32' ? 28 : 40;
  if (api.inputSize !== expected) {
    throw new Error(`INPUT is ${api.inputSize} bytes, expected ${expected}`);
  }
  cachedApi = api;
  return api;
}

interface KeyStroke {
  /** Set for real keys, whose echo the input hook must ignore. Unicode packets have no key id. */
  key?: KeyId;
  vk: number;
  /** UTF-16 code unit for KEYEVENTF_UNICODE, or the hardware scan code. */
  scan: number;
  flags: number;
}

function parseHandle(windowId: string): bigint | null {
  if (!/^\d+$/.test(windowId)) return null;
  const value = BigInt(windowId);
  return value === 0n ? null : value;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function baseName(fullPath: string): string {
  return fullPath.slice(fullPath.lastIndexOf('\\') + 1);
}

export class Win32Platform implements Platform {
  readonly available = true;
  readonly selfElevated: boolean;

  constructor(
    private readonly api: Win32Api,
    private readonly injected: InjectedKeys,
  ) {
    this.selfElevated = this.tokenElevated(api.GetCurrentProcess()) === true;
  }

  // ── Focused window ───────────────────────────────────────────────────────

  getFocusedApp(): FocusedApp | null {
    const hwnd: Handle = this.api.GetForegroundWindow();
    if (!hwnd) return null;
    const pidOut = [0];
    this.api.GetWindowThreadProcessId(hwnd, pidOut);
    const pid = pidOut[0] ?? 0;
    if (!pid) return null;
    const { name, elevated } = this.processInfo(pid);
    return {
      windowId: hwnd.toString(),
      processName: name,
      title: this.wideString(512, (buf, max) => this.api.GetWindowTextW(hwnd, buf, max)),
      pid,
      elevated,
    };
  }

  isWindowAlive(windowId: string): boolean {
    const hwnd = parseHandle(windowId);
    return hwnd !== null && this.api.IsWindow(hwnd) !== 0;
  }

  focusWindow(windowId: string): boolean {
    const { api } = this;
    const hwnd = parseHandle(windowId);
    if (hwnd === null || api.IsWindow(hwnd) === 0) return false;
    if (api.GetForegroundWindow() === hwnd) return true;
    if (api.IsIconic(hwnd) !== 0) api.ShowWindow(hwnd, SW_RESTORE);
    api.SetForegroundWindow(hwnd);
    if (api.GetForegroundWindow() === hwnd) return true;

    // Windows only lets the process that received the last input event take
    // the foreground. Injecting a no-op key makes that us; sharing an input
    // queue with the current foreground thread covers the remaining cases.
    this.maskStartMenu();
    const foreground: Handle = api.GetForegroundWindow();
    const self: number = api.GetCurrentThreadId();
    const other: number = foreground ? api.GetWindowThreadProcessId(foreground, [0]) : 0;
    const attached = other !== 0 && other !== self && api.AttachThreadInput(self, other, 1) !== 0;
    try {
      api.BringWindowToTop(hwnd);
      api.SetForegroundWindow(hwnd);
    } finally {
      if (attached) api.AttachThreadInput(self, other, 0);
    }
    return api.GetForegroundWindow() === hwnd;
  }

  isTextTarget(app: FocusedApp | null): boolean {
    if (!app) return false;
    const hwnd = parseHandle(app.windowId);
    if (hwnd === null) return false;
    // A closed window has no class name: that is "target closed", not "no target".
    const className = this.wideString(256, (buf, max) => this.api.GetClassNameW(hwnd, buf, max));
    return !SHELL_CLASSES.has(className);
  }

  // ── Simulated input ──────────────────────────────────────────────────────

  async typeText(text: string, signal?: AbortSignal): Promise<void> {
    const chars = [...text.replace(/\r\n?/g, '\n')];
    for (let start = 0; start < chars.length; start += TYPE_CHUNK_CHARS) {
      if (signal?.aborted) return;
      if (start > 0) await delay(TYPE_CHUNK_DELAY_MS);
      if (signal?.aborted) return;
      const strokes: KeyStroke[] = [];
      for (const char of chars.slice(start, start + TYPE_CHUNK_CHARS)) {
        // Only newlines become a key press. A tab stays a character: the Tab
        // key would move focus out of a form field and lose the rest of the text.
        if (char === '\n') strokes.push(...this.keyTap('Enter'));
        else {
          // A character outside the BMP is two code units, sent as two packets.
          for (let i = 0; i < char.length; i++) {
            const unit = char.charCodeAt(i);
            strokes.push({ vk: 0, scan: unit, flags: KEYEVENTF_UNICODE });
            strokes.push({ vk: 0, scan: unit, flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP });
          }
        }
      }
      // UIPI makes SendInput insert nothing when the foreground window is elevated.
      if (this.send(strokes) !== strokes.length) throw new Error('SendInput was blocked');
    }
  }

  sendShortcut(binding: string): boolean {
    const keys = shortcutKeys(binding);
    if (!keys || keys.length === 0) return false;
    const strokes: KeyStroke[] = [];
    for (const key of keys) {
      const stroke = this.keyStroke(key, true);
      if (!stroke) return false;
      strokes.push(stroke);
    }
    for (const key of [...keys].reverse()) strokes.push(this.keyStroke(key, false)!);
    // One SendInput call is atomic: no real key event can land inside the chord.
    return this.send(strokes) === strokes.length;
  }

  async waitForModifiersUp(timeoutMs = 1500, signal?: AbortSignal): Promise<void> {
    const deadline = performance.now() + timeoutMs;
    while (this.anyModifierDown() && performance.now() < deadline && !signal?.aborted) {
      await delay(MODIFIER_POLL_MS);
    }
  }

  isKeyDown(key: KeyId): boolean | null {
    const vk = vkFromKeyId(key);
    if (vk === null) return null;
    return (this.api.GetAsyncKeyState(vk) & 0x8000) !== 0;
  }

  maskStartMenu(): void {
    // VK 0xE8 is unassigned, so no app reacts to it, but it still counts as a
    // key pressed while Win/Alt is down — which is what stops the Start menu
    // (or a window's menu bar) from opening when the modifier is released.
    this.send([
      { vk: VK_MASK_KEY, scan: 0, flags: 0 },
      { vk: VK_MASK_KEY, scan: 0, flags: KEYEVENTF_KEYUP },
    ]);
  }

  // ── Clipboard ────────────────────────────────────────────────────────────

  writeClipboardExcluded(text: string): boolean {
    const { api } = this;
    if (!this.openClipboard()) return false;
    try {
      if (api.EmptyClipboard() === 0) return false;
      if (!this.setClipboardData(CF_UNICODETEXT, Buffer.from(`${text}\0`, 'utf16le'))) return false;
      // Presence alone excludes the item from clipboard managers; the other
      // two formats hold a DWORD 0 meaning "no" (Win+V history, cloud sync).
      const zero = Buffer.alloc(4);
      for (const name of [
        'ExcludeClipboardContentFromMonitorProcessing',
        'CanIncludeInClipboardHistory',
        'CanUploadToCloudClipboard',
      ]) {
        const format: number = api.RegisterClipboardFormatW(name);
        if (format !== 0) this.setClipboardData(format, zero);
      }
      return true;
    } finally {
      api.CloseClipboard();
    }
  }

  // ── Internals ────────────────────────────────────────────────────────────

  private openClipboard(): boolean {
    // Another process (a clipboard manager, the target app) may hold the
    // clipboard for a few milliseconds; retry briefly rather than give up.
    for (let attempt = 0; attempt < CLIPBOARD_OPEN_TRIES; attempt++) {
      if (this.api.OpenClipboard(null) !== 0) return true;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, CLIPBOARD_RETRY_MS);
    }
    return false;
  }

  private setClipboardData(format: number, bytes: Buffer): boolean {
    const { api } = this;
    const memory: Handle = api.GlobalAlloc(GMEM_MOVEABLE, bytes.length);
    if (!memory) return false;
    const pointer: Handle = api.GlobalLock(memory);
    if (!pointer) {
      api.GlobalFree(memory);
      return false;
    }
    api.RtlMoveMemory(pointer, bytes, bytes.length);
    api.GlobalUnlock(memory);
    // On success the clipboard owns the memory; on failure it is still ours to free.
    if (!api.SetClipboardData(format, memory)) {
      api.GlobalFree(memory);
      return false;
    }
    return true;
  }

  private anyModifierDown(): boolean {
    return MODIFIER_VKS.some((vk) => (this.api.GetAsyncKeyState(vk) & 0x8000) !== 0);
  }

  private keyStroke(key: KeyId, down: boolean): KeyStroke | null {
    const vk = vkFromKeyId(key);
    if (vk === null) return null;
    return {
      key,
      vk,
      // Some apps (terminals, remote desktops, games) read the scan code rather than the key.
      scan: this.api.MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) & 0xff,
      flags: (down ? 0 : KEYEVENTF_KEYUP) | (isExtendedKey(key) ? KEYEVENTF_EXTENDEDKEY : 0),
    };
  }

  private keyTap(key: KeyId): KeyStroke[] {
    return [this.keyStroke(key, true)!, this.keyStroke(key, false)!];
  }

  /** Returns how many events were inserted into the input stream. */
  private send(strokes: KeyStroke[]): number {
    if (strokes.length === 0) return 0;
    const inputs = strokes.map((s) => ({
      type: INPUT_KEYBOARD,
      u: { ki: { wVk: s.vk, wScan: s.scan, dwFlags: s.flags, time: 0, dwExtraInfo: INJECT_TAG } },
    }));
    const sent: number = this.api.SendInput(inputs.length, inputs, this.api.inputSize);
    // The hook delivers the echo on a later turn of the event loop, so noting
    // it now is early enough — and nothing is expected for input that was blocked.
    for (const stroke of strokes.slice(0, sent)) {
      if (stroke.key) this.injected.expect(stroke.key, (stroke.flags & KEYEVENTF_KEYUP) === 0);
    }
    return sent;
  }

  private processInfo(pid: number): { name: string; elevated: boolean } {
    const { api } = this;
    const handle: Handle = api.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if (!handle) {
      // Access denied means the process is out of our reach, which is also
      // when Windows refuses our simulated input: treat it as elevated.
      return { name: '', elevated: api.GetLastError() === ERROR_ACCESS_DENIED };
    }
    try {
      const size = [2048];
      const buffer = Buffer.alloc(size[0]! * 2);
      const ok = api.QueryFullProcessImageNameW(handle, 0, buffer, size) !== 0;
      const name = ok ? baseName(buffer.toString('utf16le', 0, (size[0] ?? 0) * 2)) : '';
      return { name, elevated: this.tokenElevated(handle) ?? true };
    } finally {
      api.CloseHandle(handle);
    }
  }

  /** True/false from the process token; null when the token cannot be opened (access denied). */
  private tokenElevated(handle: Handle): boolean | null {
    const { api } = this;
    const tokenOut: Handle[] = [null];
    if (api.OpenProcessToken(handle, TOKEN_QUERY, tokenOut) === 0 || !tokenOut[0]) {
      return api.GetLastError() === ERROR_ACCESS_DENIED ? null : false;
    }
    try {
      const elevation = Buffer.alloc(4);
      const ok = api.GetTokenInformation(tokenOut[0], TOKEN_ELEVATION_CLASS, elevation, 4, [0]);
      return ok !== 0 && elevation.readUInt32LE(0) !== 0;
    } finally {
      api.CloseHandle(tokenOut[0]);
    }
  }

  private wideString(maxChars: number, read: (buffer: Buffer, maxChars: number) => number): string {
    const buffer = Buffer.alloc(maxChars * 2);
    const length = read(buffer, maxChars);
    return length > 0 ? buffer.toString('utf16le', 0, length * 2) : '';
  }
}

/** Throws when koffi cannot load or the struct layout is not what Win32 expects. */
export function createWin32Platform(injected: InjectedKeys = injectedKeys): Win32Platform {
  return new Win32Platform(loadApi(), injected);
}
