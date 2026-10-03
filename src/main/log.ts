/**
 * File logging. Logs never contain transcript text or audio unless the user
 * turns on debug logging; callers pass such content only through `log.debug`.
 */
import fs from 'node:fs';
import path from 'node:path';

type Level = 'debug' | 'info' | 'warn' | 'error';

const MAX_BYTES = 1_000_000;

let stream: fs.WriteStream | null = null;
let filePath = '';
let debugEnabled = false;
let written = 0;

function open(): void {
  try {
    written = fs.existsSync(filePath) ? fs.statSync(filePath).size : 0;
    stream = fs.createWriteStream(filePath, { flags: 'a' });
    stream.on('error', () => {
      stream = null;
    });
  } catch {
    stream = null;
  }
}

function rotate(): void {
  stream?.end();
  stream = null;
  try {
    fs.renameSync(filePath, `${filePath}.1`);
  } catch {
    // Keep appending if the old file cannot be moved.
  }
  open();
}

function write(level: Level, scope: string, message: string): void {
  if (level === 'debug' && !debugEnabled) return;
  const line = `${new Date().toISOString()} ${level.toUpperCase().padEnd(5)} [${scope}] ${message}\n`;
  if (process.env.FLOW_LOG_STDOUT === '1') process.stdout.write(line);
  if (!stream) return;
  stream.write(line);
  written += line.length;
  if (written > MAX_BYTES) rotate();
}

export const log = {
  /** Opens `flow.log` in the given directory. */
  init(dir: string): void {
    try {
      fs.mkdirSync(dir, { recursive: true });
      filePath = path.join(dir, 'flow.log');
      open();
    } catch {
      stream = null;
    }
  },
  setDebug(enabled: boolean): void {
    debugEnabled = enabled;
  },
  get debugEnabled(): boolean {
    return debugEnabled;
  },
  debug: (scope: string, message: string) => write('debug', scope, message),
  info: (scope: string, message: string) => write('info', scope, message),
  warn: (scope: string, message: string) => write('warn', scope, message),
  error: (scope: string, message: string) => write('error', scope, message),
};
