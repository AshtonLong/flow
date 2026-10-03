/**
 * API keys, encrypted one by one and kept in `secrets.bin`. `config.toml` only
 * holds references such as `api_key = "secret:groq"`. The cipher is injected
 * (Electron safeStorage in the app) so this module has no Electron import.
 */
import { copyFile, readFile } from 'node:fs/promises';
import { writeFileAtomic } from '../config/atomic-write';

export interface SecretCipher {
  isAvailable(): boolean;
  /** `safeStorage.encryptString` */
  encrypt(plain: string): Buffer;
  /** `safeStorage.decryptString` */
  decrypt(data: Buffer): string;
}

interface Envelope {
  v: 1;
  /** Secret id → base64 ciphertext. */
  items: Record<string, string>;
}

const REF_PREFIX = 'secret:';

/** `"secret:groq"` → `"groq"`; anything else → null. */
export function secretRef(value: string | undefined): string | null {
  if (typeof value !== 'string' || !value.startsWith(REF_PREFIX)) return null;
  const id = value.slice(REF_PREFIX.length);
  return id === '' ? null : id;
}

function parseEnvelope(text: string): Map<string, Buffer> {
  const data = JSON.parse(text) as Partial<Envelope> | null;
  const items = data?.items;
  if (data?.v !== 1 || items === null || typeof items !== 'object' || Array.isArray(items)) {
    throw new Error('Unrecognised secrets file');
  }
  const out = new Map<string, Buffer>();
  for (const [id, encoded] of Object.entries(items)) {
    if (typeof encoded !== 'string') throw new Error('Unrecognised secrets file');
    out.set(id, Buffer.from(encoded, 'base64'));
  }
  return out;
}

export class SecretStore {
  /** Ciphertext only: values are decrypted on demand and never cached in the clear. */
  private items = new Map<string, Buffer>();
  private loaded = false;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly filePath: string,
    private readonly cipher: SecretCipher,
  ) {}

  /** Reads the file. Never throws: a corrupt file is copied to `<file>.corrupt` and ignored. */
  async load(): Promise<void> {
    this.loaded = true;
    let text: string;
    try {
      text = await readFile(this.filePath, 'utf8');
    } catch {
      // Missing or unreadable: start empty.
      this.items = new Map();
      return;
    }
    try {
      this.items = parseEnvelope(text);
    } catch {
      this.items = new Map();
      // Kept for recovery, since the next `set` overwrites the original.
      await copyFile(this.filePath, `${this.filePath}.corrupt`).catch(() => undefined);
    }
  }

  has(id: string): boolean {
    return this.items.has(id);
  }

  /** The decrypted secret, or `undefined` if it is missing or cannot be decrypted. Main process only. */
  get(id: string): string | undefined {
    const data = this.items.get(id);
    if (!data) return undefined;
    try {
      return this.cipher.decrypt(data);
    } catch {
      return undefined;
    }
  }

  /** Stores a secret; an empty string deletes it. Throws if encryption is unavailable. */
  async set(id: string, value: string): Promise<void> {
    if (value === '') return this.delete(id);
    if (id === '') throw new Error('A secret needs an id.');
    if (!this.cipher.isAvailable()) {
      // Refuse rather than fall back to storing the key in the clear.
      throw new Error('Secure storage is not available on this system, so the key was not saved.');
    }
    const data = this.cipher.encrypt(value);
    await this.mutate(() => this.items.set(id, data));
  }

  async delete(id: string): Promise<void> {
    await this.mutate(() => this.items.delete(id));
  }

  ids(): string[] {
    return [...this.items.keys()];
  }

  /** Applies a change and writes the file, one change at a time. */
  private mutate(change: () => unknown): Promise<void> {
    const run = this.queue.then(async () => {
      // A store that was never loaded would otherwise overwrite the secrets already on disk.
      if (!this.loaded) await this.load();
      // `Map.delete` returns false when there was nothing to delete: nothing to write.
      if (change() === false) return;
      const envelope: Envelope = { v: 1, items: {} };
      for (const [id, data] of this.items) envelope.items[id] = data.toString('base64');
      await writeFileAtomic(this.filePath, JSON.stringify(envelope));
    });
    this.queue = run.catch(() => undefined);
    return run;
  }
}
