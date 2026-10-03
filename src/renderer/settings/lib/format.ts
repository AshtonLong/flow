/** Display formatting. Pure functions, covered by `tests/unit/settings`. */

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB';
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes >= 1e3) return `${Math.round(bytes / 1e3)} KB`;
  return `${Math.round(bytes)} B`;
}

/** Device memory, in binary units as Windows reports it: `12 GB`. */
export function formatMemory(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${Number(gb.toFixed(1))} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
}

export function formatSpeed(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return '';
  if (bytesPerSecond >= 1e6) return `${(bytesPerSecond / 1e6).toFixed(1)} MB/s`;
  return `${Math.max(1, Math.round(bytesPerSecond / 1e3))} KB/s`;
}

/** A list price: `$0.04`, `$0.111`, `$0.30`, `$1`. */
export function formatPrice(usd: number): string {
  if (usd === 0) return 'Free';
  if (Number.isInteger(usd)) return `$${usd}`;
  const fixed = usd.toFixed(3);
  return `$${fixed.endsWith('0') ? fixed.slice(0, -1) : fixed}`;
}

/** The cost of one short request, down to fractions of a cent: `0.011¢`, `1.2¢`, `$0.25`. */
export function formatCost(usd: number): string {
  if (!Number.isFinite(usd) || usd <= 0) return '0¢';
  const cents = usd * 100;
  if (cents >= 10) return `$${usd.toFixed(2)}`;
  if (cents >= 1) return `${cents.toFixed(1)}¢`;
  // Two significant digits, without exponent notation.
  const digits = Math.min(8, Math.max(2, 1 - Math.floor(Math.log10(cents))));
  return `${Number(cents.toFixed(digits))}¢`;
}

export function formatMs(ms: number): string {
  return `${Math.round(ms).toLocaleString('en-US')} ms`;
}

/** Audio or elapsed time: `0.8 s`, `12.4 s`, `1 min 5 s`. */
export function formatDuration(ms: number): string {
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds - minutes * 60);
  return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
}

const TIME = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const DAY = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });
const DAY_YEAR = new Intl.DateTimeFormat(undefined, {
  day: 'numeric',
  month: 'short',
  year: 'numeric',
});

function startOfDay(epoch: number): number {
  const d = new Date(epoch);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** `Today 14:32`, `Yesterday 09:10`, `28 Sep 14:02`, `28 Sep 2025 14:02`. */
export function formatWhen(epoch: number, now: number = Date.now()): string {
  const days = Math.round((startOfDay(now) - startOfDay(epoch)) / 86_400_000);
  const time = TIME.format(epoch);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  const sameYear = new Date(epoch).getFullYear() === new Date(now).getFullYear();
  return `${(sameYear ? DAY : DAY_YEAR).format(epoch)} ${time}`;
}

/** `2026-10-03` → `3 October 2026`. Falls back to the input when it does not parse. */
export function formatDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return new Intl.DateTimeFormat(undefined, {
    day: 'numeric',
    month: 'long',
    year: 'numeric',
  }).format(d);
}

export function plural(count: number, one: string, many: string = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

const PROMPT_LABELS: Record<string, string> = {
  clean: 'Clean',
  email: 'Email',
  casual: 'Casual',
  code_comment: 'Code comment',
  verbatim: 'Verbatim',
};

/** Display name for a prompt key; user prompts show their key as written. */
export function promptLabel(name: string): string {
  return PROMPT_LABELS[name] ?? name;
}

/** A prompt key for the config file from a name the user typed: `Stand-up notes` → `stand_up_notes`. */
export function promptKey(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '');
}

/** Catalog descriptions come without a full stop; add one so sentences can follow. */
export function sentence(text: string): string {
  const trimmed = text.trim();
  return /[.!?…]$/.test(trimmed) || trimmed === '' ? trimmed : `${trimmed}.`;
}

export function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return typeof err === 'string' ? err : 'Something went wrong';
}
