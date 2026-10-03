# Flow

Push-to-talk dictation for Windows. Hold a key, speak, release, and the text is typed at the cursor in whatever app has focus.

Flow is free and open source (MIT). Speech recognition runs on your own machine by default, and you can switch to a cloud model with your own API key. There is no account, no telemetry and no Flow server.

## Status

Version 0.1.0, pre-release.

- Windows 11, x64 only. macOS is planned after the Windows 1.0.
- English only.
- Installers are code-signed only when the release was built with a signing certificate. An unsigned installer triggers a Windows SmartScreen warning.
- Performance targets in [SPEC.md](SPEC.md) are design goals. The few figures measured so far are under [Models](#models).

## Install

1. Download `Flow-Setup-<version>.exe` from [Releases](https://github.com/AshtonLong/flow/releases).
2. Run it. The installer needs no administrator rights when you install for yourself only.
3. Flow starts in the system tray. On first run it asks for a microphone, a hotkey and a model.

The installer contains no speech model. The default model (475 MB) downloads on first run. Updates arrive through the app from GitHub Releases.

## Hotkeys

| Action          | Default               | Behaviour                                  |
| --------------- | --------------------- | ------------------------------------------ |
| Hold to talk    | Hold `Ctrl+Win`       | Records while held, transcribes on release |
| Hands-free lock | Double-tap `Ctrl+Win` | Keeps recording until the next tap         |
| Toggle          | `Ctrl+Win+Space`      | Press once to start, again to stop         |
| Cancel          | `Esc` while recording | Discards the recording, types nothing      |
| Paste last      | `Alt+Shift+V`         | Re-inserts the previous transcript         |

A binding can be a key, a combination, a lone modifier (for example `RCtrl`) or a mouse button (`Mouse3` to `Mouse5`).

## Models

The catalog lives in [resources/catalog.json](resources/catalog.json). Prices and benchmark figures are as of 2026-10-03.

### Local (free, offline)

Model files download to `%LOCALAPPDATA%\Flow\models`. Downloads resume if interrupted and are verified by SHA-256.

| Model                          | Download | Runs on | Average WER | Licence    |
| ------------------------------ | -------- | ------- | ----------- | ---------- |
| Parakeet TDT 0.6B v2 (default) | 475 MB   | CPU     | 6.05%       | CC-BY-4.0  |
| Whisper large-v3-turbo         | 1.6 GB   | GPU     | 7.75%       | MIT        |
| Whisper Small                  | 493 MB   | GPU     |             | MIT        |
| Moonshine v2 Medium            | 296 MB   | CPU     |             | MIT        |
| Moonshine v2 Tiny              | 50 MB    | CPU     |             | MIT        |
| IBM Granite Speech 4.1 2B      | 1.6 GB   | GPU     | 5.33%       | Apache-2.0 |
| Cohere Transcribe              | 1.6 GB   | GPU     | 5.42%       | Apache-2.0 |
| NVIDIA Canary-Qwen 2.5B        | 1.7 GB   | GPU     | 5.63%       | CC-BY-4.0  |

WER is the average word error rate on the Open ASR Leaderboard; lower is better.

`model.device = "auto"` uses a discrete GPU when one is present and the CPU otherwise. GPUs run through Vulkan, which ships in the installer. The speech engine publishes no prebuilt CUDA library for Windows yet; if you build one, put `transcribe.dll` and the CUDA runtime in `%LOCALAPPDATA%\Flow\gpu-pack` and Flow loads it instead.

Measured on the development machine (Ryzen 5 5600X, GTX 1070 Ti) with a 10 to 11 second clip and the model already loaded: Parakeet takes about 720 ms on the CPU and 220 ms on the GPU; the three 2B models take 600 to 840 ms on the GPU and 4.5 to 5.3 s on the CPU.

You can also register your own GGUF file:

```toml
[[local_models]]
id = "my-model"
path = "D:\\models\\my-model.gguf"
```

### Cloud (your own API key)

You pay the provider directly at its list price. Flow adds no markup and audio goes straight from your machine to the provider.

| Model                                      | Price per hour of audio |
| ------------------------------------------ | ----------------------- |
| Groq: Whisper large-v3-turbo (recommended) | $0.04                   |
| Groq: Whisper large-v3                     | $0.111                  |
| Mistral: Voxtral Mini Transcribe 2         | $0.18                   |
| OpenAI: gpt-4o-mini-transcribe             | $0.18                   |
| ElevenLabs: Scribe v2                      | $0.22                   |
| Deepgram: Nova-3                           | $0.26                   |
| OpenAI: gpt-transcribe                     | $0.27                   |
| OpenAI: gpt-4o-transcribe                  | $0.36                   |

Groq bills a minimum of 10 seconds per request. Any server that implements the OpenAI transcription API can be added as a custom endpoint; see [docs/configuration.md](docs/configuration.md#providersid).

If a cloud request fails or the machine is offline, Flow falls back to the installed local model and says so in the overlay.

### Text cleanup

Local rules (dictionary, snippets, spoken formatting, filler removal) always run and need no network. An optional language-model pass is off by default; it uses a cleanup model from the catalog (Groq gpt-oss-20b is the recommended one) or a local Ollama or LM Studio endpoint.

## Privacy

- No account, no telemetry, no analytics.
- Audio is held in memory and never written to disk. With a local model it never leaves the machine.
- With a cloud model, audio goes directly to the provider you chose. The overlay shows a cloud icon whenever that happens.
- API keys are encrypted with Windows DPAPI and never written to the config file or logs.
- Transcript history is off by default and stored locally when on.
- The only outbound traffic is model downloads, the catalog refresh, update checks and calls to providers you have configured.

## Configuration

Every setting lives in one file, `%APPDATA%\Flow\config.toml`. The settings window reads and writes that same file, and edits made in a text editor apply within a second. Every option has a default, so an empty file is valid.

```toml
version = 1

[hotkeys]
hold_to_talk = "Ctrl+Win"

[model]
active = "parakeet-tdt-0.6b-v2"
fallback = "parakeet-tdt-0.6b-v2"   # used when a cloud model fails
keep_loaded_minutes = 10            # 0 = unload at once, -1 = never unload

[providers.groq]
api_key = "secret:groq"             # a reference; the key itself is stored encrypted

[cleanup]
llm_enabled = false

[[dictionary]]
heard = ["tory", "towery"]
write = "Tauri"

[[profiles]]
name = "Terminals"
match_process = ["WindowsTerminal.exe"]
insert.paste_shortcut = "Ctrl+Shift+V"
```

The full reference is in [docs/configuration.md](docs/configuration.md).

| Data                     | Location                     |
| ------------------------ | ---------------------------- |
| Settings                 | `%APPDATA%\Flow\config.toml` |
| API keys (encrypted)     | `%APPDATA%\Flow\secrets.bin` |
| History (off by default) | `%APPDATA%\Flow\history.db`  |
| Logs                     | `%APPDATA%\Flow\logs`        |
| Models                   | `%LOCALAPPDATA%\Flow\models` |

## Development

Requires Windows 11 x64, Node.js 22 or newer (CI uses 24) and pnpm.

```sh
pnpm install
pnpm dev        # run with hot reload
pnpm test       # unit tests (Vitest)
pnpm test:e2e   # end-to-end tests against the built app (run pnpm build first)
pnpm typecheck
pnpm lint
pnpm build      # type-check and bundle into out/
pnpm dist       # build the installer into release/ (does not publish)
```

Product behaviour is specified in [SPEC.md](SPEC.md). The process model, IPC and repository layout are in [ARCHITECTURE.md](ARCHITECTURE.md).

### Releasing

Pushing a tag `vX.Y.Z` that matches the version in `package.json` runs [.github/workflows/release.yml](.github/workflows/release.yml). It builds the installer and uploads it to a draft GitHub Release. Publishing the draft makes the update available to installed copies.

To sign the build, set the repository secrets `CSC_LINK` (the `.pfx` certificate, base64-encoded) and `CSC_KEY_PASSWORD`. Without them the installer is unsigned.

### Signing the catalog

The app refreshes its model catalog from `resources/catalog.json` on the `main` branch, and accepts it only with a valid Ed25519 signature in `resources/catalog.json.sig`. The public key is embedded in [src/main/models/catalog.ts](src/main/models/catalog.ts). The private key is kept outside the repository and must never be committed.

After any change to `resources/catalog.json`:

```sh
node scripts/sign-catalog.mjs
```

This reads the private key from the path in `FLOW_CATALOG_KEY`, or `%USERPROFILE%\.flow\catalog-signing-key.pem`, and rewrites `catalog.json.sig`. Commit both files together. A unit test fails if they do not match.

- The signature covers the exact bytes of the file, with LF line endings. Format the file with Prettier before signing, not after.
- Raise `asOf` when you change the catalog. The app ignores a catalog older than the one it already has.
- `node scripts/sign-catalog.mjs --generate` creates a new key pair and prints the public key. Replacing the key means updating `CATALOG_PUBLIC_KEY` and shipping an app update; older installs keep using their bundled or cached catalog.

## Third-party licences

Flow is MIT-licensed; see [LICENSE](LICENSE). It builds on:

| Component                                                          | Licence | Notes                                                 |
| ------------------------------------------------------------------ | ------- | ----------------------------------------------------- |
| [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) | MIT     | Local speech engine, from the Handy project           |
| [Silero VAD](https://github.com/snakers4/silero-vad)               | MIT     | Voice activity detection; `resources/silero_vad.onnx` |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime)           | MIT     | Runs Silero VAD                                       |
| [Electron](https://www.electronjs.org/)                            | MIT     | App shell                                             |
| [uiohook-napi](https://github.com/SnosMe/uiohook-napi)             | MIT     | Global key and mouse hook                             |
| [koffi](https://koffi.dev/)                                        | MIT     | Win32 calls                                           |
| [better-sqlite3](https://github.com/WiseLibs/better-sqlite3)       | MIT     | History store                                         |
| [Recursive](https://www.recursive.design/)                         | OFL-1.1 | Typeface for the settings window and the overlay      |

Speech models are downloaded separately and keep their own licences, shown in the catalog:

- **Parakeet TDT 0.6B v2** by NVIDIA, licensed under [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/). Source: [nvidia/parakeet-tdt-0.6b-v2](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2). Flow uses the GGUF conversion published by [handy-computer](https://huggingface.co/handy-computer).
- **Canary-Qwen 2.5B** by NVIDIA, CC-BY-4.0. Source: [nvidia/canary-qwen-2.5b](https://huggingface.co/nvidia/canary-qwen-2.5b).
- **Whisper** by OpenAI, MIT. **Moonshine** by Useful Sensors, MIT.
- **Granite Speech 4.1 2B** by IBM and **Cohere Transcribe** by Cohere, Apache-2.0.
