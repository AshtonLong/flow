/** The one error type every transcriber, local or cloud, fails with. */

export type TranscribeErrorCode =
  | 'no-key'
  | 'auth'
  | 'network'
  | 'rate-limit'
  | 'http'
  | 'aborted'
  | 'unknown-model'
  | 'bad-response'
  | 'not-installed'
  | 'engine';

export interface TranscribeErrorOptions {
  /** HTTP status, for `auth`, `rate-limit` and `http`. */
  status?: number;
  cause?: unknown;
  /** Replaces the default overlay line for this code. */
  userMessage?: string;
}

function defaultUserMessage(code: TranscribeErrorCode, status?: number): string {
  switch (code) {
    case 'no-key':
      return 'No API key saved for this model';
    case 'auth':
      return 'The API key was rejected';
    case 'network':
      return "Couldn't reach the speech provider";
    case 'rate-limit':
      return 'The provider is rate limiting requests';
    case 'http':
      return status
        ? `The provider returned an error (${status})`
        : 'The provider returned an error';
    case 'aborted':
      return 'Cancelled';
    case 'unknown-model':
      return 'Unknown model';
    case 'bad-response':
      return 'The provider sent an unexpected response';
    case 'not-installed':
      return 'The model is not downloaded';
    case 'engine':
      return 'The speech engine failed';
  }
}

export class TranscribeError extends Error {
  readonly code: TranscribeErrorCode;
  readonly status?: number;
  /** One short line fit for the overlay. */
  readonly userMessage: string;

  constructor(code: TranscribeErrorCode, message: string, opts: TranscribeErrorOptions = {}) {
    super(message, opts.cause === undefined ? undefined : { cause: opts.cause });
    this.name = 'TranscribeError';
    this.code = code;
    this.status = opts.status;
    this.userMessage = opts.userMessage ?? defaultUserMessage(code, opts.status);
  }
}

/** True for the rejection `fetch` produces when its signal is aborted. */
export function isAbortError(err: unknown): boolean {
  if (err instanceof TranscribeError) return err.code === 'aborted';
  const name = (err as { name?: unknown } | null)?.name;
  return name === 'AbortError' || name === 'TimeoutError';
}
