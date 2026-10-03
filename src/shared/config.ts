/**
 * The config schema. `config.toml` is validated against this, and the settings
 * window edits the same shape. Every option has a default, so an empty file is
 * a valid config.
 */
import { z } from 'zod';

export const CONFIG_VERSION = 1;

export const DEFAULT_LOCAL_MODEL = 'parakeet-tdt-0.6b-v2';

/** A table whose keys all have defaults, so the table itself may be omitted. */
const section = <T extends z.ZodRawShape>(shape: T) =>
  z.object(shape).prefault({} as z.input<z.ZodObject<T>>);

const GeneralSchema = section({
  launch_at_login: z.boolean().default(false),
  theme: z.enum(['system', 'light', 'dark']).default('system'),
  language: z.literal('en').default('en'),
  debug_logging: z.boolean().default(false),
  onboarded: z.boolean().default(false),
});

const HotkeysSchema = section({
  hold_to_talk: z.string().default('Ctrl+Win'),
  toggle: z.string().default('Ctrl+Win+Space'),
  cancel: z.string().default('Esc'),
  paste_last: z.string().default('Alt+Shift+V'),
  double_tap_lock: z.boolean().default(true),
});

const AudioSchema = section({
  input_device: z.string().default('default'),
  keep_mic_warm: z.boolean().default(false),
  sounds: z.boolean().default(true),
  max_recording_seconds: z.number().int().min(5).max(3600).default(300),
});

const ModelSchema = section({
  active: z.string().min(1).default(DEFAULT_LOCAL_MODEL),
  fallback: z.string().min(1).default(DEFAULT_LOCAL_MODEL),
  device: z.enum(['auto', 'cpu', 'gpu']).default('auto'),
  keep_loaded_minutes: z.number().min(-1).max(1440).default(10),
});

const ProviderSchema = z.object({
  /** Required for custom endpoints; optional override for built-in providers. */
  base_url: z.string().url().optional(),
  /** A reference such as `secret:groq`. The key itself is never stored here. */
  api_key: z.string().startsWith('secret:').optional(),
  /** Model name sent to a custom OpenAI-compatible endpoint. */
  model: z.string().optional(),
});

const CleanupSchema = section({
  filler_removal: z.boolean().default(true),
  spoken_formatting: z.boolean().default(true),
  spoken_punctuation: z.boolean().default(true),
  llm_enabled: z.boolean().default(false),
  llm_model: z.string().default('groq/openai/gpt-oss-20b'),
  llm_prompt: z.string().default('clean'),
  llm_timeout_ms: z.number().int().min(500).max(30000).default(3000),
});

const PromptSchema = z.object({ text: z.string() });

export const DEFAULT_PROMPTS: Record<string, { text: string }> = {
  clean: {
    text: "Fix grammar and punctuation. Remove false starts. Keep the speaker's words and tone.",
  },
  email: {
    text: 'Rewrite as a clear, polite email body. Use complete sentences and paragraphs. Keep every fact and name.',
  },
  casual: {
    text: 'Tidy this into a relaxed chat message. Keep it short and conversational. Do not add greetings or sign-offs.',
  },
  code_comment: {
    text: 'Rewrite as a concise code comment. Keep identifiers and technical terms exactly as spoken. No markdown.',
  },
  verbatim: {
    text: 'Fix only obvious transcription errors and punctuation. Do not rephrase or remove anything.',
  },
};

const InsertSchema = section({
  method: z.enum(['paste', 'type']).default('paste'),
  paste_shortcut: z.string().default('Ctrl+V'),
  restore_clipboard: z.boolean().default(true),
  trailing_space: z.boolean().default(true),
  match_case: z.boolean().default(true),
});

export const OVERLAY_POSITIONS = [
  'bottom-center',
  'bottom-left',
  'bottom-right',
  'top-center',
  'top-left',
  'top-right',
] as const;

const OverlaySchema = section({
  style: z.enum(['pill', 'minimal', 'none']).default('pill'),
  position: z.enum(OVERLAY_POSITIONS).default('bottom-center'),
  size: z.enum(['small', 'medium', 'large']).default('medium'),
  show_waveform: z.boolean().default(true),
});

const HistorySchema = section({
  enabled: z.boolean().default(false),
  retention_days: z.number().int().min(1).max(3650).default(30),
});

const DictionaryEntrySchema = z.object({
  heard: z.array(z.string().min(1)).min(1),
  write: z.string().min(1),
});

const SnippetSchema = z.object({
  trigger: z.string().min(1),
  text: z.string(),
});

const LocalModelFileSchema = z.object({
  id: z.string().min(1),
  name: z.string().optional(),
  path: z.string().min(1),
});

/** Per-app override sections are loose here; the merged result is re-validated. */
const overrides = z.record(z.string(), z.unknown()).optional();

const ProfileSchema = z.object({
  name: z.string().min(1),
  match_process: z.array(z.string()).default([]),
  match_title: z.array(z.string()).default([]),
  /** `false` disables dictation entirely while the app is focused. */
  enabled: z.boolean().default(true),
  audio: overrides,
  model: overrides,
  cleanup: overrides,
  insert: overrides,
  overlay: overrides,
  history: overrides,
});

export const PROFILE_SECTIONS = [
  'audio',
  'model',
  'cleanup',
  'insert',
  'overlay',
  'history',
] as const;
export type ProfileSection = (typeof PROFILE_SECTIONS)[number];

export const ConfigSchema = z.object({
  version: z.number().int().default(CONFIG_VERSION),
  general: GeneralSchema,
  hotkeys: HotkeysSchema,
  audio: AudioSchema,
  model: ModelSchema,
  providers: z.record(z.string(), ProviderSchema).default({}),
  cleanup: CleanupSchema,
  prompts: z.record(z.string(), PromptSchema).default({}),
  insert: InsertSchema,
  overlay: OverlaySchema,
  history: HistorySchema,
  dictionary: z.array(DictionaryEntrySchema).default([]),
  snippets: z.array(SnippetSchema).default([]),
  local_models: z.array(LocalModelFileSchema).default([]),
  profiles: z.array(ProfileSchema).default([]),
});

export type Config = z.infer<typeof ConfigSchema>;
export type Profile = Config['profiles'][number];
export type DictionaryEntry = Config['dictionary'][number];
export type Snippet = Config['snippets'][number];
export type ProviderConfig = Config['providers'][string];
export type HotkeyAction = 'hold_to_talk' | 'toggle' | 'cancel' | 'paste_last';
export const HOTKEY_ACTIONS: HotkeyAction[] = ['hold_to_talk', 'toggle', 'cancel', 'paste_last'];

export type ConfigPath = (string | number)[];

export interface ConfigIssue {
  /** Path of the offending key, e.g. `["audio", "max_recording_seconds"]`. */
  path: ConfigPath;
  message: string;
  /** 1-based line in `config.toml`, when it can be located. */
  line?: number;
}

/** One edit to the config file. `value: undefined` removes the key. */
export interface ConfigPatch {
  path: ConfigPath;
  value: unknown;
}

export interface ConfigSnapshot {
  config: Config;
  issues: ConfigIssue[];
  path: string;
}

export const DEFAULT_CONFIG: Config = ConfigSchema.parse({});

function deleteAt(root: unknown, path: ConfigPath): boolean {
  if (path.length === 0) return false;
  let node = root as Record<string | number, unknown> | undefined;
  for (let i = 0; i < path.length - 1; i++) {
    const next = node?.[path[i]!];
    if (next === null || typeof next !== 'object') return false;
    node = next as Record<string | number, unknown>;
  }
  const last = path[path.length - 1]!;
  if (node === undefined || node === null) return false;
  if (Array.isArray(node) && typeof last === 'number') {
    if (last >= node.length) return false;
    node.splice(last, 1);
    return true;
  }
  if (!(last in node)) return false;
  delete node[last];
  return true;
}

/**
 * Validates leniently: a bad key is dropped (so it falls back to its default)
 * and reported, instead of rejecting the whole file. Never throws.
 */
export function validateConfig(raw: unknown): { config: Config; issues: ConfigIssue[] } {
  const issues: ConfigIssue[] = [];
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    return { config: structuredClone(DEFAULT_CONFIG), issues };
  }
  const work = structuredClone(raw) as Record<string, unknown>;
  for (let attempt = 0; attempt < 200; attempt++) {
    const result = ConfigSchema.safeParse(work);
    if (result.success) return { config: result.data, issues };
    let removed = false;
    // Deepest paths and highest array indexes first, so removals don't shift siblings.
    const sorted = [...result.error.issues].sort((a, b) => {
      if (b.path.length !== a.path.length) return b.path.length - a.path.length;
      const al = a.path[a.path.length - 1];
      const bl = b.path[b.path.length - 1];
      return typeof al === 'number' && typeof bl === 'number' ? bl - al : 0;
    });
    const seen = new Set<string>();
    for (const issue of sorted) {
      const path = issue.path.filter(
        (p): p is string | number => typeof p === 'string' || typeof p === 'number',
      );
      const key = JSON.stringify(path);
      if (seen.has(key)) continue;
      seen.add(key);
      issues.push({ path, message: issue.message });
      // An invalid field inside a list item drops the whole item.
      let target = path;
      const top = path[0];
      if (
        (top === 'dictionary' ||
          top === 'snippets' ||
          top === 'profiles' ||
          top === 'local_models') &&
        path.length > 2
      ) {
        target = path.slice(0, 2);
      }
      if (deleteAt(work, target)) removed = true;
    }
    if (!removed) break;
  }
  return { config: structuredClone(DEFAULT_CONFIG), issues };
}

/** The prompt text for a prompt name, falling back to the built-in templates. */
export function resolvePrompt(config: Config, name: string): string {
  return config.prompts[name]?.text ?? DEFAULT_PROMPTS[name]?.text ?? DEFAULT_PROMPTS.clean!.text;
}

/** Built-in and user prompt names, built-ins first. */
export function promptNames(config: Config): string[] {
  return [...new Set([...Object.keys(DEFAULT_PROMPTS), ...Object.keys(config.prompts)])];
}
