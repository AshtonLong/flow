import { describe, expect, it } from 'vitest';
import { validateConfig, type Config, type Profile } from '@shared/config';
import type { FocusedApp } from '@shared/types';
import { matchProfile, resolveProfile } from '../../../src/main/profiles/resolve';

function app(processName: string, title = ''): FocusedApp {
  return { windowId: '1', processName, title, pid: 100 };
}

function profile(fields: Partial<Profile> & { name: string }): Profile {
  return { match_process: [], match_title: [], enabled: true, ...fields };
}

function config(raw: Record<string, unknown>): Config {
  const { config: parsed, issues } = validateConfig(raw);
  expect(issues).toEqual([]);
  return parsed;
}

const matches = (fields: Partial<Profile>, focused: FocusedApp | null) =>
  matchProfile([profile({ name: 'P', ...fields })], focused) !== null;

describe('matchProfile: process', () => {
  it('matches the exe name case-insensitively, with or without .exe', () => {
    expect(matches({ match_process: ['slack.exe'] }, app('slack.exe'))).toBe(true);
    expect(matches({ match_process: ['Slack.EXE'] }, app('slack.exe'))).toBe(true);
    expect(matches({ match_process: ['slack'] }, app('Slack.exe'))).toBe(true);
    expect(matches({ match_process: ['slack.exe'] }, app('slack'))).toBe(true);
    expect(matches({ match_process: ['WindowsTerminal.exe'] }, app('windowsterminal.exe'))).toBe(
      true,
    );
  });

  it('matches the whole name, not a part of it', () => {
    expect(matches({ match_process: ['slack'] }, app('slack-helper.exe'))).toBe(false);
    expect(matches({ match_process: ['code'] }, app('vscode.exe'))).toBe(false);
    expect(matches({ match_process: ['a.b'] }, app('axb.exe'))).toBe(false);
  });

  it('supports * wildcards', () => {
    expect(matches({ match_process: ['wezterm*'] }, app('wezterm-gui.exe'))).toBe(true);
    expect(matches({ match_process: ['*term*'] }, app('WindowsTerminal.exe'))).toBe(true);
    expect(matches({ match_process: ['*.game.exe'] }, app('super.game.exe'))).toBe(true);
    expect(matches({ match_process: ['code*.exe'] }, app('Code - Insiders.exe'))).toBe(true);
    expect(matches({ match_process: ['wezterm*'] }, app('alacritty.exe'))).toBe(false);
    expect(matches({ match_process: ['*'] }, app('anything.exe'))).toBe(true);
  });

  it('treats regex characters in a pattern literally', () => {
    expect(matches({ match_process: ['c++(x)'] }, app('c++(x).exe'))).toBe(true);
    expect(matches({ match_process: ['a+'] }, app('aaa.exe'))).toBe(false);
  });

  it('accepts a full path as the process name', () => {
    expect(matches({ match_process: ['slack.exe'] }, app('C:\\Apps\\Slack\\slack.exe'))).toBe(true);
  });

  it('ignores empty patterns', () => {
    expect(matches({ match_process: [''] }, app('slack.exe'))).toBe(false);
    expect(matches({ match_process: ['  '] }, app('slack.exe'))).toBe(false);
  });
});

describe('matchProfile: title', () => {
  it('matches a case-insensitive substring', () => {
    expect(matches({ match_title: ['inbox'] }, app('chrome.exe', 'Inbox (3) - Gmail'))).toBe(true);
    expect(matches({ match_title: ['GMAIL'] }, app('chrome.exe', 'Inbox (3) - Gmail'))).toBe(true);
    expect(matches({ match_title: ['outlook'] }, app('chrome.exe', 'Inbox (3) - Gmail'))).toBe(
      false,
    );
  });

  it('treats /pattern/flags as a regex with exactly those flags', () => {
    const title = 'main.ts - flow - Visual Studio Code';
    expect(matches({ match_title: ['/\\.tsx? - /'] }, app('code.exe', title))).toBe(true);
    expect(matches({ match_title: ['/visual studio/'] }, app('code.exe', title))).toBe(false);
    expect(matches({ match_title: ['/visual studio/i'] }, app('code.exe', title))).toBe(true);
    expect(matches({ match_title: ['/^main.*Code$/'] }, app('code.exe', title))).toBe(true);
    expect(matches({ match_title: ['/^flow/'] }, app('code.exe', title))).toBe(false);
  });

  it('gives the same answer every time for a /g regex', () => {
    const profiles = [profile({ name: 'P', match_title: ['/code/gi'] })];
    const focused = app('code.exe', 'Visual Studio Code');
    for (let i = 0; i < 4; i++) expect(matchProfile(profiles, focused)).not.toBeNull();
  });

  it('falls back to plain text for an invalid regex', () => {
    expect(matches({ match_title: ['/(unclosed/'] }, app('x.exe', 'has /(unclosed/ in it'))).toBe(
      true,
    );
    expect(matches({ match_title: ['/(unclosed/'] }, app('x.exe', 'something else'))).toBe(false);
    // A path-like title pattern is not a regex either way.
    expect(matches({ match_title: ['C:/Users/'] }, app('x.exe', 'c:/users/me'))).toBe(true);
  });

  it('ignores empty patterns', () => {
    expect(matches({ match_title: [''] }, app('x.exe', 'anything'))).toBe(false);
  });
});

describe('matchProfile: choosing', () => {
  const profiles = [
    profile({ name: 'Terminals', match_process: ['wt.exe', 'wezterm-gui.exe'] }),
    profile({ name: 'Mail', match_process: ['outlook.exe'], match_title: ['gmail'] }),
    profile({ name: 'Browsers', match_process: ['chrome.exe'] }),
    profile({ name: 'Never' }),
  ];

  it('returns null without a focused app or a match', () => {
    expect(matchProfile(profiles, null)).toBeNull();
    expect(matchProfile(profiles, app('notepad.exe', 'Untitled'))).toBeNull();
    expect(matchProfile([], app('wt.exe'))).toBeNull();
  });

  it('matches a profile with both lists if either matches', () => {
    expect(matchProfile(profiles, app('outlook.exe', 'Calendar'))?.name).toBe('Mail');
    expect(matchProfile(profiles, app('firefox.exe', 'Inbox - Gmail'))?.name).toBe('Mail');
  });

  it('picks the first match in file order', () => {
    expect(matchProfile(profiles, app('chrome.exe', 'Inbox - Gmail'))?.name).toBe('Mail');
    expect(matchProfile(profiles, app('chrome.exe', 'News'))?.name).toBe('Browsers');
    expect(matchProfile(profiles, app('wezterm-gui.exe', 'gmail'))?.name).toBe('Terminals');
  });

  it('never matches a profile with no patterns', () => {
    expect(matchProfile([profile({ name: 'Never' })], app('x.exe', ''))).toBeNull();
  });
});

describe('resolveProfile', () => {
  const base = config({
    audio: { sounds: false, max_recording_seconds: 120 },
    model: { active: 'parakeet-tdt-0.6b-v2', keep_loaded_minutes: 30 },
    cleanup: { llm_enabled: false, llm_prompt: 'clean', llm_timeout_ms: 5000 },
    insert: { paste_shortcut: 'Ctrl+V', trailing_space: false },
    overlay: { style: 'minimal' },
    history: { enabled: true, retention_days: 90 },
    profiles: [
      {
        name: 'Terminals',
        match_process: ['WindowsTerminal.exe', 'wezterm-gui.exe'],
        insert: { paste_shortcut: 'Ctrl+Shift+V' },
        cleanup: { llm_enabled: false },
      },
      {
        name: 'Slack',
        match_process: ['slack.exe'],
        model: { active: 'groq/whisper-large-v3-turbo' },
        cleanup: { llm_enabled: true, llm_prompt: 'casual' },
      },
      { name: 'Vault', match_title: ['/1password|bitwarden/i'], enabled: false },
      {
        name: 'Broken',
        match_process: ['broken.exe'],
        audio: { max_recording_seconds: 1, sounds: true },
        cleanup: { llm_timeout_ms: 'fast', llm_enabled: true },
        overlay: { style: 'banner', position: 'top-left' },
        history: { retention_days: 0, enabled: 'yes' },
        insert: { method: 'telepathy', not_a_setting: 1 },
        model: { active: '' },
      },
      { name: 'Plain', match_process: ['plain.exe'] },
      {
        name: 'Everything',
        match_process: ['all.exe'],
        audio: { keep_mic_warm: true },
        model: { device: 'gpu' },
        cleanup: { filler_removal: false },
        insert: { method: 'type' },
        overlay: { size: 'large' },
        history: { enabled: false },
      },
    ],
  });

  it('returns the base config itself when nothing matches', () => {
    for (const focused of [null, app('notepad.exe', 'Untitled')]) {
      const resolved = resolveProfile(base, focused);
      expect(resolved).toEqual({ config: base, profile: null, disabled: false });
      expect(resolved.config).toBe(base);
    }
  });

  it('merges overrides shallowly into each section', () => {
    const resolved = resolveProfile(base, app('WindowsTerminal.exe', 'pwsh'));
    expect(resolved.profile).toBe('Terminals');
    expect(resolved.disabled).toBe(false);
    expect(resolved.config.insert).toEqual({ ...base.insert, paste_shortcut: 'Ctrl+Shift+V' });
    expect(resolved.config.cleanup).toEqual(base.cleanup);
    // Untouched sections keep the user's settings, not the schema defaults.
    expect(resolved.config.audio).toEqual(base.audio);
    expect(resolved.config.history).toEqual(base.history);
    expect(resolved.config.general).toEqual(base.general);
    expect(resolved.config.profiles).toEqual(base.profiles);
  });

  it('applies overrides from several sections', () => {
    const resolved = resolveProfile(base, app('slack.exe', 'general'));
    expect(resolved.profile).toBe('Slack');
    expect(resolved.config.model).toEqual({ ...base.model, active: 'groq/whisper-large-v3-turbo' });
    expect(resolved.config.cleanup).toEqual({
      ...base.cleanup,
      llm_enabled: true,
      llm_prompt: 'casual',
    });
    expect(resolved.config.insert).toEqual(base.insert);
  });

  it('covers every overridable section', () => {
    const { config: effective } = resolveProfile(base, app('all.exe'));
    expect(effective.audio.keep_mic_warm).toBe(true);
    expect(effective.model.device).toBe('gpu');
    expect(effective.cleanup.filler_removal).toBe(false);
    expect(effective.insert.method).toBe('type');
    expect(effective.overlay.size).toBe('large');
    expect(effective.history.enabled).toBe(false);
    expect(effective.history.retention_days).toBe(90);
  });

  it('reports a profile that disables dictation', () => {
    const resolved = resolveProfile(base, app('chrome.exe', 'Bitwarden Web Vault'));
    expect(resolved).toEqual({ config: base, profile: 'Vault', disabled: true });
  });

  it('returns the base settings for a profile with no overrides', () => {
    const resolved = resolveProfile(base, app('plain.exe'));
    expect(resolved).toEqual({ config: base, profile: 'Plain', disabled: false });
  });

  it('drops an invalid override and keeps the base value, not the schema default', () => {
    const resolved = resolveProfile(base, app('broken.exe'));
    expect(resolved.profile).toBe('Broken');
    const effective = resolved.config;
    // Invalid: base value (120), not the default (300).
    expect(effective.audio.max_recording_seconds).toBe(120);
    expect(effective.cleanup.llm_timeout_ms).toBe(5000);
    expect(effective.overlay.style).toBe('minimal');
    expect(effective.history.retention_days).toBe(90);
    expect(effective.history.enabled).toBe(true);
    expect(effective.insert.method).toBe('paste');
    expect(effective.model.active).toBe('parakeet-tdt-0.6b-v2');
    // Valid overrides in the same sections still apply.
    expect(effective.audio.sounds).toBe(true);
    expect(effective.cleanup.llm_enabled).toBe(true);
    expect(effective.overlay.position).toBe('top-left');
    // Unknown keys do not leak into the config.
    expect(effective.insert).toEqual(base.insert);
    expect(Object.keys(effective.insert)).not.toContain('not_a_setting');
  });

  it('does not modify the base config', () => {
    const before = structuredClone(base);
    resolveProfile(base, app('slack.exe'));
    resolveProfile(base, app('broken.exe'));
    expect(base).toEqual(before);
  });

  it('resolves the spec example: a terminal pastes with Ctrl+Shift+V and skips cleanup', () => {
    const spec = config({
      cleanup: { llm_enabled: true },
      profiles: [
        {
          name: 'Terminals',
          match_process: ['WindowsTerminal.exe', 'wezterm-gui.exe'],
          insert: { paste_shortcut: 'Ctrl+Shift+V' },
          cleanup: { llm_enabled: false },
        },
      ],
    });
    const { config: effective } = resolveProfile(spec, app('wezterm-gui.exe', 'zsh'));
    expect(effective.insert.paste_shortcut).toBe('Ctrl+Shift+V');
    expect(effective.cleanup.llm_enabled).toBe(false);
    expect(resolveProfile(spec, app('notepad.exe')).config.cleanup.llm_enabled).toBe(true);
  });
});
