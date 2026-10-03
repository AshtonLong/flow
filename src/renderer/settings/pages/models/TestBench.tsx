/**
 * Test bench: record one phrase, run it through any set of ready models, and
 * compare text, latency and cost side by side.
 */
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Checkbox } from 'radix-ui';
import { Check, LoaderCircle, Mic, Square } from 'lucide-react';
import { cloudCost, type ModelEntry } from '@shared/catalog';
import type { BenchResult } from '@shared/ipc';
import { SAMPLE_RATE } from '@shared/types';
import { recordClip, toCaptureError, type CaptureError } from '../../../common/capture';
import { Button } from '../../components/Button';
import { Group } from '../../components/Card';
import { captureHelp, LevelMeter, type LevelMeterHandle } from '../../components/LevelMeter';
import { Notice } from '../../components/Notice';
import { useCatalog } from '../../lib/catalog';
import { useConfig } from '../../lib/config';
import { errorMessage, formatCost, formatDuration, formatMs } from '../../lib/format';
import { modelLabel, readyModels } from '../../lib/models';

type Recorder = Awaited<ReturnType<typeof recordClip>>;

const MAX_CLIP_MS = 60_000;
const MIN_CLIP_MS = 400;

function costOf(entry: ModelEntry | undefined, result: BenchResult, audioMs: number): string {
  if (!entry || entry.kind === 'local') return 'Free';
  if (result.cost !== undefined) return formatCost(result.cost);
  return formatCost(cloudCost(entry, audioMs));
}

export function TestBench() {
  const { config } = useConfig();
  const { snapshot } = useCatalog();
  const ready = useMemo(() => (snapshot ? readyModels(snapshot) : []), [snapshot]);

  const [recording, setRecording] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [clip, setClip] = useState<Float32Array | null>(null);
  const [micError, setMicError] = useState<CaptureError | null>(null);
  const [selected, setSelected] = useState<Set<string> | null>(null);
  const [order, setOrder] = useState<string[]>([]);
  const [results, setResults] = useState<Record<string, BenchResult>>({});
  const [running, setRunning] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);

  const recorder = useRef<Recorder | null>(null);
  const meter = useRef<LevelMeterHandle>(null);
  const startedAt = useRef(0);
  // Results can arrive before `bench.run` resolves with the run id, so they are
  // held until the id is known. Anything from another run is dropped.
  const runId = useRef<number | null>(null);
  const awaitingId = useRef(false);
  const early = useRef<BenchResult[]>([]);
  const headingId = useId();

  // Local models and the active model are ticked to begin with. Cloud models
  // are opt-in, because testing one sends the recording to its provider.
  const chosen = useMemo(() => {
    if (selected) return selected;
    return new Set(
      ready.filter((m) => m.kind === 'local' || snapshot?.status[m.id]?.active).map((m) => m.id),
    );
  }, [selected, ready, snapshot]);
  const picked = ready.filter((m) => chosen.has(m.id));

  useEffect(() => {
    const apply = (result: BenchResult) => {
      setResults((prev) => ({ ...prev, [result.modelId]: result }));
      if (result.done) setRunning(false);
    };
    const off = window.flow.bench.onResult((result) => {
      if (runId.current === null) {
        if (awaitingId.current) early.current.push(result);
        return;
      }
      if (result.runId === runId.current) apply(result);
    });
    return off;
  }, []);

  useEffect(
    () => () => {
      recorder.current?.cancel();
      recorder.current = null;
    },
    [],
  );

  const stop = async () => {
    const active = recorder.current;
    if (!active) return;
    recorder.current = null;
    setRecording(false);
    const audio = await active.stop();
    setClip(audio);
  };

  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => {
      const ms = performance.now() - startedAt.current;
      setElapsed(ms);
      if (ms >= MAX_CLIP_MS) void stop();
    }, 100);
    return () => clearInterval(timer);
  }, [recording]);

  const record = async () => {
    setMicError(null);
    setRunError(null);
    try {
      const next = await recordClip(config.audio.input_device, (level) =>
        meter.current?.set(level),
      );
      recorder.current = next;
      startedAt.current = performance.now();
      setElapsed(0);
      setClip(null);
      setResults({});
      setOrder([]);
      setRecording(true);
    } catch (err) {
      setMicError(toCaptureError(err));
    }
  };

  const clipMs = clip ? (clip.length / SAMPLE_RATE) * 1000 : 0;
  const tooShort = clip !== null && clipMs < MIN_CLIP_MS;

  const run = async () => {
    if (!clip || picked.length === 0) return;
    const ids = picked.map((m) => m.id);
    setOrder(ids);
    setResults({});
    setRunError(null);
    setRunning(true);
    runId.current = null;
    early.current = [];
    awaitingId.current = true;
    try {
      const id = await window.flow.bench.run(clip, ids);
      runId.current = id;
      awaitingId.current = false;
      const held = early.current.filter((r) => r.runId === id);
      early.current = [];
      if (held.length > 0) {
        setResults((prev) => {
          const next = { ...prev };
          for (const r of held) next[r.modelId] = r;
          return next;
        });
        if (held.some((r) => r.done)) setRunning(false);
      }
    } catch (err) {
      awaitingId.current = false;
      setRunning(false);
      setRunError(errorMessage(err));
    }
  };

  const toggle = (id: string, on: boolean) => {
    const next = new Set(chosen);
    if (on) next.add(id);
    else next.delete(id);
    setSelected(next);
  };

  const finished = order.filter((id) => results[id]).length;

  return (
    <Group
      title="Test bench"
      description="Record one phrase and run it through several models to compare them on your own voice."
    >
      <div className="card">
        <div className="flex items-center gap-4 px-4 py-3">
          <div className="min-w-0 flex-1">
            <div>1. Record a phrase</div>
            <div className="mt-0.5 text-caption text-fg-2" aria-live="polite">
              {recording
                ? `Recording… ${formatDuration(elapsed)}`
                : clip
                  ? tooShort
                    ? 'That was too short. Record a full sentence.'
                    : `Recorded ${formatDuration(clipMs)} of audio. It stays in memory and is never saved.`
                  : 'A sentence or two, the way you normally dictate.'}
            </div>
          </div>
          {recording && (
            <div className="w-[160px] shrink-0">
              <LevelMeter ref={meter} label="Recording level" />
            </div>
          )}
          {recording ? (
            <Button variant="accent" icon={<Square size={13} />} onClick={() => void stop()}>
              Stop
            </Button>
          ) : (
            <Button icon={<Mic size={15} />} disabled={running} onClick={() => void record()}>
              {clip ? 'Record again' : 'Record'}
            </Button>
          )}
        </div>
        {micError && (
          <div className="px-4 pb-3">
            <Notice tone="danger">{captureHelp(micError)}</Notice>
          </div>
        )}

        <div className="border-t border-line px-4 py-3">
          <div id={headingId}>2. Choose models</div>
          <div className="mt-0.5 text-caption text-fg-2">
            Installed and connected models. Ticking a cloud model sends the recording to its
            provider.
          </div>
          {ready.length === 0 ? (
            <p className="mt-2 text-fg-2">Download or connect a model above first.</p>
          ) : (
            <ul aria-labelledby={headingId} className="mt-2.5 grid grid-cols-2 gap-x-6 gap-y-2">
              {ready.map((entry) => {
                const id = `bench-${entry.id}`;
                return (
                  <li key={entry.id} className="flex items-center gap-2.5">
                    <Checkbox.Root
                      id={id}
                      className="checkbox"
                      checked={chosen.has(entry.id)}
                      disabled={running}
                      onCheckedChange={(on) => toggle(entry.id, on === true)}
                    >
                      <Checkbox.Indicator>
                        <Check size={14} strokeWidth={2.5} aria-hidden="true" />
                      </Checkbox.Indicator>
                    </Checkbox.Root>
                    <label htmlFor={id} className="min-w-0 truncate">
                      {modelLabel(snapshot, entry.id)}
                    </label>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        <div className="flex items-center gap-4 border-t border-line px-4 py-3">
          <div className="min-w-0 flex-1">
            <div>3. Compare</div>
            <div className="mt-0.5 text-caption text-fg-2">
              {!clip || tooShort
                ? 'Record a phrase first.'
                : picked.length === 0
                  ? 'Tick at least one model.'
                  : `Runs the recording through ${picked.length === 1 ? 'the ticked model' : `${picked.length} models`}.`}
            </div>
          </div>
          <Button
            variant="accent"
            disabled={!clip || tooShort || picked.length === 0 || running || recording}
            onClick={() => void run()}
          >
            {running && <LoaderCircle size={15} aria-hidden="true" className="spin" />}
            {running ? 'Running…' : 'Run test'}
          </Button>
        </div>
        {runError && (
          <div className="px-4 pb-3">
            <Notice tone="danger">The test could not start: {runError}</Notice>
          </div>
        )}

        <p className="sr-only" aria-live="polite">
          {order.length > 0 &&
            (running
              ? `${finished} of ${order.length} results in.`
              : `Test finished. ${finished} of ${order.length} results.`)}
        </p>

        {order.length > 0 && (
          <div className="border-t border-line">
            <table className="w-full table-fixed border-collapse text-left">
              <caption className="sr-only">Test results</caption>
              <thead>
                <tr className="text-caption text-fg-2">
                  <th scope="col" className="w-[190px] px-4 py-2 font-normal">
                    Model
                  </th>
                  <th scope="col" className="px-2 py-2 font-normal">
                    Text
                  </th>
                  <th scope="col" className="w-[88px] px-2 py-2 text-right font-normal">
                    Latency
                  </th>
                  <th scope="col" className="w-[84px] py-2 pr-4 pl-2 text-right font-normal">
                    Cost
                  </th>
                </tr>
              </thead>
              <tbody>
                {order.map((id) => {
                  const result = results[id];
                  const entry = ready.find((m) => m.id === id);
                  return (
                    <tr key={id} className="border-t border-line align-top">
                      <th scope="row" className="px-4 py-2.5 font-normal">
                        {modelLabel(snapshot, id)}
                      </th>
                      <td className="selectable px-2 py-2.5">
                        {!result ? (
                          <span className="text-fg-3">Waiting…</span>
                        ) : result.ok ? (
                          result.text || <span className="text-fg-3">No speech found</span>
                        ) : (
                          <span className="text-danger">{result.error ?? 'Failed'}</span>
                        )}
                      </td>
                      <td className="px-2 py-2.5 text-right tabular-nums">
                        {result?.elapsedMs !== undefined ? formatMs(result.elapsedMs) : ''}
                      </td>
                      <td className="py-2.5 pr-4 pl-2 text-right tabular-nums">
                        {result?.ok ? costOf(entry, result, clipMs) : ''}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </Group>
  );
}
