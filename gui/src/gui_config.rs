//! GUI-only state (window geometry, splitter sizes, column widths) kept in
//! `gui.toml` next to the core configuration.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "gui.toml";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GuiConfig {
    /// Whether a URL with a range pattern added without the dialog is expanded.
    pub expand_ranges_on_direct_add: bool,
    pub main_window: Option<WindowGeometry>,
    pub settings_window: Option<WindowGeometry>,
    pub add_dialog: Option<WindowGeometry>,
    /// Logical px.
    pub tree_width: f32,
    pub details_visible: bool,
    /// Logical px.
    pub details_height: f32,
    pub queue_columns: Vec<ColumnWidth>,
    pub history_columns: Vec<ColumnWidth>,
    pub sort: Option<SortState>,
}

/// Outer position in physical px, inner size in logical px.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColumnWidth {
    pub id: String,
    pub width: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SortState {
    pub column: String,
    pub ascending: bool,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            // Matches the TUI's direct paste/IPC paths and the CLI default.
            expand_ranges_on_direct_add: true,
            main_window: None,
            settings_window: None,
            add_dialog: None,
            tree_width: 200.0,
            details_visible: true,
            details_height: 220.0,
            queue_columns: Vec::new(),
            history_columns: Vec::new(),
            sort: None,
        }
    }
}

impl GuiConfig {
    /// `gui.toml` inside the active config directory.
    pub fn default_path() -> Result<PathBuf> {
        Ok(ggg::util::paths::find_config_directory()?.join(FILE_NAME))
    }

    /// Never fails: GUI state is a convenience and must not block start-up.
    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!("Cannot read {}: {e}; using defaults", path.display());
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!("Invalid {}: {e}; using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Writes a sibling temp file and renames it, so a crash mid-write cannot
    /// leave a truncated `gui.toml`.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serialize GUI config")?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("create {}", dir.display()))?;
        }
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            // Do not leave the temp file behind when the rename fails.
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("replace {}", path.display()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(x: i32) -> WindowGeometry {
        WindowGeometry {
            x,
            y: -20,
            width: 1024,
            height: 640,
            maximized: x % 2 == 0,
        }
    }

    #[test]
    fn roundtrip_preserves_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.toml");
        let config = GuiConfig {
            expand_ranges_on_direct_add: false,
            main_window: Some(geometry(10)),
            settings_window: Some(geometry(-5)),
            add_dialog: Some(geometry(7)),
            tree_width: 187.5,
            details_visible: false,
            details_height: 301.25,
            queue_columns: vec![
                ColumnWidth { id: "name".into(), width: 320.5 },
                ColumnWidth { id: "size".into(), width: 80.0 },
            ],
            history_columns: vec![ColumnWidth { id: "finished".into(), width: 140.0 }],
            sort: Some(SortState { column: "name".into(), ascending: false }),
        };

        config.save_to(&path).unwrap();
        assert_eq!(GuiConfig::load_from(&path), config);
        assert!(!dir.path().join("gui.toml.tmp").exists());

        // Saving again over an existing file must replace it.
        let mut changed = config.clone();
        changed.sort = None;
        changed.save_to(&path).unwrap();
        assert_eq!(GuiConfig::load_from(&path), changed);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = GuiConfig::load_from(&dir.path().join("gui.toml"));
        assert_eq!(config, GuiConfig::default());
        assert!(config.expand_ranges_on_direct_add);
        assert!(config.details_visible);
    }

    #[test]
    fn invalid_toml_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.toml");

        std::fs::write(&path, "this is = = not toml [").unwrap();
        assert_eq!(GuiConfig::load_from(&path), GuiConfig::default());

        // Valid TOML with a wrong field type is also rejected as a whole.
        std::fs::write(&path, "tree_width = \"wide\"\n").unwrap();
        assert_eq!(GuiConfig::load_from(&path), GuiConfig::default());
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.toml");
        std::fs::write(&path, "tree_width = 250.0\n").unwrap();
        let config = GuiConfig::load_from(&path);
        assert_eq!(config.tree_width, 250.0);
        assert!(config.details_visible);
        assert!(config.expand_ranges_on_direct_add);
    }
}
