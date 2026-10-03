import { useState } from 'react';
import { Pencil, Plus, Trash2 } from 'lucide-react';
import { promptNames, type Profile, type ProfileSection } from '@shared/config';
import { Button, IconButton } from '../components/Button';
import { EmptyState, Group, PageHeader, SettingRow } from '../components/Card';
import { ChipsInput } from '../components/ChipsInput';
import { ConfirmDialog, Modal } from '../components/Dialog';
import { Badge, Notice } from '../components/Notice';
import { Select, type Option } from '../components/Select';
import { TextField } from '../components/TextField';
import { Toggle } from '../components/Toggle';
import { useCatalog } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { promptLabel } from '../lib/format';
import { modelLabel, readyModels } from '../lib/models';
import {
  getOverride,
  matchSummary,
  otherOverrides,
  overrideSummary,
  serializeProfiles,
  setOverride,
  STARTERS,
  uniqueName,
} from '../lib/profiles';

const DEFAULT = '__default__';

/** A tri-state picker: "Use default" leaves the key out of the file. */
function OverrideSelect({
  profile,
  section,
  name,
  options,
  onChange,
  disabled,
  width = 220,
  encode = String,
  decode = (v: string) => v as unknown,
}: {
  profile: Profile;
  section: ProfileSection;
  name: string;
  options: Option[];
  onChange: (profile: Profile) => void;
  disabled?: boolean;
  width?: number;
  encode?: (value: unknown) => string;
  decode?: (value: string) => unknown;
}) {
  const current = getOverride(profile, section, name);
  const value = current === undefined ? DEFAULT : encode(current);
  const all: Option[] = [{ value: DEFAULT, label: 'Use default' }, ...options];
  if (!all.some((o) => o.value === value)) all.push({ value, label: value, hint: 'From the file' });
  return (
    <Select
      value={value}
      options={all}
      width={width}
      disabled={disabled}
      onChange={(v) =>
        onChange(setOverride(profile, section, name, v === DEFAULT ? undefined : decode(v)))
      }
    />
  );
}

const ON_OFF: Option[] = [
  { value: 'true', label: 'On' },
  { value: 'false', label: 'Off' },
];
const toBool = (v: string) => v === 'true';

const SHORTCUTS: Option[] = [
  { value: 'Ctrl+V', label: 'Ctrl+V' },
  { value: 'Ctrl+Shift+V', label: 'Ctrl+Shift+V' },
  { value: 'Shift+Insert', label: 'Shift+Insert' },
];

function Editor({
  profile,
  nameTaken,
  onChange,
}: {
  profile: Profile;
  nameTaken: (name: string) => boolean;
  onChange: (profile: Profile) => void;
}) {
  const { config } = useConfig();
  const { snapshot } = useCatalog();
  const off = !profile.enabled;
  const others = otherOverrides(profile);
  const modelOptions: Option[] = snapshot
    ? readyModels(snapshot).map((m) => ({ value: m.id, label: modelLabel(snapshot, m.id) }))
    : [];
  const current = getOverride(profile, 'model', 'active');
  if (typeof current === 'string' && !modelOptions.some((o) => o.value === current)) {
    modelOptions.push({ value: current, label: modelLabel(snapshot, current), hint: 'Not ready' });
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-[3px]">
        <SettingRow
          label="Name"
          description="Shown in this list only."
          control={
            <TextField
              value={profile.name}
              width={260}
              validate={(v) =>
                !v.trim()
                  ? 'A profile needs a name.'
                  : v.trim() !== profile.name && nameTaken(v.trim())
                    ? 'Another profile has this name.'
                    : null
              }
              onCommit={(name) => onChange({ ...profile, name: name.trim() })}
            />
          }
        />
        <div className="card px-4 py-3">
          <div>Apps</div>
          <div className="mt-0.5 mb-2 text-caption text-fg-2">
            The program file names this profile applies to, such as slack.exe. Press Enter after
            each one.
          </div>
          <ChipsInput
            values={profile.match_process}
            label="Process names"
            placeholder="slack.exe"
            mono
            onChange={(match_process) => onChange({ ...profile, match_process })}
          />
        </div>
        <div className="card px-4 py-3">
          <div>Window titles</div>
          <div className="mt-0.5 mb-2 text-caption text-fg-2">
            Also applies when the window title contains any of these. Useful for one site in a
            browser.
          </div>
          <ChipsInput
            values={profile.match_title}
            label="Window title contains"
            placeholder="Gmail"
            onChange={(match_title) => onChange({ ...profile, match_title })}
          />
        </div>
        <SettingRow
          label="Disable dictation in this app"
          description="Hotkeys do nothing while it is focused. Good for games and password managers."
          control={<Toggle checked={off} onChange={(v) => onChange({ ...profile, enabled: !v })} />}
        />
      </div>

      <div>
        <h3 className="font-semibold">Overrides</h3>
        <p className="mt-0.5 mb-2 text-caption text-fg-2">
          {off
            ? 'Dictation is off in this app, so these have no effect.'
            : '“Use default” follows your main settings and keeps the key out of the config file.'}
        </p>
        <div className="flex flex-col gap-[3px]">
          <SettingRow
            label="Model"
            description="The speech model to use in this app."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="model"
                name="active"
                options={modelOptions}
                onChange={onChange}
                disabled={off}
                width={260}
              />
            }
          />
          <SettingRow
            label="AI cleanup"
            description="Turn the language-model pass on or off here."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="cleanup"
                name="llm_enabled"
                options={ON_OFF}
                decode={toBool}
                onChange={onChange}
                disabled={off}
              />
            }
          />
          <SettingRow
            label="Cleanup prompt"
            description="The style AI cleanup rewrites into."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="cleanup"
                name="llm_prompt"
                options={promptNames(config).map((p) => ({ value: p, label: promptLabel(p) }))}
                onChange={onChange}
                disabled={off}
              />
            }
          />
          <SettingRow
            label="Remove filler words"
            description="Whether “um”, “uh” and repeats are stripped."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="cleanup"
                name="filler_removal"
                options={ON_OFF}
                decode={toBool}
                onChange={onChange}
                disabled={off}
              />
            }
          />
          <SettingRow
            label="Insertion method"
            description="Paste the text, or type it key by key where paste is blocked."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="insert"
                name="method"
                options={[
                  { value: 'paste', label: 'Paste' },
                  { value: 'type', label: 'Type' },
                ]}
                onChange={onChange}
                disabled={off}
              />
            }
          />
          <SettingRow
            label="Paste shortcut"
            description="The keys this app uses for paste. Terminals often want Ctrl+Shift+V."
            disabled={off}
            control={
              <OverrideSelect
                profile={profile}
                section="insert"
                name="paste_shortcut"
                options={SHORTCUTS}
                onChange={onChange}
                disabled={off}
              />
            }
          />
        </div>
        {others.length > 0 && (
          <Notice className="mt-3">
            This profile also sets {others.join(', ')} in the config file. Those are kept as they
            are.
          </Notice>
        )}
      </div>
    </div>
  );
}

export function Profiles() {
  const { config, set } = useConfig();
  const { snapshot } = useCatalog();
  const [editing, setEditing] = useState<number | null>(null);
  const [removing, setRemoving] = useState<number | null>(null);
  const profiles = config.profiles;

  const write = (next: Profile[]) => set(['profiles'], serializeProfiles(next));
  const replace = (index: number, profile: Profile) =>
    write(profiles.map((p, i) => (i === index ? profile : p)));

  const add = (profile: Profile) => {
    write([...profiles, { ...profile, name: uniqueName(profiles, profile.name) }]);
    return profiles.length;
  };

  const startersLeft = STARTERS.filter(
    (s) => !profiles.some((p) => p.name.toLowerCase() === s.profile.name.toLowerCase()),
  );
  const edited = editing !== null ? profiles[editing] : undefined;
  const doomed = removing !== null ? profiles[removing] : undefined;

  return (
    <>
      <PageHeader
        title="Profiles"
        lead="Change how Flow behaves in particular apps, matched by program name or window title."
      />

      <Group
        title="Your profiles"
        action={
          <Button
            icon={<Plus size={15} />}
            onClick={() =>
              setEditing(
                add({ name: 'New profile', match_process: [], match_title: [], enabled: true }),
              )
            }
          >
            Add profile
          </Button>
        }
      >
        {profiles.length === 0 && (
          <EmptyState>No profiles yet. Add one, or start from a suggestion below.</EmptyState>
        )}
        {profiles.map((profile, index) => (
          <div key={`${profile.name}-${index}`} className="card flex items-center gap-4 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-semibold">{profile.name}</span>
                {!profile.enabled && <Badge tone="warn">Dictation off</Badge>}
              </div>
              <p className="mt-0.5 truncate text-caption text-fg-2">{matchSummary(profile)}</p>
              {profile.enabled && (
                <p className="mt-0.5 truncate text-caption text-fg-2">
                  {overrideSummary(profile, snapshot)}
                </p>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button icon={<Pencil size={14} />} onClick={() => setEditing(index)}>
                Edit
              </Button>
              <IconButton
                label={`Delete ${profile.name}`}
                icon={<Trash2 size={15} />}
                onClick={() => setRemoving(index)}
              />
            </div>
          </div>
        ))}
      </Group>

      {startersLeft.length > 0 && (
        <Group title="Suggestions" description="Common setups. Add one, then adjust the app list.">
          {startersLeft.map((starter) => (
            <div key={starter.id} className="card flex items-center gap-4 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div>{starter.profile.name}</div>
                <p className="mt-0.5 truncate text-caption text-fg-2">
                  {starter.description}. Matches {starter.profile.match_process.join(', ')}.
                </p>
              </div>
              <Button onClick={() => add(starter.profile)}>Add</Button>
            </div>
          ))}
        </Group>
      )}

      <Modal
        open={edited !== undefined}
        onOpenChange={(open) => {
          if (!open) setEditing(null);
        }}
        title={edited ? `Edit ${edited.name}` : 'Edit profile'}
        description="Changes are saved as you make them."
        width={640}
      >
        {edited && editing !== null && (
          <Editor
            profile={edited}
            nameTaken={(name) =>
              profiles.some((p, i) => i !== editing && p.name.toLowerCase() === name.toLowerCase())
            }
            onChange={(profile) => replace(editing, profile)}
          />
        )}
      </Modal>

      <ConfirmDialog
        open={doomed !== undefined}
        onOpenChange={(open) => {
          if (!open) setRemoving(null);
        }}
        title={doomed ? `Delete ${doomed.name}?` : 'Delete profile?'}
        description="The profile is removed from the config file. The apps it matched go back to your main settings."
        confirmLabel="Delete profile"
        onConfirm={() => {
          if (removing !== null) write(profiles.filter((_, i) => i !== removing));
          setRemoving(null);
        }}
      />
    </>
  );
}
