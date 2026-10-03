/**
 * First run: three steps instead of the settings pages. Pick a microphone,
 * set the hold-to-talk hotkey, choose a model, then try a dictation right here.
 */
import { useEffect, useId, useRef, useState } from 'react';
import { RadioGroup } from 'radix-ui';
import { Download, Mic, Square } from 'lucide-react';
import type { CloudModelEntry, LocalModelEntry } from '@shared/catalog';
import { DEFAULT_CONFIG, DEFAULT_LOCAL_MODEL } from '@shared/config';
import type { KeyTestResult } from '@shared/ipc';
import { recordClip, toCaptureError } from '../../common/capture';
import { Button } from '../components/Button';
import { HotkeyRecorder } from '../components/HotkeyRecorder';
import { KeyCaps } from '../components/KeyCaps';
import { captureHelp, LevelMeter, MicMeter, type LevelMeterHandle } from '../components/LevelMeter';
import { MicSelect } from '../components/MicSelect';
import { Badge, Notice, ProgressBar } from '../components/Notice';
import { downloadOf, useCatalog } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { errorMessage, formatBytes, formatDuration, formatPrice, formatSpeed } from '../lib/format';
import { useMicrophones } from '../lib/hooks';
import { findProvider, modelLabel, providerConnected } from '../lib/models';
import { KeyForm } from '../pages/models/CloudProviders';

const STEPS = ['Microphone', 'Hotkey', 'Model'] as const;

function StepMicrophone() {
  const { config, set } = useConfig();
  const { mics, refresh } = useMicrophones();
  return (
    <>
      <h2 className="font-display text-subtitle font-[760]">Pick your microphone</h2>
      <p className="mt-1 text-fg-2">Say something. The bar should move as you speak.</p>
      <div className="card mt-5 px-4 py-4">
        <MicSelect
          value={config.audio.input_device}
          mics={mics}
          width="100%"
          aria-label="Microphone"
          onChange={(v) => set(['audio', 'input_device'], v)}
        />
        <div className="mt-4">
          <MicMeter device={config.audio.input_device} onListening={refresh} />
        </div>
      </div>
      <p className="mt-3 text-caption text-fg-2">
        Flow only listens while you hold the hotkey. This check stops when you move on.
      </p>
    </>
  );
}

function StepHotkey() {
  const { config, set } = useConfig();
  return (
    <>
      <h2 className="font-display text-subtitle font-[760]">Set your hotkey</h2>
      <p className="mt-1 text-fg-2">
        Hold it to talk, release it and the text is typed wherever your cursor is.
      </p>
      <div className="card mt-5 flex min-h-16 items-center justify-between gap-4 px-4 py-3">
        <span>Hold to talk</span>
        <HotkeyRecorder
          actionName="Hold to talk"
          value={config.hotkeys.hold_to_talk}
          defaultValue={DEFAULT_CONFIG.hotkeys.hold_to_talk}
          onChange={(binding) => set(['hotkeys', 'hold_to_talk'], binding)}
        />
      </div>
      <p className="mt-3 text-caption text-fg-2">
        Any key, combination, lone modifier (such as Right Ctrl) or mouse button works. You can
        change it later on the Hotkeys page.
      </p>
    </>
  );
}

function LocalChoice({ entry, selected }: { entry: LocalModelEntry; selected: boolean }) {
  const catalog = useCatalog();
  const [error, setError] = useState<string | null>(null);
  const ready = catalog.snapshot?.status[entry.id]?.ready ?? false;
  const download = downloadOf(catalog, entry.id);
  const busy = download?.state === 'downloading' || download?.state === 'verifying';

  const start = async () => {
    setError(null);
    catalog.clearProgress(entry.id);
    try {
      await window.flow.models.download(entry.id);
      await catalog.reload();
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  if (!selected) return null;
  return (
    <div className="mt-3" aria-live="polite">
      {ready && <Badge tone="ok">Installed</Badge>}
      {!ready && !busy && (
        <Button icon={<Download size={15} />} onClick={() => void start()}>
          {download?.state === 'error'
            ? 'Retry download'
            : `Download ${formatBytes(entry.sizeBytes)}`}
        </Button>
      )}
      {busy && download && (
        <div>
          <ProgressBar
            label={`Downloading ${entry.name}`}
            value={
              download.state === 'verifying' || download.totalBytes <= 0
                ? undefined
                : download.receivedBytes / download.totalBytes
            }
          />
          <div className="mt-1.5 flex items-center justify-between text-caption text-fg-2 tabular-nums">
            <span>
              {download.state === 'verifying'
                ? 'Verifying the download…'
                : `${download.totalBytes > 0 ? Math.floor((download.receivedBytes / download.totalBytes) * 100) : 0}% of ${formatBytes(download.totalBytes || entry.sizeBytes)}${download.speed > 0 ? `, ${formatSpeed(download.speed)}` : ''}`}
            </span>
            <Button
              size="sm"
              variant="subtle"
              onClick={() => void window.flow.models.cancelDownload(entry.id)}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
      {(error || download?.state === 'error') && (
        <Notice tone="danger" className="mt-2">
          The download failed{error || download?.error ? `: ${error ?? download?.error}` : '.'}
        </Notice>
      )}
    </div>
  );
}

function CloudChoice({ entry, selected }: { entry: CloudModelEntry; selected: boolean }) {
  const catalog = useCatalog();
  const [result, setResult] = useState<KeyTestResult | null>(null);
  const provider = findProvider(catalog.snapshot, entry.provider);
  if (!selected || !provider || !catalog.snapshot) return null;
  const connected = providerConnected(catalog.snapshot, provider.id);
  return (
    <div className="mt-3">
      {connected ? (
        <Badge tone="ok">{provider.name} connected</Badge>
      ) : (
        <KeyForm
          provider={provider}
          onDone={(r) => {
            setResult(r);
            void catalog.reload();
          }}
        />
      )}
      <div aria-live="polite">
        {result && !result.ok && (
          <Notice tone="danger" className="mt-2">
            The key was saved, but the test failed: {result.message}
          </Notice>
        )}
      </div>
    </div>
  );
}

type Recorder = Awaited<ReturnType<typeof recordClip>>;

function TestDictation({ modelId }: { modelId: string }) {
  const { config } = useConfig();
  const { snapshot } = useCatalog();
  const [text, setText] = useState('');
  const [status, setStatus] = useState<{ ok: boolean; message: string } | null>(null);
  const [recording, setRecording] = useState(false);
  const [working, setWorking] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const meter = useRef<LevelMeterHandle>(null);
  const recorder = useRef<Recorder | null>(null);
  const areaId = useId();

  useEffect(() => {
    area.current?.focus();
  }, []);

  // A real dictation is typed into the focused box by Flow itself. If the text
  // did not land there (the box lost focus), show it from the event instead.
  useEffect(
    () =>
      window.flow.dictation.onEvent((event) => {
        if (!event.ok) {
          setStatus({ ok: false, message: event.error ?? 'That dictation failed.' });
          return;
        }
        setStatus({
          ok: true,
          message: `It works. ${modelLabel(snapshot, event.model)} took ${formatDuration(event.elapsedMs)}${event.fellBack ? ' (the fallback model was used)' : ''}.`,
        });
        const heard = event.text.trim();
        window.setTimeout(() => {
          const box = area.current;
          if (heard && box && !box.value.includes(heard)) {
            setText((prev) => (prev && !/\s$/.test(prev) ? `${prev} ${heard}` : `${prev}${heard}`));
          }
        }, 250);
      }),
    [snapshot],
  );

  useEffect(
    () => () => {
      recorder.current?.cancel();
      recorder.current = null;
    },
    [],
  );

  const record = async () => {
    setStatus(null);
    try {
      recorder.current = await recordClip(config.audio.input_device, (level) =>
        meter.current?.set(level),
      );
      setRecording(true);
    } catch (err) {
      setStatus({ ok: false, message: captureHelp(toCaptureError(err)) });
    }
  };

  const stop = async () => {
    const active = recorder.current;
    if (!active) return;
    recorder.current = null;
    setRecording(false);
    setWorking(true);
    try {
      const audio = await active.stop();
      const transcript = await window.flow.dictation.test(audio);
      const heard = transcript.text.trim();
      if (heard) {
        setText((prev) => (prev && !/\s$/.test(prev) ? `${prev} ${heard}` : `${prev}${heard}`));
        setStatus({
          ok: true,
          message: `It works. ${modelLabel(snapshot, transcript.model)} took ${formatDuration(transcript.elapsedMs)}.`,
        });
      } else {
        setStatus({
          ok: false,
          message: 'No speech was found in that recording. Try again a little louder.',
        });
      }
    } catch (err) {
      setStatus({ ok: false, message: errorMessage(err) });
    } finally {
      setWorking(false);
    }
  };

  return (
    <div className="mt-5">
      <label htmlFor={areaId} className="flex flex-wrap items-center gap-x-2 gap-y-1 font-semibold">
        Try it: click in the box, hold <KeyCaps binding={config.hotkeys.hold_to_talk} /> and speak
      </label>
      <textarea
        id={areaId}
        ref={area}
        rows={3}
        className="input mt-2 w-full"
        placeholder="Your words appear here when you release the hotkey."
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="mt-2 flex items-center gap-3">
        {recording ? (
          <Button icon={<Square size={13} />} onClick={() => void stop()}>
            Stop
          </Button>
        ) : (
          <Button icon={<Mic size={15} />} disabled={working} onClick={() => void record()}>
            {working ? 'Transcribing…' : 'Record a test instead'}
          </Button>
        )}
        {recording ? (
          <div className="flex-1">
            <LevelMeter ref={meter} label="Recording level" />
          </div>
        ) : (
          <span className="text-caption text-fg-2">
            Uses {modelLabel(snapshot, modelId)} without the hotkey.
          </span>
        )}
      </div>
      <div aria-live="polite">
        {status && (
          <Notice tone={status.ok ? 'ok' : 'danger'} className="mt-3">
            {status.message}
          </Notice>
        )}
      </div>
    </div>
  );
}

function StepModel() {
  const catalog = useCatalog();
  const { config } = useConfig();
  const { snapshot } = catalog;
  const [choice, setChoice] = useState<'local' | 'cloud'>('local');
  const [error, setError] = useState<string | null>(null);

  const local =
    snapshot?.catalog.local.find((m) => m.id === DEFAULT_LOCAL_MODEL) ?? snapshot?.catalog.local[0];
  const cloud = snapshot?.catalog.cloud.find((m) => m.recommended) ?? snapshot?.catalog.cloud[0];
  const cloudProvider = cloud ? findProvider(snapshot, cloud.provider) : undefined;

  const chosen = choice === 'local' ? local : cloud;
  const chosenReady =
    snapshot && chosen
      ? chosen.kind === 'local'
        ? (snapshot.status[chosen.id]?.ready ?? false)
        : providerConnected(snapshot, chosen.provider)
      : false;
  const chosenId = chosen?.id;
  const active = config.model.active;

  // The choice becomes the active model as soon as it can be used.
  useEffect(() => {
    if (!chosenId || !chosenReady || active === chosenId) return;
    let cancelled = false;
    window.flow.models
      .activate(chosenId)
      .then(() => catalog.reload())
      .catch((err: unknown) => {
        if (!cancelled) setError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, [chosenId, chosenReady, active]);

  if (!snapshot) {
    return catalog.error ? (
      <Notice tone="danger">The model catalog could not be loaded: {catalog.error}</Notice>
    ) : (
      <p className="text-fg-2">Loading the catalog…</p>
    );
  }

  return (
    <>
      <h2 className="font-display text-subtitle font-[760]">Choose a model</h2>
      <p className="mt-1 text-fg-2">
        The model turns your voice into text. You can switch at any time on the Models page.
      </p>
      <RadioGroup.Root
        className="mt-5 flex flex-col gap-[3px]"
        value={choice}
        onValueChange={(v) => setChoice(v as 'local' | 'cloud')}
        aria-label="Model"
      >
        {local && (
          <div className="card px-4 py-3">
            <div className="flex items-start gap-3">
              <RadioGroup.Item value="local" id="choice-local" className="radio mt-0.5">
                <RadioGroup.Indicator className="radio-dot" />
              </RadioGroup.Item>
              <div className="min-w-0 flex-1">
                <label htmlFor="choice-local" className="flex flex-wrap items-center gap-2">
                  <span className="font-semibold">On this PC: {local.name}</span>
                  <Badge tone="accent">Recommended</Badge>
                </label>
                <p className="mt-0.5 text-caption text-fg-2">
                  Free and offline. Your audio never leaves this PC. A one-time{' '}
                  {formatBytes(local.sizeBytes)} download.
                </p>
                <LocalChoice entry={local} selected={choice === 'local'} />
              </div>
            </div>
          </div>
        )}
        {cloud && (
          <div className="card px-4 py-3">
            <div className="flex items-start gap-3">
              <RadioGroup.Item value="cloud" id="choice-cloud" className="radio mt-0.5">
                <RadioGroup.Indicator className="radio-dot" />
              </RadioGroup.Item>
              <div className="min-w-0 flex-1">
                <label htmlFor="choice-cloud" className="font-semibold">
                  In the cloud: {cloudProvider?.name ?? cloud.provider} {cloud.name}
                </label>
                <p className="mt-0.5 text-caption text-fg-2">
                  Nothing to download, and fast on any PC. Needs your own{' '}
                  {cloudProvider?.name ?? cloud.provider} key; about{' '}
                  {formatPrice(cloud.pricePerHour)} per hour of speech, billed by them. Audio is
                  sent to the provider.
                </p>
                <CloudChoice entry={cloud} selected={choice === 'cloud'} />
              </div>
            </div>
          </div>
        )}
      </RadioGroup.Root>
      {error && (
        <Notice tone="danger" className="mt-3">
          {error}
        </Notice>
      )}
      {chosen && chosenReady && <TestDictation key={chosen.id} modelId={chosen.id} />}
    </>
  );
}

export function Onboarding() {
  const { set } = useConfig();
  const [step, setStep] = useState(0);
  const heading = useRef<HTMLDivElement>(null);
  const last = step === STEPS.length - 1;
  const finish = () => set(['general', 'onboarded'], true);

  // Moving between steps puts focus at the top of the new one.
  const shown = useRef(step);
  useEffect(() => {
    if (shown.current === step) return;
    shown.current = step;
    heading.current?.focus();
  }, [step]);

  return (
    <div className="flex h-full flex-col">
      <div className="drag flex h-10 shrink-0 items-center gap-2 px-4 text-caption">
        <span className="logo text-[19px]">
          flow
          <span className="logo-caret" aria-hidden="true" />
        </span>
      </div>
      <main className="relative min-h-0 flex-1 overflow-y-auto px-6 pb-6">
        <div className="mx-auto flex min-h-full max-w-[560px] flex-col">
          <header className="pt-4 pb-6">
            <h1 className="font-display text-title font-[840]">Set up Flow</h1>
            <ol className="mt-4 grid grid-cols-3 gap-2" aria-label="Setup steps">
              {STEPS.map((name, i) => (
                <li key={name} aria-current={i === step ? 'step' : undefined}>
                  <div
                    className={`h-[3px] rounded-full ${i <= step ? 'bg-accent' : 'bg-line-strong'}`}
                  />
                  <div className={`mt-1.5 text-caption ${i === step ? 'text-fg' : 'text-fg-2'}`}>
                    {i + 1}. {name}
                  </div>
                </li>
              ))}
            </ol>
          </header>

          <div ref={heading} tabIndex={-1} className="flex-1 outline-none">
            {step === 0 && <StepMicrophone />}
            {step === 1 && <StepHotkey />}
            {step === 2 && <StepModel />}
          </div>

          <footer className="mt-8 flex items-center gap-2">
            <button type="button" className="link" onClick={finish}>
              Skip setup
            </button>
            <div className="ml-auto flex gap-2">
              {step > 0 && (
                <Button className="min-w-[96px]" onClick={() => setStep(step - 1)}>
                  Back
                </Button>
              )}
              <Button
                variant="accent"
                className="min-w-[96px]"
                onClick={() => (last ? finish() : setStep(step + 1))}
              >
                {last ? 'Finish' : 'Next'}
              </Button>
            </div>
          </footer>
        </div>
      </main>
    </div>
  );
}
