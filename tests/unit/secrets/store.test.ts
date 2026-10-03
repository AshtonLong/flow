import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { secretRef, SecretStore, type SecretCipher } from '../../../src/main/secrets/store';

/** Stands in for safeStorage: reversible, and the output does not contain the input. */
function fakeCipher(available = true): SecretCipher & { available: boolean } {
  return {
    available,
    isAvailable() {
      return this.available;
    },
    encrypt(plain) {
      const bytes = Buffer.from(plain, 'utf8').map((byte) => byte ^ 0x5a);
      return Buffer.concat([Buffer.from('v10'), bytes]);
    },
    decrypt(data) {
      if (data.subarray(0, 3).toString() !== 'v10') throw new Error('not ours');
      return Buffer.from(data.subarray(3).map((byte) => byte ^ 0x5a)).toString('utf8');
    },
  };
}

let dir: string;
let file: string;

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'flow-secrets-'));
  file = join(dir, 'Flow', 'secrets.bin');
});

afterEach(async () => {
  await rm(dir, { recursive: true, force: true });
});

describe('secretRef', () => {
  it('extracts the id from a reference', () => {
    expect(secretRef('secret:groq')).toBe('groq');
    expect(secretRef('secret:custom-lan')).toBe('custom-lan');
    expect(secretRef('secret:a:b')).toBe('a:b');
  });

  it('returns null for anything else', () => {
    expect(secretRef(undefined)).toBeNull();
    expect(secretRef('')).toBeNull();
    expect(secretRef('secret:')).toBeNull();
    expect(secretRef('gsk_live_key')).toBeNull();
    expect(secretRef('Secret:groq')).toBeNull();
    expect(secretRef(' secret:groq')).toBeNull();
  });
});

describe('SecretStore', () => {
  it('starts empty when there is no file, without creating one', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    expect(store.ids()).toEqual([]);
    expect(store.has('groq')).toBe(false);
    expect(store.get('groq')).toBeUndefined();
    expect(existsSync(file)).toBe(false);
  });

  it('stores, reads back and lists secrets', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await store.set('groq', 'gsk_abc123');
    await store.set('openai', 'sk-ünïcode-🔑');
    expect(store.has('groq')).toBe(true);
    expect(store.get('groq')).toBe('gsk_abc123');
    expect(store.get('openai')).toBe('sk-ünïcode-🔑');
    expect(store.ids().sort()).toEqual(['groq', 'openai']);
  });

  it('persists across instances', async () => {
    const first = new SecretStore(file, fakeCipher());
    await first.load();
    await first.set('groq', 'gsk_abc123');

    const second = new SecretStore(file, fakeCipher());
    expect(second.has('groq')).toBe(false);
    await second.load();
    expect(second.get('groq')).toBe('gsk_abc123');
  });

  it('writes an envelope of separately encrypted values and never the plaintext', async () => {
    const cipher = fakeCipher();
    const store = new SecretStore(file, cipher);
    await store.load();
    await store.set('groq', 'gsk_abc123');
    await store.set('openai', 'sk-xyz789');

    const text = await readFile(file, 'utf8');
    expect(text).not.toContain('gsk_abc123');
    expect(text).not.toContain('sk-xyz789');
    const envelope = JSON.parse(text) as { v: number; items: Record<string, string> };
    expect(envelope.v).toBe(1);
    expect(Object.keys(envelope.items).sort()).toEqual(['groq', 'openai']);
    expect(Buffer.from(envelope.items.groq!, 'base64')).toEqual(cipher.encrypt('gsk_abc123'));
    expect(Buffer.from(envelope.items.openai!, 'base64')).toEqual(cipher.encrypt('sk-xyz789'));
    expect(existsSync(`${file}.tmp`)).toBe(false);
  });

  it('replaces a value', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await store.set('groq', 'old');
    await store.set('groq', 'new');
    expect(store.get('groq')).toBe('new');
    expect(store.ids()).toEqual(['groq']);
  });

  it('deletes with delete() and with an empty string', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await store.set('groq', 'a');
    await store.set('openai', 'b');
    await store.delete('groq');
    await store.set('openai', '');
    await store.delete('never-existed');
    expect(store.ids()).toEqual([]);

    const reloaded = new SecretStore(file, fakeCipher());
    await reloaded.load();
    expect(reloaded.ids()).toEqual([]);
  });

  it('refuses to store anything when encryption is unavailable', async () => {
    const store = new SecretStore(file, fakeCipher(false));
    await store.load();
    await expect(store.set('groq', 'gsk_abc123')).rejects.toThrow(
      /Secure storage is not available/,
    );
    expect(store.has('groq')).toBe(false);
    expect(existsSync(file)).toBe(false);
  });

  it('can still delete when encryption is unavailable', async () => {
    const cipher = fakeCipher();
    const store = new SecretStore(file, cipher);
    await store.load();
    await store.set('groq', 'a');
    cipher.available = false;
    await store.set('groq', '');
    expect(store.has('groq')).toBe(false);
  });

  it('rejects an empty id', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await expect(store.set('', 'value')).rejects.toThrow();
  });

  it('treats a corrupt file as empty and keeps a copy', async () => {
    const seeded = new SecretStore(file, fakeCipher());
    await seeded.set('groq', 'a');
    for (const garbage of [
      'not json at all',
      '{"v":2,"items":{}}',
      '{"v":1,"items":[1]}',
      'null',
    ]) {
      await writeFile(file, garbage);
      const store = new SecretStore(file, fakeCipher());
      await expect(store.load()).resolves.toBeUndefined();
      expect(store.ids()).toEqual([]);
      expect(await readFile(`${file}.corrupt`, 'utf8')).toBe(garbage);
    }
    // The store is usable again afterwards.
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await store.set('groq', 'fresh');
    const reloaded = new SecretStore(file, fakeCipher());
    await reloaded.load();
    expect(reloaded.get('groq')).toBe('fresh');
  });

  it('returns undefined for a value that no longer decrypts', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await store.set('groq', 'a');
    // For example the file was copied from another Windows account.
    await writeFile(
      file,
      JSON.stringify({ v: 1, items: { groq: Buffer.from('xxxxxx').toString('base64') } }),
    );
    const other = new SecretStore(file, fakeCipher());
    await other.load();
    expect(other.has('groq')).toBe(true);
    expect(other.get('groq')).toBeUndefined();
  });

  it('loads before the first write so existing secrets are not lost', async () => {
    const first = new SecretStore(file, fakeCipher());
    await first.load();
    await first.set('groq', 'a');

    const second = new SecretStore(file, fakeCipher());
    await second.set('openai', 'b'); // no load() call
    const third = new SecretStore(file, fakeCipher());
    await third.load();
    expect(third.ids().sort()).toEqual(['groq', 'openai']);
  });

  it('keeps every value when writes overlap', async () => {
    const store = new SecretStore(file, fakeCipher());
    await store.load();
    await Promise.all(Array.from({ length: 12 }, (_, i) => store.set(`id${i}`, `value${i}`)));
    const reloaded = new SecretStore(file, fakeCipher());
    await reloaded.load();
    expect(reloaded.ids()).toHaveLength(12);
    expect(reloaded.get('id7')).toBe('value7');
  });
});
