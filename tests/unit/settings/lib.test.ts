import { describe, expect, it } from 'vitest';
import { DEFAULT_CONFIG, validateConfig, type Profile } from '@shared/config';
import {
  formatBytes,
  formatCost,
  formatDuration,
  formatMemory,
  formatPrice,
  formatSpeed,
  formatWhen,
  plural,
  promptKey,
  promptLabel,
  sentence,
} from '../../../src/renderer/settings/lib/format';
import { customEndpoints } from '../../../src/renderer/settings/lib/models';
import { applyPatches, formatPath } from '../../../src/renderer/settings/lib/patch';
import {
  matchSummary,
  otherOverrides,
  overrideSummary,
  serializeProfile,
  setOverride,
  STARTERS,
  uniqueName,
} from '../../../src/renderer/settings/lib/profiles';

describe('applyPatches', () => {
  it('sets a nested value without touching the original', () => {
    const next = applyPatches(DEFAULT_CONFIG, [{ path: ['audio', 'sounds'], value: false }]);
    expect(next.audio.sounds).toBe(false);
    expect(DEFAULT_CONFIG.audio.sounds).toBe(true);
  });

  it('creates missing tables on the way down', () => {
    const next = applyPatches({} as Record<string, unknown>, [
      { path: ['prompts', 'standup', 'text'], value: 'Short notes.' },
    ]);
    expect(next).toEqual({ prompts: { standup: { text: 'Short notes.' } } });
  });

  it('removes a key when the value is undefined', () => {
    const start = { prompts: { clean: { text: 'x' }, mine: { text: 'y' } } };
    const next = applyPatches(start, [{ path: ['prompts', 'clean'], value: undefined }]);
    expect(next).toEqual({ prompts: { mine: { text: 'y' } } });
  });

  it('splices an array item and ignores removals under a missing parent', () => {
    const start = { list: ['a', 'b', 'c'] };
    expect(applyPatches(start, [{ path: ['list', 1], value: undefined }])).toEqual({
      list: ['a', 'c'],
    });
    expect(applyPatches(start, [{ path: ['nope', 'x'], value: undefined }])).toEqual(start);
  });

  it('replaces whole arrays', () => {
    const next = applyPatches(DEFAULT_CONFIG, [
      { path: ['dictionary'], value: [{ heard: ['tory'], write: 'Tauri' }] },
    ]);
    expect(validateConfig(next).config.dictionary).toEqual([{ heard: ['tory'], write: 'Tauri' }]);
  });
});

describe('formatPath', () => {
  it('joins keys with dots and indexes with brackets', () => {
    expect(formatPath(['audio', 'max_recording_seconds'])).toBe('audio.max_recording_seconds');
    expect(formatPath(['profiles', 1, 'name'])).toBe('profiles[1].name');
  });
});

describe('format', () => {
  it('formats sizes and speeds', () => {
    expect(formatBytes(475_491_840)).toBe('475 MB');
    expect(formatBytes(1_625_935_520)).toBe('1.6 GB');
    expect(formatBytes(0)).toBe('0 MB');
    expect(formatSpeed(12_300_000)).toBe('12.3 MB/s');
    expect(formatSpeed(400_000)).toBe('400 KB/s');
    expect(formatSpeed(0)).toBe('');
    expect(formatMemory(12 * 1024 ** 3)).toBe('12 GB');
  });

  it('formats list prices as the spec writes them', () => {
    expect(formatPrice(0.04)).toBe('$0.04');
    expect(formatPrice(0.111)).toBe('$0.111');
    expect(formatPrice(0.3)).toBe('$0.30');
    expect(formatPrice(0.075)).toBe('$0.075');
    expect(formatPrice(1)).toBe('$1');
    expect(formatPrice(0)).toBe('Free');
  });

  it('formats request costs down to fractions of a cent', () => {
    expect(formatCost(0.04 / 360)).toBe('0.011¢');
    expect(formatCost(0.000002)).toBe('0.0002¢');
    expect(formatCost(0.012)).toBe('1.2¢');
    expect(formatCost(0.25)).toBe('$0.25');
    expect(formatCost(0)).toBe('0¢');
  });

  it('formats durations', () => {
    expect(formatDuration(800)).toBe('0.8 s');
    expect(formatDuration(65_000)).toBe('1 min 5 s');
    expect(formatDuration(300_000)).toBe('5 min');
  });

  it('labels recent times relative to today', () => {
    const now = new Date(2026, 9, 3, 15, 0).getTime();
    expect(formatWhen(new Date(2026, 9, 3, 9, 5).getTime(), now)).toMatch(/^Today /);
    expect(formatWhen(new Date(2026, 9, 2, 23, 50).getTime(), now)).toMatch(/^Yesterday /);
    expect(formatWhen(new Date(2026, 8, 28, 14, 2).getTime(), now)).not.toMatch(/Today|Yesterday/);
    expect(formatWhen(new Date(2025, 8, 28, 14, 2).getTime(), now)).toContain('2025');
  });

  it('handles prompt names and small text helpers', () => {
    expect(promptLabel('code_comment')).toBe('Code comment');
    expect(promptLabel('standup')).toBe('standup');
    expect(promptKey(' Stand-up notes! ')).toBe('stand_up_notes');
    expect(promptKey('***')).toBe('');
    expect(plural(1, 'problem')).toBe('1 problem');
    expect(plural(3, 'entry', 'entries')).toBe('3 entries');
    expect(sentence('Lowest price')).toBe('Lowest price.');
    expect(sentence('Done already.')).toBe('Done already.');
  });
});

describe('profiles', () => {
  const slack: Profile = {
    name: 'Slack',
    match_process: ['slack.exe'],
    match_title: [],
    enabled: true,
    cleanup: { llm_enabled: true, llm_prompt: 'casual' },
    overlay: { style: 'minimal' },
  };

  it('omits defaults and empty sections when serialising', () => {
    expect(
      serializeProfile({ name: 'Empty', match_process: [], match_title: [], enabled: true }),
    ).toEqual({ name: 'Empty' });
    expect(serializeProfile({ ...slack, enabled: false })).toEqual({
      name: 'Slack',
      match_process: ['slack.exe'],
      enabled: false,
      cleanup: { llm_enabled: true, llm_prompt: 'casual' },
      overlay: { style: 'minimal' },
    });
  });

  it('sets and clears overrides, dropping emptied sections', () => {
    const withModel = setOverride(slack, 'model', 'active', 'groq/whisper-large-v3-turbo');
    expect(withModel.model).toEqual({ active: 'groq/whisper-large-v3-turbo' });
    expect(setOverride(withModel, 'model', 'active', undefined).model).toBeUndefined();
    const cleared = setOverride(slack, 'cleanup', 'llm_prompt', undefined);
    expect(cleared.cleanup).toEqual({ llm_enabled: true });
    expect(slack.cleanup).toEqual({ llm_enabled: true, llm_prompt: 'casual' });
  });

  it('lists overrides the editor has no picker for', () => {
    expect(otherOverrides(slack)).toEqual(['overlay.style']);
  });

  it('summarises matches and overrides', () => {
    expect(matchSummary(slack)).toBe('slack.exe');
    expect(matchSummary({ ...slack, match_process: [], match_title: ['Gmail'] })).toBe(
      'title contains “Gmail”',
    );
    expect(overrideSummary(slack, null)).toBe('Casual prompt, 1 more set in the file');
    expect(overrideSummary({ ...slack, enabled: false }, null)).toBe(
      'Dictation is off in this app',
    );
  });

  it('ships starters that survive config validation', () => {
    const { config, issues } = validateConfig({
      profiles: STARTERS.map((s) => serializeProfile(s.profile)),
    });
    expect(issues).toEqual([]);
    expect(config.profiles.map((p) => p.name)).toEqual([
      'Terminals',
      'Code editors',
      'Password managers',
    ]);
    expect(config.profiles[2]?.enabled).toBe(false);
    expect(config.profiles[0]?.insert).toEqual({ paste_shortcut: 'Ctrl+Shift+V' });
  });

  it('picks a free name', () => {
    expect(uniqueName([slack], 'New profile')).toBe('New profile');
    expect(uniqueName([slack], 'slack')).toBe('slack 2');
  });
});

describe('customEndpoints', () => {
  it('lists provider tables with a base URL that are not catalog providers', () => {
    const { config } = validateConfig({
      providers: {
        groq: { api_key: 'secret:groq' },
        'custom-lan': {
          base_url: 'http://192.168.1.20:8000/v1',
          model: 'whisper-1',
          api_key: 'secret:custom-lan',
        },
      },
    });
    expect(customEndpoints(config, null)).toEqual([
      {
        id: 'custom-lan',
        baseUrl: 'http://192.168.1.20:8000/v1',
        model: 'whisper-1',
        hasKeyRef: true,
      },
    ]);
  });
});
