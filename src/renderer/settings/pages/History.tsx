import { useCallback, useEffect, useRef, useState } from 'react';
import { Check, Copy, RefreshCw, Search, Trash2 } from 'lucide-react';
import type { HistoryEntry } from '@shared/types';
import { Button, IconButton } from '../components/Button';
import { EmptyState, Group, PageHeader, SettingRow } from '../components/Card';
import { ConfirmDialog } from '../components/Dialog';
import { Notice } from '../components/Notice';
import { NumberField } from '../components/TextField';
import { Toggle } from '../components/Toggle';
import { useCatalog } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { errorMessage, formatDuration, formatWhen } from '../lib/format';
import { useDebounced, useFlash } from '../lib/hooks';
import { modelLabel } from '../lib/models';

const PAGE_SIZE = 40;

function Row({
  entry,
  onChanged,
  onDeleted,
  onError,
}: {
  entry: HistoryEntry;
  onChanged: (entry: HistoryEntry) => void;
  onDeleted: (id: number) => void;
  onError: (message: string) => void;
}) {
  const { snapshot } = useCatalog();
  const [copied, flashCopied] = useFlash();
  const [rerunning, setRerunning] = useState(false);

  const copy = async () => {
    try {
      await window.flow.app.copyText(entry.text);
      flashCopied();
    } catch (err) {
      onError(errorMessage(err));
    }
  };
  const rerun = async () => {
    setRerunning(true);
    try {
      const next = await window.flow.history.rerun(entry.id);
      if (next) onChanged(next);
    } catch (err) {
      onError(errorMessage(err));
    } finally {
      setRerunning(false);
    }
  };
  const remove = async () => {
    try {
      await window.flow.history.delete(entry.id);
      onDeleted(entry.id);
    } catch (err) {
      onError(errorMessage(err));
    }
  };

  return (
    <li className="card flex items-start gap-3 py-3 pr-2 pl-4">
      <div className="min-w-0 flex-1">
        <p className="selectable whitespace-pre-wrap break-words">{entry.text}</p>
        <dl className="mt-1.5 flex flex-wrap gap-x-4 gap-y-0.5 text-caption text-fg-2">
          <div>
            <dt className="sr-only">Time</dt>
            <dd>{formatWhen(entry.createdAt)}</dd>
          </div>
          {entry.app && (
            <div>
              <dt className="sr-only">App</dt>
              <dd>{entry.app}</dd>
            </div>
          )}
          <div>
            <dt className="sr-only">Model</dt>
            <dd>{modelLabel(snapshot, entry.model)}</dd>
          </div>
          <div>
            <dt className="sr-only">Length</dt>
            <dd>{formatDuration(entry.audioMs)} of audio</dd>
          </div>
        </dl>
      </div>
      <div className="flex shrink-0 items-center">
        <IconButton
          label={copied ? 'Copied' : 'Copy text'}
          icon={copied ? <Check size={15} /> : <Copy size={15} />}
          onClick={() => void copy()}
        />
        <IconButton
          label="Re-run cleanup with the current settings"
          icon={<RefreshCw size={15} className={rerunning ? 'spin' : undefined} />}
          disabled={rerunning}
          onClick={() => void rerun()}
        />
        <IconButton label="Delete" icon={<Trash2 size={15} />} onClick={() => void remove()} />
      </div>
      <span className="sr-only" aria-live="polite">
        {copied ? 'Copied to the clipboard' : ''}
      </span>
    </li>
  );
}

function HistoryList() {
  const [query, setQuery] = useState('');
  const debounced = useDebounced(query.trim(), 250);
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  // Guards against an older search finishing after a newer one.
  const request = useRef(0);

  const load = useCallback(async (search: string, offset: number) => {
    const id = ++request.current;
    setLoading(true);
    try {
      const page = await window.flow.history.search(search, PAGE_SIZE, offset);
      if (id !== request.current) return;
      setEntries((prev) => (offset === 0 ? page : [...prev, ...page]));
      setHasMore(page.length === PAGE_SIZE);
      setError(null);
    } catch (err) {
      if (id === request.current) setError(errorMessage(err));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(debounced, 0);
  }, [debounced, load]);

  // A dictation made while the page is open appears at the top.
  useEffect(
    () =>
      window.flow.dictation.onEvent((event) => {
        if (event.ok) void load(debounced, 0);
      }),
    [debounced, load],
  );

  const clear = async () => {
    try {
      await window.flow.history.clear();
      setEntries([]);
      setHasMore(false);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  return (
    <section aria-label="Past dictations" className="mb-8">
      <div className="mb-2 flex items-center gap-2">
        <div className="input flex flex-1 items-center gap-2">
          <Search size={15} aria-hidden="true" className="shrink-0 text-fg-2" />
          <input
            type="search"
            aria-label="Search history"
            placeholder="Search past dictations"
            className="h-full min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-3"
            autoComplete="off"
            spellCheck={false}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <Button
          disabled={entries.length === 0 && debounced === ''}
          onClick={() => setConfirmClear(true)}
        >
          Clear all
        </Button>
      </div>

      {error && (
        <Notice tone="danger" className="mb-2">
          {error}
        </Notice>
      )}

      <p className="sr-only" aria-live="polite">
        {loading ? '' : `${entries.length} ${entries.length === 1 ? 'entry' : 'entries'} shown`}
      </p>

      {entries.length === 0 && !loading && (
        <EmptyState>
          {debounced
            ? `Nothing in your history matches “${debounced}”.`
            : 'Nothing here yet. Your next dictation will appear in this list.'}
        </EmptyState>
      )}

      <ul className="flex flex-col gap-[3px]">
        {entries.map((entry) => (
          <Row
            key={entry.id}
            entry={entry}
            onError={setError}
            onChanged={(next) =>
              setEntries((prev) => prev.map((e) => (e.id === next.id ? next : e)))
            }
            onDeleted={(id) => setEntries((prev) => prev.filter((e) => e.id !== id))}
          />
        ))}
      </ul>

      {hasMore && (
        <div className="mt-3 flex justify-center">
          <Button disabled={loading} onClick={() => void load(debounced, entries.length)}>
            {loading ? 'Loading…' : 'Load more'}
          </Button>
        </div>
      )}

      <ConfirmDialog
        open={confirmClear}
        onOpenChange={setConfirmClear}
        title="Clear all history?"
        description="Every saved transcript is deleted from this PC. This cannot be undone."
        confirmLabel="Clear history"
        onConfirm={() => void clear()}
      />
    </section>
  );
}

export function History() {
  const { config, set } = useConfig();

  if (!config.history.enabled) {
    return (
      <>
        <PageHeader title="History" />
        <EmptyState
          action={
            <Button variant="accent" onClick={() => set(['history', 'enabled'], true)}>
              Turn on history
            </Button>
          }
        >
          History is off. Turn it on to keep a searchable list of what you dictate, stored only on
          this PC.
        </EmptyState>
      </>
    );
  }

  return (
    <>
      <PageHeader title="History" />
      <Group title="Storage">
        <SettingRow
          label="Keep history"
          description="Saves each transcript on this PC only. Audio is never saved. Turning this off stops new entries."
          control={
            <Toggle
              checked={config.history.enabled}
              onChange={(v) => set(['history', 'enabled'], v)}
            />
          }
        />
        <SettingRow
          label="Delete entries after"
          description="Older transcripts are removed automatically."
          control={
            <NumberField
              value={config.history.retention_days}
              min={1}
              max={3650}
              unit="days"
              onCommit={(v) => set(['history', 'retention_days'], v)}
            />
          }
        />
      </Group>
      <HistoryList />
    </>
  );
}
