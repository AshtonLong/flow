import { describe, expect, it } from 'vitest';
import {
  TranscribeError,
  type TranscribeErrorCode,
  isAbortError,
} from '../../../src/main/transcribe/errors';
import { errorDetail, httpError } from '../../../src/main/transcribe/cloud/http';
import { limitHints } from '../../../src/main/transcribe/cloud/hints';

describe('TranscribeError', () => {
  it('carries code, status, cause and a one-line user message', () => {
    const cause = new Error('socket hang up');
    const err = new TranscribeError('http', 'Groq responded 500: boom', { status: 500, cause });
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe('TranscribeError');
    expect(err.code).toBe('http');
    expect(err.status).toBe(500);
    expect(err.cause).toBe(cause);
    expect(err.message).toBe('Groq responded 500: boom');
    expect(err.userMessage).toBe('The provider returned an error (500)');
  });

  it('has a short user message for every code', () => {
    const codes: TranscribeErrorCode[] = [
      'no-key',
      'auth',
      'network',
      'rate-limit',
      'http',
      'aborted',
      'unknown-model',
      'bad-response',
      'not-installed',
      'engine',
    ];
    for (const code of codes) {
      const { userMessage } = new TranscribeError(code, 'detail');
      expect(userMessage.length).toBeGreaterThan(0);
      expect(userMessage.length).toBeLessThanOrEqual(60);
      expect(userMessage).not.toContain('\n');
    }
  });

  it('lets the thrower supply the user message', () => {
    const err = new TranscribeError('auth', 'x', { userMessage: 'Groq rejected the API key' });
    expect(err.userMessage).toBe('Groq rejected the API key');
  });
});

describe('isAbortError', () => {
  it.each([
    [new DOMException('aborted', 'AbortError'), true],
    [new TranscribeError('aborted', 'Cancelled'), true],
    [new TranscribeError('network', 'x'), false],
    [new TypeError('fetch failed'), false],
    [null, false],
    ['AbortError', false],
  ])('%s → %s', (err, expected) => {
    expect(isAbortError(err)).toBe(expected);
  });
});

describe('httpError', () => {
  it.each([
    [401, 'auth'],
    [403, 'auth'],
    [429, 'rate-limit'],
    [400, 'http'],
    [404, 'http'],
    [500, 'http'],
    [503, 'http'],
  ])('maps %i to %s', (status, code) => {
    const err = httpError('Groq', status, '{"error":{"message":"nope"}}');
    expect(err.code).toBe(code);
    expect(err.status).toBe(status);
    expect(err.message).toBe(`Groq responded ${status}: nope`);
    expect(err.userMessage).toContain('Groq');
  });
});

describe('errorDetail', () => {
  it.each([
    [
      'OpenAI and Groq',
      '{"error":{"message":"Invalid API Key","type":"invalid_request_error"}}',
      'Invalid API Key',
    ],
    ['a string error', '{"error":"bad model"}', 'bad model'],
    [
      'ElevenLabs',
      '{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}',
      'Invalid API key',
    ],
    ['FastAPI servers', '{"detail":"Not Found"}', 'Not Found'],
    ['Mistral', '{"message":"Unauthorized","request_id":"abc"}', 'Unauthorized'],
    [
      'Deepgram',
      '{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials."}',
      'Invalid credentials.',
    ],
    ['plain text', 'upstream\n  timeout', 'upstream timeout'],
    ['nothing', '', ''],
  ])('reads %s', (_name, body, expected) => {
    expect(errorDetail(body)).toBe(expected);
  });

  it('caps a long body', () => {
    expect(errorDetail('x'.repeat(5000))).toHaveLength(300);
  });
});

describe('limitHints', () => {
  it('trims, collapses spaces and drops blanks and duplicates', () => {
    expect(limitHints([' Tauri ', 'tauri', '', '  ', 'Baker   Street'])).toEqual([
      'Tauri',
      'Baker Street',
    ]);
  });

  it('caps the number of terms, keeping the first ones', () => {
    expect(limitHints(['a', 'b', 'c', 'd'], { maxTerms: 2 })).toEqual(['a', 'b']);
  });

  it('caps the total length with whole terms only', () => {
    expect(limitHints(['alpha', 'a-very-long-term', 'beta'], { maxTotalChars: 12 })).toEqual([
      'alpha',
      'beta',
    ]);
  });

  it('drops terms that are too long or have too many words', () => {
    expect(
      limitHints(['short', 'x'.repeat(50), 'one two three four five six'], {
        maxTermChars: 49,
        maxWords: 5,
      }),
    ).toEqual(['short']);
  });
});
