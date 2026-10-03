import type { Clipboard, ClipboardItem } from 'electron';
import { describe, expect, it } from 'vitest';
import type { KeyId } from '@shared/hotkeys';
import type { FocusedApp } from '@shared/types';
import {
  CLIPBOARD_SETTLE_MS,
  type ClipboardItemLike,
  type ClipboardLike,
  FOCUS_SETTLE_MS,
  type InsertOptions,
  Inserter,
  type InserterDeps,
  MODIFIER_WAIT_MS,
  PASTE_READ_MS,
} from '../../../src/main/insert/inserter';
import type { Platform } from '../../../src/main/platform/types';

// Compile-time proof that Electron's own objects fit without a cast (type-only import).
const electronClipboardFits = (clipboard: Clipboard): ClipboardLike => clipboard;
const electronItemFits =
  (Item: typeof ClipboardItem): NonNullable<InserterDeps['makeItem']> =>
  (data) =>
    new Item(data);

const NOTEPAD: FocusedApp = {
  windowId: '100',
  processName: 'notepad.exe',
  title: 'Untitled - Notepad',
  pid: 1,
  elevated: false,
};
const OTHER: FocusedApp = { windowId: '200', processName: 'chrome.exe', title: 'Tab', pid: 2 };

const TEXT = 'text/plain';
const HTML = 'text/html';
const PNG = 'image/png';
const raw = (format: string) => `electron application/osclipboard;format="${format}"`;
const EXCLUSION = [
  raw('ExcludeClipboardContentFromMonitorProcessing'),
  raw('CanUploadToCloudClipboard'),
  raw('CanIncludeInClipboardHistory'),
];

/** One clipboard item: MIME type → content (strings stand in for any bytes). */
type Data = Record<string, string>;

const makeItem = (data: Record<string, string | Blob>): ClipboardItemLike => ({
  types: Object.keys(data),
  getType: async (type) => {
    const value = data[type];
    if (value === undefined) throw new Error(`no ${type}`);
    return typeof value === 'string' ? new Blob([value], { type }) : value;
  },
});

/**
 * Mimics Electron's async clipboard. Items returned by `read()` are lazy, as
 * the real ones are: `getType` reads whatever the clipboard holds when called.
 */
class FakeClipboard implements ClipboardLike {
  items: Data[] = [];
  log: string[] = [];
  /** Types whose `getType` rejects, or resolves to a non-Blob. */
  unreadable = new Set<string>();
  bookmarks = new Set<string>();
  /** `write()` rejects when an item carries one of these types. */
  unwritable: (type: string) => boolean = () => false;
  failRead = false;
  failWriteText = false;

  get text(): string {
    return this.items[0]?.[TEXT] ?? '';
  }
  get types(): string[] {
    return this.items.flatMap((item) => Object.keys(item));
  }

  async read(): Promise<ClipboardItemLike[]> {
    if (this.failRead) throw new Error('clipboard is locked');
    return this.items.map((item, index) => ({
      types: Object.keys(item),
      getType: async (type: string) => {
        if (this.unreadable.has(type)) throw new Error(`cannot read ${type}`);
        if (this.bookmarks.has(type)) return { title: 'A bookmark', url: 'https://example.com' };
        const live = this.items[index]?.[type];
        if (live === undefined) throw new Error(`${type} is gone`);
        return new Blob([live], { type });
      },
    }));
  }
  async readText(): Promise<string> {
    return this.text;
  }
  async write(items: ClipboardItemLike[]): Promise<void> {
    const next: Data[] = [];
    for (const item of items) {
      const data: Data = {};
      for (const type of item.types) {
        if (this.unwritable(type)) throw new Error(`cannot write ${type}`);
        data[type] = await ((await item.getType(type)) as Blob).text();
      }
      next.push(data);
    }
    this.log.push(`write:${next.map((d) => Object.keys(d).join(',')).join('|')}`);
    this.items = next;
  }
  async writeText(text: string): Promise<void> {
    if (this.failWriteText) throw new Error('clipboard is locked');
    this.log.push(`writeText:${text}`);
    this.items = [{ [TEXT]: text }];
  }
  clear(): void {
    this.log.push('clear');
    this.items = [];
  }
}

interface World {
  /** What the simulated target app received. */
  received: string[];
  /** Everything that happened, in order. */
  trace: string[];
  clipboard: FakeClipboard;
  platform: FakePlatform;
  inserter: Inserter;
  sleeps: number[];
  /** Runs at the start of every sleep, with the duration. */
  onSleep: (ms: number) => void;
}

class FakePlatform implements Platform {
  available = true;
  selfElevated = false;
  windows = new Map<string, FocusedApp>([
    [NOTEPAD.windowId, NOTEPAD],
    [OTHER.windowId, OTHER],
  ]);
  foreground: string | null = NOTEPAD.windowId;
  shellWindows = new Set<string>();
  canFocus = true;
  canExclude = true;
  canSend = true;
  heldModifiers = new Set<KeyId>();
  releaseDuringWait = true;
  typeError: Error | null = null;

  constructor(
    private readonly world: () => World,
    private readonly clipboard: FakeClipboard,
  ) {}

  getFocusedApp(): FocusedApp | null {
    return this.foreground ? (this.windows.get(this.foreground) ?? null) : null;
  }
  isWindowAlive(windowId: string): boolean {
    return this.windows.has(windowId);
  }
  focusWindow(windowId: string): boolean {
    this.world().trace.push(`focus:${windowId}`);
    if (!this.canFocus || !this.windows.has(windowId)) return false;
    this.foreground = windowId;
    return true;
  }
  isTextTarget(app: FocusedApp | null): boolean {
    return app !== null && !this.shellWindows.has(app.windowId);
  }
  async typeText(text: string, signal?: AbortSignal): Promise<void> {
    this.world().trace.push(`type:${text}`);
    if (this.typeError) throw this.typeError;
    if (signal?.aborted) return;
    this.world().received.push(`${this.foreground}:${text}`);
  }
  sendShortcut(binding: string): boolean {
    this.world().trace.push(`shortcut:${binding}`);
    if (!this.canSend) return false;
    // The target app reads whatever is on the clipboard at this moment.
    this.world().received.push(`${this.foreground}:${this.clipboard.text}`);
    return true;
  }
  async waitForModifiersUp(timeoutMs?: number): Promise<void> {
    this.world().trace.push(`wait:${timeoutMs}`);
    if (this.releaseDuringWait) this.heldModifiers.clear();
  }
  isKeyDown(key: KeyId): boolean | null {
    return this.heldModifiers.has(key);
  }
  maskStartMenu(): void {}
  /** Like the real one: plain text plus the three history-exclusion marker formats. */
  writeClipboardExcluded(text: string): boolean {
    if (!this.canExclude) return false;
    this.clipboard.log.push(`excluded:${text}`);
    this.clipboard.items = [
      { [TEXT]: text, ...Object.fromEntries(EXCLUSION.map((t) => [t, '\0\0\0\0'])) },
    ];
    return true;
  }
}

function setup(
  options: {
    tapShortcut?: ((binding: string) => boolean) | null;
    makeItem?: InserterDeps['makeItem'] | null;
    realSleep?: boolean;
  } = {},
): World {
  const clipboard = new FakeClipboard();
  const world = {
    received: [],
    trace: [],
    clipboard,
    sleeps: [],
    onSleep: () => {},
  } as unknown as World;
  world.platform = new FakePlatform(() => world, clipboard);
  world.inserter = new Inserter({
    platform: world.platform,
    clipboard,
    makeItem: options.makeItem === null ? undefined : (options.makeItem ?? makeItem),
    tapShortcut:
      options.tapShortcut === null
        ? undefined
        : (options.tapShortcut ??
          ((binding) => {
            world.trace.push(`tap:${binding}`);
            world.received.push(`${world.platform.foreground}:${clipboard.text}`);
            return true;
          })),
    sleep: options.realSleep
      ? (ms) => new Promise((resolve) => setTimeout(resolve, ms / 10))
      : async (ms) => {
          world.sleeps.push(ms);
          world.onSleep(ms);
        },
  });
  return world;
}

const paste = (overrides: Partial<InsertOptions> = {}): InsertOptions => ({
  method: 'paste',
  pasteShortcut: 'Ctrl+V',
  restoreClipboard: true,
  target: NOTEPAD,
  ...overrides,
});

describe('Electron compatibility', () => {
  it('accepts Electron’s clipboard and ClipboardItem without a cast', () => {
    expect(typeof electronClipboardFits).toBe('function');
    expect(typeof electronItemFits).toBe('function');
  });
});

describe('Inserter: paste', () => {
  it('pastes through the clipboard and restores what was there', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello world', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello world']);
    expect(w.clipboard.text).toBe('previous');
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`, 'shortcut:Ctrl+V']);
    expect(w.sleeps).toEqual([CLIPBOARD_SETTLE_MS, PASTE_READ_MS]);
  });

  it('keeps the transcript and the restored text out of clipboard history', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.log).toEqual(['excluded:hello', 'excluded:previous']);
  });

  it('treats text that was itself written excluded as text-only', async () => {
    const w = setup();
    w.platform.writeClipboardExcluded('previous');
    w.clipboard.log = [];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.log).toEqual(['excluded:hello', 'excluded:previous']);
    expect(w.clipboard.text).toBe('previous');
  });

  it('falls back to a plain clipboard write when exclusion is unavailable', async () => {
    const w = setup();
    w.platform.canExclude = false;
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello']);
    expect(w.clipboard.log).toEqual(['writeText:hello', 'writeText:previous']);
  });

  it('restores rich content with every format it had', async () => {
    const w = setup();
    const original = { [TEXT]: 'plain', [HTML]: '<b>plain</b>', [PNG]: 'PNG-BYTES' };
    w.clipboard.items = [{ ...original }];
    await w.inserter.insert('hello', paste());
    expect(w.received).toEqual(['100:hello']);
    expect(w.clipboard.items).toEqual([original]);
    expect(w.clipboard.log.at(-1)).toBe(`write:${TEXT},${HTML},${PNG}`);
  });

  it('restores an image-only clipboard', async () => {
    const w = setup();
    w.clipboard.items = [{ [PNG]: 'PNG-BYTES' }];
    await w.inserter.insert('hello', paste());
    expect(w.received).toEqual(['100:hello']);
    expect(w.clipboard.items).toEqual([{ [PNG]: 'PNG-BYTES' }]);
  });

  it('restores an html-only clipboard', async () => {
    const w = setup();
    w.clipboard.items = [{ [HTML]: '<i>only html</i>' }];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [HTML]: '<i>only html</i>' }]);
  });

  it('restores several clipboard items', async () => {
    const w = setup();
    const original: Data[] = [{ [TEXT]: 'first' }, { [PNG]: 'second' }];
    w.clipboard.items = structuredClone(original);
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual(original);
  });

  it('restores raw OS formats along with the content', async () => {
    const w = setup();
    const original = { [HTML]: '<b>x</b>', [raw('Custom App Format')]: 'app-data' };
    w.clipboard.items = [{ ...original }];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([original]);
  });

  it('reads lazy clipboard items out in full before overwriting the clipboard', async () => {
    const w = setup();
    // The fake's getType reads live state, exactly like Electron's items: a
    // snapshot that kept the items instead of their data would restore "hello".
    w.clipboard.items = [{ [TEXT]: 'previous', [HTML]: '<b>previous</b>' }];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'previous', [HTML]: '<b>previous</b>' }]);
  });

  it('copies the bytes of a Blob that is itself lazy', async () => {
    const w = setup();
    w.clipboard.items = [{ [HTML]: '<b>previous</b>' }];
    class LiveBlob extends Blob {
      override async arrayBuffer(): Promise<ArrayBuffer> {
        const live = w.clipboard.items[0]?.[HTML] ?? 'GONE';
        return new TextEncoder().encode(live).buffer as ArrayBuffer;
      }
    }
    w.clipboard.read = async () => [
      { types: [HTML], getType: async () => new LiveBlob([], { type: HTML }) },
    ];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [HTML]: '<b>previous</b>' }]);
  });

  it('skips a type that cannot be read and restores the rest', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'plain', [HTML]: '<b>plain</b>', [PNG]: 'PNG-BYTES' }];
    w.clipboard.unreadable.add(PNG);
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'plain', [HTML]: '<b>plain</b>' }]);
  });

  it('skips values that are not Blobs (bookmarks)', async () => {
    const w = setup();
    const bookmark = 'electron application/bookmark';
    w.clipboard.items = [{ [TEXT]: 'https://example.com', [bookmark]: 'x' }];
    w.clipboard.bookmarks.add(bookmark);
    await w.inserter.insert('hello', paste());
    // Only plain text was saved, so it is restored as text.
    expect(w.clipboard.text).toBe('https://example.com');
    expect(w.clipboard.log.at(-1)).toBe('excluded:https://example.com');
  });

  it('never writes OLE bookkeeping formats back', async () => {
    const w = setup();
    w.clipboard.items = [
      { [PNG]: 'PNG-BYTES', [raw('DataObject')]: 'hwnd', [raw('Ole Private Data')]: 'ptr' },
    ];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [PNG]: 'PNG-BYTES' }]);
  });

  it('retries without raw OS formats when the full restore is rejected', async () => {
    const w = setup();
    w.clipboard.items = [{ [HTML]: '<b>x</b>', [PNG]: 'PNG-BYTES', [raw('Weird')]: 'data' }];
    w.clipboard.unwritable = (type) => type === raw('Weird');
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [HTML]: '<b>x</b>', [PNG]: 'PNG-BYTES' }]);
  });

  it('falls back to the plain text when no item can be written', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'plain', [HTML]: '<b>plain</b>' }];
    w.clipboard.unwritable = () => true;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'plain' }]);
  });

  it('restores only the plain text when no makeItem is supplied', async () => {
    const w = setup({ makeItem: null });
    w.clipboard.items = [{ [TEXT]: 'plain', [HTML]: '<b>plain</b>' }];
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'plain' }]);
  });

  it('leaves the transcript on the clipboard when nothing can be restored', async () => {
    const w = setup();
    w.clipboard.items = [{ [PNG]: 'PNG-BYTES' }];
    w.clipboard.unreadable.add(PNG);
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    // The original was not empty, so the clipboard is not cleared either.
    expect(w.clipboard.text).toBe('hello');
    expect(w.clipboard.log).not.toContain('clear');
  });

  it('leaves the transcript on the clipboard when the clipboard cannot be read', async () => {
    const w = setup();
    w.clipboard.items = [{ [PNG]: 'PNG-BYTES' }];
    w.clipboard.failRead = true;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello']);
    expect(w.clipboard.text).toBe('hello');
  });

  it('clears the clipboard when it was empty before', async () => {
    const w = setup();
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([]);
    expect(w.clipboard.log.at(-1)).toBe('clear');
  });

  it('leaves the transcript on the clipboard when restore is off', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste({ restoreClipboard: false }));
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.clipboard.text).toBe('hello');
  });

  it('does not restore when the user copied something else meanwhile', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous', [HTML]: '<b>previous</b>' }];
    w.onSleep = (ms) => {
      if (ms === PASTE_READ_MS) w.clipboard.items = [{ [TEXT]: 'copied by the user' }];
    };
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'copied by the user' }]);
  });

  it('does not restore when the user copied an image meanwhile', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    w.onSleep = (ms) => {
      if (ms === PASTE_READ_MS) w.clipboard.items = [{ [PNG]: 'screenshot' }];
    };
    await w.inserter.insert('hello', paste());
    expect(w.clipboard.items).toEqual([{ [PNG]: 'screenshot' }]);
  });

  it('still restores when the clipboard reports the text with CRLF line endings', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    w.onSleep = (ms) => {
      if (ms === PASTE_READ_MS) w.clipboard.items = [{ [TEXT]: 'line one\r\nline two' }];
    };
    await w.inserter.insert('line one\nline two', paste());
    expect(w.clipboard.text).toBe('previous');
  });

  it('uses the configured paste shortcut, normalised', async () => {
    const w = setup();
    await w.inserter.insert('ls -la', paste({ pasteShortcut: 'shift+ctrl+v' }));
    expect(w.trace).toContain('shortcut:Ctrl+Shift+V');
  });

  it('falls back to Ctrl+V when the configured shortcut does not parse', async () => {
    const w = setup();
    const result = await w.inserter.insert('hello', paste({ pasteShortcut: 'Ctrl+Nope' }));
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.trace).toContain('shortcut:Ctrl+V');
  });

  it('falls back to tapShortcut when the platform cannot send input', async () => {
    const w = setup();
    w.platform.canSend = false;
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`, 'shortcut:Ctrl+V', 'tap:Ctrl+V']);
    expect(w.received).toEqual(['100:hello']);
    expect(w.clipboard.text).toBe('previous');
  });

  it('leaves the text on the clipboard when no shortcut could be sent', async () => {
    const w = setup({ tapShortcut: () => false });
    w.platform.canSend = false;
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(w.clipboard.text).toBe('hello');
    expect(w.received).toEqual([]);
  });

  it('works without a tapShortcut fallback', async () => {
    const w = setup({ tapShortcut: null });
    w.platform.canSend = false;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(w.clipboard.text).toBe('hello');
  });

  it('waits for held modifiers to be released before pasting', async () => {
    const w = setup();
    w.platform.heldModifiers = new Set(['LAlt', 'LShift']);
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.trace[0]).toBe(`wait:${MODIFIER_WAIT_MS}`);
  });

  it('uses the clipboard when a modifier is still held after the wait', async () => {
    const w = setup();
    w.platform.heldModifiers = new Set(['LWin']);
    w.platform.releaseDuringWait = false;
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste());
    // Ctrl+V with Win held would be Ctrl+Win+V: never send it.
    expect(result).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(w.received).toEqual([]);
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`]);
    expect(w.clipboard.text).toBe('hello');
  });

  it('does nothing for empty text', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    expect(await w.inserter.insert('', paste())).toEqual({ outcome: 'pasted' });
    expect(w.trace).toEqual([]);
    expect(w.clipboard.log).toEqual([]);
  });
});

describe('Inserter: target checks', () => {
  it('uses the clipboard when there is no target', async () => {
    const w = setup();
    const result = await w.inserter.insert('hello', paste({ target: null }));
    expect(result).toEqual({ outcome: 'clipboard', reason: 'no-target' });
    // Already on the clipboard by the time the result is returned.
    expect(w.clipboard.text).toBe('hello');
    expect(w.clipboard.log).toEqual(['writeText:hello']);
    expect(w.trace).toEqual([]);
  });

  it('uses the clipboard when the target is the desktop or taskbar', async () => {
    const w = setup();
    w.platform.shellWindows.add(NOTEPAD.windowId);
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'no-target' });
    expect(w.received).toEqual([]);
  });

  it('uses the clipboard when the target window was closed', async () => {
    const w = setup();
    w.platform.windows.delete(NOTEPAD.windowId);
    w.platform.foreground = OTHER.windowId;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'target-closed' });
    expect(w.clipboard.text).toBe('hello');
    expect(w.trace).toEqual([]);
  });

  it('uses the clipboard for an elevated target', async () => {
    const w = setup();
    const admin = { ...NOTEPAD, elevated: true };
    const result = await w.inserter.insert('hello', paste({ target: admin }));
    expect(result).toEqual({ outcome: 'clipboard', reason: 'elevated' });
    expect(w.received).toEqual([]);
    expect(w.trace).toEqual([]);
  });

  it('inserts into an elevated target when Flow is elevated too', async () => {
    const w = setup();
    w.platform.selfElevated = true;
    const admin = { ...NOTEPAD, elevated: true };
    const result = await w.inserter.insert('hello', paste({ target: admin }));
    expect(result).toEqual({ outcome: 'pasted' });
  });

  it('refocuses the target when focus moved during transcription', async () => {
    const w = setup();
    w.platform.foreground = OTHER.windowId;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello']);
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`, 'focus:100', 'shortcut:Ctrl+V']);
    expect(w.sleeps[0]).toBe(FOCUS_SETTLE_MS);
  });

  it('uses the clipboard when the target cannot be refocused', async () => {
    const w = setup();
    w.platform.foreground = OTHER.windowId;
    w.platform.canFocus = false;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'target-closed' });
    expect(w.received).toEqual([]);
    expect(w.clipboard.text).toBe('hello');
  });

  it('uses the clipboard when focus does not stick', async () => {
    const w = setup();
    w.platform.foreground = OTHER.windowId;
    w.onSleep = (ms) => {
      if (ms === FOCUS_SETTLE_MS) w.platform.foreground = OTHER.windowId;
    };
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'target-closed' });
    expect(w.received).toEqual([]);
  });

  it('uses the clipboard when the target closes while waiting for modifiers', async () => {
    const w = setup();
    const wait = w.platform.waitForModifiersUp.bind(w.platform);
    w.platform.waitForModifiersUp = async (ms) => {
      await wait(ms);
      w.platform.windows.delete(NOTEPAD.windowId);
      w.platform.foreground = OTHER.windowId;
    };
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'target-closed' });
    expect(w.received).toEqual([]);
  });

  it('refocuses when nothing has the foreground', async () => {
    const w = setup();
    w.platform.foreground = null;
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello']);
  });
});

describe('Inserter: type', () => {
  const type = (overrides: Partial<InsertOptions> = {}) => paste({ method: 'type', ...overrides });

  it('types the text and leaves the clipboard alone', async () => {
    const w = setup();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', type());
    expect(result).toEqual({ outcome: 'typed' });
    expect(w.received).toEqual(['100:hello']);
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`, 'type:hello']);
    expect(w.clipboard.log).toEqual([]);
  });

  it('applies the same target checks', async () => {
    const w = setup();
    expect(await w.inserter.insert('a', type({ target: null }))).toEqual({
      outcome: 'clipboard',
      reason: 'no-target',
    });
    expect(await w.inserter.insert('b', type({ target: { ...NOTEPAD, elevated: true } }))).toEqual({
      outcome: 'clipboard',
      reason: 'elevated',
    });
    w.platform.foreground = OTHER.windowId;
    expect(await w.inserter.insert('c', type())).toEqual({ outcome: 'typed' });
    expect(w.received).toEqual(['100:c']);
  });

  it('puts the text on the clipboard when typing fails', async () => {
    const w = setup();
    w.platform.typeError = new Error('SendInput was blocked');
    const result = await w.inserter.insert('hello', type());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(w.clipboard.text).toBe('hello');
  });

  it('falls back to paste when the platform is unavailable', async () => {
    const w = setup();
    w.platform.available = false;
    w.platform.canSend = false;
    const result = await w.inserter.insert('hello', type());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.trace).toContain('tap:Ctrl+V');
    expect(w.trace).not.toContain('type:hello');
  });
});

describe('Inserter: degraded platform', () => {
  it('pastes blind, without a target, through tapShortcut', async () => {
    const w = setup();
    w.platform.available = false;
    w.platform.canSend = false;
    w.platform.canExclude = false;
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    const result = await w.inserter.insert('hello', paste({ target: null }));
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.trace).toEqual([`wait:${MODIFIER_WAIT_MS}`, 'shortcut:Ctrl+V', 'tap:Ctrl+V']);
    expect(w.clipboard.log).toEqual(['writeText:hello', 'writeText:previous']);
  });
});

describe('Inserter: failures and ordering', () => {
  it('never throws: any error puts the text on the clipboard', async () => {
    const w = setup();
    w.platform.sendShortcut = () => {
      throw new Error('koffi exploded');
    };
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(w.clipboard.text).toBe('hello');
  });

  it('survives a clipboard that cannot be written', async () => {
    const w = setup();
    w.platform.canExclude = false;
    w.clipboard.failWriteText = true;
    await expect(w.inserter.insert('hello', paste())).resolves.toEqual({
      outcome: 'clipboard',
      reason: 'failed',
    });
    await expect(w.inserter.insert('hello', paste({ target: null }))).resolves.toEqual({
      outcome: 'clipboard',
      reason: 'no-target',
    });
    expect(w.received).toEqual([]);
  });

  it('a failed restore does not turn a paste into a failure', async () => {
    const w = setup();
    w.clipboard.items = [{ [HTML]: '<i>previous</i>' }];
    w.clipboard.write = async () => {
      throw new Error('clipboard is locked');
    };
    w.clipboard.readText = async () => {
      throw new Error('clipboard is locked');
    };
    const result = await w.inserter.insert('hello', paste());
    expect(result).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:hello']);
  });

  it('serialises back-to-back insertions in call order, clipboard work included', async () => {
    // Real (short) sleeps, so overlapping calls would interleave if not queued.
    const w = setup({ realSleep: true });
    w.clipboard.items = [{ [TEXT]: 'previous', [HTML]: '<b>previous</b>' }];
    const results = await Promise.all([
      w.inserter.insert('one ', paste()),
      w.inserter.insert('two ', paste({ method: 'type' })),
      w.inserter.insert('three', paste()),
    ]);
    expect(results.map((r) => r.outcome)).toEqual(['pasted', 'typed', 'pasted']);
    expect(w.received).toEqual(['100:one ', '100:two ', '100:three']);
    // Each paste saw the original clipboard, never the previous transcript.
    expect(w.clipboard.items).toEqual([{ [TEXT]: 'previous', [HTML]: '<b>previous</b>' }]);
  });

  it('a failed insertion does not block the queue', async () => {
    const w = setup();
    w.platform.typeError = new Error('blocked');
    const first = w.inserter.insert('one', paste({ method: 'type' }));
    const second = w.inserter.insert('two', paste());
    expect(await first).toEqual({ outcome: 'clipboard', reason: 'failed' });
    expect(await second).toEqual({ outcome: 'pasted' });
    expect(w.received).toEqual(['100:two']);
  });

  it('copy writes plain text to the clipboard without awaiting or throwing', async () => {
    const w = setup();
    w.inserter.copy('last transcript');
    await Promise.resolve();
    expect(w.clipboard.text).toBe('last transcript');
    expect(w.clipboard.log).toEqual(['writeText:last transcript']);

    w.clipboard.failWriteText = true;
    expect(() => w.inserter.copy('again')).not.toThrow();
    // The rejection is swallowed; an unhandled one would fail the test run.
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
});

describe('Inserter: abort', () => {
  it('an already aborted call inserts nothing and leaves the clipboard alone', async () => {
    const w = setup();
    const controller = new AbortController();
    controller.abort();
    w.clipboard.items = [{ [TEXT]: 'previous' }];
    await w.inserter.insert('hello', paste({ signal: controller.signal }));
    expect(w.received).toEqual([]);
    expect(w.clipboard.log).toEqual([]);
    expect(w.clipboard.text).toBe('previous');
  });

  it('an abort after the clipboard was written restores it and sends nothing', async () => {
    const w = setup();
    const controller = new AbortController();
    w.clipboard.items = [{ [PNG]: 'PNG-BYTES' }];
    w.onSleep = (ms) => {
      if (ms === CLIPBOARD_SETTLE_MS) controller.abort();
    };
    await w.inserter.insert('hello', paste({ signal: controller.signal }));
    expect(w.received).toEqual([]);
    expect(w.clipboard.items).toEqual([{ [PNG]: 'PNG-BYTES' }]);
  });

  it('an abort while waiting for modifiers inserts nothing', async () => {
    const w = setup();
    const controller = new AbortController();
    const wait = w.platform.waitForModifiersUp.bind(w.platform);
    w.platform.waitForModifiersUp = async (ms) => {
      await wait(ms);
      controller.abort();
    };
    await w.inserter.insert('hello', paste({ method: 'type', signal: controller.signal }));
    expect(w.received).toEqual([]);
    expect(w.clipboard.log).toEqual([]);
  });

  it('passes the signal to typeText', async () => {
    const w = setup();
    const controller = new AbortController();
    let seen: AbortSignal | undefined;
    w.platform.typeText = async (_text, signal) => {
      seen = signal;
    };
    await w.inserter.insert('hello', paste({ method: 'type', signal: controller.signal }));
    expect(seen).toBe(controller.signal);
  });
});
