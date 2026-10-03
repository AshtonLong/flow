# Configuration reference

Every Flow setting lives in one TOML file:

```text
%APPDATA%\Flow\config.toml
```

The settings window reads and writes this same file, so anything the window can do, a text editor can do. Edits apply when you save, without a restart. This page lists every key; the schema it is generated from is [src/shared/config.ts](../src/shared/config.ts).

## Rules

- **Every key is optional.** An empty file is a valid config; anything left out keeps its default.
- **A bad value never breaks the app.** An invalid key falls back to its default and the settings window shows which line is wrong. In a list (`[[dictionary]]`, `[[snippets]]`, `[[local_models]]`, `[[profiles]]`), an item with an invalid field is dropped as a whole.
- **API keys are never stored here.** A provider holds a reference such as `api_key = "secret:groq"`. The key itself is encrypted with Windows DPAPI in `%APPDATA%\Flow\secrets.bin`. Enter keys in the settings window.
- **The file is versioned.** Flow migrates an older file on launch and keeps a backup next to it.

## Top level

| Key       | Type    | Default | Meaning                                                                     |
| --------- | ------- | ------- | --------------------------------------------------------------------------- |
| `version` | integer | `1`     | Config format version. Leave it alone; Flow uses it to migrate older files. |

## `[general]`

| Key               | Type                                | Default    | Meaning                                                                                               |
| ----------------- | ----------------------------------- | ---------- | ----------------------------------------------------------------------------------------------------- |
| `launch_at_login` | boolean                             | `false`    | Start Flow in the tray when you sign in to Windows.                                                   |
| `theme`           | `"system"` \| `"light"` \| `"dark"` | `"system"` | Settings window theme. `system` follows Windows.                                                      |
| `language`        | `"en"`                              | `"en"`     | Dictation language. Only English is supported in v1.                                                  |
| `debug_logging`   | boolean                             | `false`    | Write verbose logs, which may include transcript text. Off, the logs contain no transcripts or audio. |
| `onboarded`       | boolean                             | `false`    | Set to `true` once the first-run setup is finished. While `false`, the setup opens at launch.         |

## `[hotkeys]`

| Key               | Type    | Default            | Meaning                                                                          |
| ----------------- | ------- | ------------------ | -------------------------------------------------------------------------------- |
| `hold_to_talk`    | binding | `"Ctrl+Win"`       | Records while held and transcribes on release.                                   |
| `toggle`          | binding | `"Ctrl+Win+Space"` | Press once to start recording, again to stop.                                    |
| `cancel`          | binding | `"Esc"`            | Discards the current recording or transcription. Only active during a dictation. |
| `paste_last`      | binding | `"Alt+Shift+V"`    | Inserts the previous transcript again.                                           |
| `double_tap_lock` | boolean | `true`             | Double-tapping the hold-to-talk binding keeps recording until the next tap.      |

A binding is one or more tokens joined with `+`. Case does not matter.

| Token                    | Values                                                                                                                                                                                                                                                                                                                        |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Modifiers, either side   | `Ctrl`, `Alt`, `Shift`, `Win`                                                                                                                                                                                                                                                                                                 |
| Modifiers, one side      | `LCtrl`, `RCtrl`, `LAlt`, `RAlt`, `LShift`, `RShift`, `LWin`, `RWin`                                                                                                                                                                                                                                                          |
| Letters and digits       | `A` to `Z`, `0` to `9`                                                                                                                                                                                                                                                                                                        |
| Function and numpad keys | `F1` to `F24`, `Numpad0` to `Numpad9`                                                                                                                                                                                                                                                                                         |
| Named keys               | `Space`, `Esc`, `Enter`, `Tab`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `CapsLock`, `ScrollLock`, `NumLock`, `PrintScreen`, `Pause`, `Minus`, `Equal`, `BracketLeft`, `BracketRight`, `Backslash`, `Semicolon`, `Quote`, `Comma`, `Period`, `Slash`, `Backquote` |
| Mouse buttons            | `Mouse3` (middle), `Mouse4` (back), `Mouse5` (forward)                                                                                                                                                                                                                                                                        |

A binding can be a lone modifier (`RCtrl`), a combination (`Ctrl+Win`) or a mouse button (`Mouse4`). A longer binding that contains a shorter one, such as `Ctrl+Win+Space` over `Ctrl+Win`, is not a conflict. Two identical bindings are, and the settings window flags them.

## `[audio]`

| Key                     | Type               | Default     | Meaning                                                                                                                                         |
| ----------------------- | ------------------ | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `input_device`          | string             | `"default"` | Microphone to record from: `default` for the Windows default device, or a device id. Choose it in the settings window rather than typing an id. |
| `keep_mic_warm`         | boolean            | `false`     | Keep the microphone open between dictations. Recording starts faster, but the Windows microphone indicator stays on.                            |
| `sounds`                | boolean            | `true`      | Play a sound when recording starts and stops.                                                                                                   |
| `max_recording_seconds` | integer, 5 to 3600 | `300`       | Longest single recording. Recording stops at this limit and the audio is transcribed.                                                           |

## `[model]`

| Key                   | Type                           | Default                  | Meaning                                                                                                                        |
| --------------------- | ------------------------------ | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| `active`              | model id                       | `"parakeet-tdt-0.6b-v2"` | The model that transcribes your speech. See [Model ids](#model-ids).                                                           |
| `fallback`            | model id                       | `"parakeet-tdt-0.6b-v2"` | Local model used when a cloud model fails or the machine is offline.                                                           |
| `device`              | `"auto"` \| `"cpu"` \| `"gpu"` | `"auto"`                 | Where local models run. `auto` uses the GPU when one is available, otherwise the CPU.                                          |
| `keep_loaded_minutes` | number, -1 to 1440             | `10`                     | How long a local model stays in memory after the last dictation. `0` unloads at once (lightest); `-1` never unloads (fastest). |

### Model ids

| Kind                    | Form                                                    | Example                       |
| ----------------------- | ------------------------------------------------------- | ----------------------------- |
| Local, from the catalog | a name with no slash                                    | `parakeet-tdt-0.6b-v2`        |
| Local, your own file    | the `id` of a [`[[local_models]]`](#local_models) entry | `my-model`                    |
| Cloud, from the catalog | `<provider>/<model>`                                    | `groq/whisper-large-v3-turbo` |
| Custom endpoint         | `custom/<provider id>`                                  | `custom/lan`                  |

The catalog ids are listed in [resources/catalog.json](../resources/catalog.json) and shown on the Models page of the settings window.

## `[providers.<id>]`

One table per provider. The built-in provider ids are `groq`, `openai`, `mistral`, `elevenlabs`, `deepgram`, `anthropic`, `ollama` and `lmstudio`. Any other id defines a custom endpoint.

| Key        | Type              | Default       | Meaning                                                                                                                            |
| ---------- | ----------------- | ------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `api_key`  | `"secret:<name>"` | none          | A reference to a stored key, never the key itself. Without this line, a built-in provider looks for a key stored under its own id. |
| `base_url` | URL               | none          | Required for a custom endpoint. For a built-in provider it overrides the default address.                                          |
| `model`    | string            | `"whisper-1"` | Model name sent to a custom endpoint. Ignored for built-in providers.                                                              |

A custom endpoint is any server that implements the OpenAI transcription API, such as a self-hosted Whisper server. Give it a table with a `base_url`, then select it as `custom/<id>`:

```toml
[providers.lan]
base_url = "http://192.168.1.20:8000/v1"
model = "large-v3"

[model]
active = "custom/lan"
```

A custom endpoint with no `api_key` line is treated as needing no key.

## `[cleanup]`

Local rules run on every dictation and need no network. The language-model pass is optional.

| Key                  | Type                  | Default                     | Meaning                                                                                                                  |
| -------------------- | --------------------- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `filler_removal`     | boolean               | `true`                      | Remove "um", "uh" and repeated words.                                                                                    |
| `spoken_formatting`  | boolean               | `true`                      | Turn spoken "new line" and "new paragraph" into line breaks.                                                             |
| `spoken_punctuation` | boolean               | `true`                      | Turn spoken punctuation, such as "comma" or "question mark", into the symbol.                                            |
| `llm_enabled`        | boolean               | `false`                     | Rewrite each transcript with a language model after the local rules.                                                     |
| `llm_model`          | cleanup model id      | `"groq/openai/gpt-oss-20b"` | Model for the rewrite: a cleanup model from the catalog, or `ollama/<model>` or `lmstudio/<model>` for a local endpoint. |
| `llm_prompt`         | prompt name           | `"clean"`                   | Which prompt to use. See [`[prompts.<name>]`](#promptsname).                                                             |
| `llm_timeout_ms`     | integer, 500 to 30000 | `3000`                      | Give up on the rewrite after this long and insert the locally cleaned text instead.                                      |

## `[prompts.<name>]`

Named instructions for the language-model pass. Five are built in: `clean`, `email`, `casual`, `code_comment` and `verbatim`. Define a table with a built-in name to replace its text, or with a new name to add a prompt. An unknown `llm_prompt` falls back to `clean`.

| Key    | Type   | Default  | Meaning                                                                                      |
| ------ | ------ | -------- | -------------------------------------------------------------------------------------------- |
| `text` | string | required | The instruction given to the model. The transcript is passed as data, never as instructions. |

```toml
[prompts.clean]
text = "Fix grammar and punctuation. Remove false starts. Keep the speaker's words and tone."

[prompts.standup]
text = "Rewrite as three short bullet points."
```

## `[insert]`

| Key                 | Type                  | Default    | Meaning                                                                                                                                                   |
| ------------------- | --------------------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `method`            | `"paste"` \| `"type"` | `"paste"`  | `paste` puts the text on the clipboard and sends the paste shortcut. `type` sends the characters as keystrokes: slower, but works where paste is blocked. |
| `paste_shortcut`    | binding               | `"Ctrl+V"` | Shortcut sent by the paste method. Terminals usually need `Ctrl+Shift+V`.                                                                                 |
| `restore_clipboard` | boolean               | `true`     | Put the previous clipboard contents back after pasting.                                                                                                   |
| `trailing_space`    | boolean               | `true`     | Add a space after the inserted text so the next dictation follows on.                                                                                     |
| `match_case`        | boolean               | `true`     | Match the capitalisation of the first word to the text before the cursor, when it can be read.                                                            |

## `[overlay]`

| Key             | Type                                                                                                        | Default           | Meaning                                           |
| --------------- | ----------------------------------------------------------------------------------------------------------- | ----------------- | ------------------------------------------------- |
| `style`         | `"pill"` \| `"minimal"` \| `"none"`                                                                         | `"pill"`          | Look of the recording indicator. `none` hides it. |
| `position`      | `"bottom-center"` \| `"bottom-left"` \| `"bottom-right"` \| `"top-center"` \| `"top-left"` \| `"top-right"` | `"bottom-center"` | Where it appears on the screen.                   |
| `size`          | `"small"` \| `"medium"` \| `"large"`                                                                        | `"medium"`        | Size of the indicator.                            |
| `show_waveform` | boolean                                                                                                     | `true`            | Show a live waveform while recording.             |

## `[history]`

| Key              | Type               | Default | Meaning                                                                |
| ---------------- | ------------------ | ------- | ---------------------------------------------------------------------- |
| `enabled`        | boolean            | `false` | Save transcripts to `%APPDATA%\Flow\history.db`. Audio is never saved. |
| `retention_days` | integer, 1 to 3650 | `30`    | Delete saved transcripts older than this.                              |

## `[[dictionary]]`

Replaces misheard words with the right spelling. The `write` terms are also passed to the speech model as a hint where the model supports it. Repeat the table for each entry.

| Key     | Type                          | Default  | Meaning                        |
| ------- | ----------------------------- | -------- | ------------------------------ |
| `heard` | list of strings, at least one | required | What the model tends to write. |
| `write` | string                        | required | What to write instead.         |

```toml
[[dictionary]]
heard = ["tory", "towery"]
write = "Tauri"
```

## `[[snippets]]`

A spoken trigger that expands to saved text.

| Key       | Type   | Default  | Meaning                          |
| --------- | ------ | -------- | -------------------------------- |
| `trigger` | string | required | The phrase to say.               |
| `text`    | string | required | The text to insert in its place. |

```toml
[[snippets]]
trigger = "my address"
text = "221B Baker Street, London"
```

## `[[local_models]]`

Registers a model file of your own, in GGUF format, next to the catalog models.

| Key    | Type   | Default       | Meaning                                                                             |
| ------ | ------ | ------------- | ----------------------------------------------------------------------------------- |
| `id`   | string | required      | The id to use in `model.active`. An id that the catalog already uses is ignored.    |
| `name` | string | the file name | Name shown in the settings window.                                                  |
| `path` | string | required      | Path to the file. A relative path is resolved against `%LOCALAPPDATA%\Flow\models`. |

```toml
[[local_models]]
id = "my-model"
name = "My fine-tune"
path = "D:\\models\\my-model.gguf"
```

Flow does not verify or delete files registered this way.

## `[[profiles]]`

A profile changes settings while a particular app has focus. The first profile in the file that matches wins.

| Key                                                         | Type            | Default  | Meaning                                                                                                                           |
| ----------------------------------------------------------- | --------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `name`                                                      | string          | required | Name shown in the settings window.                                                                                                |
| `match_process`                                             | list of strings | `[]`     | Executable names, such as `slack.exe`. Case and the `.exe` suffix are ignored; `*` matches any run of characters.                 |
| `match_title`                                               | list of strings | `[]`     | Window titles. Plain text matches anywhere in the title, ignoring case. Text written as `/pattern/flags` is a regular expression. |
| `enabled`                                                   | boolean         | `true`   | `false` turns dictation off while the app has focus.                                                                              |
| `audio`, `model`, `cleanup`, `insert`, `overlay`, `history` | table           | none     | Overrides for the section of the same name, using the same keys.                                                                  |

A profile matches when any entry of `match_process` or of `match_title` matches. `[general]`, `[hotkeys]` and `[providers]` cannot be overridden. An override with an unknown key is ignored, and one with an invalid value keeps your normal setting.

```toml
[[profiles]]
name = "Terminals"
match_process = ["WindowsTerminal.exe", "wezterm-gui.exe"]
insert.paste_shortcut = "Ctrl+Shift+V"
cleanup.llm_enabled = false

[[profiles]]
name = "Slack"
match_process = ["slack.exe"]
model.active = "groq/whisper-large-v3-turbo"
cleanup.llm_enabled = true
cleanup.llm_prompt = "casual"

[[profiles]]
name = "Password manager"
match_title = ["/^1Password/"]
enabled = false
```

## Full example

```toml
version = 1

[general]
launch_at_login = true
theme = "system"

[hotkeys]
hold_to_talk = "Ctrl+Win"
toggle = "Ctrl+Win+Space"
cancel = "Esc"
paste_last = "Alt+Shift+V"
double_tap_lock = true

[audio]
input_device = "default"
keep_mic_warm = false
sounds = true
max_recording_seconds = 300

[model]
active = "parakeet-tdt-0.6b-v2"
fallback = "parakeet-tdt-0.6b-v2"
device = "auto"
keep_loaded_minutes = 10

[providers.groq]
api_key = "secret:groq"

[cleanup]
filler_removal = true
spoken_formatting = true
spoken_punctuation = true
llm_enabled = false
llm_model = "groq/openai/gpt-oss-20b"
llm_prompt = "clean"
llm_timeout_ms = 3000

[insert]
method = "paste"
paste_shortcut = "Ctrl+V"
restore_clipboard = true
trailing_space = true
match_case = true

[overlay]
style = "pill"
position = "bottom-center"
size = "medium"
show_waveform = true

[history]
enabled = false
retention_days = 30
```
