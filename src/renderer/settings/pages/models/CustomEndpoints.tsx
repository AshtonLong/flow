import { useId, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { CUSTOM_PREFIX } from '@shared/catalog';
import { Button, IconButton } from '../../components/Button';
import { EmptyState, Group } from '../../components/Card';
import { ConfirmDialog } from '../../components/Dialog';
import { Badge } from '../../components/Notice';
import { useCatalog } from '../../lib/catalog';
import { useConfig } from '../../lib/config';
import { customEndpoints, type CustomEndpoint } from '../../lib/models';
import type { Act } from '../Models';

const ID_PATTERN = /^[a-z0-9][a-z0-9._-]*$/;

/** What the user types becomes the table name in `[providers.<id>]`. */
export function endpointId(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/\s+/g, '-')
    .replace(/[^a-z0-9._-]/g, '')
    .replace(/^[._-]+/, '')
    .slice(0, 64);
}

function isHttpUrl(text: string): boolean {
  try {
    const url = new URL(text);
    return url.protocol === 'http:' || url.protocol === 'https:';
  } catch {
    return false;
  }
}

function AddForm({ taken, onClose, act }: { taken: Set<string>; onClose: () => void; act: Act }) {
  const { patch } = useConfig();
  const [name, setName] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [model, setModel] = useState('');
  const [key, setKey] = useState('');
  const [tried, setTried] = useState(false);
  const ids = { name: useId(), url: useId(), model: useId(), key: useId(), error: useId() };

  const id = endpointId(name);
  const nameError = !id
    ? 'Give the endpoint a short name.'
    : !ID_PATTERN.test(id)
      ? 'Use letters, digits, dots and dashes.'
      : taken.has(id)
        ? `“${id}” is already in use.`
        : null;
  const urlError = isHttpUrl(baseUrl.trim())
    ? null
    : 'Enter a full URL, such as http://192.168.1.20:8000/v1.';
  const modelError = model.trim() ? null : 'Enter the model name the server expects.';
  const firstError = nameError ?? urlError ?? modelError;

  const submit = async () => {
    setTried(true);
    if (firstError) return;
    const secret = key.trim();
    await act(async () => {
      if (secret) await window.flow.secrets.set(id, secret);
      await patch([
        {
          path: ['providers', id],
          value: {
            base_url: baseUrl.trim().replace(/\/+$/, ''),
            model: model.trim(),
            ...(secret ? { api_key: `secret:${id}` } : {}),
          },
        },
      ]);
    });
    onClose();
  };

  return (
    <form
      className="card flex flex-col gap-3 px-4 py-4"
      aria-label="Add a custom endpoint"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.name}>Name</label>
          <input
            id={ids.name}
            className="input"
            placeholder="custom-lan"
            autoComplete="off"
            spellCheck={false}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <span className="text-caption text-fg-2">
            {id && id !== name
              ? `Saved as “${id}” in the config file.`
              : 'A short id for this server.'}
          </span>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.model}>Model name</label>
          <input
            id={ids.model}
            className="input"
            placeholder="whisper-1"
            autoComplete="off"
            spellCheck={false}
            value={model}
            onChange={(e) => setModel(e.target.value)}
          />
          <span className="text-caption text-fg-2">Sent to the server with each request.</span>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.url}>Base URL</label>
          <input
            id={ids.url}
            className="input font-mono text-[13px]"
            placeholder="http://192.168.1.20:8000/v1"
            autoComplete="off"
            spellCheck={false}
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
          />
          <span className="text-caption text-fg-2">The address up to and including /v1.</span>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.key}>API key (optional)</label>
          <input
            id={ids.key}
            type="password"
            className="input font-mono text-[13px]"
            autoComplete="off"
            spellCheck={false}
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
          <span className="text-caption text-fg-2">
            Encrypted on this PC, not saved in the file.
          </span>
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Button type="submit" variant="accent">
          Add endpoint
        </Button>
        <Button variant="subtle" onClick={onClose}>
          Cancel
        </Button>
        {tried && firstError && (
          <span id={ids.error} role="alert" className="text-caption text-danger">
            {firstError}
          </span>
        )}
      </div>
    </form>
  );
}

function EndpointRow({ endpoint, act }: { endpoint: CustomEndpoint; act: Act }) {
  const { config, patch } = useConfig();
  const { snapshot } = useCatalog();
  const [confirm, setConfirm] = useState(false);
  const modelId = `${CUSTOM_PREFIX}${endpoint.id}`;
  const active = config.model.active === modelId;

  const remove = () =>
    act(async () => {
      if (active) await window.flow.models.activate(config.model.fallback);
      await patch([{ path: ['providers', endpoint.id], value: undefined }]);
      if (snapshot?.keys[endpoint.id]) await window.flow.secrets.delete(endpoint.id);
    });

  return (
    <div className="card flex items-center gap-6 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-semibold">{endpoint.id}</span>
          {endpoint.model && <span className="text-fg-2">{endpoint.model}</span>}
          {snapshot?.keys[endpoint.id] && <Badge>Key stored</Badge>}
        </div>
        <p className="selectable mt-0.5 truncate font-mono text-caption text-fg-2">
          {endpoint.baseUrl}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {active ? (
          <Badge tone="accent">Active</Badge>
        ) : (
          <Button onClick={() => void act(() => window.flow.models.activate(modelId))}>
            Set active
          </Button>
        )}
        <IconButton
          label={`Remove ${endpoint.id}`}
          icon={<Trash2 size={15} />}
          onClick={() => setConfirm(true)}
        />
      </div>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Remove ${endpoint.id}?`}
        description={
          active
            ? 'This endpoint is the active model. Flow switches to your fallback model and forgets the endpoint and its key.'
            : 'Flow forgets this endpoint and its key.'
        }
        confirmLabel="Remove endpoint"
        onConfirm={() => void remove()}
      />
    </div>
  );
}

export function CustomEndpoints({ act }: { act: Act }) {
  const { config } = useConfig();
  const { snapshot } = useCatalog();
  const [adding, setAdding] = useState(false);
  const endpoints = customEndpoints(config, snapshot);
  const taken = new Set([
    ...Object.keys(config.providers),
    ...(snapshot?.catalog.providers.map((p) => p.id) ?? []),
  ]);

  return (
    <Group
      title="Custom endpoint"
      description="Any server that speaks the OpenAI transcription API: a self-hosted Whisper, or a GPU machine on your network."
      action={
        !adding && (
          <Button icon={<Plus size={15} />} onClick={() => setAdding(true)}>
            Add endpoint
          </Button>
        )
      }
    >
      {endpoints.map((endpoint) => (
        <EndpointRow key={endpoint.id} endpoint={endpoint} act={act} />
      ))}
      {endpoints.length === 0 && !adding && <EmptyState>No custom endpoints yet.</EmptyState>}
      {adding && <AddForm taken={taken} act={act} onClose={() => setAdding(false)} />}
    </Group>
  );
}
