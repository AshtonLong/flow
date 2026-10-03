/** One HTTP round trip for a cloud adapter, with every failure mapped to a TranscribeError. */
import { TranscribeError } from '../errors';

export interface HttpContext {
  /** Provider name for error messages, e.g. `Groq`. */
  provider: string;
  fetch: typeof fetch;
  /** The caller's cancel signal (Esc). */
  signal: AbortSignal;
  /** Gives up on a provider that never answers, so the local fallback can run. */
  timeoutMs: number;
}

/** Pulls the human-readable message out of the error body shapes the providers use. */
export function errorDetail(body: string): string {
  const text = body.trim();
  if (!text) return '';
  try {
    const json = JSON.parse(text) as Record<string, unknown>;
    const candidates = [
      (json.error as Record<string, unknown> | undefined)?.message,
      json.error,
      (json.detail as Record<string, unknown> | undefined)?.message,
      json.detail,
      json.message,
      json.err_msg,
    ];
    const found = candidates.find((c): c is string => typeof c === 'string' && c.trim() !== '');
    if (found) return found.trim().slice(0, 300);
  } catch {
    // Not JSON: fall through to the raw text.
  }
  return text.replace(/\s+/g, ' ').slice(0, 300);
}

/** The TranscribeError for a non-2xx response. */
export function httpError(provider: string, status: number, body: string): TranscribeError {
  const detail = errorDetail(body);
  const message = `${provider} responded ${status}${detail ? `: ${detail}` : ''}`;
  if (status === 401 || status === 403) {
    return new TranscribeError('auth', message, {
      status,
      userMessage: `${provider} rejected the API key`,
    });
  }
  if (status === 429) {
    return new TranscribeError('rate-limit', message, {
      status,
      userMessage: `${provider} rate limit reached`,
    });
  }
  return new TranscribeError('http', message, {
    status,
    userMessage: `${provider} returned an error (${status})`,
  });
}

/** Sends the request and returns the parsed JSON body. Rejects only with TranscribeError. */
export async function requestJson(
  ctx: HttpContext,
  url: string,
  init: RequestInit,
): Promise<unknown> {
  if (ctx.signal.aborted) throw new TranscribeError('aborted', 'Cancelled');
  const timeout = new AbortController();
  const timer = setTimeout(() => timeout.abort(), ctx.timeoutMs);
  let status: number;
  let ok: boolean;
  let body: string;
  try {
    const response = await ctx.fetch(url, {
      ...init,
      signal: AbortSignal.any([ctx.signal, timeout.signal]),
    });
    status = response.status;
    ok = response.ok;
    body = await response.text();
  } catch (err) {
    if (ctx.signal.aborted) throw new TranscribeError('aborted', 'Cancelled', { cause: err });
    if (timeout.signal.aborted) {
      throw new TranscribeError('network', `${ctx.provider} did not respond in time`, {
        cause: err,
        userMessage: `${ctx.provider} timed out`,
      });
    }
    const reason = err instanceof Error ? err.message : String(err);
    throw new TranscribeError('network', `Request to ${ctx.provider} failed: ${reason}`, {
      cause: err,
      userMessage: `Couldn't reach ${ctx.provider}`,
    });
  } finally {
    clearTimeout(timer);
  }
  if (!ok) throw httpError(ctx.provider, status, body);
  try {
    return JSON.parse(body) as unknown;
  } catch (err) {
    throw new TranscribeError('bad-response', `${ctx.provider} sent a response that is not JSON`, {
      status,
      cause: err,
    });
  }
}
