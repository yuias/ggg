use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

// Global config directory override (for --config flag and tests)
static CONFIG_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Set config directory override (used by --config flag and tests)
pub fn set_config_dir_override(path: Option<PathBuf>) {
    let mut override_path = CONFIG_DIR_OVERRIDE.write().unwrap();
    *override_path = path;
}

/// Get current config directory override
pub fn get_config_dir_override() -> Option<PathBuf> {
    CONFIG_DIR_OVERRIDE.read().unwrap().clone()
}

/// Find config directory by searching in priority order:
/// 1. Override from --config flag or set_config_dir_override() (highest priority)
/// 2. Environment variable GGG_CONFIG_DIR
/// 3. Executable directory (`<exe_dir>/config/`)
/// 4. XDG-style home config directory (`~/.config/ggg/` on every platform)
/// 5. Platform user config directory (`%APPDATA%\ggg\` on Windows, `~/.config/ggg` on Unix)
///
/// If no config directory is found, creates one in the XDG-style home config directory.
///
/// Test binaries without an override or `GGG_CONFIG_DIR` stop before step 3 and
/// get a per-process scratch directory instead.
pub fn find_config_directory() -> Result<PathBuf> {
    // Priority 1: Override from --config flag or tests
    if let Some(override_path) = get_config_dir_override() {
        if override_path.exists() || std::env::var("GGG_TEST_MODE").is_ok() {
            tracing::debug!("Using config directory override: {:?}", override_path);
            return Ok(override_path);
        }
        tracing::warn!("Config directory override does not exist: {:?}", override_path);
    }

    // Priority 2: Environment variable
    if let Ok(env_path) = std::env::var("GGG_CONFIG_DIR") {
        let env_config = PathBuf::from(env_path);
        if env_config.exists() {
            tracing::debug!("Found config directory from GGG_CONFIG_DIR: {:?}", env_config);
            return Ok(env_config);
        }
    }

    // A test without an override would otherwise read and overwrite the
    // developer's real settings, queues and history (and `Config::save` deletes
    // folder directories it does not know). Redirecting instead of panicking
    // keeps parallel tests stable while a serial test clears the override.
    if is_test_binary() {
        return test_fallback_config_dir();
    }

    find_installed_config_directory()
}

/// Priorities 3-5 of `find_config_directory`: directories tied to the
/// installation and the user account, created in the home config directory
/// when none exists.
fn find_installed_config_directory() -> Result<PathBuf> {
    // Priority 3: Executable directory (portable install alongside the binary)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let exe_config = exe_dir.join("config");
            if exe_config.exists() {
                tracing::debug!("Found config directory at: {:?}", exe_config);
                return Ok(exe_config);
            }
        }
    }

    // Priority 4: XDG-style home config directory (`~/.config/ggg`)
    if let Ok(xdg_config) = get_xdg_config_dir() {
        if xdg_config.exists() {
            tracing::debug!("Found config directory at: {:?}", xdg_config);
            return Ok(xdg_config);
        }
    }

    // Priority 5: Platform user config directory (`%APPDATA%\ggg` on Windows)
    if let Ok(user_config) = get_user_config_dir() {
        if user_config.exists() {
            tracing::debug!("Found config directory at: {:?}", user_config);
            return Ok(user_config);
        }
    }

    // Fallback: Create in the XDG-style home config directory
    let xdg_config = get_xdg_config_dir()?;
    std::fs::create_dir_all(&xdg_config)
        .context("Failed to create home config directory")?;
    tracing::info!("Created config directory at: {:?}", xdg_config);
    Ok(xdg_config)
}

/// Get platform-specific user config directory
/// - Windows: `%APPDATA%\ggg`
/// - Unix: `~/.config/ggg`
fn get_user_config_dir() -> Result<PathBuf> {
    let base_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine user config directory"))?;
    Ok(base_dir.join("ggg"))
}

/// Get the XDG-style home config directory (`~/.config/ggg`) on every platform.
///
/// On Unix this matches `dirs::config_dir()`, but on Windows it deliberately
/// resolves to `%USERPROFILE%\.config\ggg` rather than `%APPDATA%\ggg`, giving
/// a Linux-like location that is preferred over the Roaming AppData directory.
fn get_xdg_config_dir() -> Result<PathBuf> {
    let home_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    Ok(home_dir.join(".config").join("ggg"))
}

/// Get absolute path to settings.toml (application-level)
pub fn get_app_config_path() -> Result<PathBuf> {
    let config_dir = find_config_directory()?;
    Ok(config_dir.join("settings.toml"))
}

/// Get absolute path to folder-specific settings.toml
pub fn get_folder_config_path(folder_name: &str) -> Result<PathBuf> {
    let config_dir = find_config_directory()?;
    let folder_dir = config_dir.join(folder_name);
    Ok(folder_dir.join("settings.toml"))
}

/// Get absolute path to folder-specific queue.toml
pub fn get_folder_queue_path(folder_id: &str) -> Result<PathBuf> {
    let config_dir = find_config_directory()?;
    let folder_dir = config_dir.join(folder_id);
    Ok(folder_dir.join("queue.toml"))
}

/// Resolve the default download directory at runtime.
///
/// Resolution order:
/// 1. Platform user Downloads directory (`dirs::download_dir()`)
/// 2. Current working directory + "Downloads"
/// 3. Executable directory + "Downloads"
/// 4. Fallback: relative "Downloads"
///
/// The platform directory is preferred so downloads land in a predictable
/// place even when the process (GUI/daemon) is launched from an arbitrary cwd.
pub fn resolve_default_download_directory() -> PathBuf {
    if let Some(downloads) = dirs::download_dir() {
        return downloads;
    }
    if let Ok(cwd) = std::env::current_dir() {
        return cwd.join("Downloads");
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            return exe_dir.join("Downloads");
        }
    }
    PathBuf::from("Downloads")
}

/// Resolve the default scripts directory at runtime.
///
/// Uses the config directory as the base, falling back to relative "./scripts".
pub fn resolve_default_scripts_directory() -> PathBuf {
    match find_config_directory() {
        Ok(config_dir) => config_dir.join("scripts"),
        Err(_) => PathBuf::from("./scripts"),
    }
}

/// Resolve a relative path against the config directory.
///
/// If the path is already absolute, it is returned as-is.
/// If relative, it is joined to `find_config_directory()`.
/// Falls back to the original path if the config directory cannot be determined.
pub fn resolve_relative_to_config(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match find_config_directory() {
        Ok(config_dir) => config_dir.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// Get the platform-specific data directory for locale overrides.
///
/// Returns `<data_dir>/ggg/locales` where `<data_dir>` is:
/// - Linux: `$XDG_DATA_HOME` or `~/.local/share`
/// - macOS: `~/Library/Application Support`
/// - Windows: `%APPDATA%`
pub fn get_locale_data_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine user data directory"))?;
    Ok(data_dir.join("ggg").join("locales"))
}

/// Unit tests set `cfg(test)`; integration-test and doctest binaries do not,
/// but cargo builds all of them into `target/<profile>/deps/`, where a real
/// `ggg` binary never runs from.
fn is_test_binary() -> bool {
    cfg!(test)
        || std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().and_then(|p| p.file_name()).map(|n| n == "deps"))
            .unwrap_or(false)
}

/// One scratch directory per test process, never the real config directory.
fn test_fallback_config_dir() -> Result<PathBuf> {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        std::env::temp_dir().join(format!("ggg-test-config-{}", std::process::id()))
    });
    std::fs::create_dir_all(dir).context("Failed to create test config directory")?;
    Ok(dir.clone())
}

/// Get absolute path to application-wide logs directory
pub fn get_logs_dir() -> Result<PathBuf> {
    let config_dir = find_config_directory()?;
    Ok(config_dir.join(".logs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    // Helper function to ensure clean test state
    fn reset_test_state() {
        set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };
        unsafe { std::env::remove_var("GGG_CONFIG_DIR") };
    }

    #[test]
    #[serial]
    fn test_get_app_config_path() {
        reset_test_state();
        // Create temporary config directory for test
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        // Set override for test
        set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let path = get_app_config_path().unwrap();
        assert_eq!(path, config_dir.join("settings.toml"));

        // Clean up
        set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };
    }

    #[test]
    #[serial]
    fn test_get_folder_config_path() {
        reset_test_state();
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let path = get_folder_config_path("test_folder").unwrap();
        assert_eq!(path, config_dir.join("test_folder").join("settings.toml"));
    }

    #[test]
    #[serial]
    fn test_get_folder_config_path_with_special_chars() {
        reset_test_state();
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let path = get_folder_config_path("folder-with-dash").unwrap();
        assert!(path.to_str().unwrap().contains("folder-with-dash"));
    }

    #[test]
    #[serial]
    fn test_find_config_directory_creates_if_missing() {
        reset_test_state();

        // This test verifies that find_config_directory() returns a valid path
        let config_dir = find_config_directory().unwrap();
        assert!(config_dir.ends_with("config") || config_dir.to_str().unwrap().contains("ggg"));
    }

    #[test]
    #[serial]
    fn test_config_dir_override() {
        reset_test_state();
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let found_dir = find_config_directory().unwrap();
        assert_eq!(found_dir, config_dir);
    }

    #[test]
    #[serial]
    fn test_config_dir_from_env_variable() {
        reset_test_state();

        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();
        fs::create_dir_all(&config_dir).unwrap();

        unsafe { std::env::set_var("GGG_CONFIG_DIR", config_dir.to_str().unwrap()) };

        let found_dir = find_config_directory().unwrap();
        assert_eq!(found_dir, config_dir);

        // Keep temp_dir alive
        drop(temp_dir);
    }

    #[test]
    #[serial]
    fn test_find_config_directory_ignores_cwd() {
        reset_test_state();

        // A `./config` in the current working directory must NOT be picked up:
        // cwd was removed from the search order.
        let temp_dir = TempDir::new().unwrap();
        let cwd_config = temp_dir.path().join("config");
        fs::create_dir_all(&cwd_config).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let found_dir = find_config_directory().unwrap();
        assert_ne!(found_dir, cwd_config);

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial]
    fn test_find_config_directory_prefers_xdg_over_appdata() {
        reset_test_state();

        // When the XDG-style home config exists, it wins over the platform
        // user config directory (`%APPDATA%\ggg` on Windows).
        let xdg_config = get_xdg_config_dir().unwrap();
        if xdg_config.exists() {
            // Read-only here: the directory exists, so nothing is created.
            let found_dir = find_installed_config_directory().unwrap();
            assert_eq!(found_dir, xdg_config);
        }
    }

    #[test]
    #[serial]
    fn test_binary_without_override_never_gets_real_config_dir() {
        reset_test_state();
        let found = find_config_directory().unwrap();
        assert_ne!(found, get_xdg_config_dir().unwrap());
        assert_ne!(found, get_user_config_dir().unwrap());
        assert!(found.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn test_get_xdg_config_dir_is_home_dot_config() {
        let xdg_dir = get_xdg_config_dir().unwrap();
        assert!(xdg_dir.ends_with(PathBuf::from(".config").join("ggg")));
    }

    #[test]
    fn test_get_user_config_dir_returns_valid_path() {
        let user_dir = get_user_config_dir().unwrap();

        // Should contain 'ggg' in the path
        assert!(user_dir.to_str().unwrap().contains("ggg"));

        // On Windows, should contain AppData or similar
        #[cfg(windows)]
        {
            let path_str = user_dir.to_str().unwrap();
            assert!(path_str.contains("AppData") || path_str.contains("config"));
        }

        // On Unix, should contain .config
        #[cfg(unix)]
        {
            assert!(user_dir.to_str().unwrap().contains(".config"));
        }
    }

    #[test]
    #[serial]
    fn test_path_normalization() {
        reset_test_state();
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let app_path = get_app_config_path().unwrap();
        let folder_path = get_folder_config_path("test").unwrap();

        assert!(app_path.is_absolute());
        assert!(folder_path.is_absolute());
        assert!(folder_path.components().count() > app_path.components().count());
    }
}
