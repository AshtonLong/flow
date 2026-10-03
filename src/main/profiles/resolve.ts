/** Per-app profiles: which profile matches the focused app, and the config that results. */
import {
  PROFILE_SECTIONS,
  validateConfig,
  type Config,
  type Profile,
  type ProfileSection,
} from '@shared/config';
import type { FocusedApp } from '@shared/types';

export interface ResolvedProfile {
  /** Effective config: base config with the matching profile's overrides merged and re-validated. */
  config: Config;
  /** Name of the matching profile, if any. */
  profile: string | null;
  /** The profile sets `enabled = false`: dictation is off in this app. */
  disabled: boolean;
}

/** Lower-cased executable name without directory or `.exe`. */
function normaliseProcess(name: string): string {
  const base = name.trim().toLowerCase().split(/[\\/]/).pop() ?? '';
  return base.endsWith('.exe') ? base.slice(0, -4) : base;
}

function matchesProcess(pattern: string, processName: string): boolean {
  const wanted = normaliseProcess(pattern);
  if (wanted === '') return false;
  if (!wanted.includes('*')) return wanted === processName;
  const source = wanted
    .split('*')
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*');
  return new RegExp(`^${source}$`).test(processName);
}

function matchesTitle(pattern: string, title: string): boolean {
  if (pattern === '') return false;
  const regex = /^\/(.+)\/([a-z]*)$/s.exec(pattern);
  if (regex) {
    try {
      // `g` and `y` make `test` stateful and add nothing to a yes/no match.
      return new RegExp(regex[1]!, regex[2]!.replace(/[gy]/g, '')).test(title);
    } catch {
      // Not a valid regex after all: fall through and treat it as plain text.
    }
  }
  return title.toLowerCase().includes(pattern.toLowerCase());
}

/**
 * The first profile, in file order, whose process or title patterns match the
 * focused app. A profile with both lists matches if either does.
 */
export function matchProfile(profiles: Profile[], app: FocusedApp | null): Profile | null {
  if (!app) return null;
  const processName = normaliseProcess(app.processName);
  for (const profile of profiles) {
    if (
      profile.match_process.some((pattern) => matchesProcess(pattern, processName)) ||
      profile.match_title.some((pattern) => matchesTitle(pattern, app.title))
    ) {
      return profile;
    }
  }
  return null;
}

/** Applies the matching profile's overrides to `config`, section by section. */
export function resolveProfile(config: Config, app: FocusedApp | null): ResolvedProfile {
  const profile = matchProfile(config.profiles, app);
  if (!profile) return { config, profile: null, disabled: false };

  const resolved: ResolvedProfile = {
    config,
    profile: profile.name,
    disabled: profile.enabled === false,
  };
  const merged: Record<string, unknown> = structuredClone(config);
  const sections = merged as Record<ProfileSection, Record<string, unknown>>;
  let overridden = false;
  for (const section of PROFILE_SECTIONS) {
    const overrides = profile[section];
    if (!overrides) continue;
    for (const [key, value] of Object.entries(overrides)) {
      // Only settings that exist: an unknown key is a typo, not a new option.
      if (!Object.hasOwn(config[section], key)) continue;
      sections[section][key] = value;
      overridden = true;
    }
  }
  if (!overridden) return resolved;

  // `validateConfig` resets a bad key to the schema default, but a bad override
  // should fall back to the user's own setting. So put the base value back for
  // each key it rejects, then validate again.
  let result = validateConfig(merged);
  for (let attempt = 0; attempt < 3 && result.issues.length > 0; attempt++) {
    for (const issue of result.issues) {
      const [section, key] = issue.path;
      if (!PROFILE_SECTIONS.includes(section as ProfileSection)) continue;
      const name = section as ProfileSection;
      const base: Record<string, unknown> = config[name];
      if (typeof key === 'string') sections[name][key] = base[key];
      else sections[name] = structuredClone(base);
    }
    result = validateConfig(merged);
  }
  return result.issues.length === 0 ? { ...resolved, config: result.config } : resolved;
}
