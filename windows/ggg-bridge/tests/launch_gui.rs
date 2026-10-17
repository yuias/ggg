#![cfg(windows)]
//! Process tests for the bridge starting the GUI that sits next to it.
//!
//! The launched GUI inherits the bridge's environment, so each test gives it a
//! temporary config directory and a unique link pipe (`GGG_PIPE`): it never
//! meets the user's real configuration or a GUI the user is running. Every wait
//! is bounded, and the bridge and the GUI it started are killed at the end,
//! also when a test fails. The test URL points at an unreachable port and
//! folders do not auto-start, so nothing is downloaded.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

fn unique_pipe() -> String {
    format!(r"\\.\pipe\ggg-test-{}", uuid::Uuid::new_v4())
}

fn write_frame(stdin: &mut ChildStdin, json: &str) {
    stdin
        .write_all(&(json.len() as u32).to_le_bytes())
        .and_then(|()| stdin.write_all(json.as_bytes()))
        .and_then(|()| stdin.flush())
        .expect("write frame to bridge");
}

fn read_frame(stdout: &mut ChildStdout) -> std::io::Result<String> {
    let mut len = [0u8; 4];
    stdout.read_exact(&mut len)?;
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    stdout.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// A running bridge plus the GUI it may have started. Dropping it kills both.
struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    pipe: String,
    /// Look for a GUI to kill on drop (it was started or may still be starting).
    expect_gui: bool,
}

impl Bridge {
    fn spawn(exe: &Path, config: &Path, pipe: &str, expect_gui: bool) -> Self {
        let mut child = Command::new(exe)
            .env("GGG_CONFIG_DIR", config)
            .env("GGG_PIPE", pipe)
            .env("GGG_TEST_MODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ggg-bridge");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        Self {
            child,
            stdin,
            stdout,
            pipe: pipe.to_string(),
            expect_gui,
        }
    }

    /// Sends one frame and returns the reply, or `None` after `timeout`.
    fn request(&mut self, json: &str, timeout: Duration) -> Option<String> {
        write_frame(self.stdin.as_mut().expect("stdin open"), json);
        let mut stdout = self.stdout.take().expect("stdout open");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_frame(&mut stdout));
        });
        rx.recv_timeout(timeout).ok().and_then(Result::ok)
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if !self.expect_gui {
            return;
        }
        // The GUI is a grandchild we have no handle for; its link pipe names it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Ok(pid) = ggg_ipc::server_process_id(&self.pipe) {
                break Some(pid);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if let Some(pid) = pid {
            kill_pid(pid);
            // Wait for the pipe to go so the GUI no longer holds the config dir.
            let deadline = Instant::now() + Duration::from_secs(5);
            while ggg_ipc::server_process_id(&self.pipe).is_ok() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn bridge_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ggg-bridge"))
}

#[test]
fn add_url_starts_gui_when_pipe_is_missing() {
    let exe = bridge_exe();
    assert!(
        exe.with_file_name("ggg-gui.exe").is_file(),
        "ggg-gui.exe is missing next to the bridge: build the workspace first (cargo build --workspace)"
    );

    let config = tempfile::tempdir().expect("create temp config dir");
    let pipe = unique_pipe();
    assert!(
        ggg_ipc::server_process_id(&pipe).is_err(),
        "the test pipe must not exist yet"
    );

    let mut bridge = Bridge::spawn(&exe, config.path(), &pipe, true);
    let reply = bridge
        .request(
            r#"{"type":"add_url","url":"http://127.0.0.1:1/b"}"#,
            REPLY_TIMEOUT,
        )
        .expect("no reply from the bridge within 30 s");
    let value: serde_json::Value = serde_json::from_str(&reply).expect("reply is JSON");
    assert_eq!(value["type"], "ok", "unexpected reply: {reply}");

    // The pipe is now served by the GUI the bridge started, not by the bridge.
    let gui_pid = ggg_ipc::server_process_id(&pipe).expect("GUI serves the test pipe");
    assert_ne!(gui_pid, bridge.child.id());

    bridge.close_stdin();
    assert!(
        bridge.wait_exit(Duration::from_secs(10)),
        "the bridge did not exit after stdin closed"
    );
    // The GUI is a separate process and stays up after the bridge exits.
    assert_eq!(ggg_ipc::server_process_id(&pipe).ok(), Some(gui_pid));
}

#[test]
fn bridge_without_sibling_gui_reports_not_running() {
    // A copy of the bridge in an otherwise empty directory has no ggg-gui.exe
    // beside it, so nothing may be started.
    let dir = tempfile::tempdir().expect("create temp dir");
    let exe = dir.path().join("ggg-bridge.exe");
    std::fs::copy(bridge_exe(), &exe).expect("copy ggg-bridge");
    let config = tempfile::tempdir().expect("create temp config dir");
    let pipe = unique_pipe();

    let mut bridge = Bridge::spawn(&exe, config.path(), &pipe, false);
    let reply = bridge
        .request(
            r#"{"type":"add_url","url":"http://127.0.0.1:1/c"}"#,
            Duration::from_secs(15),
        )
        .expect("no reply from the bridge within 15 s");
    let value: serde_json::Value = serde_json::from_str(&reply).expect("reply is JSON");
    assert_eq!(value["type"], "error", "unexpected reply: {reply}");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|m| m.contains("not running")),
        "unexpected reply: {reply}"
    );
    assert!(ggg_ipc::server_process_id(&pipe).is_err());
}
