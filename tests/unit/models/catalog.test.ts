import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { CatalogSchema } from '@shared/catalog';
import { DEFAULT_CONFIG, DEFAULT_LOCAL_MODEL } from '@shared/config';
import {
  CATALOG_PUBLIC_KEY,
  CATALOG_SIG_URL,
  CATALOG_URL,
  parseCatalog,
  verifyCatalog,
} from '../../../src/main/models/catalog';
import { encode, localEntry, randomBytes, signBytes, testCatalog, testKey } from './helpers';

const bundledPath = resolve(__dirname, '../../../resources/catalog.json');
const bundledBytes = readFileSync(bundledPath);
const bundledSignature = readFileSync(`${bundledPath}.sig`, 'utf8');

describe('bundled catalog', () => {
  const catalog = parseCatalog(bundledBytes);

  it('validates against the schema', () => {
    expect(CatalogSchema.safeParse(JSON.parse(bundledBytes.toString('utf8'))).success).toBe(true);
    expect(catalog.version).toBe(1);
    expect(catalog.asOf).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });

  it('carries a signature that verifies with the embedded public key', () => {
    expect(verifyCatalog(bundledBytes, bundledSignature)).toBe(true);
    expect(verifyCatalog(bundledBytes, bundledSignature, CATALOG_PUBLIC_KEY)).toBe(true);
  });

  it('still verifies after a checkout rewrote its line endings', () => {
    const text = bundledBytes.toString('utf8').replaceAll('\r\n', '\n');
    expect(verifyCatalog(Buffer.from(text, 'utf8'), bundledSignature)).toBe(true);
    const crlf = Buffer.from(text.replaceAll('\n', '\r\n'), 'utf8');
    expect(verifyCatalog(crlf, bundledSignature)).toBe(true);
  });

  it('has unique ids and known providers', () => {
    const ids = [...catalog.local, ...catalog.cloud, ...catalog.cleanup].map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
    const providers = new Set(catalog.providers.map((p) => p.id));
    expect(providers.size).toBe(catalog.providers.length);
    for (const entry of [...catalog.cloud, ...catalog.cleanup]) {
      expect(providers.has(entry.provider), entry.id).toBe(true);
      expect(entry.id).toBe(`${entry.provider}/${entry.model}`);
    }
  });

  it('lists local models with https URLs that end in their file name', () => {
    expect(catalog.local.length).toBeGreaterThan(0);
    for (const entry of catalog.local) {
      const url = new URL(entry.url);
      expect(url.protocol).toBe('https:');
      expect(url.hostname).toBe('huggingface.co');
      expect(url.pathname.endsWith(`/resolve/main/${entry.file}`), entry.id).toBe(true);
      expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(entry.sizeBytes).toBeGreaterThan(0);
      expect(entry.id.includes('/')).toBe(false);
    }
  });

  it('contains the config defaults', () => {
    expect(catalog.local.some((e) => e.id === DEFAULT_LOCAL_MODEL)).toBe(true);
    expect(catalog.cleanup.some((e) => e.id === DEFAULT_CONFIG.cleanup.llm_model)).toBe(true);
    expect(catalog.cloud.filter((e) => e.recommended).map((e) => e.id)).toEqual([
      'groq/whisper-large-v3-turbo',
    ]);
    expect(catalog.cleanup.filter((e) => e.recommended)).toHaveLength(1);
  });
});

describe('verifyCatalog', () => {
  const key = testKey();
  const bytes = encode(testCatalog([localEntry('a', randomBytes(64))]));
  const signature = signBytes(bytes, key);

  it('accepts a good signature', () => {
    expect(verifyCatalog(bytes, signature, key.publicPem)).toBe(true);
    expect(verifyCatalog(bytes, `${signature}\n`, key.publicPem)).toBe(true);
  });

  it('rejects tampered bytes', () => {
    const tampered = Buffer.from(bytes);
    tampered[tampered.indexOf('models.test')] = 'n'.charCodeAt(0);
    expect(verifyCatalog(tampered, signature, key.publicPem)).toBe(false);
  });

  it('rejects a signature made with another key', () => {
    expect(verifyCatalog(bytes, signature, testKey().publicPem)).toBe(false);
    expect(verifyCatalog(bytes, signature)).toBe(false);
    expect(verifyCatalog(bundledBytes, bundledSignature, key.publicPem)).toBe(false);
  });

  it('returns false, without throwing, for malformed input', () => {
    expect(verifyCatalog(bytes, '', key.publicPem)).toBe(false);
    expect(verifyCatalog(bytes, 'not base64 !!', key.publicPem)).toBe(false);
    expect(verifyCatalog(bytes, signature, 'not a key')).toBe(false);
  });
});

describe('parseCatalog', () => {
  const entry = localEntry('a', randomBytes(64));

  it('accepts bytes or a string', () => {
    const catalog = testCatalog([entry]);
    expect(parseCatalog(encode(catalog))).toEqual(catalog);
    expect(parseCatalog(JSON.stringify(catalog))).toEqual(catalog);
  });

  it('throws on invalid JSON and on a schema failure', () => {
    expect(() => parseCatalog('{')).toThrow();
    expect(() => parseCatalog('{"version":1}')).toThrow();
    const broken = { ...testCatalog([entry]), local: [{ ...entry, sizeBytes: 'big' }] };
    expect(() => parseCatalog(JSON.stringify(broken))).toThrow();
  });

  it('rejects a download URL that is not https', () => {
    const catalog = testCatalog([{ ...entry, url: 'http://models.test/a.gguf' }]);
    expect(() => parseCatalog(encode(catalog))).toThrow(/not https/);
  });

  it('rejects a file name that could leave the models directory', () => {
    for (const file of ['../a.gguf', '..\\a.gguf', 'sub/a.gguf', 'C:a.gguf', '..', '']) {
      const catalog = testCatalog([{ ...entry, file }]);
      expect(() => parseCatalog(encode(catalog)), file).toThrow(/unsafe file name/);
    }
  });

  it('rejects a manifest version newer than this build', () => {
    const catalog = { ...testCatalog([entry]), version: 2 };
    expect(() => parseCatalog(encode(catalog))).toThrow(/newer/);
  });
});

describe('remote location', () => {
  it('points at the repository over https', () => {
    expect(CATALOG_URL).toBe(
      'https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json',
    );
    expect(CATALOG_SIG_URL).toBe(`${CATALOG_URL}.sig`);
  });
});
