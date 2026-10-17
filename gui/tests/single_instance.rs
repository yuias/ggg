#![cfg(windows)]
//! Process tests for the single-instance start-up of `ggg-gui`.
//!
//! Each test uses its own temporary config directory and its own unique shared
//! pipe name (`GGG_PIPE`), so it never meets a GUI the user is running. Every
//! wait is bounded and every spawned process is killed on drop. The test URLs
//! point at an unreachable port and folders do not auto-start, so nothing is
//! downloaded.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use ggg::ipc::instance::gui_instance_pipe_name;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(200);

/// A started `ggg-gui` that is killed when dropped, also when a test fails.
struct Gui {
    child: Child,
}

impl Gui {
    fn spawn(config: &Path, pipe: &str, urls: &[&str]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_ggg-gui"))
            .arg("--config")
            .arg(config)
            .args(urls)
            .env("GGG_TEST_MODE", "1")
            .env("GGG_PIPE", pipe)
            .env_remove("GGG_CONFIG_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ggg-gui");
        Self { child }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique_pipe() -> String {
    format!(r"\\.\pipe\ggg-test-{}", uuid::Uuid::new_v4())
}

/// Polls `probe` until it returns true or `timeout` passes.
fn wait_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if probe() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Waits until the primary answers on its instance pipe; fails fast if it exits.
fn wait_for_instance_pipe(gui: &mut Gui, config: &Path) {
    let name = gui_instance_pipe_name(config);
    let ready = wait_until(STARTUP_TIMEOUT, || {
        assert!(gui.running(), "ggg-gui exited before its instance pipe answered");
        ggg_ipc::ping(&name).is_ok()
    });
    assert!(ready, "instance pipe {name} did not answer within {STARTUP_TIMEOUT:?}");
}

/// All log text written under `<config>/.logs` so far.
fn read_logs(config: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(config.join(".logs")) else {
        return String::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect()
}

fn wait_for_log(config: &Path, needle: &str, timeout: Duration) {
    let found = wait_until(timeout, || read_logs(config).contains(needle));
    assert!(found, "log line {needle:?} not found; logs:\n{}", read_logs(config));
}

fn temp_config() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temp config dir")
}

#[test]
fn second_start_forwards_and_exits() {
    let tmp = temp_config();
    let config: PathBuf = tmp.path().to_path_buf();
    let mut primary = Gui::spawn(&config, &unique_pipe(), &[]);
    wait_for_instance_pipe(&mut primary, &config);

    // A different shared pipe: the second start must still find the first one
    // through the per-config-dir instance pipe alone.
    let mut second = Gui::spawn(&config, &unique_pipe(), &["http://127.0.0.1:1/a"]);
    let status = second
        .wait_exit(Duration::from_secs(15))
        .expect("second ggg-gui did not exit within 15 s");
    assert!(status.success(), "second ggg-gui exit status: {status:?}");
    assert!(primary.running(), "the primary must keep running");

    wait_for_log(
        &config,
        "forwarded activation with 1 URL(s)",
        Duration::from_secs(10),
    );
    wait_for_log(
        &config,
        "activation request with 1 URL(s)",
        Duration::from_secs(10),
    );
}

#[test]
fn different_config_dirs_run_side_by_side() {
    let tmp_a = temp_config();
    let tmp_b = temp_config();
    let mut a = Gui::spawn(tmp_a.path(), &unique_pipe(), &[]);
    let mut b = Gui::spawn(tmp_b.path(), &unique_pipe(), &[]);

    wait_for_instance_pipe(&mut a, tmp_a.path());
    wait_for_instance_pipe(&mut b, tmp_b.path());

    assert!(a.running() && b.running(), "both instances must keep running");
    assert_ne!(a.id(), b.id());
}
