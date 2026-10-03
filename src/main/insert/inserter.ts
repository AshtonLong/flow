/**
 * Places a transcript at the cursor: clipboard paste (default) or simulated
 * typing. Every path that cannot insert leaves the text on the clipboard, so a
 * dictation is never silently lost.
 */
import { SIDED_MODIFIERS, normalizeBinding } from '@shared/hotkeys';
import type { FocusedApp, InsertResult } from '@shared/types';
import type { Platform } from '../platform/types';

/** Electron's `ClipboardItem` satisfies this. */
export interface ClipboardItemLike {
  readonly types: string[];
  /** A `Blob` for most types; anything else (a bookmark object) is not saved. */
  getType(type: string): Promise<unknown>;
}

/** Electron's `clipboard` (the async API, Electron 44+) satisfies this without a cast. */
export interface ClipboardLike {
  read(): Promise<ClipboardItemLike[]>;
  readText(): Promise<string>;
  write(items: ClipboardItemLike[]): Promise<void>;
  writeText(text: string): Promise<void>;
  clear(): void;
}

export interface InsertOptions {
  method: 'paste' | 'type';
  /** For example `Ctrl+V`, or `Ctrl+Shift+V` in a terminal. */
  pasteShortcut: string;
  restoreClipboard: boolean;
  /** The window that had focus at key-up. */
  target: FocusedApp | null;
  /**
   * Stops an insertion that has not sent its keys yet (or typing, mid-way).
   * Nothing is put on the clipboard; the result of an aborted call is meaningless.
   */
  signal?: AbortSignal;
}

export interface InserterDeps {
  platform: Platform;
  clipboard: ClipboardLike;
  /**
   * Builds a clipboard item from saved data: `(data) => new ClipboardItem(data)`.
   * Kept as a dependency so this module does not import `electron`. Without it
   * only plain text can be restored after a paste.
   */
  makeItem?: (data: Record<string, string | Blob>) => ClipboardItemLike;
  /** Fallback shortcut sender (uiohook keyTap) used when the platform cannot send input. */
  tapShortcut?: (binding: string) => boolean;
  /** Injectable for tests. */
  sleep?: (ms: number) => Promise<void>;
}

/**
 * How long to wait for Ctrl/Alt/Shift/Win to be released before inserting. A
 * held modifier would turn the paste into another shortcut (Ctrl+Win+V), and
 * the user may well be holding the hotkey for their next dictation, so this is
 * long: the text arrives late but in order. After it, the clipboard is used.
 */
export const MODIFIER_WAIT_MS = 30_000;
/** After bringing the target window back to the foreground, before sending keys. */
export const FOCUS_SETTLE_MS = 60;
/** After writing the clipboard, before pasting: clipboard listeners run first. */
export const CLIPBOARD_SETTLE_MS = 30;
/** After the paste shortcut, before restoring: the target app reads the clipboard asynchronously. */
export const PASTE_READ_MS = 150;

const DEFAULT_PASTE_SHORTCUT = 'Ctrl+V';
const TEXT = 'text/plain';

/** How Electron names a raw OS clipboard format. */
const RAW_FORMAT = /^electron application\/osclipboard;format="(.*)"$/i;
/** Marker formats written by `writeClipboardExcluded`; they carry no content of their own. */
const EXCLUSION_FORMATS = new Set([
  'excludeclipboardcontentfrommonitorprocessing',
  'canincludeinclipboardhistory',
  'canuploadtocloudclipboard',
]);
/** OLE bookkeeping: these hold handles owned by the app that copied, and must not be written back. */
const OLE_FORMATS = new Set(['dataobject', 'ole private data']);

const rawFormat = (type: string) => RAW_FORMAT.exec(type)?.[1]?.toLowerCase() ?? null;

type Reason = NonNullable<InsertResult['reason']>;

/** The clipboard as it was before the paste, read out in full. */
interface Snapshot {
  /** One map per clipboard item: MIME type → its data. */
  items: Map<string, Blob>[];
  /** Nothing was on the clipboard at all. */
  empty: boolean;
  /** Set when plain text was the only content: restored without touching clipboard history. */
  textOnly: string | null;
}

interface BlobLike {
  type: string;
  arrayBuffer(): Promise<ArrayBuffer>;
}

function isBlobLike(value: unknown): value is BlobLike {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as BlobLike).arrayBuffer === 'function'
  );
}

const sameText = (a: string, b: string) => a.replace(/\r\n/g, '\n') === b.replace(/\r\n/g, '\n');

export class Inserter {
  private readonly platform: Platform;
  private readonly clipboard: ClipboardLike;
  private readonly makeItem?: (data: Record<string, string | Blob>) => ClipboardItemLike;
  private readonly tapShortcut?: (binding: string) => boolean;
  private readonly sleep: (ms: number) => Promise<void>;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(deps: InserterDeps) {
    this.platform = deps.platform;
    this.clipboard = deps.clipboard;
    this.makeItem = deps.makeItem;
    this.tapShortcut = deps.tapShortcut;
    this.sleep = deps.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  }

  /** Places text at the cursor. Calls are serialised so back-to-back dictations insert in order. Never throws. */
  insert(text: string, options: InsertOptions): Promise<InsertResult> {
    const run = this.queue.then(() => this.run(text, options));
    this.queue = run.catch(() => undefined);
    return run;
  }

  /** Writes text to the clipboard ("copy last transcript"). Fire-and-forget; never throws. */
  copy(text: string): void {
    void this.writeText(text);
  }

  private async run(text: string, options: InsertOptions): Promise<InsertResult> {
    const { platform } = this;
    const { signal, target } = options;
    const aborted: InsertResult = { outcome: 'clipboard', reason: 'failed' };
    try {
      if (signal?.aborted) return aborted;
      if (!text) return { outcome: options.method === 'type' ? 'typed' : 'pasted' };

      const blocked = this.blockedReason(target);
      if (blocked) return await this.toClipboard(text, blocked);

      await platform.waitForModifiersUp(MODIFIER_WAIT_MS, signal);
      if (signal?.aborted) return aborted;
      if (SIDED_MODIFIERS.some((key) => platform.isKeyDown(key) === true)) {
        return await this.toClipboard(text, 'failed');
      }
      // Focus may have moved while transcribing, or while waiting just now.
      if (!(await this.focusTarget(target))) return await this.toClipboard(text, 'target-closed');
      if (signal?.aborted) return aborted;

      if (options.method === 'type' && platform.available) {
        await platform.typeText(text, signal);
        return { outcome: 'typed' };
      }
      return await this.paste(text, options);
    } catch {
      return this.toClipboard(text, 'failed');
    }
  }

  /** Why the text cannot be inserted at all, judged before anything is touched. */
  private blockedReason(target: FocusedApp | null): Reason | null {
    const { platform } = this;
    // Without the platform nothing is known about windows: paste into whatever has focus.
    if (!platform.available) return null;
    if (!target || !platform.isTextTarget(target)) return 'no-target';
    if (!platform.isWindowAlive(target.windowId)) return 'target-closed';
    // Windows drops simulated input aimed at a higher-privileged window.
    if (target.elevated && !platform.selfElevated) return 'elevated';
    return null;
  }

  private async focusTarget(target: FocusedApp | null): Promise<boolean> {
    const { platform } = this;
    if (!platform.available || !target) return true;
    if (platform.getFocusedApp()?.windowId === target.windowId) return true;
    if (!platform.isWindowAlive(target.windowId)) return false;
    if (!platform.focusWindow(target.windowId)) return false;
    await this.sleep(FOCUS_SETTLE_MS);
    return platform.getFocusedApp()?.windowId === target.windowId;
  }

  private async paste(text: string, options: InsertOptions): Promise<InsertResult> {
    const { platform, clipboard } = this;
    // Read out in full before the clipboard is overwritten: clipboard items are lazy.
    const saved = options.restoreClipboard ? await this.snapshot() : null;
    if (!platform.writeClipboardExcluded(text)) await clipboard.writeText(text);
    await this.sleep(CLIPBOARD_SETTLE_MS);
    if (options.signal?.aborted) {
      if (saved) await this.restore(saved);
      return { outcome: 'clipboard', reason: 'failed' };
    }

    const shortcut = normalizeBinding(options.pasteShortcut) ?? DEFAULT_PASTE_SHORTCUT;
    const sent = platform.sendShortcut(shortcut) || this.tapShortcut?.(shortcut) === true;
    // No key was pressed, so the transcript stays on the clipboard for a manual paste.
    if (!sent) return { outcome: 'clipboard', reason: 'failed' };

    await this.sleep(PASTE_READ_MS);
    if (saved) await this.restore(saved, text);
    return { outcome: 'pasted' };
  }

  private async toClipboard(text: string, reason: Reason): Promise<InsertResult> {
    await this.writeText(text);
    return { outcome: 'clipboard', reason };
  }

  /** A plain write: text the user is meant to find on the clipboard belongs in its history. */
  private async writeText(text: string): Promise<void> {
    try {
      await this.clipboard.writeText(text);
    } catch {
      // Nothing more can be done; the caller still has the transcript.
    }
  }

  /** Null when the clipboard cannot be read at all; it is then left holding the transcript. */
  private async snapshot(): Promise<Snapshot | null> {
    let items: ClipboardItemLike[];
    try {
      items = await this.clipboard.read();
    } catch {
      return null;
    }
    const saved: Map<string, Blob>[] = [];
    for (const item of items) {
      const data = new Map<string, Blob>();
      for (const type of item.types) {
        try {
          const value = await item.getType(type);
          // Copy the bytes now. Anything that is not a Blob (a bookmark object) is skipped.
          if (isBlobLike(value)) {
            data.set(type, new Blob([await value.arrayBuffer()], { type: value.type }));
          }
        } catch {
          // A format that cannot be read cannot be restored; keep the rest.
        }
      }
      if (data.size > 0) saved.push(data);
    }
    const types = saved.flatMap((data) => [...data.keys()]);
    const plain = saved.length === 1 ? saved[0]!.get(TEXT) : undefined;
    const onlyText =
      plain !== undefined &&
      types.every((type) => type === TEXT || EXCLUSION_FORMATS.has(rawFormat(type) ?? ''));
    return {
      items: saved,
      empty: items.every((item) => item.types.length === 0),
      textOnly: onlyText ? await plain.text() : null,
    };
  }

  /**
   * Puts the saved content back. With `ours` given, only if the clipboard
   * still holds that text: the user may have copied something meanwhile.
   */
  private async restore(saved: Snapshot, ours?: string): Promise<void> {
    const { platform, clipboard } = this;
    try {
      if (ours !== undefined && !sameText(await clipboard.readText(), ours)) return;
      if (saved.empty) {
        clipboard.clear();
        return;
      }
      if (saved.textOnly !== null) {
        // Already in the clipboard history from when it was first copied; do not add it again.
        if (!platform.writeClipboardExcluded(saved.textOnly)) {
          await clipboard.writeText(saved.textOnly);
        }
        return;
      }
      // Everything first; then without raw OS formats, which cannot all be written back.
      const keepAll = (type: string) => !OLE_FORMATS.has(rawFormat(type) ?? '');
      const keepStandard = (type: string) => rawFormat(type) === null;
      for (const keep of [keepAll, keepStandard]) {
        if (await this.writeItems(saved, keep)) return;
      }
      const text = saved.items.map((data) => data.get(TEXT)).find((blob) => blob !== undefined);
      if (text) await clipboard.writeText(await text.text());
      // Otherwise nothing could be put back: the transcript stays on the clipboard.
    } catch {
      // The paste already happened; a failed restore is not worth reporting.
    }
  }

  private async writeItems(saved: Snapshot, keep: (type: string) => boolean): Promise<boolean> {
    const { makeItem } = this;
    if (!makeItem) return false;
    try {
      const items = saved.items
        .map((data) => Object.fromEntries([...data].filter(([type]) => keep(type))))
        .filter((data) => Object.keys(data).length > 0)
        .map((data) => makeItem(data));
      if (items.length === 0) return false;
      await this.clipboard.write(items);
      return true;
    } catch {
      return false;
    }
  }
}
