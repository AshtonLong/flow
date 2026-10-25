# flow-ui — the settings window

Port of `src/renderer/settings/**` (React) to egui 0.37 / eframe (glow). The
window is its own process (`Flow.exe --settings-window`) and talks to the main
process over stdin/stdout JSON lines (`flow_core::protocol::settings`).

```rust
// The app's --settings-window branch:
let code = flow_ui::run(flow_ui::pipe_backend(), flow_ui::RunOptions {
    page: Some("models".into()),
    ..Default::default()
});
```

Preview on the mock backend (port of `mock.ts`):

```sh
cargo run -p flow-ui --example preview -- --page general --theme dark
cargo run -p flow-ui --example preview -- --page general --theme light --noissues --screenshot out.png
cargo run -p flow-ui --example preview -- --gallery            # every component
cargo run -p flow-ui --example preview -- --gallery --demo select --screenshot select.png
```

Flags: `--page`, `--theme light|dark`, `--screenshot <png>`, `--scroll <pt>`,
`--contrast high`, `--motion reduced`, `--gallery`, `--demo select|confirm|modal`
(with `--onboarding`: `--demo hotkey|model|cloud|try` starts on a later step),
and the mock's states `--fresh --onboarding --noissues --history-off
--downloading <id> --degraded`. Screenshots are taken once config, catalog and
app info have loaded, at the window's own size (1000×700 at 100 %).

## Layout

| Module | Ports | What |
| --- | --- | --- |
| `backend/` | `window.flow`, `mock.ts` | `Backend` trait, `pipe_backend`, `mock_backend`, `Link`, `Pending<T>` |
| `stores/` | `lib/config.tsx`, `lib/catalog.tsx`, `lib/hooks.ts`, `lib/theme.ts` | `ConfigStore` (optimistic writes), `CatalogStore`, `AppInfoStore`, `Microphones`, `Flash`, `Debounced`, `SystemTheme` |
| `util/` | `lib/format.ts`, `patch.ts`, `listDraft.ts`, `models.ts`, `profiles.ts` | pure helpers, with the TS unit tests |
| `design/` | `brand.css`, `styles.css` | tokens, Recursive families, text styles, box shadows, lucide icons, motion, focus rings |
| `components/` | `components/*.tsx` | buttons, cards/rows, toggle, select, text fields, chips, dialogs, hotkey recorder, key caps, meters, notices, issues banner, error boundary, tooltip |
| `pages/<page>.rs` | `pages/*.tsx` | one module per page; `pages/models/*` for the Models sections |
| `onboarding/` | `onboarding/Onboarding.tsx` | the first-run flow: steps, the model choice, the key form, the dictation test |
| `shell/` | `App.tsx`, `main.tsx` | the eframe app: title strip, caption buttons, nav rail, status footer, content panel, Boot/Onboarding/Shell |
| `adapters/` | `common/capture.ts`, `shared/hotkeys.ts` | the window's view of `flow_audio` (microphone list, level monitor) and the hotkey grammar from `flow_core::hotkeys` |
| `platform.rs` | — | Win32: system theme fallback, high-contrast colours, DWM, clipboard |

## Writing a page

A page is a struct implementing `pages::Page`; the shell creates it when the
page opens and drops it when another page opens (React unmounts inactive tab
panels the same way). Keep page-local state (`useState`) in the struct.
`pages/general.rs` is the worked example.

```rust
#[derive(Default)]
pub struct Hotkeys {
    confirm_reset: bool,               // a dialog's open flag
    checking: Option<Pending<String>>, // a call in flight
}

impl Page for Hotkeys {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let hotkeys = cx.config.config().hotkeys.clone(); // read (clone what the closures need)
        page_header(ui, "Hotkeys", Some("Hold, tap or toggle…"));
        Group::new("Dictation").show(ui, |ui| {
            SettingRow::new("Hold to talk", "Hold the keys while you speak.").show(ui, |ui| {
                let recorder = HotkeyRecorder::new("hold_to_talk", &hotkeys.hold_to_talk, "Hold to talk")
                    .default_value("Ctrl+Win");
                if let Some(binding) = recorder.show(ui, cx.link, cx.events) {
                    cx.config.set(path!["hotkeys", "hold_to_talk"], binding); // write
                }
            });
        });
    }
}
```

### Config: `cx.config` (`useConfig`)

- `config()` / `snapshot()` read the current (optimistic) state.
- `set(path, value)` writes one value (any `Serialize`: `bool`, `&str`, numbers,
  the schema enums). `remove(path)` deletes a key (`set(path, undefined)`).
  `patch(Vec<ConfigPatch>)` writes several in one file update and returns a
  `Pending<()>` that settles when the main process answered (drop it if you
  don't care).
- Writes are optimistic exactly as in `lib/config.tsx`: the store patches its
  copy and re-validates it with `validate_config` at once; while writes are in
  flight only the reply to the newest one is applied, and `ConfigChanged` from
  elsewhere is ignored. A failure shows "The change was not saved: …" in the
  shell's error notice and reloads the file.
- Paths: `flow_core::path!["audio", "sounds"]`, `path!["dictionary", 0usize]`.

### Calling the main process: `cx.link`

- `cx.link.call::<T>(Request::…)` returns `Pending<T>`, where `T` is the type
  the `Request` variant documents. Keep it in the page struct and poll it each
  frame — the reply's arrival repaints the window:

  ```rust
  if Button::new("Check for updates").disabled(self.checking.is_some()).show(ui).clicked() {
      self.checking = Some(cx.link.call(Request::AppCheckUpdates));
  }
  if let Some(result) = take_settled(&mut self.checking) {
      self.update = Some(result.unwrap_or_else(|e| e)); // Err(String) is the error message
  }
  ```
- `cx.link.send(Request::…)` fires and forgets (errors are logged).
- `cx.events` holds the pushes that arrived since the last frame
  (`BenchResult`, `HotkeysCaptureProgress`, `DictationEvent`, …). Config,
  catalog and theme events are already applied to the stores.
- `cx.catalog` (`useCatalog`): `snapshot()`, `progress()`, `download_of(id)`,
  `reload()`, `clear_progress(id)`. `cx.app_info` (`useAppInfo`): `info()`,
  `refresh()`. `cx.mics` (`useMicrophones`): `list()`, `refresh()`.
- `cx.go_to(PageId::Models)` (`useGoTo`).

### Components

Builders ending in `show(ui)`. Controls return what changed instead of taking
`onChange` callbacks. Transient state (drafts, open menus) lives in egui memory
under an id derived from the `id_salt` you pass (use the config path); resources
(an open microphone) are structs the page owns.

| TSX | Rust |
| --- | --- |
| `<PageHeader title lead>` | `page_header(ui, title, Some(lead))`, `page_header_with(ui, title, \|ui\| …)` |
| `<Group title description action>` | `Group::new(t).description(d).action(\|ui\| …).show(ui, \|ui\| cards)` |
| `<SettingRow label description control>{children}` | `SettingRow::new(label, desc).disabled(b).show(ui, \|ui\| control)`; `.show_with(ui, control, children)`; `.show_children(ui, children)`; `SettingRow::new_with(label, \|ui\| rich description)` |
| `<Card>` / `<EmptyState>` | `card(ui, Margin, \|ui\| …)` / `empty_state(ui, sentence, \|ui\| action)` |
| `<Button variant size danger icon>` | `Button::new("Save").accent().small().danger(true).icon(Icon::Download, 15.0).min_width(120.0).disabled(b).show(ui)` → `Response` |
| `<IconButton label icon>` | `IconButton::new("Delete", Icon::Trash2, 15.0).small().show(ui)` (label = tooltip) |
| `.link` / `<ExternalLink>` | `link(ui, "Pause dictation", TextStyle::CAPTION)`; open URLs with `cx.link.send(Request::AppOpenExternal(url))` |
| `<Toggle checked onChange>` | `Toggle::new(on).disabled(b).hide_state().show(ui).changed: Option<bool>` |
| `<Select value options onChange width>` | `Select::new("overlay.style", Some(&v), &[SelectOption::new(v, "Pill").hint("…")]).width(280.0).show(ui).changed` |
| `<TextField value onCommit validate mono>` | `TextField::new("insert.paste_shortcut", &v).width(200.0).mono().validate(&f).show(ui).committed` |
| `<NumberField min max unit>` | `NumberField::new("audio.max_recording_seconds", v, 5..=3600).unit("seconds").show(ui).committed` |
| `<TextArea>` | `TextArea::new(id, &v).rows(4).show(ui).committed` |
| `<ChipsInput values onChange>` | `ChipsInput::new(id, &values).placeholder("…").mono().show(ui) -> Option<Vec<String>>` |
| `<ConfirmDialog>` / `<Modal>` | `ConfirmDialog::new(id, title, desc, "Remove model").show(ui.ctx(), &mut self.open) -> bool`; `Modal::new(id, title).description(d).width(560.0).show(ui.ctx(), &mut self.open, \|ui\| …)` |
| `<HotkeyRecorder>` | `HotkeyRecorder::new(id, &binding, "Hold to talk").default_value("Ctrl+Win").show(ui, cx.link, cx.events)` |
| `<KeyCaps binding muted>` | `key_caps(ui, binding, muted)` |
| `<MicSelect>` / `<MicMeter>` | `mic_select(ui, id, &device, cx.mics.list(), 280.0)`; `MicMeter` field + `meter.show(ui, &device)` |
| `<Notice tone action>` | `Notice::new(Tone::Danger, &text).show(ui)` / `.show_with_action(ui, \|ui\| …)` |
| `<Badge tone>` / `<ProgressBar>` / `.spin` | `badge(ui, "Active", BadgeTone::Accent)`, `progress_bar(ui, Some(0.4))`, `spinner(ui, 15.0, color)` |
| `.checkbox` / `.radio` | `checkbox(ui, checked, disabled) -> (Response, Option<bool>)`, `radio(ui, checked, disabled) -> Response` |
| Tooltip | `components::tooltip::show(ui, &response, "label")` (500 ms delay) |
| `<ErrorBoundary>` | done by the shell around every page |

### Layout rules

- The style has **zero item spacing**: write CSS margins as `ui.add_space(px)`.
  Inside a `SettingRow` control the gap is 8 px (`gap-2`), cards in a `Group`
  are 3 px apart, and a `Group` ends with its 32 px margin.
- Rows centre items of unknown size by measuring them in the previous pass and
  asking egui to redo the pass when a size changes (`components::measure`).
  Use it for your own flex-like rows.
- A horizontal layout with `Align::Center` needs a finite height: inside a
  scroll area use `ui.allocate_ui_with_layout(vec2(w, h), layout, …)`.
- Right-aligned rows are laid out right to left, so add their items last first.

## Design tokens

`design::tokens(ui.ctx())` returns the current `Tokens` (light, dark or high
contrast), field names following the CSS custom properties: `fg`, `fg_2`,
`fg_3`, `fg_disabled`, `mica` (paper), `layer`, `card`, `inset`, `flyout`,
`line`, `line_strong`, `ctl`, `ctl_hover`, `ctl_wall`, `input_line`,
`subtle_hover`, `subtle_press`, `switch_off`, `accent_fill`, `on_accent`,
`danger`/`danger_bg`, `warn`/`warn_bg`, `ok`/`ok_bg`, `voice` (orange, live
microphone only), `cloud`, `key_ink_top`, shadows `shadow_card`,
`shadow_flyout`, `shadow_dialog`, and `hc_border` / `hc_outline` for the
high-contrast extras.

Text: `TextStyle::CAPTION` (12/16), `BODY` (14/20), `SUBTITLE` (20/28),
`TITLE` (28/36); `.weight(Weight::Semibold)` (`font-semibold`; also `Normal`
400, `Body` 440, `Medium` 560, `Strong` 620, `Heavy` 760, `Black` 840),
`.mono()` (`font-mono`), `.size(13.0, 20.0)` (`text-[13px]`), `.heading()`
(`h1`/`h2` letter spacing). Draw with `design::text::{label, paragraph,
paragraph_max, galley, galley_truncated, Rich}` — these snap baselines the way
Chromium does, so text lands on the same pixels as in the Electron app.

Surfaces: `design::shadow::{paint_surface, surface, paint_box_shadows}` paint
CSS box-shadows (Gaussian, σ = blur / 2). Icons: `design::icons::{paint, show}`
with lucide's `size` and `strokeWidth`. Motion: `design::motion::{toggle,
value, snappy, ease}` (durations drop to zero under reduced motion). Focus:
`design::focus::ring(ui, &response, rect, radius)` draws the keyboard-only ring.
As in Chromium, the arrow keys never move focus by themselves (`focus::update`
cancels egui's spatial navigation); a composite widget moves it explicitly,
e.g. `focus::vertical_list(ui, &triggers)` for Radix's Accordion headers.
Dialogs trap Tab, focus on open and take Escape as Radix does (see
`components/dialog.rs`).

## Performance

The window repaints only on input, replies and events (each arrival calls
`request_repaint`), and while an animation runs. Timers use
`request_repaint_after` (tooltips, the text-field debounce, `Flash`). Avoid
`request_repaint()` every frame unless something moves (the spinner and the
indeterminate progress bar do).

## Tests

`cargo test -p flow-ui`. `src/testing.rs` has a headless harness that runs real
egui passes with simulated clicks and keys against an in-memory backend
(`Harness`, `PageHarness`); use it for page behaviour.
