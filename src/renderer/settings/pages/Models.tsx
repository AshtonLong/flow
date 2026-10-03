import { useEffect, useRef, useState } from 'react';
import { Download, Trash2 } from 'lucide-react';
import type { LocalModelEntry } from '@shared/catalog';
import { DEFAULT_LOCAL_MODEL, type Config } from '@shared/config';
import type { AppInfo } from '@shared/types';
import { Button, ExternalLink, IconButton } from '../components/Button';
import { EmptyState, Group, PageHeader, SettingRow } from '../components/Card';
import { ConfirmDialog } from '../components/Dialog';
import { Badge, Notice, ProgressBar } from '../components/Notice';
import { Select, type Option } from '../components/Select';
import { downloadOf, useCatalog, type CatalogStore } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { errorMessage, formatBytes, formatMemory, formatSpeed, sentence } from '../lib/format';
import { useAppInfo } from '../lib/hooks';
import { installedLocal, isGpuDevice, modelLabel } from '../lib/models';
import { CloudProviders } from './models/CloudProviders';
import { CustomEndpoints } from './models/CustomEndpoints';
import { TestBench } from './models/TestBench';

/** Runs a backend call and reports a failure on the page. */
export type Act = (work: () => Promise<unknown>) => Promise<void>;

function LocalRow({
  entry,
  act,
  hasGpu,
}: {
  entry: LocalModelEntry;
  act: Act;
  /** Null when devices have not been probed yet. */
  hasGpu: boolean | null;
}) {
  const catalog = useCatalog();
  const { config } = useConfig();
  const [confirm, setConfirm] = useState(false);
  const status = catalog.snapshot?.status[entry.id];
  const download = downloadOf(catalog, entry.id);
  const ready = status?.ready ?? false;
  const active = status?.active ?? false;
  const busy = download?.state === 'downloading' || download?.state === 'verifying';
  const isFallback = config.model.fallback === entry.id;

  const start = () => {
    catalog.clearProgress(entry.id);
    void act(() => window.flow.models.download(entry.id));
  };

  return (
    <div className="card px-4 py-3">
      <div className="flex items-start gap-6">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-semibold">{entry.name}</span>
            {entry.id === DEFAULT_LOCAL_MODEL && <Badge>Default</Badge>}
            {status?.loaded && <Badge>In memory</Badge>}
          </div>
          <p className="mt-0.5 text-caption text-fg-2">{sentence(entry.description)}</p>
          <dl className="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-caption text-fg-2">
            <div className="flex gap-1.5">
              <dt className="sr-only">Download size</dt>
              <dd>{formatBytes(entry.sizeBytes)}</dd>
            </div>
            <div className="flex gap-1.5">
              <dt>Runs on</dt>
              <dd className="text-fg">{entry.runsOn === 'gpu' ? 'GPU' : 'CPU'}</dd>
            </div>
            <div className="flex gap-1.5">
              <dt>Licence</dt>
              <dd className="text-fg">
                {entry.licenceUrl?.startsWith('https://') ? (
                  <ExternalLink href={entry.licenceUrl}>{entry.licence}</ExternalLink>
                ) : (
                  entry.licence
                )}
              </dd>
            </div>
            {entry.wer !== undefined && (
              <div className="flex gap-1.5">
                <dt>Word error rate</dt>
                <dd className="text-fg">{entry.wer.toFixed(2)}%</dd>
              </div>
            )}
          </dl>
        </div>

        <div className="flex shrink-0 items-center gap-1 pt-0.5">
          {active && <Badge tone="accent">Active</Badge>}
          {ready && !active && (
            <Button onClick={() => void act(() => window.flow.models.activate(entry.id))}>
              Set active
            </Button>
          )}
          {ready && !active && (
            <IconButton
              label={`Remove ${entry.name}`}
              icon={<Trash2 size={15} />}
              onClick={() => setConfirm(true)}
            />
          )}
          {!ready && !busy && (
            <Button icon={<Download size={15} />} onClick={start}>
              {download?.state === 'error' ? 'Retry' : 'Download'}
            </Button>
          )}
          {busy && (
            <Button onClick={() => void act(() => window.flow.models.cancelDownload(entry.id))}>
              Cancel
            </Button>
          )}
        </div>
      </div>

      {busy && download && (
        <div className="mt-3">
          <ProgressBar
            label={`Downloading ${entry.name}`}
            value={
              download.state === 'verifying' || download.totalBytes <= 0
                ? undefined
                : download.receivedBytes / download.totalBytes
            }
          />
          <div className="mt-1.5 flex justify-between text-caption text-fg-2 tabular-nums">
            {download.state === 'verifying' ? (
              <span>Verifying the download…</span>
            ) : (
              <>
                <span>
                  {download.totalBytes > 0
                    ? `${Math.floor((download.receivedBytes / download.totalBytes) * 100)}%`
                    : 'Starting…'}
                  {download.speed > 0 && `, ${formatSpeed(download.speed)}`}
                </span>
                <span>
                  {formatBytes(download.receivedBytes)} of{' '}
                  {formatBytes(download.totalBytes || entry.sizeBytes)}
                </span>
              </>
            )}
          </div>
        </div>
      )}
      {download?.state === 'error' && (
        <Notice tone="danger" className="mt-3">
          The download failed{download.error ? `: ${download.error}` : '.'} Retry picks up where it
          stopped.
        </Notice>
      )}
      {!ready && !busy && entry.gated && (
        <Notice className="mt-3">
          Gated download: accept the terms on the model page first
          {entry.licenceUrl?.startsWith('https://') && (
            <>
              {' ('}
              <ExternalLink href={entry.licenceUrl}>open the page</ExternalLink>
              {')'}
            </>
          )}
          .
        </Notice>
      )}
      {!ready && entry.runsOn === 'gpu' && hasGpu === false && (
        <Notice tone="warn" className="mt-3">
          No GPU was detected on this PC. This model will be slow on the CPU.
        </Notice>
      )}
      {entry.note && <p className="mt-2 text-caption text-fg-2">{entry.note}</p>}

      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Remove ${entry.name}?`}
        description={
          isFallback
            ? `This deletes the ${formatBytes(entry.sizeBytes)} model file. It is your fallback model, so cloud failures will have nothing to fall back to until you pick another.`
            : `This deletes the ${formatBytes(entry.sizeBytes)} model file from this PC. You can download it again at any time.`
        }
        confirmLabel="Remove model"
        onConfirm={() => void act(() => window.flow.models.remove(entry.id))}
      />
    </div>
  );
}

/** Reads out download milestones without flooding a screen reader. */
function useDownloadAnnouncements(catalog: CatalogStore): string {
  const [message, setMessage] = useState('');
  const steps = useRef<Record<string, number>>({});
  useEffect(() => {
    for (const [id, p] of Object.entries(catalog.progress)) {
      const name = modelLabel(catalog.snapshot, id);
      if (p.state === 'error') {
        if (steps.current[id] !== -1) setMessage(`${name}: download failed.`);
        steps.current[id] = -1;
      } else if (p.state === 'verifying') {
        if (steps.current[id] !== 101) setMessage(`${name}: verifying the download.`);
        steps.current[id] = 101;
      } else if (p.state === 'downloading' && p.totalBytes > 0) {
        const step = Math.floor((p.receivedBytes / p.totalBytes) * 4) * 25;
        if (steps.current[id] !== step) setMessage(`${name}: ${step}% downloaded.`);
        steps.current[id] = step;
      }
    }
    for (const id of Object.keys(steps.current)) {
      if (!(id in catalog.progress)) {
        if (catalog.snapshot?.status[id]?.ready) {
          setMessage(`${modelLabel(catalog.snapshot, id)} is ready.`);
        }
        delete steps.current[id];
      }
    }
  }, [catalog.progress, catalog.snapshot]);
  return message;
}

const DEVICES: Option<Config['model']['device']>[] = [
  { value: 'auto', label: 'Automatic' },
  { value: 'cpu', label: 'CPU' },
  { value: 'gpu', label: 'GPU' },
];

const MEMORY: { minutes: number; label: string }[] = [
  { minutes: 0, label: 'Unload immediately' },
  { minutes: 10, label: '10 minutes (default)' },
  { minutes: 30, label: '30 minutes' },
  { minutes: 60, label: '60 minutes' },
  { minutes: -1, label: 'Always loaded' },
];

function describeDevices(info: AppInfo | null): string {
  if (!info) return '';
  if (info.devices.length === 0) return 'Devices are detected the first time a local model runs.';
  const names = info.devices.map((d) => {
    const memory = d.memoryTotal > 0 ? `, ${formatMemory(d.memoryTotal)}` : '';
    return `${d.description || d.name} (${isGpuDevice(d) ? 'GPU' : 'CPU'}${memory})`;
  });
  return `Detected: ${names.join('; ')}.`;
}

function ModelSettings({ info }: { info: AppInfo | null }) {
  const { config, set } = useConfig();
  const { snapshot } = useCatalog();

  const minutes = config.model.keep_loaded_minutes;
  const memoryOptions: Option[] = MEMORY.map((m) => ({ value: String(m.minutes), label: m.label }));
  if (!MEMORY.some((m) => m.minutes === minutes)) {
    memoryOptions.push({
      value: String(minutes),
      label: `${minutes} minutes`,
      hint: 'From the file',
    });
  }

  const fallbackOptions: Option[] = snapshot
    ? installedLocal(snapshot).map((m) => ({ value: m.id, label: m.name }))
    : [];
  if (!fallbackOptions.some((o) => o.value === config.model.fallback)) {
    fallbackOptions.push({
      value: config.model.fallback,
      label: modelLabel(snapshot, config.model.fallback),
      hint: 'Not installed',
    });
  }

  return (
    <Group title="Model settings">
      <SettingRow
        label="Device"
        description={`Automatic uses the GPU when one is found, otherwise the CPU. ${describeDevices(info)}`}
        control={
          <Select
            value={config.model.device}
            options={DEVICES}
            onChange={(v) => set(['model', 'device'], v)}
          />
        }
      />
      <SettingRow
        label="Keep model in memory"
        description="How long a local model stays loaded after you last dictated. Longer is faster to start; shorter frees memory."
        control={
          <Select
            value={String(minutes)}
            options={memoryOptions}
            onChange={(v) => set(['model', 'keep_loaded_minutes'], Number(v))}
          />
        }
      />
      <SettingRow
        label="Fallback model"
        description="The installed model Flow switches to when a cloud request fails or the PC is offline."
        control={
          <Select
            value={config.model.fallback}
            options={fallbackOptions}
            width={240}
            onChange={(v) => set(['model', 'fallback'], v)}
          />
        }
      />
    </Group>
  );
}

export function Models() {
  const catalog = useCatalog();
  const { info } = useAppInfo();
  const [actionError, setActionError] = useState<string | null>(null);
  const announcement = useDownloadAnnouncements(catalog);
  const { snapshot } = catalog;

  const act: Act = async (work) => {
    try {
      setActionError(null);
      await work();
      await catalog.reload();
    } catch (err) {
      setActionError(errorMessage(err));
    }
  };

  if (!snapshot) {
    return (
      <>
        <PageHeader title="Models" />
        {catalog.error ? (
          <Notice
            tone="danger"
            action={
              <Button size="sm" onClick={() => void catalog.reload()}>
                Try again
              </Button>
            }
          >
            The model catalog could not be loaded: {catalog.error}
          </Notice>
        ) : (
          <p className="text-fg-2">Loading the catalog…</p>
        )}
      </>
    );
  }

  const hasGpu = info && info.devices.length > 0 ? info.devices.some(isGpuDevice) : null;
  const customLocal = snapshot.custom.filter((m): m is LocalModelEntry => m.kind === 'local');
  const core = [...snapshot.catalog.local.filter((m) => m.tier === 'core'), ...customLocal];
  const accuracy = snapshot.catalog.local.filter((m) => m.tier === 'accuracy');

  return (
    <>
      <PageHeader
        title="Models"
        lead="Pick the speech model that turns your voice into text. Local models are free and work offline; cloud models use your own API key."
      />
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      {actionError && (
        <Notice tone="danger" className="mb-5">
          {actionError}
        </Notice>
      )}

      <Group title="Local" description="Free, offline. Audio never leaves this PC.">
        {core.map((entry) => (
          <LocalRow key={entry.id} entry={entry} act={act} hasGpu={hasGpu} />
        ))}
      </Group>

      <Group
        title="GPU accuracy tier"
        description="The most accurate open models. They need a recent graphics card with CUDA."
      >
        {accuracy.length > 0 ? (
          accuracy.map((entry) => (
            <LocalRow key={entry.id} entry={entry} act={act} hasGpu={hasGpu} />
          ))
        ) : (
          <EmptyState>No GPU accuracy models are in the catalog yet.</EmptyState>
        )}
      </Group>

      <CloudProviders act={act} />
      <CustomEndpoints act={act} />
      <ModelSettings info={info} />
      <TestBench />
    </>
  );
}
