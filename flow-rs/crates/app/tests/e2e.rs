//! End-to-end tests against the built `Flow.exe` in test mode (`FLOW_TEST=1`):
//! the real main process, config file, engine process and model, with the
//! microphone replaced by `tests/fixtures/jfk.wav` and hotkeys sent on stdin.
//! Nothing is pasted and the real keyboard hook is not installed.
//!
//! They need the staged engine (and some the local model), so they are ignored
//! by default: `cargo test -p flow --test e2e -- --ignored`

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use flow_core::config::ConfigPatch;
use flow_core::path;
use flow_core::protocol::settings::{Request, RequestEnvelope};
use serde_json::{Value, json};

const ONBOARDED: &str = "version = 1\n\n[general]\nonboarded = true\n";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Flow").join("models")
}

struct Flow {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<serde_json::Value>,
    user_data: tempfile::TempDir,
    next_id: u64,
}

impl Flow {
    fn launch(config: &str) -> Self {
        Self::launch_with(config, &["--hidden"])
    }

    fn launch_with(config: &str, args: &[&str]) -> Self {
        Self::launch_env(config, args, &[])
    }

    fn launch_env(config: &str, args: &[&str], env: &[(&str, &str)]) -> Self {
        let user_data = tempfile::tempdir().unwrap();
        std::fs::write(user_data.path().join("config.toml"), config).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_Flow"))
            .args(args)
            .envs(env.iter().copied())
            .env("FLOW_TEST", "1")
            .env("FLOW_USER_DATA", user_data.path())
            .env("FLOW_MODELS_DIR", models_dir())
            .env("FLOW_FAKE_MIC", repo().join("tests/fixtures/jfk.wav"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("launch Flow.exe");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    let _ = tx.send(value);
                }
            }
        });
        Self { child, stdin, lines, user_data, next_id: 1 }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.user_data.path().join(name)
    }

    /// Sends a settings-window request and returns its result (panics on an error reply).
    fn call(&mut self, request: Request) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&RequestEnvelope { id, request }).unwrap();
        self.send(&format!("ipc {line}"));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) if value["type"] == "reply" && value["id"] == id => {
                    let reply = &value["reply"];
                    assert_eq!(reply["status"], "ok", "{reply}");
                    return reply["result"].clone();
                }
                Ok(_) => {}
                Err(_) => panic!("no reply to request {id}"),
            }
        }
    }

    /// True if a line of `kind` arrives within `timeout`.
    fn sees(&self, kind: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) if value["type"] == kind => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    /// Polls `ConfigGet` until `ok` holds, for up to two seconds (file edits hot-reload).
    fn config_until(&mut self, ok: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = self.call(Request::ConfigGet);
            if ok(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "config never matched: {snapshot}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn wait_for(&self, kind: &str, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) if value["type"] == kind => return value,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {kind}"),
            }
        }
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

#[test]
#[ignore = "needs the local Parakeet model and the staged engine"]
fn dictates_jfk_with_the_local_model() {
    let mut flow = Flow::launch(ONBOARDED);
    flow.wait_for("ready", Duration::from_secs(20));
    flow.send("hotkey hold-start");
    std::thread::sleep(Duration::from_millis(400));
    flow.send("hotkey hold-end");
    let insert = flow.wait_for("insert", Duration::from_secs(90));
    let text = insert["text"].as_str().unwrap().to_lowercase();
    assert!(text.contains("ask not what your country can do for you"), "{text}");
    let dictation = flow.wait_for("dictation", Duration::from_secs(10));
    assert_eq!(dictation["event"]["ok"], true);
}

#[test]
#[ignore = "needs the staged engine"]
fn starts_in_the_tray_and_quits_on_request() {
    let flow = Flow::launch(ONBOARDED);
    flow.wait_for("ready", Duration::from_secs(20));
}

#[test]
#[ignore = "needs the staged engine; opens the settings window on screen"]
fn first_run_opens_the_settings_window() {
    let flow = Flow::launch_with("", &[]);
    assert!(flow.sees("settings", Duration::from_secs(20)), "the settings window did not open");
}

#[test]
#[ignore = "needs the staged engine"]
fn starts_in_the_tray_with_no_window_once_set_up() {
    let flow = Flow::launch_with(ONBOARDED, &[]);
    flow.wait_for("ready", Duration::from_secs(20));
    assert!(!flow.sees("settings", Duration::from_secs(2)), "a window opened");
}

#[test]
#[ignore = "needs the staged engine"]
fn settings_edits_patch_config_toml_in_place_and_file_edits_hot_reload() {
    let config = "# my settings\nversion = 1\n\n[general]\nonboarded = true\n\n[audio]\nsounds = true   # the beeps\n";
    let mut flow = Flow::launch(config);
    flow.wait_for("ready", Duration::from_secs(20));
    let file = flow.file("config.toml");

    // UI → file: only the value changes; comments and layout survive.
    flow.call(Request::ConfigSet(vec![ConfigPatch::set(path!["audio", "sounds"], false)]));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("sounds = false   # the beeps"), "{text}");
    assert!(text.contains("# my settings"), "{text}");

    // File → app: an edit in a text editor applies within a second, with no restart.
    std::fs::write(&file, text.replace("sounds = false", "sounds = true")).unwrap();
    flow.config_until(|s| s["config"]["audio"]["sounds"] == true);

    // A bad value falls back to its default and is reported with its line.
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[overlay]\nstyle = \"banner\"\n");
    std::fs::write(&file, text).unwrap();
    let snapshot = flow.config_until(|s| s["issues"].as_array().is_some_and(|i| !i.is_empty()));
    assert_eq!(snapshot["config"]["overlay"]["style"], "pill");
    assert_eq!(snapshot["issues"][0]["path"], json!(["overlay", "style"]));
    assert_eq!(snapshot["issues"][0]["line"], 11);
}

#[test]
#[ignore = "needs the staged engine"]
fn api_keys_are_encrypted_and_never_written_to_the_config_file() {
    let mut flow = Flow::launch(ONBOARDED);
    flow.wait_for("ready", Duration::from_secs(20));
    let key = "gsk_test_not_a_real_key_123456";
    flow.call(Request::SecretsSet { provider: "groq".into(), key: key.into() });
    assert_eq!(flow.call(Request::SecretsHas("groq".into())), true);

    let config = std::fs::read_to_string(flow.file("config.toml")).unwrap();
    assert!(config.contains("api_key = \"secret:groq\""), "{config}");
    assert!(!config.contains(key));
    let secrets = std::fs::read(flow.file("secrets.bin")).unwrap();
    assert!(!secrets.windows(key.len()).any(|w| w == key.as_bytes()));

    let catalog = flow.call(Request::ModelsList);
    assert_eq!(catalog["keys"]["groq"], true);
    assert_eq!(catalog["status"]["groq/whisper-large-v3-turbo"]["ready"], true);
    assert!(!catalog.to_string().contains(key));

    flow.call(Request::SecretsDelete("groq".into()));
    assert_eq!(flow.call(Request::SecretsHas("groq".into())), false);
}

fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if &bytes[offset..offset + 4] == b"data" {
            let data = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
            return data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
        }
        offset += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}

#[test]
#[ignore = "needs the local Parakeet model and the staged engine"]
fn transcribes_a_recorded_phrase_and_runs_the_test_bench() {
    let mut flow = Flow::launch(ONBOARDED);
    flow.wait_for("ready", Duration::from_secs(20));
    let samples = read_wav(&repo().join("tests/fixtures/jfk.wav"));

    let transcript = flow.call(Request::DictationTest(flow_core::protocol::Pcm(samples.clone())));
    assert_eq!(transcript["model"], "parakeet-tdt-0.6b-v2");
    let text = transcript["text"].as_str().unwrap().to_lowercase();
    assert!(text.contains("ask not what your country can do for you"), "{text}");
    // One second for a 10 s clip once the model is loaded; a cold load is included here.
    assert!(transcript["elapsedMs"].as_f64().unwrap() < 15_000.0, "{transcript}");

    let info = flow.call(Request::AppInfo);
    assert!(!info["devices"].as_array().unwrap().is_empty(), "{info}");
    assert_eq!(info["degraded"], json!([]));

    // The test bench runs the same phrase through chosen models side by side.
    let run = flow.call(Request::BenchRun {
        audio: flow_core::protocol::Pcm(samples),
        model_ids: vec!["parakeet-tdt-0.6b-v2".into()],
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let result = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = flow.lines.recv_timeout(left).expect("no bench result");
        let event = &line["event"];
        if line["type"] == "event"
            && event["event"] == "BenchResult"
            && event["payload"]["runId"] == run
            && event["payload"]["done"] == true
        {
            break event["payload"].clone();
        }
    };
    assert_eq!(result["ok"], true, "{result}");
    assert!(result["text"].as_str().unwrap().to_lowercase().contains("fellow americans"), "{result}");
    eprintln!("warm Parakeet: {} ms for an 11 s clip", result["elapsedMs"]);
}

/// A window with a text box, owned by the test, for the real inserter to type into.
mod target {
    use std::sync::mpsc;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, ES_AUTOHSCROLL, GetForegroundWindow, GetMessageW, MSG, SW_SHOW,
        SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
        WM_GETTEXT, WM_GETTEXTLENGTH, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };
    use windows::core::w;

    pub struct Target {
        top: isize,
        edit: isize,
    }

    impl Target {
        pub fn open() -> Self {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                // The built-in STATIC class works as a plain parent window.
                let top = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Flow e2e target"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    200,
                    200,
                    520,
                    140,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                let edit = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("EDIT"),
                    w!(""),
                    WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    10,
                    10,
                    480,
                    28,
                    Some(top),
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                let _ = ShowWindow(top, SW_SHOW);
                let _ = SetForegroundWindow(top);
                let _ = SetFocus(Some(edit));
                tx.send((top.0 as isize, edit.0 as isize)).unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            });
            let (top, edit) = rx.recv().unwrap();
            Self { top, edit }
        }

        /// True while this window is in the foreground, so a paste lands here.
        pub fn has_focus(&self) -> bool {
            unsafe { GetForegroundWindow().0 as isize == self.top }
        }

        pub fn text(&self) -> String {
            unsafe {
                let edit = HWND(self.edit as *mut _);
                let len = SendMessageW(edit, WM_GETTEXTLENGTH, None, None).0 as usize;
                let mut buf = vec![0u16; len + 1];
                let got =
                    SendMessageW(edit, WM_GETTEXT, Some(WPARAM(buf.len())), Some(LPARAM(buf.as_mut_ptr() as isize)));
                String::from_utf16_lossy(&buf[..got.0 as usize])
            }
        }
    }

    impl Drop for Target {
        fn drop(&mut self) {
            unsafe {
                let _ = SendMessageW(HWND(self.top as *mut _), WM_CLOSE, None, None);
            }
        }
    }
}

#[test]
#[ignore = "needs the local Parakeet model and the staged engine; types into a window of its own"]
fn hold_to_talk_types_the_transcript_into_the_focused_field() {
    let config = format!("{ONBOARDED}\n[history]\nenabled = true\n");
    let mut flow = Flow::launch_env(&config, &["--hidden"], &[("FLOW_TEST_REAL_INSERT", "1")]);
    flow.wait_for("ready", Duration::from_secs(20));
    let target = target::Target::open();
    std::thread::sleep(Duration::from_millis(300));
    // The paste goes to whatever window has focus, so make sure it is ours.
    if !target.has_focus() {
        eprintln!("skipped: another window has focus; not pasting into it");
        return;
    }
    flow.send("hotkey hold-start");
    std::thread::sleep(Duration::from_millis(1500));
    if !target.has_focus() {
        flow.send("hotkey cancel");
        eprintln!("skipped: focus moved to another window during the recording");
        return;
    }
    flow.send("hotkey hold-end");
    flow.wait_for("insert", Duration::from_secs(60));

    // Text lands at the cursor, with the trailing space.
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let text = target.text();
        if text.to_lowercase().contains("ask not what your country can do for you") || Instant::now() > deadline {
            break text;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(text.to_lowercase().contains("ask not what your country can do for you"), "{text:?}");
    assert!(text.ends_with(' '), "{text:?}");

    // History is on in this config, so the dictation is recorded locally.
    let deadline = Instant::now() + Duration::from_secs(5);
    let found = loop {
        let found = flow.call(Request::HistorySearch { query: "fellow".into(), limit: 10, offset: 0 });
        if !found.as_array().unwrap().is_empty() || Instant::now() > deadline {
            break found;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    let entry = &found[0];
    assert_eq!(entry["model"], "parakeet-tdt-0.6b-v2");
    eprintln!("key release to text: {} ms for {} ms of audio", entry["elapsedMs"], entry["audioMs"]);
}
