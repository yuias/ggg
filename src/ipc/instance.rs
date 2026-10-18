//! Pipe name of the GUI single-instance channel.
//!
//! Pure and cross-platform (no `#[cfg(windows)]`), like the control protocol:
//! the name derivation is useful on every target even though the named-pipe
//! transport is Windows-only.

use std::path::Path;

use super::control::protocol::config_dir_key;

pub const GUI_INSTANCE_PIPE_PREFIX: &str = r"\\.\pipe\ggg-gui-";

/// `GUI_INSTANCE_PIPE_PREFIX` + `config_dir_key(config_dir)`. One name per
/// config directory, so GUIs on different `--config` dirs run independently.
pub fn gui_instance_pipe_name(config_dir: &Path) -> String {
    format!("{GUI_INSTANCE_PIPE_PREFIX}{}", config_dir_key(config_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::control::protocol::validate_pipe_name;

    #[test]
    fn name_ignores_case_and_trailing_separator() {
        let plain = gui_instance_pipe_name(Path::new("./some/config/dir"));
        assert_eq!(plain, gui_instance_pipe_name(Path::new("./SOME/CONFIG/DIR")));
        assert_eq!(plain, gui_instance_pipe_name(Path::new("./some/config/dir/")));
    }

    #[test]
    fn name_differs_per_dir() {
        let a = gui_instance_pipe_name(Path::new("./some/config/dir-a"));
        let b = gui_instance_pipe_name(Path::new("./some/config/dir-b"));
        assert_ne!(a, b);
    }

    #[test]
    fn name_is_a_valid_pipe_name() {
        let name = gui_instance_pipe_name(Path::new("./some/config/dir"));
        assert!(name.starts_with(GUI_INSTANCE_PIPE_PREFIX));
        assert!(validate_pipe_name(&name).is_ok(), "{name}");
        assert_eq!(name.len(), GUI_INSTANCE_PIPE_PREFIX.len() + 16);
    }
}
