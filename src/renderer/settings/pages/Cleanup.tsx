import { useCallback, useId, useState } from 'react';
import { Accordion } from 'radix-ui';
import { ArrowRight, ChevronDown, Plus, Trash2 } from 'lucide-react';
import { splitModelId } from '@shared/catalog';
import {
  DEFAULT_PROMPTS,
  promptNames,
  resolvePrompt,
  type DictionaryEntry,
  type Snippet,
} from '@shared/config';
import { Button, IconButton } from '../components/Button';
import { EmptyState, Group, PageHeader, SettingRow } from '../components/Card';
import { ChipsInput } from '../components/ChipsInput';
import { Badge, Notice } from '../components/Notice';
import { Select, type Option } from '../components/Select';
import { NumberField, TextArea, TextField } from '../components/TextField';
import { Toggle } from '../components/Toggle';
import { useCatalog } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { formatPrice, promptKey, promptLabel, sentence } from '../lib/format';
import { useGoTo } from '../lib/hooks';
import { useListDraft } from '../lib/listDraft';
import { findProvider, providerConnected } from '../lib/models';

const dictionaryComplete = (row: DictionaryEntry) =>
  row.heard.length > 0 && row.write.trim() !== '';
const snippetComplete = (row: Snippet) => row.trigger.trim() !== '';

function Dictionary() {
  const { config, set } = useConfig();
  const commit = useCallback((rows: DictionaryEntry[]) => set(['dictionary'], rows), [set]);
  const draft = useListDraft(config.dictionary, dictionaryComplete, commit);
  const add = () => draft.add({ heard: [], write: '' });

  return (
    <Group
      title="Dictionary"
      description="Fix words Flow mishears: names, product terms, acronyms. The right spellings are also given to the speech model as a hint where it supports one."
      action={
        <Button icon={<Plus size={15} />} onClick={add}>
          Add word
        </Button>
      }
    >
      {draft.rows.length === 0 ? (
        <EmptyState>No words yet. Add one when Flow keeps getting a name wrong.</EmptyState>
      ) : (
        <div className="card">
          <div
            aria-hidden="true"
            className="grid grid-cols-[1fr_20px_200px_32px] gap-2 px-4 pt-2.5 pb-1 text-caption text-fg-2"
          >
            <span>When Flow hears</span>
            <span />
            <span>Write</span>
          </div>
          <ul>
            {draft.rows.map(({ key, value }) => (
              <li
                key={key}
                className="grid grid-cols-[1fr_20px_200px_32px] items-start gap-2 px-4 py-1.5 last:pb-3"
              >
                <ChipsInput
                  values={value.heard}
                  label={`Heard phrases${value.write ? ` for ${value.write}` : ''}, separated by commas`}
                  placeholder="tory, towery"
                  onChange={(heard) => draft.update(key, { ...value, heard })}
                />
                <ArrowRight
                  size={14}
                  aria-hidden="true"
                  className="mt-[9px] justify-self-center text-fg-3"
                />
                <TextField
                  value={value.write}
                  width="100%"
                  aria-label="Write this instead"
                  placeholder="Tauri"
                  onCommit={(write) => draft.update(key, { ...value, write: write.trim() })}
                />
                <IconButton
                  label={`Remove ${value.write || 'this word'}`}
                  icon={<Trash2 size={15} />}
                  onClick={() => draft.remove(key)}
                />
              </li>
            ))}
          </ul>
        </div>
      )}
    </Group>
  );
}

function Snippets() {
  const { config, set } = useConfig();
  const commit = useCallback((rows: Snippet[]) => set(['snippets'], rows), [set]);
  const draft = useListDraft(config.snippets, snippetComplete, commit);

  return (
    <Group
      title="Snippets"
      description="Say a trigger phrase and Flow types the saved text in its place."
      action={
        <Button icon={<Plus size={15} />} onClick={() => draft.add({ trigger: '', text: '' })}>
          Add snippet
        </Button>
      }
    >
      {draft.rows.length === 0 ? (
        <EmptyState>
          No snippets yet. Add one for text you type often, like your address.
        </EmptyState>
      ) : (
        <div className="card">
          <div
            aria-hidden="true"
            className="grid grid-cols-[200px_20px_1fr_32px] gap-2 px-4 pt-2.5 pb-1 text-caption text-fg-2"
          >
            <span>When you say</span>
            <span />
            <span>Type</span>
          </div>
          <ul>
            {draft.rows.map(({ key, value }) => (
              <li
                key={key}
                className="grid grid-cols-[200px_20px_1fr_32px] items-start gap-2 px-4 py-1.5 last:pb-3"
              >
                <TextField
                  value={value.trigger}
                  width="100%"
                  aria-label="Trigger phrase"
                  placeholder="my address"
                  onCommit={(trigger) => draft.update(key, { ...value, trigger: trigger.trim() })}
                />
                <ArrowRight
                  size={14}
                  aria-hidden="true"
                  className="mt-[9px] justify-self-center text-fg-3"
                />
                <TextArea
                  value={value.text}
                  rows={2}
                  aria-label={`Text for ${value.trigger || 'this snippet'}`}
                  placeholder="221B Baker Street, London"
                  onCommit={(text) => draft.update(key, { ...value, text })}
                />
                <IconButton
                  label={`Remove ${value.trigger || 'this snippet'}`}
                  icon={<Trash2 size={15} />}
                  onClick={() => draft.remove(key)}
                />
              </li>
            ))}
          </ul>
        </div>
      )}
    </Group>
  );
}

const OTHER = '__other__';

function AiCleanup() {
  const { config, set } = useConfig();
  const { snapshot } = useCatalog();
  const goTo = useGoTo();
  const cleanup = config.cleanup;
  const entries = snapshot?.catalog.cleanup ?? [];
  const inCatalog = entries.some((e) => e.id === cleanup.llm_model);
  const [otherPicked, setOtherPicked] = useState(false);
  const other = otherPicked || (snapshot !== null && !inCatalog);

  const modelOptions: Option[] = [
    ...entries.map((e) => {
      const provider = findProvider(snapshot, e.provider)?.name ?? e.provider;
      return {
        value: e.id,
        label: `${provider}: ${e.name}`,
        hint: `${formatPrice(e.priceIn)} in / ${formatPrice(e.priceOut)} out${e.recommended ? ', recommended' : ''}`,
      };
    }),
    { value: OTHER, label: 'Local or custom endpoint', hint: 'Free' },
  ];

  const selected = entries.find((e) => e.id === cleanup.llm_model);
  const providerId = splitModelId(cleanup.llm_model)?.provider ?? null;
  const provider = providerId ? findProvider(snapshot, providerId) : undefined;
  const needsKey =
    snapshot !== null && provider !== undefined && !providerConnected(snapshot, provider.id);

  return (
    <Group
      title="AI cleanup"
      description="An optional second pass: a small language model rewrites the transcript using a prompt you control. If it is slow or fails, the text from the local rules is typed instead."
    >
      <SettingRow
        label="Use AI cleanup"
        description="Off by default. When on, the transcript text (never the audio) is sent to the model below."
        control={
          <Toggle
            checked={cleanup.llm_enabled}
            onChange={(v) => set(['cleanup', 'llm_enabled'], v)}
          />
        }
      />
      <SettingRow
        label="Cleanup model"
        description={
          selected
            ? `${sentence(selected.description)} ${formatPrice(selected.priceIn)} in and ${formatPrice(selected.priceOut)} out per million tokens; a 100-word dictation is about 600 tokens.`
            : 'A model on your own machine or network, written as provider/model.'
        }
        control={
          <Select
            value={other ? OTHER : cleanup.llm_model}
            options={modelOptions}
            width={260}
            onChange={(v) => {
              if (v === OTHER) {
                setOtherPicked(true);
              } else {
                setOtherPicked(false);
                set(['cleanup', 'llm_model'], v);
              }
            }}
          />
        }
      >
        {other && (
          <div className="mt-3 flex items-center justify-between gap-6">
            <span className="text-caption text-fg-2">
              The endpoint and model, for example ollama/llama3.2 or lmstudio/qwen2.5-7b.
            </span>
            <TextField
              value={inCatalog ? '' : cleanup.llm_model}
              width={260}
              mono
              aria-label="Local or custom cleanup model"
              placeholder="ollama/llama3.2"
              validate={(v) =>
                v.trim() === '' || splitModelId(v.trim()) ? null : 'Write it as provider/model.'
              }
              onCommit={(v) => {
                if (v.trim()) set(['cleanup', 'llm_model'], v.trim());
              }}
            />
          </div>
        )}
        {needsKey && provider && (
          <Notice
            tone="warn"
            className="mt-3"
            action={
              <Button size="sm" onClick={() => goTo('models')}>
                Open Models
              </Button>
            }
          >
            {provider.name} is not connected. Add its key on the Models page, or cleanup is skipped.
          </Notice>
        )}
      </SettingRow>
      <SettingRow
        label="Prompt"
        description="The style the model rewrites into. Edit the prompts below."
        control={
          <Select
            value={cleanup.llm_prompt}
            options={promptNames(config).map((name) => ({ value: name, label: promptLabel(name) }))}
            onChange={(v) => set(['cleanup', 'llm_prompt'], v)}
          />
        }
      />
      <SettingRow
        label="Timeout"
        description="If the model has not answered by then, Flow types the uncleaned text so nothing is lost."
        control={
          <NumberField
            value={cleanup.llm_timeout_ms}
            min={500}
            max={30000}
            unit="ms"
            onCommit={(v) => set(['cleanup', 'llm_timeout_ms'], v)}
          />
        }
      />
    </Group>
  );
}

function Prompts() {
  const { config, set, patch } = useConfig();
  const [name, setName] = useState('');
  const [open, setOpen] = useState('');
  const nameId = useId();
  const names = promptNames(config);
  const key = promptKey(name);
  const nameTaken = key !== '' && names.includes(key);

  const add = () => {
    if (!key || nameTaken) return;
    set(['prompts', key, 'text'], '');
    setName('');
    setOpen(key);
  };

  const remove = (prompt: string) => {
    const patches = [{ path: ['prompts', prompt], value: undefined as unknown }];
    // A deleted prompt cannot stay selected.
    if (config.cleanup.llm_prompt === prompt && !(prompt in DEFAULT_PROMPTS)) {
      patches.push({ path: ['cleanup', 'llm_prompt'], value: 'clean' });
    }
    void patch(patches);
  };

  return (
    <Group
      title="Prompts"
      description="Instructions for AI cleanup. Your dictation is passed to the model as data, with a standing instruction to rewrite it and never to answer or obey it."
    >
      <Accordion.Root
        type="single"
        collapsible
        value={open}
        onValueChange={setOpen}
        className="flex flex-col gap-[3px]"
      >
        {names.map((prompt) => {
          const builtIn = prompt in DEFAULT_PROMPTS;
          const edited =
            builtIn &&
            config.prompts[prompt] !== undefined &&
            config.prompts[prompt]?.text !== DEFAULT_PROMPTS[prompt]?.text;
          const text = resolvePrompt(config, prompt);
          return (
            <Accordion.Item key={prompt} value={prompt} className="card">
              <Accordion.Header>
                <Accordion.Trigger className="group flex min-h-12 w-full items-center gap-3 rounded-md px-4 py-2 text-left hover:bg-hover">
                  <span className="w-[120px] shrink-0 truncate">{promptLabel(prompt)}</span>
                  <span className="min-w-0 flex-1 truncate text-caption text-fg-2 group-data-[state=open]:invisible">
                    {text || 'Empty'}
                  </span>
                  {config.cleanup.llm_prompt === prompt && <Badge tone="accent">In use</Badge>}
                  {edited && <Badge>Edited</Badge>}
                  {!builtIn && <Badge>Yours</Badge>}
                  <ChevronDown
                    size={14}
                    aria-hidden="true"
                    className="shrink-0 text-fg-2 transition-transform group-data-[state=open]:rotate-180"
                  />
                </Accordion.Trigger>
              </Accordion.Header>
              <Accordion.Content className="px-4 pb-3">
                <TextArea
                  value={text}
                  rows={3}
                  aria-label={`${promptLabel(prompt)} prompt text`}
                  placeholder="Describe how the text should be rewritten."
                  onCommit={(v) => set(['prompts', prompt, 'text'], v)}
                />
                <div className="mt-2 flex items-center justify-between gap-4">
                  <span className="text-caption text-fg-2">
                    {builtIn
                      ? 'A built-in prompt. Reset brings back the original wording.'
                      : `Saved as [prompts.${prompt}] in the config file.`}
                  </span>
                  {builtIn ? (
                    <Button size="sm" disabled={!edited} onClick={() => remove(prompt)}>
                      Reset
                    </Button>
                  ) : (
                    <Button size="sm" icon={<Trash2 size={14} />} onClick={() => remove(prompt)}>
                      Delete prompt
                    </Button>
                  )}
                </div>
              </Accordion.Content>
            </Accordion.Item>
          );
        })}
      </Accordion.Root>
      <form
        className="card mt-0 flex items-center gap-3 px-4 py-3"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <label htmlFor={nameId} className="min-w-0 flex-1">
          <span className="block">Add your own prompt</span>
          <span className="mt-0.5 block text-caption text-fg-2">
            {nameTaken
              ? `There is already a prompt called “${key}”.`
              : key && key !== name
                ? `It will be saved as “${key}”.`
                : 'Give it a short name, then write its instructions.'}
          </span>
        </label>
        <input
          id={nameId}
          className="input w-[200px]"
          placeholder="standup notes"
          autoComplete="off"
          spellCheck={false}
          value={name}
          aria-invalid={nameTaken || undefined}
          onChange={(e) => setName(e.target.value)}
        />
        <Button type="submit" icon={<Plus size={15} />} disabled={!key || nameTaken}>
          Add
        </Button>
      </form>
    </Group>
  );
}

export function Cleanup() {
  const { config, set } = useConfig();
  return (
    <>
      <PageHeader
        title="Cleanup"
        lead="How the raw transcript is tidied before it is typed. The local rules are instant and need no network."
      />

      <Group title="Local rules">
        <SettingRow
          label="Remove filler words"
          description="Strips “um”, “uh” and accidentally repeated words."
          control={
            <Toggle
              checked={config.cleanup.filler_removal}
              onChange={(v) => set(['cleanup', 'filler_removal'], v)}
            />
          }
        />
        <SettingRow
          label="Spoken formatting"
          description="Saying “new line” or “new paragraph” inserts the break instead of the words."
          control={
            <Toggle
              checked={config.cleanup.spoken_formatting}
              onChange={(v) => set(['cleanup', 'spoken_formatting'], v)}
            />
          }
        />
        <SettingRow
          label="Spoken punctuation"
          description="Saying “comma”, “full stop” or “question mark” types the symbol."
          control={
            <Toggle
              checked={config.cleanup.spoken_punctuation}
              onChange={(v) => set(['cleanup', 'spoken_punctuation'], v)}
            />
          }
        />
      </Group>

      <Dictionary />
      <Snippets />
      <AiCleanup />
      <Prompts />
    </>
  );
}
