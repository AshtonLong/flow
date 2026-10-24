//! End-to-end tests against the built `Flow.exe` in test mode (`FLOW_TEST=1`):
//! the real main process, config file, engine process and model, with the
//! microphone replaced by `tests/fixtures/jfk.wav` and hotkeys sent on stdin.
//! Nothing is pasted and the real keyboard hook is not installed.
//!
//! They need a local model, so they are ignored by default:
//! `cargo test -p flow --test e2e -- --ignored`

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    _user_data: tempfile::TempDir,
}

impl Flow {
    fn launch(config: &str) -> Self {
        let user_data = tempfile::tempdir().unwrap();
        std::fs::write(user_data.path().join("config.toml"), config).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_Flow"))
            .arg("--hidden")
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
        Self { child, stdin, lines, _user_data: user_data }
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
