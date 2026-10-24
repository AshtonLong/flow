//! PORT: tests/unit/config/store.test.ts

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use flow_core::config::migrations::PlainObject;
use flow_core::config::{
    parse_toml, strip_api_keys, Config, ConfigIssue, ConfigPatch, ConfigSnapshot, ConfigStore, ConfigStoreOptions,
    Migration, PathSeg, ProviderConfig, ThemeSetting, CONFIG_VERSION, DEFAULT_CONFIG_TEXT, MIGRATIONS,
};
use flow_core::path;
use serde_json::{json, Value};

fn example() -> String {
    include_str!("fixtures/example.toml").replace("\r\n", "\n")
}

/// One temporary directory per test, with `Flow/config.toml` inside. Stores
/// opened through it are closed before the directory goes.
struct Fixture {
    stores: Vec<Arc<ConfigStore>>,
    file: PathBuf,
    dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for store in self.stores.drain(..) {
            store.close();
        }
    }
}

/// A store and every `changed` snapshot it emits.
struct Opened {
    store: Arc<ConfigStore>,
    events: Receiver<ConfigSnapshot>,
}

impl Opened {
    /// Snapshots emitted so far and not yet taken.
    fn take(&self) -> Vec<ConfigSnapshot> {
        self.events.try_iter().collect()
    }

    /// The next `changed` event, or a panic after `ms`.
    fn next_change(&self, ms: u64) -> ConfigSnapshot {
        self.events.recv_timeout(Duration::from_millis(ms)).unwrap_or_else(|_| panic!("no change within {ms} ms"))
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::Builder::new().prefix("flow-config-").tempdir().unwrap();
        let file = dir.path().join("Flow").join("config.toml");
        Self { stores: Vec::new(), file, dir }
    }

    fn open(&mut self, options: ConfigStoreOptions) -> Opened {
        let store = ConfigStore::with_options(&self.file, options);
        let (tx, events) = mpsc::channel();
        store.on_changed(move |snapshot| {
            let _ = tx.send(snapshot.clone());
        });
        self.stores.push(store.clone());
        Opened { store, events }
    }

    fn open_default(&mut self) -> Opened {
        self.open(ConfigStoreOptions::default())
    }

    fn read(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap()
    }

    fn seed(&self, text: &str) {
        std::fs::create_dir_all(self.dir.path().join("Flow")).unwrap();
        std::fs::write(&self.file, text).unwrap();
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}{suffix}", self.file.display()))
    }

    fn flow_dir_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path().join("Flow"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Lets a freshly started watcher settle before the test edits the file.
fn watch(store: &Arc<ConfigStore>) {
    store.watch();
    sleep(100);
}

fn joined(path: &[PathSeg]) -> String {
    path.iter().map(ToString::to_string).collect::<Vec<_>>().join(".")
}

fn lines_by_path(issues: &[ConfigIssue]) -> HashMap<String, Option<u32>> {
    issues.iter().map(|issue| (joined(&issue.path), issue.line)).collect()
}

fn expected_lines(pairs: &[(&str, u32)]) -> HashMap<String, Option<u32>> {
    pairs.iter().map(|(path, line)| (path.to_string(), Some(*line))).collect()
}

fn set(path: Vec<PathSeg>, value: Value) -> ConfigPatch {
    ConfigPatch { path, value: Some(value) }
}

fn file_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

mod loading {
    use super::*;

    #[test]
    fn returns_defaults_before_loading() {
        let mut fx = Fixture::new();
        let opened = fx.open_default();
        assert_eq!(opened.store.path(), fx.file.as_path());
        assert_eq!(*opened.store.get(), Config::default());
        assert_eq!(opened.store.issues(), vec![]);
    }

    #[test]
    fn creates_a_missing_file_with_a_commented_header_and_the_version() {
        let mut fx = Fixture::new();
        let opened = fx.open_default();
        opened.store.load();
        let text = fx.read();
        assert_eq!(text, DEFAULT_CONFIG_TEXT);
        assert!(text.starts_with('#'));
        assert!(text.contains(&format!("version = {CONFIG_VERSION}\n")));
        let parsed = parse_toml(&text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get("version"), Some(&toml::Value::Integer(CONFIG_VERSION)));
        assert_eq!(*opened.store.get(), Config::default());
        assert_eq!(opened.store.issues(), vec![]);
        assert_eq!(
            opened.store.snapshot(),
            ConfigSnapshot { config: Config::default(), issues: vec![], path: file_string(&fx.file) }
        );
    }

    #[test]
    fn accepts_an_empty_file_and_leaves_it_alone() {
        let mut fx = Fixture::new();
        fx.seed("");
        let opened = fx.open_default();
        opened.store.load();
        assert_eq!(*opened.store.get(), Config::default());
        assert_eq!(opened.store.issues(), vec![]);
        assert_eq!(fx.read(), "");
    }

    #[test]
    fn loads_the_spec_example_without_issues() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        assert_eq!(opened.store.issues(), vec![]);
        let config = opened.store.get();
        assert!(config.general.launch_at_login);
        assert_eq!(
            config.providers.get("custom-lan"),
            Some(&ProviderConfig {
                base_url: Some("http://192.168.1.20:8000/v1".into()),
                api_key: Some("secret:custom-lan".into()),
                model: None,
            })
        );
        let names: Vec<&str> = config.profiles.iter().map(|profile| profile.name.as_str()).collect();
        assert_eq!(names, vec!["Terminals", "Slack"]);
        assert_eq!(
            config.profiles[0].insert.clone().map(Value::Object),
            Some(json!({ "paste_shortcut": "Ctrl+Shift+V" }))
        );
        assert_eq!(fx.read(), example());
    }

    #[test]
    fn falls_back_to_the_default_for_a_bad_key_and_reports_its_line() {
        let mut fx = Fixture::new();
        fx.seed(
            &[
                "version = 1",
                "",
                "[audio]",
                "sounds = \"loud\"   # line 4",
                "max_recording_seconds = 2",
                "keep_mic_warm = true",
                "",
                "[overlay]",
                "style = \"banner\"",
                "",
                "[providers.groq]",
                "api_key = \"gsk_plaintext\"",
                "",
                "[[profiles]]",
                "name = \"A\"",
                "insert.method = \"bogus\"",
                "",
            ]
            .join("\n"),
        );
        let opened = fx.open_default();
        opened.store.load();
        let config = opened.store.get();
        assert!(config.audio.sounds);
        assert_eq!(config.audio.max_recording_seconds, 300);
        assert!(config.audio.keep_mic_warm);
        assert_eq!(config.overlay.style, flow_core::config::OverlayStyle::Pill);
        assert_eq!(config.providers.get("groq"), Some(&ProviderConfig::default()));
        // Override values are only checked when a profile is applied.
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(
            lines_by_path(&opened.store.issues()),
            expected_lines(&[
                ("audio.sounds", 4),
                ("audio.max_recording_seconds", 5),
                ("overlay.style", 9),
                ("providers.groq.api_key", 12),
            ])
        );
        for issue in opened.store.issues() {
            assert_ne!(issue.message, "");
        }
    }

    #[test]
    fn reports_the_line_of_a_broken_list_item_dotted_keys_and_inline_values() {
        let mut fx = Fixture::new();
        fx.seed(
            &[
                "audio.sounds = 5",
                "insert = { method = \"paste\",",
                "  paste_shortcut = 7 }",
                "",
                "[[dictionary]]",
                "heard = [\"a\"]",
                "write = \"A\"",
                "",
                "[[dictionary]]",
                "heard = [\"b\"]",
                "",
                "[[snippets]]",
                "trigger = \"\"",
                "text = \"x\"",
                "",
            ]
            .join("\n"),
        );
        let opened = fx.open_default();
        opened.store.load();
        let config = opened.store.get();
        assert_eq!(config.dictionary.len(), 1);
        assert_eq!(config.dictionary[0].heard, vec!["a".to_string()]);
        assert_eq!(config.dictionary[0].write, "A");
        assert_eq!(config.snippets, vec![]);
        assert_eq!(
            lines_by_path(&opened.store.issues()),
            expected_lines(&[
                ("audio.sounds", 1),
                ("insert.paste_shortcut", 3),
                // The missing `write` has no line of its own: point at the item.
                ("dictionary.1.write", 9),
                ("snippets.0.trigger", 13),
            ])
        );
    }

    #[test]
    fn uses_defaults_and_reports_the_line_of_a_syntax_error_on_first_load() {
        let mut fx = Fixture::new();
        fx.seed("version = 1\n[audio]\nsounds = = true\n");
        let opened = fx.open_default();
        opened.store.load();
        assert_eq!(*opened.store.get(), Config::default());
        let issues = opened.store.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, Vec::<PathSeg>::new());
        assert!(issues[0].message.contains("Invalid TOML"), "{}", issues[0].message);
        assert_eq!(issues[0].line, Some(3));
        assert!(!issues[0].message.contains('\n'));
        assert_eq!(fx.read(), "version = 1\n[audio]\nsounds = = true\n");
    }

    #[test]
    fn keeps_the_last_good_config_when_a_later_edit_has_a_syntax_error() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = false\n");
        let opened = fx.open_default();
        opened.store.load();
        assert!(!opened.store.get().audio.sounds);

        fx.seed("[audio]\nsounds = false\nkeep_mic_warm = tru\n");
        opened.take();
        opened.store.load();
        let snapshot = opened.next_change(3000);
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].line, Some(3));

        fx.seed("[audio]\nsounds = false\nkeep_mic_warm = true\n");
        opened.store.load();
        assert_eq!(opened.store.issues(), vec![]);
        assert!(opened.store.get().audio.keep_mic_warm);
    }

    #[test]
    fn reads_a_file_saved_with_a_byte_order_mark_and_keeps_the_mark_when_writing() {
        let mut fx = Fixture::new();
        fx.seed("\u{feff}[audio]\nsounds = false\n");
        let opened = fx.open_default();
        opened.store.load();
        assert_eq!(opened.store.issues(), vec![]);
        assert!(!opened.store.get().audio.sounds);
        opened.store.set(&[set(path!["audio", "sounds"], json!(true))]).unwrap();
        assert_eq!(fx.read(), "\u{feff}[audio]\nsounds = true\n");
    }

    #[test]
    fn never_throws_even_when_the_path_cannot_be_read() {
        let mut fx = Fixture::new();
        // A directory where the file should be.
        std::fs::create_dir_all(&fx.file).unwrap();
        let opened = fx.open_default();
        opened.store.load();
        assert_eq!(*opened.store.get(), Config::default());
        let issues = opened.store.issues();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("Could not read config.toml"), "{}", issues[0].message);
    }

    #[test]
    fn emits_changed_on_a_later_load_only_when_something_changed() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = false\n");
        let opened = fx.open_default();
        opened.store.load();
        opened.store.load();
        assert_eq!(opened.take().len(), 0);
        fx.seed("[audio]\nsounds = true\n");
        opened.store.load();
        let events = opened.take();
        assert_eq!(events.len(), 1);
        assert!(events[0].config.audio.sounds);
    }
}

mod set {
    use super::*;

    #[test]
    fn patches_the_file_in_place_keeps_comments_and_emits_changed() {
        let mut fx = Fixture::new();
        let example = example();
        fx.seed(&example);
        let opened = fx.open_default();
        opened.store.load();
        opened.take();

        let snapshot = opened
            .store
            .set(&[
                set(path!["general", "theme"], json!("dark")),
                set(path!["audio", "sounds"], json!(false)),
                set(path!["providers", "openai", "api_key"], json!("secret:openai")),
                set(path!["profiles", 1usize, "cleanup", "llm_prompt"], json!("email")),
            ])
            .unwrap();

        assert_eq!(snapshot.config.general.theme, ThemeSetting::Dark);
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(
            snapshot.config.providers.get("openai"),
            Some(&ProviderConfig { api_key: Some("secret:openai".into()), ..ProviderConfig::default() })
        );
        assert_eq!(
            snapshot.config.profiles[1].cleanup.clone().map(Value::Object),
            Some(json!({ "llm_enabled": true, "llm_prompt": "email" }))
        );
        assert_eq!(snapshot.issues, vec![]);
        assert_eq!(snapshot.path, file_string(&fx.file));
        assert_eq!(*opened.store.get(), snapshot.config);
        assert_eq!(opened.take(), vec![snapshot]);

        let text = fx.read();
        assert!(text.contains("theme = \"dark\"              # system | light | dark"));
        let comment = |line: &str| line[line.find('#').unwrap()..].to_string();
        let before: Vec<String> = example.split('\n').filter(|line| line.contains('#')).map(comment).collect();
        let after: Vec<String> = text.split('\n').filter(|line| line.contains('#')).map(comment).collect();
        assert_eq!(after, before);
        assert_eq!(text.split('\n').count(), example.split('\n').count() + 3);
        assert!(!fx.sibling(".tmp").exists());
    }

    #[test]
    fn does_not_rewrite_the_file_when_nothing_changes() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let before = std::fs::metadata(&fx.file).unwrap().modified().unwrap();
        sleep(20);
        let snapshot = opened.store.set(&[set(path!["audio", "sounds"], json!(true))]).unwrap();
        assert!(snapshot.config.audio.sounds);
        assert_eq!(std::fs::metadata(&fx.file).unwrap().modified().unwrap(), before);
        assert_eq!(fx.read(), example());
    }

    #[test]
    fn creates_the_file_when_setting_before_it_exists() {
        let mut fx = Fixture::new();
        let opened = fx.open_default();
        let snapshot = opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap();
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(fx.read(), format!("{DEFAULT_CONFIG_TEXT}\n[audio]\nsounds = false\n"));
    }

    #[test]
    fn writes_an_invalid_value_but_reports_it_and_falls_back_to_the_default() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = true\n");
        let opened = fx.open_default();
        opened.store.load();
        let snapshot = opened.store.set(&[set(path!["audio", "max_recording_seconds"], json!(1))]).unwrap();
        assert_eq!(snapshot.config.audio.max_recording_seconds, 300);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].path, path!["audio", "max_recording_seconds"]);
        assert_eq!(snapshot.issues[0].line, Some(3));
    }

    #[test]
    fn rejects_and_leaves_the_file_alone_while_it_has_a_syntax_error() {
        let mut fx = Fixture::new();
        let broken = "[audio]\nsounds = = true\n";
        fx.seed(broken);
        let opened = fx.open_default();
        opened.store.load();
        let error = opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap_err();
        assert!(error.to_string().contains("syntax error on line 2"), "{error}");
        assert_eq!(fx.read(), broken);
    }

    #[test]
    fn rejects_a_patch_for_a_list_item_that_does_not_exist() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let error = opened.store.set(&[set(path!["profiles", 9usize, "name"], json!("x"))]).unwrap_err();
        assert!(error.to_string().contains("No such list item"), "{error}");
        assert_eq!(fx.read(), example());
        // The store still works afterwards.
        assert!(opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).is_ok());
    }

    #[test]
    fn applies_concurrent_calls_one_after_another() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let patches = [
            set(path!["audio", "sounds"], json!(false)),
            set(path!["general", "theme"], json!("light")),
            set(path!["history", "enabled"], json!(true)),
        ];
        let handles: Vec<_> = patches
            .into_iter()
            .map(|patch| {
                let store = opened.store.clone();
                std::thread::spawn(move || store.set(&[patch]))
            })
            .collect();
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }
        // Each call saw the others' writes: the last one carries all three.
        let last = opened.take().pop().unwrap().config;
        assert_eq!((last.audio.sounds, last.general.theme, last.history.enabled), (false, ThemeSetting::Light, true));
        let text = fx.read();
        assert!(text.contains("sounds = false\n"));
        assert!(text.contains("theme = \"light\" "));
        assert!(text.contains("[history]\nenabled = true\n"));
    }

    /// Not in the TS suite: a listener may call back into the store without
    /// deadlocking, and events still arrive in order.
    #[test]
    fn lets_listeners_call_back_into_the_store() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = true\n");
        let opened = fx.open_default();
        opened.store.load();
        let store = Arc::downgrade(&opened.store);
        opened.store.on_changed(move |snapshot| {
            let Some(store) = store.upgrade() else { return };
            assert_eq!(*store.get(), snapshot.config);
            if !snapshot.config.audio.sounds && !snapshot.config.audio.keep_mic_warm {
                store.set(&[set(path!["audio", "keep_mic_warm"], json!(true))]).unwrap();
            }
        });
        opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap();
        let events = opened.take();
        assert_eq!(events.len(), 2);
        assert!(!events[0].config.audio.keep_mic_warm);
        assert!(events[1].config.audio.keep_mic_warm);
        assert_eq!(fx.read(), "[audio]\nsounds = false\nkeep_mic_warm = true\n");
    }

    #[test]
    fn patches_the_file_as_it_is_on_disk_not_as_it_was_last_loaded() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = true\n");
        let opened = fx.open_default();
        opened.store.load();
        fx.seed("# edited by hand\n[audio]\nsounds = true\nkeep_mic_warm = true\n");
        let snapshot = opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap();
        assert_eq!(fx.read(), "# edited by hand\n[audio]\nsounds = false\nkeep_mic_warm = true\n");
        assert!(snapshot.config.audio.keep_mic_warm);
    }
}

mod watching {
    use super::*;

    #[test]
    fn applies_an_edit_made_in_a_text_editor_within_a_second() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);

        let started = Instant::now();
        std::fs::write(&fx.file, example().replacen("sounds = true", "sounds = false", 1)).unwrap();
        let snapshot = opened.next_change(3000);
        assert!(started.elapsed() < Duration::from_millis(1000), "{:?}", started.elapsed());
        assert!(!snapshot.config.audio.sounds);
        assert!(!opened.store.get().audio.sounds);
    }

    #[test]
    fn sees_an_editor_that_saves_by_replacing_the_file() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);

        let swap = fx.dir.path().join("Flow").join("config.toml.swp");
        std::fs::write(&swap, example().replacen("theme = \"system\"", "theme = \"light\"", 1)).unwrap();
        std::fs::rename(&swap, &fx.file).unwrap();
        assert_eq!(opened.next_change(3000).config.general.theme, ThemeSetting::Light);

        // And it keeps watching after the replacement.
        std::fs::write(&fx.file, example().replacen("theme = \"system\"", "theme = \"dark\"", 1)).unwrap();
        assert_eq!(opened.next_change(3000).config.general.theme, ThemeSetting::Dark);
    }

    #[test]
    fn reports_a_syntax_error_from_a_hand_edit_and_recovers_when_it_is_fixed() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = false\n");
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);

        std::fs::write(&fx.file, "[audio]\nsounds = fals\n").unwrap();
        let snapshot = opened.next_change(3000);
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].path, Vec::<PathSeg>::new());
        assert_eq!(snapshot.issues[0].line, Some(2));

        std::fs::write(&fx.file, "[audio]\nsounds = true\n").unwrap();
        let snapshot = opened.next_change(3000);
        assert!(snapshot.config.audio.sounds);
        assert_eq!(snapshot.issues, vec![]);
    }

    #[test]
    fn ignores_the_echo_of_its_own_writes() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);
        opened.take();

        opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap();
        sleep(600);
        assert_eq!(opened.take().len(), 1);
    }

    #[test]
    fn ignores_a_save_that_does_not_change_the_content() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);
        opened.take();
        std::fs::write(&fx.file, example()).unwrap();
        sleep(600);
        assert_eq!(opened.take().len(), 0);
    }

    #[test]
    fn stops_reloading_after_close() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        watch(&opened.store);
        opened.store.close();
        opened.take();
        std::fs::write(&fx.file, example().replacen("sounds = true", "sounds = false", 1)).unwrap();
        sleep(500);
        assert_eq!(opened.take().len(), 0);
        assert!(opened.store.get().audio.sounds);
    }
}

mod migrations {
    use super::*;

    /// A made-up v1 → v2 → v3 history, to exercise the framework.
    fn one_to_two(mut raw: PlainObject) -> Result<PlainObject, String> {
        if let Some(Value::Object(audio)) = raw.get_mut("audio")
            && let Some(sounds) = audio.shift_remove("sounds")
        {
            audio.insert("feedback_sounds".into(), sounds);
        }
        Ok(raw)
    }

    fn two_to_three(mut raw: PlainObject) -> Result<PlainObject, String> {
        let mut general = match raw.get("general") {
            Some(Value::Object(general)) => general.clone(),
            _ => PlainObject::new(),
        };
        general.insert("theme".into(), json!("dark"));
        raw.insert("general".into(), Value::Object(general));
        Ok(raw)
    }

    fn history() -> Vec<Migration> {
        vec![Migration { from: 1, to: 2, migrate: one_to_two }, Migration { from: 2, to: 3, migrate: two_to_three }]
    }

    fn options(migrations: Vec<Migration>, version: i64) -> ConfigStoreOptions {
        ConfigStoreOptions { migrations: Some(migrations), version: Some(version), debounce: None }
    }

    #[test]
    fn has_no_migrations_yet_version_1_is_the_first_format() {
        assert_eq!(CONFIG_VERSION, 1);
        assert!(MIGRATIONS.is_empty());
    }

    #[test]
    fn migrates_an_older_file_in_place_in_order_and_keeps_a_backup() {
        let mut fx = Fixture::new();
        let example = example();
        fx.seed(&example);
        let opened = fx.open(options(history(), 3));
        opened.store.load();

        assert_eq!(std::fs::read_to_string(fx.sibling(".bak-v1")).unwrap(), example);
        let expected = example
            .replacen("version = 1", "version = 3", 1)
            .replacen("theme = \"system\"   ", "theme = \"dark\"   ", 1)
            .replacen("sounds = true\nmax_recording_seconds = 300\n", "max_recording_seconds = 300\n", 1)
            .replacen("max_recording_seconds = 300\n", "max_recording_seconds = 300\nfeedback_sounds = true\n", 1);
        assert_eq!(fx.read(), expected);
        assert_eq!(opened.store.get().version, 3);
        assert_eq!(opened.store.get().general.theme, ThemeSetting::Dark);
        assert_eq!(opened.store.issues(), vec![]);
    }

    #[test]
    fn does_nothing_to_a_file_already_at_the_current_version() {
        let mut fx = Fixture::new();
        let text = example().replacen("version = 1", "version = 3", 1);
        fx.seed(&text);
        let opened = fx.open(options(history(), 3));
        opened.store.load();
        assert_eq!(fx.read(), text);
        assert_eq!(fx.flow_dir_entries(), vec!["config.toml"]);
    }

    #[test]
    fn starts_from_the_right_step_for_a_file_part_way_along() {
        let mut fx = Fixture::new();
        fx.seed("version = 2\n\n[audio]\nsounds = true # stays: the 1→2 step is not re-run\n");
        let opened = fx.open(options(history(), 3));
        opened.store.load();
        assert_eq!(
            fx.read(),
            "version = 3\n\n[audio]\nsounds = true # stays: the 1→2 step is not re-run\n\n[general]\ntheme = \"dark\"\n"
        );
        assert!(fx.sibling(".bak-v2").exists());
    }

    #[test]
    fn treats_a_file_with_no_version_as_the_first_version() {
        let mut fx = Fixture::new();
        fx.seed("[audio]\nsounds = false\n");
        let opened = fx.open(options(vec![history()[0]], 2));
        opened.store.load();
        assert_eq!(fx.read(), "version = 2\n\n[audio]\nfeedback_sounds = false\n");
        assert_eq!(std::fs::read_to_string(fx.sibling(".bak-v1")).unwrap(), "[audio]\nsounds = false\n");
    }

    #[test]
    fn does_not_overwrite_an_existing_backup() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        std::fs::write(fx.sibling(".bak-v1"), "the true original").unwrap();
        let opened = fx.open(options(history(), 3));
        opened.store.load();
        assert_eq!(std::fs::read_to_string(fx.sibling(".bak-v1")).unwrap(), "the true original");
        assert_eq!(opened.store.get().version, 3);
    }

    #[test]
    fn leaves_the_file_alone_and_reports_it_when_a_migration_throws() {
        fn boom(_: PlainObject) -> Result<PlainObject, String> {
            Err("boom".into())
        }
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open(options(vec![Migration { from: 1, to: 2, migrate: boom }], 2));
        opened.store.load();
        assert_eq!(fx.read(), example());
        assert!(!fx.sibling(".bak-v1").exists());
        let issues = opened.store.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, path!["version"]);
        assert!(issues[0].message.contains("boom"), "{}", issues[0].message);
        assert_eq!(issues[0].line, Some(1));
        assert!(opened.store.get().general.launch_at_login);
    }

    #[test]
    fn reports_a_missing_migration_step_and_a_file_from_a_newer_version() {
        let mut fx = Fixture::new();
        fx.seed("version = 1\n");
        let older = fx.open(options(vec![], 2));
        older.store.load();
        let issues = older.store.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, path!["version"]);
        assert!(issues[0].message.contains("No migration"), "{}", issues[0].message);
        assert_eq!(issues[0].line, Some(1));
        assert_eq!(fx.read(), "version = 1\n");

        fx.seed("# from the future\nversion = 9\n[audio]\nsounds = false\n");
        let newer = fx.open_default();
        newer.store.load();
        let issues = newer.store.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, path!["version"]);
        assert!(issues[0].message.contains("newer"), "{}", issues[0].message);
        assert_eq!(issues[0].line, Some(2));
        assert!(!newer.store.get().audio.sounds);
        assert_eq!(fx.read(), "# from the future\nversion = 9\n[audio]\nsounds = false\n");
    }
}

mod export_and_import {
    use super::*;

    #[test]
    fn exports_the_file_without_any_provider_api_key_line() {
        let mut fx = Fixture::new();
        let example = example();
        fx.seed(&example);
        let opened = fx.open_default();
        opened.store.load();
        let target = fx.path("exports").join("flow-config.toml");
        opened.store.export_to(&target).unwrap();
        let exported = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            exported,
            example.replacen("api_key = \"secret:groq\"\n", "", 1).replacen("api_key = \"secret:custom-lan\"\n", "", 1)
        );
        assert!(!exported.contains("api_key"));
        assert!(exported.contains("base_url = \"http://192.168.1.20:8000/v1\""));
        // The live file is untouched.
        assert_eq!(fx.read(), example);
    }

    #[test]
    fn strips_keys_however_they_are_spelled_and_from_a_file_that_does_not_parse() {
        let inline = "providers = { groq = { api_key = \"secret:groq\", model = \"m\" } }\n[audio]\nsounds = true\n";
        assert_eq!(strip_api_keys(inline), "providers = { groq = { model = \"m\" } }\n[audio]\nsounds = true\n");
        let dotted = "[providers]\ngroq.api_key = \"secret:groq\"\ngroq.model = \"m\"\n";
        assert_eq!(strip_api_keys(dotted), "[providers]\ngroq.model = \"m\"\n");
        let broken = "[providers.groq]\napi_key = \"secret:groq\"\nmodel = = \"m\"\n";
        assert_eq!(strip_api_keys(broken), "[providers.groq]\nmodel = = \"m\"\n");
        // A snippet that happens to mention api_key is not a provider key.
        let other = "[[snippets]]\ntrigger = \"k\"\ntext = \"api_key = 1\"\n";
        assert_eq!(strip_api_keys(other), other);
    }

    #[test]
    fn imports_a_file_backs_up_the_current_one_replaces_it_drops_key_references() {
        let mut fx = Fixture::new();
        let example = example();
        fx.seed(&example);
        let opened = fx.open_default();
        opened.store.load();
        opened.take();

        let incoming = fx.path("incoming.toml");
        std::fs::write(
            &incoming,
            "# shared by a friend\nversion = 1\n\n[audio]\nsounds = false\n\n[providers.groq]\napi_key = \"secret:groq\"\nmodel = \"whisper\"\n",
        )
        .unwrap();
        let snapshot = opened.store.import_from(&incoming).unwrap();

        assert_eq!(
            fx.read(),
            "# shared by a friend\nversion = 1\n\n[audio]\nsounds = false\n\n[providers.groq]\nmodel = \"whisper\"\n"
        );
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(snapshot.config.providers.len(), 1);
        assert_eq!(
            snapshot.config.providers.get("groq"),
            Some(&ProviderConfig { model: Some("whisper".into()), ..ProviderConfig::default() })
        );
        assert_eq!(snapshot.config.profiles, vec![]);
        assert_eq!(snapshot.issues, vec![]);
        assert_eq!(opened.take(), vec![snapshot]);

        let pattern = regex::Regex::new(r"^config\.toml\.bak-\d{8}-\d{6}$").unwrap();
        let backups: Vec<String> = fx.flow_dir_entries().into_iter().filter(|name| pattern.is_match(name)).collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(fx.dir.path().join("Flow").join(&backups[0])).unwrap(), example);
    }

    #[test]
    fn round_trips_an_export_through_import() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let target = fx.path("export.toml");
        opened.store.export_to(&target).unwrap();
        opened.store.set(&[set(path!["audio", "sounds"], json!(false))]).unwrap();
        let snapshot = opened.store.import_from(&target).unwrap();
        assert!(snapshot.config.audio.sounds);
        let providers: Vec<(&str, &ProviderConfig)> =
            snapshot.config.providers.iter().map(|(id, provider)| (id.as_str(), provider)).collect();
        assert_eq!(
            providers,
            vec![
                ("groq", &ProviderConfig::default()),
                (
                    "custom-lan",
                    &ProviderConfig { base_url: Some("http://192.168.1.20:8000/v1".into()), ..ProviderConfig::default() }
                ),
            ]
        );
        assert_eq!(snapshot.config.profiles.len(), 2);
    }

    #[test]
    fn rejects_a_file_that_is_not_valid_toml_and_changes_nothing() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let incoming = fx.path("incoming.toml");
        std::fs::write(&incoming, "version = 1\n[audio\nsounds = false\n").unwrap();
        let error = opened.store.import_from(&incoming).unwrap_err();
        assert!(error.to_string().contains("not valid TOML (line 2)"), "{error}");
        assert_eq!(fx.read(), example());
        assert_eq!(fx.flow_dir_entries(), vec!["config.toml"]);
        assert!(opened.store.get().audio.sounds);
    }

    #[test]
    fn rejects_a_toml_file_that_is_not_a_flow_config_and_a_missing_file() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let incoming = fx.path("Cargo.toml");
        std::fs::write(&incoming, "[package]\nname = \"x\"\n").unwrap();
        let error = opened.store.import_from(&incoming).unwrap_err();
        assert!(error.to_string().contains("does not look like a Flow config"), "{error}");
        assert!(opened.store.import_from(&fx.path("nope.toml")).is_err());
        assert_eq!(fx.read(), example());
    }

    #[test]
    fn imports_a_file_with_invalid_values_and_reports_them() {
        let mut fx = Fixture::new();
        fx.seed(&example());
        let opened = fx.open_default();
        opened.store.load();
        let incoming = fx.path("incoming.toml");
        std::fs::write(&incoming, "[audio]\nsounds = false\nmax_recording_seconds = 1\n").unwrap();
        let snapshot = opened.store.import_from(&incoming).unwrap();
        assert!(!snapshot.config.audio.sounds);
        assert_eq!(snapshot.config.audio.max_recording_seconds, 300);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].path, path!["audio", "max_recording_seconds"]);
        assert_eq!(snapshot.issues[0].line, Some(3));
    }

    #[test]
    fn migrates_an_older_imported_file() {
        fn history_on(mut raw: PlainObject) -> Result<PlainObject, String> {
            raw.insert("history".into(), json!({ "enabled": true }));
            Ok(raw)
        }
        let mut fx = Fixture::new();
        fx.seed("version = 2\n");
        let opened = fx.open(ConfigStoreOptions {
            migrations: Some(vec![Migration { from: 1, to: 2, migrate: history_on }]),
            version: Some(2),
            debounce: None,
        });
        opened.store.load();
        let incoming = fx.path("old.toml");
        std::fs::write(&incoming, "version = 1\n\n[audio]\nsounds = false\n").unwrap();
        let snapshot = opened.store.import_from(&incoming).unwrap();
        assert_eq!(snapshot.config.version, 2);
        assert!(snapshot.config.history.enabled);
        assert_eq!(fx.read(), "version = 2\n\n[audio]\nsounds = false\n\n[history]\nenabled = true\n");
        assert_eq!(std::fs::read_to_string(fx.sibling(".bak-v1")).unwrap(), "version = 1\n\n[audio]\nsounds = false\n");
    }
}
