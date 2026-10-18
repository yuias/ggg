//! Starting the GUI that sits next to the bridge executable.
//!
//! The bridge only ever starts `ggg-gui.exe` from its own directory, with no
//! arguments and no PATH lookup, so a message from the extension cannot choose
//! what gets executed.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const GUI_EXE: &str = "ggg-gui.exe";
/// How long to wait for a freshly started GUI to answer on the link pipe.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(10);
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const ERROR_ACCESS_DENIED: i32 = 5;

/// `exe.parent()/ggg-gui.exe`.
pub fn sibling_gui_path(exe: &Path) -> Option<PathBuf> {
    exe.parent().map(|dir| dir.join(GUI_EXE))
}

/// Calls `probe` every `interval` until it returns true or `wait` elapses.
/// The probe always runs at least once.
pub fn wait_until(
    mut probe: impl FnMut() -> bool,
    wait: Duration,
    interval: Duration,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if probe() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(interval.min(deadline - now));
    }
}

/// Starts the sibling GUI; returns false (and does nothing) if it is missing
/// or cannot be spawned.
pub fn launch_gui() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(gui) = sibling_gui_path(&exe).filter(|p| p.is_file()) else {
        return false;
    };
    let Some(dir) = gui.parent() else {
        return false;
    };

    // Breakaway keeps the GUI alive if the host's job object kills its
    // members when the host exits. Jobs that forbid breakaway reject the flag
    // with ERROR_ACCESS_DENIED; the GUI is then started inside the job.
    match spawn(&gui, dir, CREATE_BREAKAWAY_FROM_JOB) {
        Ok(()) => true,
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => match spawn(&gui, dir, 0) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("bridge: failed to start {GUI_EXE}: {e}");
                false
            }
        },
        Err(e) => {
            eprintln!("bridge: failed to start {GUI_EXE}: {e}");
            false
        }
    }
}

fn spawn(gui: &Path, dir: &Path, creation_flags: u32) -> std::io::Result<()> {
    // The bridge's own stdio is the Native Messaging channel to the browser;
    // a child inheriting it would keep it open and could write into it.
    Command::new(gui)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(creation_flags)
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_path_is_next_to_the_exe() {
        let exe = Path::new("dir").join("ggg-bridge.exe");
        assert_eq!(sibling_gui_path(&exe), Some(Path::new("dir").join(GUI_EXE)));
    }

    #[test]
    fn sibling_path_needs_a_parent() {
        assert_eq!(sibling_gui_path(Path::new("")), None);
    }

    #[test]
    fn wait_until_returns_true_when_probe_turns_true() {
        let mut calls = 0;
        let ready = wait_until(
            || {
                calls += 1;
                calls == 3
            },
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(ready);
        assert_eq!(calls, 3);
    }

    #[test]
    fn wait_until_gives_up_after_the_wait() {
        let start = Instant::now();
        let ready = wait_until(|| false, Duration::from_millis(50), Duration::from_millis(5));
        assert!(!ready);
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
