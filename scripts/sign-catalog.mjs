#!/usr/bin/env node
/**
 * Signs resources/catalog.json with the Ed25519 catalog key and writes the
 * detached signature (base64) to resources/catalog.json.sig.
 *
 *   node scripts/sign-catalog.mjs              sign the catalog
 *   node scripts/sign-catalog.mjs --generate   create a new key pair and print the public key
 *   node scripts/sign-catalog.mjs --public     print the public key of the existing private key
 *
 * The private key lives OUTSIDE the repository: at the path in FLOW_CATALOG_KEY,
 * or ~/.flow/catalog-signing-key.pem. Never commit it.
 */
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign, verify } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const catalogPath = join(repoRoot, 'resources', 'catalog.json');
const signaturePath = `${catalogPath}.sig`;
const keyPath = resolve(
  process.env.FLOW_CATALOG_KEY || join(homedir(), '.flow', 'catalog-signing-key.pem'),
);

function fail(message) {
  console.error(`sign-catalog: ${message}`);
  process.exit(1);
}

function publicPem(privateKey) {
  return createPublicKey(privateKey).export({ type: 'spki', format: 'pem' }).toString();
}

function loadPrivateKey() {
  if (!existsSync(keyPath)) {
    fail(`no private key at ${keyPath}. Run with --generate, or set FLOW_CATALOG_KEY.`);
  }
  const key = createPrivateKey(readFileSync(keyPath));
  if (key.asymmetricKeyType !== 'ed25519') fail(`${keyPath} is not an Ed25519 key.`);
  return key;
}

// `relative` is absolute when the key is on another drive, which is outside the repo too.
const fromRepo = relative(repoRoot, keyPath);
if (!fromRepo.startsWith('..') && !isAbsolute(fromRepo)) {
  fail(`the private key path (${keyPath}) is inside the repository. Keep it outside.`);
}

const args = process.argv.slice(2);

if (args.includes('--generate')) {
  if (existsSync(keyPath)) fail(`${keyPath} already exists; refusing to overwrite it.`);
  const { privateKey } = generateKeyPairSync('ed25519');
  mkdirSync(dirname(keyPath), { recursive: true });
  // `wx` fails rather than overwrites if the file appeared in the meantime.
  writeFileSync(keyPath, privateKey.export({ type: 'pkcs8', format: 'pem' }), {
    flag: 'wx',
    mode: 0o600,
  });
  console.log(`Private key written to ${keyPath}. Back it up; never commit it.`);
  console.log('Public key (embed as CATALOG_PUBLIC_KEY in src/main/models/catalog.ts):\n');
  console.log(publicPem(privateKey));
  process.exit(0);
}

if (args.includes('--public')) {
  console.log(publicPem(loadPrivateKey()));
  process.exit(0);
}

const privateKey = loadPrivateKey();
let bytes = readFileSync(catalogPath);

// The signature covers the exact bytes, so pin the line endings to LF before signing.
if (bytes.includes('\r\n')) {
  bytes = Buffer.from(bytes.toString('utf8').replaceAll('\r\n', '\n'), 'utf8');
  writeFileSync(catalogPath, bytes);
  console.log('Normalised resources/catalog.json to LF line endings.');
}

try {
  JSON.parse(bytes.toString('utf8'));
} catch (error) {
  fail(`resources/catalog.json is not valid JSON: ${error.message}`);
}

const signature = sign(null, bytes, privateKey);
if (!verify(null, bytes, createPublicKey(privateKey), signature)) fail('self-check failed.');
writeFileSync(signaturePath, `${signature.toString('base64')}\n`);

console.log(`Signed ${relative(repoRoot, catalogPath)} (${bytes.length} bytes).`);
console.log(`Signature written to ${relative(repoRoot, signaturePath)}.`);
console.log(`Public key:\n${publicPem(privateKey)}`);
