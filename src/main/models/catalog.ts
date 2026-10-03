/** Parsing and Ed25519 signature verification of the model catalog manifest. */
import { createPublicKey, verify } from 'node:crypto';
import { CatalogSchema, type Catalog } from '@shared/catalog';

/**
 * Public half of the catalog signing key (Ed25519, SPKI PEM). The private half
 * lives outside the repository; see `scripts/sign-catalog.mjs`.
 */
export const CATALOG_PUBLIC_KEY = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAPaBKDIhOEL41T4LvtPwy0i8S3/3h67ZcVVAYRzVglmA=
-----END PUBLIC KEY-----
`;

/** Where the refreshed catalog and its detached signature are published. */
export const CATALOG_URL =
  'https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json';
export const CATALOG_SIG_URL = `${CATALOG_URL}.sig`;

/** Highest manifest `version` this build understands. */
export const CATALOG_VERSION = 1;

/** A catalog with nothing in it, used only if the bundled file is unreadable. */
export const EMPTY_CATALOG: Catalog = {
  version: CATALOG_VERSION,
  asOf: '',
  providers: [],
  local: [],
  cloud: [],
  cleanup: [],
};

const ED25519_SIGNATURE_BYTES = 64;

function hasCrlf(bytes: Uint8Array): boolean {
  for (let i = 1; i < bytes.length; i++) {
    if (bytes[i] === 0x0a && bytes[i - 1] === 0x0d) return true;
  }
  return false;
}

/**
 * True if `signatureBase64` is a valid signature of `bytes` under the catalog key.
 * Never throws: a malformed key or signature is simply not valid.
 */
export function verifyCatalog(
  bytes: Uint8Array,
  signatureBase64: string,
  publicKeyPem: string = CATALOG_PUBLIC_KEY,
): boolean {
  try {
    const signature = Buffer.from(signatureBase64.trim(), 'base64');
    if (signature.length !== ED25519_SIGNATURE_BYTES) return false;
    const key = createPublicKey(publicKeyPem);
    if (key.asymmetricKeyType !== 'ed25519') return false;
    if (verify(null, bytes, key, signature)) return true;
    // The catalog is signed with LF line endings. A Windows checkout may have
    // rewritten them as CRLF, which changes the bytes but not the JSON.
    if (!hasCrlf(bytes)) return false;
    const lf = Buffer.from(Buffer.from(bytes).toString('utf8').replaceAll('\r\n', '\n'), 'utf8');
    return verify(null, lf, key, signature);
  } catch {
    return false;
  }
}

/**
 * Parses and validates a catalog manifest. Throws on invalid JSON, a schema
 * failure, a manifest version newer than this build, or a local model whose
 * download URL or file name is unsafe.
 */
export function parseCatalog(bytes: Uint8Array | string): Catalog {
  const text = typeof bytes === 'string' ? bytes : Buffer.from(bytes).toString('utf8');
  const catalog = CatalogSchema.parse(JSON.parse(text));
  if (catalog.version > CATALOG_VERSION) {
    throw new Error(`Catalog version ${catalog.version} is newer than this app understands`);
  }
  for (const entry of catalog.local) {
    if (!isHttpsUrl(entry.url)) {
      throw new Error(`Catalog entry ${entry.id} has a download URL that is not https`);
    }
    // Files are stored flat in the models directory; a path here could escape it.
    if (entry.file === '' || /^\.+$/.test(entry.file) || /[\\/:]/.test(entry.file)) {
      throw new Error(`Catalog entry ${entry.id} has an unsafe file name`);
    }
    if (!/^[0-9a-f]{64}$/i.test(entry.sha256)) {
      throw new Error(`Catalog entry ${entry.id} has a malformed SHA-256`);
    }
  }
  return catalog;
}

/** True for a well-formed `https:` URL. */
export function isHttpsUrl(url: string): boolean {
  try {
    return new URL(url).protocol === 'https:';
  } catch {
    return false;
  }
}
