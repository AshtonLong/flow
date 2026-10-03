/**
 * The overlay window: microphone capture, the waveform and the status pill.
 * It never takes focus and ignores the mouse, so it cannot interrupt typing.
 */
import type { CaptureStart, OverlayAppearance, OverlayPhase, OverlayState } from '@shared/types';
import { CaptureError, startCapture, toCaptureError, type CaptureHandle } from '../common/capture';

const api = window.flowOverlay;

const root = document.documentElement;
const stage = document.getElementById('stage') as HTMLDivElement;
const wave = document.getElementById('wave') as HTMLCanvasElement;
const label = document.getElementById('label') as HTMLSpanElement;
const lock = document.getElementById('lock') as unknown as SVGElement;
const cloud = document.getElementById('cloud') as unknown as SVGElement;
const queued = document.getElementById('queued') as HTMLSpanElement;

let appearance: OverlayAppearance | null = null;
let phase: OverlayPhase = 'hidden';

// ── Capture ────────────────────────────────────────────────────────────────

let handle: CaptureHandle | null = null;
/** Device the open handle belongs to, so a warm handle is not reused for another mic. */
let handleDevice = '';
/** Bumped on every start and stop; a start that resolves late can tell it is stale. */
let generation = 0;
let recording = false;
let announced = false;

function onChunk(pcm: Float32Array): void {
  if (!recording) return;
  if (!announced) {
    announced = true;
    // Audio is flowing: main may now show the pill, which means "speak now".
    api.captureStarted();
  }
  api.captureChunk(pcm);
}

function dropHandle(): void {
  handle?.dispose();
  handle = null;
  handleDevice = '';
}

async function beginCapture(opts: CaptureStart): Promise<void> {
  const gen = ++generation;
  recording = true;
  announced = false;
  try {
    if (handle && !handle.disposed && handleDevice === opts.deviceId) {
      handle.resume(onChunk);
      return;
    }
    dropHandle();
    const opened = await startCapture({ device: opts.deviceId, onChunk });
    if (gen !== generation) {
      // Stopped (or restarted) before the microphone opened.
      if (opts.keepWarm && !handle) {
        await opened.stop(true);
        handle = opened;
        handleDevice = opts.deviceId;
      } else {
        opened.dispose();
      }
      return;
    }
    handle = opened;
    handleDevice = opts.deviceId;
    opened.onEnded = () => {
      if (handle === opened) dropHandle();
      if (recording) {
        recording = false;
        api.captureError('device-missing', 'Microphone disconnected');
      }
    };
  } catch (err) {
    if (gen !== generation) return;
    recording = false;
    const failure = err instanceof CaptureError ? err : toCaptureError(err);
    api.captureError(failure.reason, failure.message);
  }
}

let keepWarm = false;

async function endCapture(): Promise<void> {
  generation++;
  if (!recording) {
    api.captureStopped();
    return;
  }
  const current = handle;
  if (current && !current.disposed) {
    // Chunks keep flowing during the flush, so recording stays on until it completes.
    await current.stop(keepWarm);
    if (!keepWarm && handle === current) dropHandle();
  }
  recording = false;
  api.captureStopped();
}

api.onCaptureStart((opts) => {
  keepWarm = opts.keepWarm;
  void beginCapture(opts);
});
api.onCaptureStop(() => void endCapture());

// A warm microphone must not outlive a device change (unplug, new default).
navigator.mediaDevices.addEventListener('devicechange', () => {
  if (!recording) dropHandle();
});

// ── Waveform ───────────────────────────────────────────────────────────────

const BARS = 13;
const levels = new Float32Array(BARS);
let raf = 0;

function drawWave(): void {
  raf = 0;
  if (phase !== 'listening' || !appearance?.showWaveform) return;
  const ctx = wave.getContext('2d');
  const analyser = handle?.analyser;
  if (!ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const width = wave.clientWidth;
  const height = wave.clientHeight;
  if (wave.width !== Math.round(width * dpr)) {
    wave.width = Math.round(width * dpr);
    wave.height = Math.round(height * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);

  if (analyser) {
    const data = new Uint8Array(analyser.frequencyBinCount);
    analyser.getByteFrequencyData(data);
    // Speech energy sits in the low bins (0–4 kHz at a 16 kHz context).
    const usable = Math.floor(data.length * 0.5);
    for (let i = 0; i < BARS; i++) {
      const from = Math.floor((i / BARS) * usable);
      const to = Math.max(from + 1, Math.floor(((i + 1) / BARS) * usable));
      let sum = 0;
      for (let j = from; j < to; j++) sum += data[j]!;
      const target = Math.min(1, sum / (to - from) / 170);
      // Fast attack, slow release keeps the bars lively without flicker.
      levels[i] = target > levels[i]! ? target : levels[i]! * 0.82 + target * 0.18;
    }
  }

  // Thirteen bars, 3 px wide with a 2.5 px gap, in the voice colour: the site's pill.
  const gap = (width / 69) * 2.5;
  const barWidth = (width - gap * (BARS - 1)) / BARS;
  ctx.fillStyle = getComputedStyle(root).getPropertyValue('--voice').trim() || '#ff5a1f';
  for (let i = 0; i < BARS; i++) {
    // Mirror around the centre so the shape reads as a voice, not a spectrum.
    const mirrored = levels[Math.abs(i - (BARS - 1) / 2) | 0]!;
    const h = Math.max(height * 0.14, mirrored * height);
    const x = i * (barWidth + gap);
    ctx.beginPath();
    ctx.roundRect(x, (height - h) / 2, barWidth, h, barWidth / 2);
    ctx.fill();
  }
  raf = requestAnimationFrame(drawWave);
}

// ── Sounds ─────────────────────────────────────────────────────────────────

let audio: AudioContext | null = null;

function blip(from: number, to: number, ms: number, volume = 0.05): void {
  if (!appearance?.sounds) return;
  try {
    audio ??= new AudioContext();
    const t = audio.currentTime;
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.type = 'sine';
    osc.frequency.setValueAtTime(from, t);
    osc.frequency.exponentialRampToValueAtTime(to, t + ms / 1000);
    gain.gain.setValueAtTime(0, t);
    gain.gain.linearRampToValueAtTime(volume, t + 0.008);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + ms / 1000);
    osc.connect(gain).connect(audio.destination);
    osc.start(t);
    osc.stop(t + ms / 1000 + 0.02);
  } catch {
    // Sounds are a nicety; never let them break capture.
  }
}

function playTransition(previous: OverlayPhase, next: OverlayPhase): void {
  if (next === 'listening' && previous !== 'listening') blip(520, 780, 90);
  else if (previous === 'listening' && next === 'transcribing') blip(700, 480, 90);
  else if (next === 'error') blip(300, 200, 180, 0.06);
}

// ── State ──────────────────────────────────────────────────────────────────

/** What the pill says in each state when main sends no message of its own. */
const LABELS: Record<OverlayPhase, string> = {
  hidden: '',
  listening: 'Listening',
  transcribing: 'Transcribing',
  done: 'Typed',
  error: '',
  notice: '',
};

function applyState(state: OverlayState): void {
  const previous = phase;
  phase = state.phase;
  stage.dataset.phase = state.phase;
  // While fading out, keep the last words so the pill does not collapse first.
  if (state.phase !== 'hidden') label.textContent = state.message ?? LABELS[state.phase];
  lock.toggleAttribute('hidden', !(state.phase === 'listening' && state.locked));
  cloud.toggleAttribute('hidden', !state.cloud);
  const waiting = state.queued ?? 0;
  const showQueue = state.phase === 'listening' && waiting > 0;
  queued.toggleAttribute('hidden', !showQueue);
  queued.textContent = showQueue ? `+${waiting}` : '';
  if (previous !== phase) playTransition(previous, phase);
  if (phase === 'listening' && !raf) raf = requestAnimationFrame(drawWave);
  if (phase !== 'listening') levels.fill(0);
}

function applyAppearance(next: OverlayAppearance): void {
  appearance = next;
  stage.dataset.style = next.style;
  stage.dataset.size = next.size;
  stage.dataset.waveform = String(next.showWaveform);
  // The pill is black in both themes, as on the site; only contrast and motion follow Windows.
  root.dataset.contrast = next.theme.highContrast ? 'high' : 'normal';
  root.dataset.motion = next.theme.reducedMotion ? 'reduced' : 'full';
}

api.onState(applyState);
api.onAppearance(applyAppearance);
applyState({ phase: 'hidden', cloud: false });
api.ready();
