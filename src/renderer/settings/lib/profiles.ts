/** Per-app profile helpers: tri-state overrides, tidy serialisation, starters. */
import type { CatalogSnapshot } from '@shared/catalog';
import { PROFILE_SECTIONS, type Profile, type ProfileSection } from '@shared/config';
import { promptLabel } from './format';
import { modelLabel } from './models';

/** A profile as written to the file: defaults and empty sections left out. */
export type ProfileRecord = Record<string, unknown> & { name: string };

export function getOverride(profile: Profile, section: ProfileSection, key: string): unknown {
  return profile[section]?.[key];
}

/** Sets or (with `undefined`) clears one override, dropping the section when it empties. */
export function setOverride(
  profile: Profile,
  section: ProfileSection,
  key: string,
  value: unknown,
): Profile {
  const next: Profile = { ...profile };
  const current = { ...(profile[section] ?? {}) };
  if (value === undefined) delete current[key];
  else current[key] = value;
  if (Object.keys(current).length === 0) delete next[section];
  else next[section] = current;
  return next;
}

/** Omits unset keys so the file holds only what the profile actually overrides. */
export function serializeProfile(profile: Profile): ProfileRecord {
  const out: ProfileRecord = { name: profile.name };
  if (profile.match_process.length > 0) out.match_process = profile.match_process;
  if (profile.match_title.length > 0) out.match_title = profile.match_title;
  if (!profile.enabled) out.enabled = false;
  for (const section of PROFILE_SECTIONS) {
    const overrides = profile[section];
    if (overrides && Object.keys(overrides).length > 0) out[section] = overrides;
  }
  return out;
}

export function serializeProfiles(profiles: Profile[]): ProfileRecord[] {
  return profiles.map(serializeProfile);
}

/** Override keys the editor has pickers for; anything else is listed as "set in the file". */
const EDITABLE: Record<string, readonly string[]> = {
  model: ['active'],
  cleanup: ['llm_enabled', 'llm_prompt', 'filler_removal'],
  insert: ['method', 'paste_shortcut'],
};

export function otherOverrides(profile: Profile): string[] {
  const keys: string[] = [];
  for (const section of PROFILE_SECTIONS) {
    for (const key of Object.keys(profile[section] ?? {})) {
      if (!EDITABLE[section]?.includes(key)) keys.push(`${section}.${key}`);
    }
  }
  return keys;
}

export function matchSummary(profile: Profile): string {
  const parts: string[] = [];
  if (profile.match_process.length > 0) parts.push(profile.match_process.join(', '));
  if (profile.match_title.length > 0) {
    parts.push(`title contains ${profile.match_title.map((t) => `“${t}”`).join(' or ')}`);
  }
  return parts.join('; ') || 'Matches no app yet';
}

export function overrideSummary(profile: Profile, catalog: CatalogSnapshot | null): string {
  if (!profile.enabled) return 'Dictation is off in this app';
  const parts: string[] = [];
  const model = getOverride(profile, 'model', 'active');
  if (typeof model === 'string') parts.push(modelLabel(catalog, model));
  const llm = getOverride(profile, 'cleanup', 'llm_enabled');
  const prompt = getOverride(profile, 'cleanup', 'llm_prompt');
  if (llm === false) parts.push('AI cleanup off');
  else if (llm === true && typeof prompt !== 'string') parts.push('AI cleanup on');
  if (typeof prompt === 'string' && llm !== false) parts.push(`${promptLabel(prompt)} prompt`);
  const filler = getOverride(profile, 'cleanup', 'filler_removal');
  if (filler === false) parts.push('Keeps filler words');
  else if (filler === true) parts.push('Removes filler words');
  const method = getOverride(profile, 'insert', 'method');
  if (method === 'type') parts.push('Types instead of pasting');
  else if (method === 'paste') parts.push('Pastes');
  const shortcut = getOverride(profile, 'insert', 'paste_shortcut');
  if (typeof shortcut === 'string') parts.push(`Pastes with ${shortcut}`);
  const others = otherOverrides(profile).length;
  if (others > 0) parts.push(`${others} more set in the file`);
  return parts.join(', ') || 'No overrides yet';
}

export interface Starter {
  id: string;
  description: string;
  profile: Profile;
}

/** The three one-click starters from the spec. */
export const STARTERS: Starter[] = [
  {
    id: 'terminals',
    description: 'Pastes with Ctrl+Shift+V and skips AI cleanup',
    profile: {
      name: 'Terminals',
      match_title: [],
      enabled: true,
      match_process: ['WindowsTerminal.exe', 'wezterm-gui.exe', 'alacritty.exe'],
      insert: { paste_shortcut: 'Ctrl+Shift+V' },
      cleanup: { llm_enabled: false },
    },
  },
  {
    id: 'editors',
    description: 'Code comment prompt, keeps filler words',
    profile: {
      name: 'Code editors',
      match_title: [],
      enabled: true,
      match_process: ['Code.exe', 'Cursor.exe', 'devenv.exe', 'idea64.exe'],
      cleanup: { llm_prompt: 'code_comment', filler_removal: false },
    },
  },
  {
    id: 'passwords',
    description: 'Turns dictation off',
    profile: {
      name: 'Password managers',
      match_title: [],
      match_process: ['1Password.exe', 'Bitwarden.exe', 'KeePassXC.exe'],
      enabled: false,
    },
  },
];

/** `New profile`, `New profile 2`, … not already used. */
export function uniqueName(existing: Profile[], wanted: string): string {
  const names = new Set(existing.map((p) => p.name.toLowerCase()));
  if (!names.has(wanted.toLowerCase())) return wanted;
  for (let n = 2; ; n++) {
    const candidate = `${wanted} ${n}`;
    if (!names.has(candidate.toLowerCase())) return candidate;
  }
}
