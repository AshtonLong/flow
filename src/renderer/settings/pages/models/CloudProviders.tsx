import { useId, useState } from 'react';
import { LoaderCircle } from 'lucide-react';
import type { CloudModelEntry, ProviderEntry } from '@shared/catalog';
import type { KeyTestResult } from '@shared/ipc';
import { Button, ExternalLink } from '../../components/Button';
import { ConfirmDialog } from '../../components/Dialog';
import { Badge, Notice } from '../../components/Notice';
import { useCatalog } from '../../lib/catalog';
import { errorMessage, formatDate, formatPrice, sentence } from '../../lib/format';
import { cloudByProvider, providerConnected } from '../../lib/models';
import type { Act } from '../Models';

/** Key entry for one provider: store it, test it, show the result. */
export function KeyForm({
  provider,
  onDone,
  onCancel,
}: {
  provider: ProviderEntry;
  /** Called after the key was stored, with the test result. */
  onDone: (result: KeyTestResult) => void;
  onCancel?: () => void;
}) {
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  const inputId = useId();

  const submit = async () => {
    const trimmed = key.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    try {
      await window.flow.secrets.set(provider.id, trimmed);
      const result = await window.flow.secrets.test(provider.id);
      setKey('');
      onDone(result);
    } catch (err) {
      onDone({ ok: false, message: errorMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label htmlFor={inputId} className="text-caption text-fg-2">
        {provider.name} API key. It is encrypted on this PC and never written to the config file.
      </label>
      <div className="flex flex-wrap items-center gap-2">
        <input
          id={inputId}
          type="password"
          autoComplete="off"
          spellCheck={false}
          className="input min-w-[220px] flex-1 font-mono text-[13px]"
          placeholder="Paste the key"
          value={key}
          onChange={(e) => setKey(e.target.value)}
        />
        <Button type="submit" variant="accent" disabled={!key.trim() || busy}>
          {busy && <LoaderCircle size={15} aria-hidden="true" className="spin" />}
          {busy ? 'Testing…' : 'Save and test'}
        </Button>
        {onCancel && (
          <Button variant="subtle" onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
      {provider.keyUrl?.startsWith('https://') && (
        <p className="text-caption text-fg-2">
          No key yet?{' '}
          <ExternalLink href={provider.keyUrl}>Get a key from {provider.name}</ExternalLink>
        </p>
      )}
    </form>
  );
}

function ModelRow({
  entry,
  connected,
  hintId,
  act,
}: {
  entry: CloudModelEntry;
  connected: boolean;
  hintId: string;
  act: Act;
}) {
  const { snapshot } = useCatalog();
  const active = snapshot?.status[entry.id]?.active ?? false;
  return (
    <div className="flex items-center gap-6 border-t border-line px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span>{entry.name}</span>
          {entry.recommended && <Badge tone="accent">Recommended</Badge>}
        </div>
        <p className="mt-0.5 text-caption text-fg-2">
          {sentence(entry.description)}
          {entry.minBilledSeconds
            ? ` Billed for at least ${entry.minBilledSeconds} seconds per request.`
            : ''}
        </p>
      </div>
      <div className="shrink-0 text-right">
        <div className="tabular-nums">{formatPrice(entry.pricePerHour)}</div>
        <div className="text-caption text-fg-2">per hour</div>
      </div>
      <div className="flex w-[96px] shrink-0 justify-end">
        {active ? (
          <Badge tone="accent">Active</Badge>
        ) : (
          <Button
            disabled={!connected}
            aria-describedby={connected ? undefined : hintId}
            onClick={() => void act(() => window.flow.models.activate(entry.id))}
          >
            Set active
          </Button>
        )}
      </div>
    </div>
  );
}

function ProviderCard({
  provider,
  models,
  act,
}: {
  provider: ProviderEntry;
  models: CloudModelEntry[];
  act: Act;
}) {
  const catalog = useCatalog();
  const snapshot = catalog.snapshot;
  const hintId = useId();
  const [editing, setEditing] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<KeyTestResult | null>(null);
  const [confirm, setConfirm] = useState(false);
  if (!snapshot) return null;

  const hasKey = snapshot.keys[provider.id] ?? false;
  const connected = providerConnected(snapshot, provider.id);

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      setResult(await window.flow.secrets.test(provider.id));
    } catch (err) {
      setResult({ ok: false, message: errorMessage(err) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="card">
      <div className="px-4 py-3">
        <div className="flex min-h-8 items-center gap-3">
          <h3 className="font-semibold">{provider.name}</h3>
          {provider.keyless ? (
            <Badge>No key needed</Badge>
          ) : hasKey ? (
            result && !result.ok ? (
              <Badge tone="warn">Key not working</Badge>
            ) : (
              <Badge tone="ok">Connected</Badge>
            )
          ) : (
            <span id={hintId} className="text-caption text-fg-2">
              Add your {provider.name} key to use these models.
            </span>
          )}
          <div className="ml-auto flex items-center gap-1">
            {!provider.keyless && !hasKey && !editing && (
              <Button onClick={() => setEditing(true)}>Connect</Button>
            )}
            {hasKey && !editing && (
              <>
                <Button variant="subtle" disabled={testing} onClick={() => void test()}>
                  {testing ? 'Testing…' : 'Test key'}
                </Button>
                <Button variant="subtle" onClick={() => setEditing(true)}>
                  Replace key
                </Button>
                <Button variant="subtle" onClick={() => setConfirm(true)}>
                  Remove key
                </Button>
              </>
            )}
          </div>
        </div>
        {editing && (
          <div className="mt-2">
            <KeyForm
              provider={provider}
              onCancel={() => setEditing(false)}
              onDone={(r) => {
                setResult(r);
                setEditing(false);
                void catalog.reload();
              }}
            />
          </div>
        )}
        <div aria-live="polite">
          {result && (
            <Notice tone={result.ok ? 'ok' : 'danger'} className="mt-2">
              {result.ok
                ? result.message || 'The key works.'
                : `The key was saved, but the test failed: ${result.message}`}
            </Notice>
          )}
        </div>
      </div>

      {models.map((entry) => (
        <ModelRow key={entry.id} entry={entry} connected={connected} hintId={hintId} act={act} />
      ))}

      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Remove the ${provider.name} key?`}
        description={`Flow forgets the key. ${provider.name} models stop working until you connect again.`}
        confirmLabel="Remove key"
        onConfirm={() => {
          setResult(null);
          void act(() => window.flow.secrets.delete(provider.id));
        }}
      />
    </div>
  );
}

export function CloudProviders({ act }: { act: Act }) {
  const { snapshot } = useCatalog();
  if (!snapshot) return null;
  const groups = cloudByProvider(snapshot);
  const headingId = 'cloud-models-heading';
  return (
    <section aria-labelledby={headingId} className="mb-8">
      <div className="mb-2">
        <h2 id={headingId} className="font-semibold">
          Cloud
        </h2>
        <p className="mt-0.5 max-w-[78ch] text-caption text-fg-2">
          Uses your own API key, billed by the provider at its list price. Audio goes straight from
          this PC to the provider.
        </p>
      </div>
      <div className="flex flex-col gap-[3px]">
        {groups.map(({ provider, models }) => (
          <ProviderCard key={provider.id} provider={provider} models={models} act={act} />
        ))}
      </div>
      <p className="mt-2 text-caption text-fg-2">
        Prices are list prices in US dollars, read on {formatDate(snapshot.catalog.asOf)}. Check the
        provider&rsquo;s page before relying on them.
      </p>
    </section>
  );
}
