# Flow in Rust

A port of Flow from Electron/TypeScript to Rust. Same app, same look, same
settings file, same behaviour; no Chromium and no Node.js, so it starts faster
and uses a fraction of the memory.

The Electron app in the repository root remains the reference: every module
here is a port of a TypeScript module, with its unit tests ported too.
`PORTING.md` maps one to the other and holds the conventions.

## How it is built

| Process | Electron | Rust |
| --- | --- | --- |
| Tray app | Electron main process (Node.js) | `Flow.exe`: a Win32 message loop, a 2-thread tokio runtime, the hook and capture threads |
| Overlay pill | Always-alive transparent `BrowserWindow` + mic capture | Layered Win32 window drawn with tiny-skia; capture through WASAPI (cpal) in the tray process |
| Speech engine | Electron utility process (transcribe-cpp over koffi, Silero on onnxruntime) | `Flow.exe --engine`: transcribe.cpp 0.3.0 over libloading, Silero VAD in pure Rust |
| Settings | React in a `BrowserWindow`, created on open | `Flow.exe --settings-window`: egui (glow), created on open, exits on close |

The settings window talks to the tray app over its stdin/stdout, one JSON
message per line (`flow_core::protocol::settings`, mirroring `SettingsApi` in
`src/shared/ipc.ts`). The engine uses binary frames
(`flow_core::protocol::engine`).

| Crate | What it holds |
| --- | --- |
| `flow-core` | Config schema, validation, in-place TOML patching and the store; hotkey grammar and matcher; catalog; cleanup rules; profiles; VAD segments; the IPC protocols |
| `flow-audio` | Microphone capture at 16 kHz mono, noise suppression and gain, the waveform analyser, level meter and clip recorder |
| `flow-engine` | The engine process and its client |
| `flow-win` | Global key hook, focused-app lookup, simulated input, clipboard insertion |
| `flow-cloud` | Groq, OpenAI, Mistral, ElevenLabs, Deepgram and custom endpoints; the LLM cleanup pass |
| `flow-store` | Model catalog and downloads, transcript history (SQLite), encrypted API keys (DPAPI) |
| `flow-shell` | Overlay pill, tray icon, start and stop sounds |
| `flow-ui` | The settings window |
| `flow` | `Flow.exe`: startup, the dictation session, the transcriber router, the settings-window host, updates |

## Drop-in replacement

The Rust build uses the Electron build's files and registrations, so switching
keeps everything:

- `%APPDATA%\Flow\config.toml`, `history.db`, `catalog.json`, `logs\flow.log`
- `%APPDATA%\Flow\secrets.bin`: keys saved by the Electron build (Electron
  `safeStorage`) are still read; new keys are encrypted with DPAPI directly
- `%LOCALAPPDATA%\Flow\models` and its `installed.json`; `%LOCALAPPDATA%\Flow\gpu-pack`
- The `computer.flow.dictation` launch-at-login entry, the install folder
  `%LOCALAPPDATA%\Programs\Flow`, the uninstall entry and the Start-menu shortcut
- Microphone names saved by Chromium (`Microphone (…) (1532:0531)`) still match

## Develop

Requires Windows 10/11 x64 and Rust (stable; MSVC or GNU toolchain).

```sh
cd flow-rs
pwsh scripts/stage-engine.ps1 -Dest target/debug   # once: puts transcribe.dll and ggml next to Flow.exe
cargo run -p flow --bin Flow                       # the tray app (debug build)
cargo test --workspace
cargo run -p flow-ui --example preview -- --page models   # the settings window on a mock backend
```

Run a test instance with `FLOW_USER_DATA` and `FLOW_MODELS_DIR` pointing at
temporary folders to keep it away from your real settings.

The end-to-end tests drive the real `Flow.exe` in test mode (`FLOW_TEST=1`:
commands on stdin, events on stdout, a WAV file as the microphone; see
`crates/app/src/test_mode.rs`). They need the staged engine and the Parakeet
model in `%LOCALAPPDATA%\Flow\models`:

```sh
cargo test -p flow --test e2e -- --ignored
```

`vendor/epaint` is epaint 0.37 with one change so text wraps where Chromium
wraps it (`vendor/epaint/FLOW.md`).

## Package

```sh
pwsh scripts/package.ps1          # dist/Flow-Setup-<version>.exe and dist/latest.yml (needs NSIS 3)
pwsh scripts/test-installer.ps1   # installs a sandbox build to a temp folder, dictates with it, uninstalls
```

The installer is per-user and needs no administrator rights. It installs where
the Electron build did and replaces it in place.
