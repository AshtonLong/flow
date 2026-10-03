/**
 * The engine: an Electron utility process that owns voice detection and local
 * models. It is started on the first dictation and exits when the main process
 * decides the model has been idle long enough, which returns all its memory.
 */
import type {
  EngineDevice,
  EngineLoadResult,
  EngineModelRef,
  EngineProbeResult,
  EngineRequest,
  EngineResponse,
  EngineResult,
  EngineTranscribeResult,
  EngineTrimResult,
} from '@shared/engine-protocol';
import { ENGINE_ENV } from '@shared/engine-protocol';
import type { BackendDevice } from '@shared/types';
import { SAMPLE_RATE } from '@shared/types';
import { SileroVad, energyProbs, extractSegments, segmentsFromProbs } from './vad';

type Lib = typeof import('transcribe-cpp');
type Model = Awaited<ReturnType<Lib['TranscribeModel']['load']>>;

interface ParentPort {
  on(event: 'message', listener: (event: { data: EngineRequest }) => void): void;
  postMessage(message: unknown): void;
}
const port = (process as unknown as { parentPort: ParentPort }).parentPort;

let lib: Lib | null = null;
let loaded: { ref: EngineModelRef; model: Model } | null = null;
let vad: SileroVad | null = null;
let vadTried = false;
/** Serialises model work: the native library allows one compute per model. */
let queue: Promise<unknown> = Promise.resolve();
const inflight = new Map<number, AbortController>();

async function getLib(): Promise<Lib> {
  if (!lib) {
    lib = await import('transcribe-cpp');
    // The native library is chatty; route its diagnostics only when debugging.
    const debug = process.env[ENGINE_ENV.debug] === '1';
    lib.setLogHandler(debug ? (_level, message) => console.error(`[transcribe] ${message}`) : null);
  }
  return lib;
}

async function getVad(): Promise<SileroVad | null> {
  if (vadTried) return vad;
  vadTried = true;
  const modelPath = process.env[ENGINE_ENV.vadModel];
  if (!modelPath) return null;
  try {
    vad = await SileroVad.create(modelPath);
  } catch (err) {
    console.error(`[engine] Silero VAD unavailable, using energy detection: ${String(err)}`);
    vad = null;
  }
  return vad;
}

function listDevices(l: Lib): BackendDevice[] {
  return l.getAvailableBackends().map((d) => ({
    name: d.name,
    kind: d.kind,
    deviceType: d.deviceType,
    description: d.description.trim(),
    memoryTotal: d.memoryTotal,
  }));
}

/**
 * Picks the compute device. `auto` uses a discrete GPU when one is present
 * (CUDA before Vulkan) and the CPU otherwise; integrated GPUs are skipped
 * because they are usually slower than the CPU path for these models.
 */
function pickDevice(l: Lib, device: EngineDevice) {
  const devices = l.getAvailableBackends();
  const cpu = devices.find((d) => d.deviceType === 'cpu');
  const rank = (kind: string) => (kind === 'cuda' ? 0 : kind === 'vulkan' ? 1 : 2);
  const gpus = devices
    .filter((d) => d.deviceType === 'gpu')
    .sort((a, b) => rank(a.kind) - rank(b.kind));
  const anyGpu = gpus[0] ?? devices.find((d) => d.deviceType === 'igpu');
  if (device === 'cpu') return { primary: cpu, fallback: undefined };
  if (device === 'gpu') return { primary: anyGpu, fallback: undefined };
  return { primary: gpus[0] ?? cpu, fallback: gpus[0] ? cpu : undefined };
}

async function ensureLoaded(ref: EngineModelRef): Promise<{ model: Model; loadMs: number }> {
  if (
    loaded &&
    loaded.ref.modelId === ref.modelId &&
    loaded.ref.path === ref.path &&
    loaded.ref.device === ref.device
  ) {
    return { model: loaded.model, loadMs: 0 };
  }
  const l = await getLib();
  if (loaded) {
    loaded.model.dispose();
    loaded = null;
  }
  const started = performance.now();
  const { primary, fallback } = pickDevice(l, ref.device);
  if (!primary) {
    throw new Error(
      ref.device === 'gpu' ? 'No GPU is available for local models' : 'No compute device found',
    );
  }
  let model: Model;
  try {
    model = await l.TranscribeModel.load(ref.path, { device: primary });
  } catch (err) {
    if (!fallback) throw err;
    console.error(`[engine] ${primary.name} failed (${String(err)}); falling back to CPU`);
    model = await l.TranscribeModel.load(ref.path, { device: fallback });
  }
  loaded = { ref, model };
  return { model, loadMs: performance.now() - started };
}

async function handleProbe(): Promise<EngineProbeResult> {
  const l = await getLib();
  const detector = await getVad();
  return {
    version: l.version().version,
    devices: listDevices(l),
    vad: detector ? 'silero' : 'energy',
  };
}

async function handleLoad(ref: EngineModelRef): Promise<EngineLoadResult> {
  const { model, loadMs } = await ensureLoaded(ref);
  return { modelId: ref.modelId, backend: model.backend, loadMs };
}

async function handleTranscribe(
  req: Extract<EngineRequest, { type: 'transcribe' }>,
  signal: AbortSignal,
): Promise<EngineTranscribeResult> {
  const { model, loadMs } = await ensureLoaded(req);
  if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
  const hints = req.hints.filter(Boolean);
  const options: Parameters<Model['transcribe']>[1] = {
    language: req.language,
    timestamps: 'none',
    signal,
  };
  if (hints.length > 0) {
    if (model.supports('vocabulary')) options.vocabulary = hints;
    else if (model.supports('initial_prompt')) {
      options.family = { kind: 'whisper', initialPrompt: hints.join(', ') };
    } else if (model.supports('context_prompt')) options.prompt = hints.join(', ');
  }
  const started = performance.now();
  const result = await model.transcribe(req.audio, options);
  if (result.aborted) throw new DOMException('Aborted', 'AbortError');
  return {
    text: result.text.trim(),
    backend: model.backend,
    loadMs,
    inferMs: performance.now() - started,
  };
}

async function handleTrim(audio: Float32Array): Promise<EngineTrimResult> {
  const detector = await getVad();
  const totalMs = (audio.length / SAMPLE_RATE) * 1000;
  let probs: Float32Array;
  let used: 'silero' | 'energy' = 'energy';
  if (detector) {
    try {
      probs = await detector.probabilities(audio);
      used = 'silero';
    } catch (err) {
      console.error(`[engine] VAD run failed: ${String(err)}`);
      probs = energyProbs(audio);
    }
  } else {
    probs = energyProbs(audio);
  }
  const segments = segmentsFromProbs(probs, totalMs);
  if (segments.length === 0) return { audio: null, speechMs: 0, detector: used };
  const trimmed = extractSegments(audio, segments);
  return { audio: trimmed, speechMs: (trimmed.length / SAMPLE_RATE) * 1000, detector: used };
}

function isAbort(err: unknown): boolean {
  const name = (err as { name?: string })?.name;
  return name === 'AbortError' || name === 'Aborted';
}

function reply(response: EngineResponse): void {
  port.postMessage(response);
}

function enqueue(id: number, work: (signal: AbortSignal) => Promise<EngineResult>): void {
  const controller = new AbortController();
  inflight.set(id, controller);
  queue = queue
    .then(() => work(controller.signal))
    .then(
      (result) => reply({ id, ok: true, result }),
      (err) =>
        reply({
          id,
          ok: false,
          error: err instanceof Error ? err.message : String(err),
          aborted: isAbort(err) || controller.signal.aborted,
        }),
    )
    .finally(() => inflight.delete(id));
}

port.on('message', ({ data: req }) => {
  switch (req.type) {
    case 'cancel':
      inflight.get(req.target)?.abort();
      return;
    case 'probe':
      enqueue(req.id, () => handleProbe());
      return;
    case 'load':
      enqueue(req.id, () => handleLoad(req));
      return;
    case 'transcribe':
      enqueue(req.id, (signal) => handleTranscribe(req, signal));
      return;
    case 'trim':
      // Voice detection does not touch the model, so it runs alongside a load.
      handleTrim(req.audio).then(
        (result) => reply({ id: req.id, ok: true, result }),
        (err) => reply({ id: req.id, ok: false, error: String(err) }),
      );
      return;
    case 'unload':
      enqueue(req.id, async () => {
        loaded?.model.dispose();
        loaded = null;
        return null;
      });
      return;
  }
});

port.postMessage({ id: 0, hello: true });
