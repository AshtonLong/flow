# Porting Flow to Rust

This directory is a port of the Electron/TypeScript app in the repository root
(`../src`) to Rust. The goal: **look the same, feel the same, function the
same**, with far less memory and faster startup. Read this whole file before
working on any crate.

## Principles

1. **Faithful port.** Behaviour, user-visible strings, timings, defaults, file
   formats and file locations match the TypeScript exactly. When in doubt, read
   the TS and do what it does. Port doc comments too (adapted to Rust), and keep
   the TS's comment density and tone.
2. **Drop-in replacement.** The Rust app reads and writes the same files as the
   Electron app: `%APPDATA%\Flow\config.toml`, `secrets.bin`, `history.db`,
   `logs\flow.log`, `catalog.json` (cached), `%LOCALAPPDATA%\Flow\models\`
   (+ `installed.json`), `%LOCALAPPDATA%\Flow\gpu-pack\`. A user switching from
   the Electron build keeps their settings, keys, history and models.
3. **Tests come along.** Every `tests/unit/**/*.test.ts` file is ported to Rust
   tests for the module it covers (same cases, same names in snake_case). The
   TS tests define expected behaviour, including exact strings.
4. **Idle cost near zero.** No polling, no timers while idle, no GUI toolkit in
   the main process. The settings window is a separate process that exists only
   while open. The engine process exits after `keep_loaded_minutes`.

## Processes

| Process | Command | Lifetime | Owns |
| --- | --- | --- | --- |
| Main | `Flow.exe [--hidden] [--settings]` | App lifetime | Tray, hotkey hook, session, capture, overlay pill, config, models, cloud calls, cleanup, insertion, history, secrets |
| Engine | `Flow.exe --engine` | First dictation → idle timeout | `transcribe.dll` (local models), Silero VAD |
| Settings | `Flow.exe --settings-window [--page models]` | While the window is open | egui settings window |

- Main ↔ engine: binary frames over the child's stdin/stdout
  (`flow_core::protocol::engine`).
- Main ↔ settings: JSON lines over the child's stdin/stdout
  (`flow_core::protocol::settings`), mirroring `SettingsApi` in `src/shared/ipc.ts`.
- The overlay pill is a native layered window in the main process (no web view),
  drawn with tiny-skia. Microphone capture (cpal/WASAPI) also lives in the main
  process.

## Crates (`crates/`)

| Crate | Package | Ports | Depends on |
| --- | --- | --- | --- |
| `core` | `flow-core` | `src/shared/*`, config, hotkeys + matcher, cleanup rules, profiles, catalog parse/verify, VAD segment logic, wav, split, transcribe errors | — |
| `audio` | `flow-audio` | `src/renderer/common/capture.ts` + worklet, the capture half of `src/renderer/overlay/main.ts` | core |
| `engine` | `flow-engine` | `src/engine/*` (engine process) and `src/main/transcribe/engine-client.ts` | core |
| `win` | `flow-win` | `src/main/platform/*`, `src/main/input/{hook,keymap}.ts`, `src/main/insert/*` | core |
| `cloud` | `flow-cloud` | `src/main/transcribe/cloud/*`, `src/main/cleanup/llm.ts` | core |
| `store` | `flow-store` | `src/main/models/*`, `src/main/history/*`, `src/main/secrets/*` | core |
| `shell` | `flow-shell` | the drawing half of `src/renderer/overlay/*`, `src/main/windows/{overlay,tray}.ts`, sounds | core, audio |
| `ui` | `flow-ui` | `src/renderer/settings/**` (React → egui) | core, audio |
| `app` | `flow` (bin `Flow`) | `src/main/index.ts`, `ipc.ts`, `session/`, `transcribe/router.ts`, `windows/{settings,theme,paths}.ts`, `updater.ts`, `log.ts` | everything |

Shared resources live in the repository root and are used in place:
`../resources/catalog.json` (+ `.sig`), `../resources/icons/*`. Rust-only
assets live in `flow-rs/resources/` (the Recursive font, lucide icon SVGs).
Test fixtures: `../tests/fixtures/jfk.wav` (11 s of speech, 16 kHz mono).

## Conventions

- Rust 2024, stable toolchain **x86_64-pc-windows-gnu** (no MSVC on this
  machine; mingw gcc is on PATH for C code such as bundled SQLite).
- Errors: `thiserror` enums in libraries, `anyhow` only in the app crate.
  No `unwrap()`/`expect()` on anything that can fail at runtime.
- Logging: the `log` crate with the TS scope as the target:
  `log::info!(target: "session", "...")`. The app installs a logger that writes
  the TS format (`<ISO time> INFO  [scope] message`) to `logs\flow.log`.
  Transcript text is logged only at `debug` level.
- Cancellation: `tokio_util::sync::CancellationToken` everywhere (it has the sync
  `is_cancelled()` and the async `cancelled()`), standing in for `AbortSignal`.
- Async: the main process runs a tokio multi-thread runtime. Blocking Win32 work
  (insertion, which may wait up to 30 s for modifiers) runs on its own threads.
- Events out of a service: a callback `Arc<dyn Fn(Event) + Send + Sync>` passed
  at construction (stands in for the TS `EventEmitter`). Callbacks must be cheap
  and must not block; the app forwards them to channels.
- Time: `std::time::Instant` for durations; injectable clocks
  (`Arc<dyn Fn() -> f64 + Send + Sync>` returning milliseconds) where the TS
  injects `now` for tests.
- Strings shown to users are copied verbatim from the TS, including typographic
  quotes, dashes and ellipses.

## Building and testing

```sh
# From flow-rs/. Use your own target dir when several builds run at once.
export CARGO_TARGET_DIR=target-<yourcrate> CARGO_BUILD_JOBS=4
cargo test -p flow-core
cargo build -p flow --bin Flow
```

- **Never kill `Flow.exe` by image name.** The user's installed Electron Flow
  (`%LOCALAPPDATA%\Programs\Flow\Flow.exe`) is running and shares the name.
  Stop only processes you started, by PID.
- Run test instances with `FLOW_USER_DATA=<temp dir>` and
  `FLOW_MODELS_DIR=<dir>` so the user's real config is never touched. The
  user's downloaded models in `%LOCALAPPDATA%\Flow\models` may be **read**
  (for engine tests) but never modified.
- Do not press the real hotkeys or inject input into other windows during tests.
- The prebuilt speech engine (transcribe.cpp 0.3.0, the version the Electron app
  shipped) is at
  `%LOCALAPPDATA%\Programs\Flow\resources\app.asar.unpacked\node_modules\@transcribe-cpp\win32-x64-cpu-vulkan\`.

## Shared types already written (`flow-core`)

Do not change these without coordinating; everything codes against them.

- `types.rs`: `SAMPLE_RATE`, `Transcript`, `FocusedApp`, `SessionState`,
  `OverlayPhase`, `OverlayState`, `OverlayAppearance`, `ThemeInfo`,
  `CaptureStart`, `CaptureErrorReason`, `InsertOutcome`, `InsertFailure`,
  `InsertResult`, `DictationEvent`, `HistoryEntry`, `NewHistoryEntry`,
  `BackendDevice`, `AppInfo`.
- `config::schema`: `Config` and its sections, enums, `DEFAULT_PROMPTS`,
  `resolve_prompt`, `prompt_names`, `PathSeg`, `ConfigPath`, `path![]`,
  `ConfigIssue`, `ConfigPatch` (`value: None` removes), `ConfigSnapshot`.
- `config::value`: `apply_patches`, `get_at`, `format_path`, `toml_to_json`.
- `catalog::types`: `Catalog`, entries, `ModelEntry` (tagged by `kind`),
  `DownloadProgress`, `ModelStatus`, `CatalogSnapshot`, `split_model_id`,
  `is_cloud_model_id`, `cloud_cost`, `CUSTOM_PREFIX`.
- `protocol::settings`: `Request`, `RequestEnvelope`, `Event`, `Downstream`,
  `Reply`, `BenchResult`, `KeyTestResult`, `limits`.
- `protocol::engine`: requests, results, `write_frame`/`read_frame`, `env`.
- `protocol::Pcm`: base64 f32 PCM for JSON.

## Module contracts

Signatures below are the integration points other crates rely on. Internals are
up to the porter, but keep these shapes (small additions are fine).

### `flow-core`

```rust
// config::validate — port of validateConfig + the zod schema (messages as zod v4 prints them)
pub struct Validated { pub config: Config, pub issues: Vec<ConfigIssue> }
pub fn validate_config(raw: &serde_json::Value) -> Validated;

// config::patch — port of patchToml (in-place TOML edits that keep comments and order)
pub fn patch_toml(source: &str, patches: &[ConfigPatch]) -> Result<String, TomlPatchError>;

// config::store — port of ConfigStore (thread-safe; notify-based watcher, 150 ms debounce)
impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Arc<Self>;   // + with_options(...) for tests
    pub fn load(&self);                                   // never fails; problems become issues
    pub fn get(&self) -> Arc<Config>;
    pub fn issues(&self) -> Vec<ConfigIssue>;
    pub fn snapshot(&self) -> ConfigSnapshot;
    pub fn path(&self) -> &Path;
    pub fn set(&self, patches: &[ConfigPatch]) -> Result<ConfigSnapshot, ConfigError>;
    pub fn watch(self: &Arc<Self>);
    pub fn on_changed(&self, listener: impl Fn(&ConfigSnapshot) + Send + Sync + 'static);
    pub fn export_to(&self, target: &Path) -> Result<(), ConfigError>;
    pub fn import_from(&self, source: &Path) -> Result<ConfigSnapshot, ConfigError>;
    pub fn close(&self);
}
pub const DEFAULT_CONFIG_TEXT: &str; pub fn strip_api_keys(text: &str) -> String;

// profiles
pub struct ResolvedProfile { pub config: Config, pub profile: Option<String>, pub disabled: bool }
pub fn match_profile<'a>(profiles: &'a [Profile], app: Option<&FocusedApp>) -> Option<&'a Profile>;
pub fn resolve_profile(config: &Config, app: Option<&FocusedApp>) -> ResolvedProfile;

// hotkeys — port of src/shared/hotkeys.ts (KeyId = String)
pub struct Binding { pub tokens: Vec<String> }
pub fn parse_binding(text: &str) -> Option<Binding>;   // + format/normalize/display_tokens/find_conflicts/...

// matcher — port of HotkeyMatcher; each call returns the events it produced
pub enum HotkeyEvent { HoldStart, HoldEnd { aborted: bool }, Lock, Toggle, Cancel, PasteLast }
impl HotkeyMatcher {
    pub fn new(opts: MatcherOptions) -> Self;
    pub fn set_bindings(&mut self, b: &HotkeysConfig) -> Vec<HotkeyEvent>;
    pub fn set_cancel_armed(&mut self, armed: bool);
    pub fn key_down(&mut self, key: &str) -> Vec<HotkeyEvent>;
    pub fn key_up(&mut self, key: &str) -> Vec<HotkeyEvent>;
    pub fn verify_held(&mut self) -> Vec<HotkeyEvent>;
    pub fn reset(&mut self) -> Vec<HotkeyEvent>;
    pub fn any_held(&self) -> bool; pub fn held_count(&self) -> usize;
}

// cleanup — port of src/main/cleanup/{index,dictionary,fillers,snippets,spacing,spoken,text}.ts
pub fn apply_rules(text: &str, config: &Config, ctx: &RulesContext) -> String;
pub fn dictionary_hints(config: &Config) -> Vec<String>;

// errors — port of src/main/transcribe/errors.ts
pub struct TranscribeError { pub code: TranscribeErrorCode, pub message: String, /* ... */ }
impl TranscribeError { pub fn user_message(&self) -> String; }

// vad — segments_from_probs, extract_segments, energy_probs, VAD_FRAME, VAD_CONTEXT
// wav — encode_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8>
// split — split_at_pauses(audio: &[f32], max_seconds: f64, sample_rate: u32) -> Vec<Range<usize>> (or slices)
// catalog::verify — parse_catalog(bytes) -> Result<Catalog, CatalogError>; verify_catalog(bytes, sig_b64, pem) -> bool
```

### `flow-audio`

```rust
pub struct Microphone { pub label: String, pub id: String }
pub fn list_microphones() -> Vec<Microphone>;            // labels as stored in audio.input_device
pub fn same_device(a: &str, b: &str) -> bool;            // Electron labels carry " (1532:0531)"; compare without it

pub enum CaptureEvent { Started, Chunk(Vec<f32>), Stopped, Error { reason: CaptureErrorReason, message: String } }
/// Port of the capture half of overlay/main.ts: start/stop with keep-warm, generations, device loss.
pub struct DictationCapture;
impl DictationCapture {
    pub fn new(sink: Arc<dyn Fn(CaptureEvent) + Send + Sync>) -> Self;
    pub fn start(&self, opts: &CaptureStart);
    pub fn stop(&self);                                   // flushes the last partial chunk, then Stopped
    pub fn analyser(&self) -> Arc<Analyser>;              // waveform source for the overlay
    pub fn device_changed(&self);                         // drop a warm stream when not recording
}
/// AnalyserNode stand-in: fftSize 512, smoothingTimeConstant 0.6, byte frequency data.
impl Analyser { pub fn byte_frequency_data(&self, out: &mut [u8]); pub fn level(&self) -> f32; }
/// Settings window: level meter and the test-bench recorder.
pub struct LevelMonitor; pub struct ClipRecorder;
```

Chunks are 1600 samples (100 ms) of 16 kHz mono f32, like the AudioWorklet.
Capture applies noise suppression and automatic gain like Chromium's
`getUserMedia({ noiseSuppression: true, autoGainControl: true })`.

### `flow-engine`

```rust
pub fn run_engine_process() -> i32;   // `Flow.exe --engine`: serve frames on stdin/stdout until EOF

pub struct EngineOptions { pub exe: PathBuf, pub args: Vec<String>, pub library_path: Option<PathBuf>, pub cuda_runtime_dir: Option<PathBuf>, pub debug: bool }
pub struct EngineError { pub message: String, pub aborted: bool }
pub struct TrimOutput { pub audio: Option<Vec<f32>>, pub speech_ms: f64, pub detector: VadKind }
impl EngineClient {
    pub fn new(opts: EngineOptions) -> Arc<Self>;
    pub fn loaded_model_id(&self) -> Option<String>;
    pub fn running(&self) -> bool;
    pub fn pid(&self) -> Option<u32>;
    pub fn devices(&self) -> Vec<BackendDevice>;
    pub fn vad(&self) -> Option<VadKind>;
    pub fn set_keep_loaded_minutes(&self, minutes: f64);
    pub fn on_loaded(&self, f: impl Fn(&str) + Send + Sync + 'static);
    pub fn shutdown(&self);
    pub async fn probe(&self) -> Result<EngineProbeResult, EngineError>;
    pub async fn load(&self, model: &EngineModelRef, cancel: Option<CancellationToken>) -> Result<EngineLoadResult, EngineError>;
    pub async fn transcribe(&self, model: &EngineModelRef, audio: &[f32], language: &str, hints: &[String], cancel: Option<CancellationToken>) -> Result<EngineTranscribeResult, EngineError>;
    pub async fn trim(&self, audio: &[f32]) -> Result<TrimOutput, EngineError>;
    pub async fn unload(&self) -> Result<(), EngineError>;
}
```

The engine loads `transcribe.dll` at runtime (libloading) with a hand-written
FFI layer for the transcribe.cpp **0.3.0** C ABI (see the npm package
`transcribe-cpp@0.3.0`, `dist/ffi.js` and `dist/_generated.js`, for every
signature and struct layout). VAD is a small pure-Rust Silero (`silero.rs`)
running the weights of `resources/silero_vad.onnx`, embedded at build time; it
matches onnxruntime to ~1e-6. The TS energy detector is the fallback.
Parakeet ignores cancellation in transcribe.cpp 0.3.0 (as it did in Electron).

### `flow-win`

```rust
pub trait Platform: Send + Sync {                        // port of platform/types.ts
    fn available(&self) -> bool;
    fn self_elevated(&self) -> bool;
    fn get_focused_app(&self) -> Option<FocusedApp>;
    fn is_window_alive(&self, window_id: &str) -> bool;
    fn focus_window(&self, window_id: &str) -> bool;
    fn is_text_target(&self, app: Option<&FocusedApp>) -> bool;
    fn type_text(&self, text: &str, cancel: Option<&CancellationToken>) -> Result<(), PlatformError>;
    fn send_shortcut(&self, binding: &str) -> bool;
    fn wait_for_modifiers_up(&self, timeout: Duration, cancel: Option<&CancellationToken>);
    fn is_key_down(&self, key: &str) -> Option<bool>;
    fn mask_start_menu(&self);
    fn write_clipboard_excluded(&self, text: &str) -> bool;
}
pub fn create_platform() -> Arc<dyn Platform>;           // Win32Platform, or NullPlatform on failure

pub struct InsertOptions { pub method: InsertMethod, pub paste_shortcut: String, pub restore_clipboard: bool, pub target: Option<FocusedApp>, pub cancel: Option<CancellationToken> }
impl Inserter {
    pub fn new(platform: Arc<dyn Platform>, clipboard: Arc<dyn Clipboard>) -> Self;
    pub fn insert(&self, text: &str, opts: &InsertOptions) -> InsertResult;   // blocking, serialised, never panics
    pub fn copy(&self, text: &str);
}
pub fn system_clipboard() -> Arc<dyn Clipboard>;

impl InputHook {                                          // port of input/hook.ts over WH_KEYBOARD_LL + WH_MOUSE_LL
    pub fn new(deps: InputHookDeps, on_hotkey: Arc<dyn Fn(HotkeyEvent) + Send + Sync>) -> Arc<Self>;
    pub fn start(&self) -> bool;
    pub fn stop(&self);
    pub fn set_bindings(&self, b: &HotkeysConfig);
    pub fn set_cancel_armed(&self, armed: bool);
    pub fn set_suspended(&self, suspended: bool);
    pub fn reset(&self);
    pub fn any_held(&self) -> bool;
    pub fn capture_binding(&self, on_progress: Arc<dyn Fn(String) + Send + Sync>) -> crossbeam_channel::Receiver<Option<String>>;
    pub fn cancel_capture(&self);
}
```

Our own injected input is tagged with a private `dwExtraInfo` value and skipped
by the hook (this replaces the TS `InjectedKeys` ledger, which existed only
because uiohook hides the injected flag). Input from other software is still seen.

### `flow-cloud`

```rust
pub struct CloudDeps { pub catalog: Arc<Catalog>, pub config: Arc<Config>, pub get_secret: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>, pub http: reqwest::Client }
pub struct TranscribeOptions { pub language: String, pub hints: Vec<String>, pub cancel: CancellationToken }
pub fn create_cloud_transcriber(model_id: &str, deps: &CloudDeps) -> Result<CloudTranscriber, TranscribeError>;
impl CloudTranscriber { pub fn id(&self) -> &str; pub async fn transcribe(&self, audio: &[f32], opts: &TranscribeOptions) -> Result<Transcript, TranscribeError>; }
pub async fn test_provider_key(provider_id: &str, deps: &CloudDeps) -> KeyTestResult;
pub async fn cleanup_with_llm(text: &str, deps: &CloudDeps, cancel: Option<&CancellationToken>) -> Result<String, CleanupError>;
```

### `flow-store`

```rust
pub enum ModelEvent { Progress(DownloadProgress), Changed }
pub struct ModelManagerDeps { models_dir, user_data_dir, bundled_catalog_path, get_config, has_secret, loaded_model_id, http, catalog_url, catalog_public_key, free_bytes }
impl ModelManager {
    pub fn new(deps: ModelManagerDeps, on_event: Arc<dyn Fn(ModelEvent) + Send + Sync>) -> Arc<Self>;
    pub async fn init(&self);
    pub fn catalog(&self) -> Arc<Catalog>;
    pub async fn refresh_catalog(&self) -> bool;
    pub fn snapshot(&self) -> CatalogSnapshot;
    pub fn local_entry(&self, id: &str) -> Option<LocalModelEntry>;   // includes [[local_models]]
    pub fn cloud_entry(&self, id: &str) -> Option<CloudModelEntry>;   // includes custom endpoints
    pub fn is_installed(&self, id: &str) -> bool;
    pub fn model_path(&self, id: &str) -> Option<PathBuf>;
    pub async fn download(self: &Arc<Self>, id: &str) -> Result<(), ModelError>;
    pub fn cancel_download(&self, id: &str);
    pub async fn remove(&self, id: &str) -> Result<(), ModelError>;
    pub fn installed_ids(&self) -> Vec<String>;
}
impl HistoryStore { pub fn open(path: &Path) -> Result<Self, HistoryError>; add, search, get, update, delete, clear, prune, close }
impl SecretStore { pub fn new(path: PathBuf, cipher: Arc<dyn SecretCipher>) -> Self; load, has, get, set, delete, ids }
pub fn system_cipher(user_data_dir: &Path) -> Arc<dyn SecretCipher>;   // DPAPI; also reads Electron safeStorage ("v10") entries
```

### `flow-shell`

```rust
impl Overlay {                                            // port of windows/overlay.ts + renderer/overlay (drawing)
    pub fn create() -> Self;                              // own thread + message loop; layered, click-through, topmost, no-activate
    pub fn configure(&self, appearance: OverlayAppearance, position: OverlayPosition);
    pub fn set_state(&self, state: OverlayState);
    pub fn set_analyser(&self, analyser: Option<Arc<flow_audio::Analyser>>);
    pub fn destroy(&self);
}
pub enum TrayAction { ActivateModel(String), SetPaused(bool), CopyLast, RetryLast, OpenSettings, Quit }
pub struct TrayState { pub active_model_id: String, pub models: Vec<TrayModel>, pub paused: bool, pub has_last: bool, pub can_retry: bool, pub hook_failed: bool }
impl Tray { pub fn create(on_action: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Self; pub fn update(&self, state: TrayState); pub fn destroy(&self); }
```

### `flow-ui`

```rust
/// The settings window's link to the main process (or to the mock).
pub trait Backend: Send + Sync {
    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>);
    fn subscribe(&self, on_event: Box<dyn Fn(Event) + Send + Sync>);
}
pub fn pipe_backend() -> Arc<dyn Backend>;                // stdin/stdout JSON lines
pub fn mock_backend() -> Arc<dyn Backend>;                // port of renderer/settings/mock.ts
pub fn run(backend: Arc<dyn Backend>, options: RunOptions) -> i32;   // RunOptions { page, screenshot: Option<PathBuf>, theme override... }
```

The look is defined by `src/renderer/common/brand.css`,
`src/renderer/settings/styles.css` and the Tailwind classes in the TSX. The
screenshots in `../screenshots/` are the reference (1000×700 window, 100 %
scale). The font is Recursive (variable; `flow-rs/resources/fonts`), set up as
named egui families per weight/axis combination via `FontTweak::coords`.
