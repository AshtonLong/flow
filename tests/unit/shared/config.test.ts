import { describe, expect, it } from 'vitest';
import { DEFAULT_CONFIG, validateConfig, resolvePrompt, promptNames } from '@shared/config';

describe('validateConfig', () => {
  it('accepts an empty config with all defaults', () => {
    const { config, issues } = validateConfig({});
    expect(issues).toEqual([]);
    expect(config).toEqual(DEFAULT_CONFIG);
    expect(config.hotkeys.hold_to_talk).toBe('Ctrl+Win');
    expect(config.model.active).toBe('parakeet-tdt-0.6b-v2');
    expect(config.history.enabled).toBe(false);
    expect(config.cleanup.llm_enabled).toBe(false);
  });

  it('drops a bad key to its default and reports it, keeping the rest', () => {
    const { config, issues } = validateConfig({
      audio: { max_recording_seconds: 'forever', sounds: false },
      overlay: { style: 'banner' },
    });
    expect(config.audio.max_recording_seconds).toBe(300);
    expect(config.audio.sounds).toBe(false);
    expect(config.overlay.style).toBe('pill');
    expect(issues.map((i) => i.path.join('.')).sort()).toEqual([
      'audio.max_recording_seconds',
      'overlay.style',
    ]);
  });

  it('drops an invalid list item without losing its siblings', () => {
    const { config, issues } = validateConfig({
      dictionary: [
        { heard: ['tory'], write: 'Tauri' },
        { heard: [], write: 'Nope' },
        { heard: ['pee npm'], write: 'pnpm' },
      ],
    });
    expect(config.dictionary.map((d) => d.write)).toEqual(['Tauri', 'pnpm']);
    expect(issues).toHaveLength(1);
  });

  it('never throws on nonsense input', () => {
    expect(validateConfig(null).config).toEqual(DEFAULT_CONFIG);
    expect(validateConfig('x').config).toEqual(DEFAULT_CONFIG);
    expect(validateConfig({ general: 5, hotkeys: [] }).config).toEqual(DEFAULT_CONFIG);
  });

  it('accepts the profile overrides from the spec example', () => {
    const { config, issues } = validateConfig({
      profiles: [
        {
          name: 'Terminals',
          match_process: ['WindowsTerminal.exe'],
          insert: { paste_shortcut: 'Ctrl+Shift+V' },
          cleanup: { llm_enabled: false },
        },
      ],
      providers: { groq: { api_key: 'secret:groq' } },
    });
    expect(issues).toEqual([]);
    expect(config.profiles[0]!.insert).toEqual({ paste_shortcut: 'Ctrl+Shift+V' });
  });

  it('rejects a raw API key in the file', () => {
    const { config, issues } = validateConfig({ providers: { groq: { api_key: 'gsk_live_abc' } } });
    expect(config.providers.groq?.api_key).toBeUndefined();
    expect(issues).toHaveLength(1);
  });
});

describe('prompts', () => {
  it('falls back to built-in templates and lets the file override them', () => {
    expect(resolvePrompt(DEFAULT_CONFIG, 'email')).toMatch(/email/i);
    const { config } = validateConfig({
      prompts: { email: { text: 'Mine.' }, pirate: { text: 'Arr.' } },
    });
    expect(resolvePrompt(config, 'email')).toBe('Mine.');
    expect(promptNames(config)).toEqual([
      'clean',
      'email',
      'casual',
      'code_comment',
      'verbatim',
      'pirate',
    ]);
    expect(resolvePrompt(config, 'missing')).toBe(resolvePrompt(DEFAULT_CONFIG, 'clean'));
  });
});
