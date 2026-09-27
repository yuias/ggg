use fluent::FluentResource;
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::FluentArgs;
use rust_embed::RustEmbed;
use std::path::Path;
use std::sync::Arc;
use unic_langid::LanguageIdentifier;

/// Embedded locale files shipped with the binary.
///
/// Currently includes only en-US as the built-in fallback.
/// To embed additional locales, add their directory names to the
/// `#[include = "..."]` list (e.g. `#[include = "ja-JP/*"]`).
#[derive(RustEmbed)]
#[folder = "locales/"]
#[include = "en-US/*"]
struct EmbeddedLocales;

/// Manages localization resources and provides translation API.
///
/// Resource resolution order (per locale):
/// 1. External override directory (XDG data dir / platform equivalent)
/// 2. Embedded resources compiled into the binary
///
/// Override granularity is per-locale: if the external directory contains
/// a locale folder (e.g. `ja-JP/`), all .ftl files for that locale are
/// loaded from the external directory and the embedded resources are
/// ignored for that locale.
pub struct LocalizationManager {
    bundle: FluentBundle<Arc<FluentResource>>,
    fallback_bundle: Option<FluentBundle<Arc<FluentResource>>>,
    current_locale: String,
}

impl LocalizationManager {
    /// Create a new LocalizationManager for the specified locale
    ///
    /// # Arguments
    /// * `locale` - Language code ("en" or "ja")
    ///
    /// # Returns
    /// * `Ok(LocalizationManager)` on success
    /// * `Err` if locale files cannot be loaded
    pub fn new(locale: &str) -> anyhow::Result<Self> {
        // Map short codes to full locale identifiers
        let locale_lower = locale.to_lowercase();
        let locale_id = match locale_lower.as_str() {
            "en" => "en-US",
            "ja" => "ja-JP",
            other => other,
        };

        tracing::info!("Loading translations for locale: {}", locale_id);

        // Load the requested locale
        let locale_id_owned = locale_id.to_string();
        let bundle = Self::load_locale_bundle(&locale_id_owned)?;

        // Load fallback locale (en-US) if not already loaded
        let fallback_bundle = if locale_id != "en-US" {
            match Self::load_locale_bundle("en-US") {
                Ok(fallback) => {
                    tracing::debug!("Loaded fallback locale: en-US");
                    Some(fallback)
                }
                Err(e) => {
                    tracing::warn!("Failed to load fallback locale en-US: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            bundle,
            fallback_bundle,
            current_locale: locale.to_string(),
        })
    }

    /// Build a no-op manager with no resources. Every key resolves to a
    /// `[missing: key]` placeholder. Used as an absolute last resort so the app
    /// can start even if no locale assets load at all, instead of panicking.
    pub fn fallback() -> Self {
        let lang_id: LanguageIdentifier = "en-US".parse().unwrap_or_default();
        let mut bundle = FluentBundle::new_concurrent(vec![lang_id]);
        bundle.set_use_isolating(false);
        Self {
            bundle,
            fallback_bundle: None,
            current_locale: "en-US".to_string(),
        }
    }

    /// Load all .ftl files for a locale into a FluentBundle.
    ///
    /// Tries the external override directory first. If it contains a
    /// matching locale folder, loads exclusively from there. Otherwise
    /// falls back to embedded resources.
    fn load_locale_bundle(
        locale_id: &str,
    ) -> anyhow::Result<FluentBundle<Arc<FluentResource>>> {
        let lang_id: LanguageIdentifier = locale_id
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid locale ID '{}': {:?}", locale_id, e))?;

        let mut bundle = FluentBundle::new_concurrent(vec![lang_id]);
        // Disable Unicode bidi isolation (FSI/PDI, U+2068/U+2069) around
        // interpolated arguments. Those control characters render as stray
        // glyphs in the CLI and the Win32 GUI's DrawTextW output.
        bundle.set_use_isolating(false);

        let resources = Self::load_from_external(locale_id)
            .unwrap_or_else(|| Self::load_from_embedded(locale_id));

        if resources.is_empty() {
            anyhow::bail!(
                "No .ftl files found for locale '{}'",
                locale_id
            );
        }

        for resource in resources {
            if let Err(errors) = bundle.add_resource(resource) {
                for error in errors {
                    tracing::error!("Failed to add resource to bundle: {:?}", error);
                }
            }
        }

        tracing::debug!("Loaded locale bundle for {}", locale_id);
        Ok(bundle)
    }

    /// Attempt to load locale files from the external override directory.
    ///
    /// Returns `Some(resources)` if the locale directory exists and
    /// contains at least one .ftl file, `None` otherwise.
    fn load_from_external(locale_id: &str) -> Option<Vec<Arc<FluentResource>>> {
        let locale_dir = match super::paths::get_locale_data_dir() {
            Ok(dir) => dir.join(locale_id),
            Err(_) => return None,
        };

        if !locale_dir.is_dir() {
            return None;
        }

        match Self::load_ftl_files_from_dir(&locale_dir) {
            Ok(resources) if !resources.is_empty() => {
                tracing::info!(
                    "Using external locale override: {}",
                    locale_dir.display()
                );
                Some(resources)
            }
            Ok(_) => {
                tracing::debug!(
                    "External locale dir exists but contains no .ftl files: {}",
                    locale_dir.display()
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to read external locale dir {}: {}",
                    locale_dir.display(),
                    e
                );
                None
            }
        }
    }

    /// Load locale files from embedded resources.
    fn load_from_embedded(locale_id: &str) -> Vec<Arc<FluentResource>> {
        let prefix = format!("{}/", locale_id);
        let mut resources = Vec::new();

        // Collect and sort filenames for deterministic load order
        let mut files: Vec<_> = EmbeddedLocales::iter()
            .filter(|path| path.starts_with(&prefix) && path.ends_with(".ftl"))
            .collect();
        files.sort();

        for file_path in files {
            if let Some(file) = EmbeddedLocales::get(&file_path) {
                match std::str::from_utf8(file.data.as_ref()) {
                    Ok(content) => match FluentResource::try_new(content.to_string()) {
                        Ok(resource) => {
                            tracing::debug!("Loaded embedded translation: {}", file_path);
                            resources.push(Arc::new(resource));
                        }
                        Err((_, errors)) => {
                            tracing::error!(
                                "Failed to parse embedded {}: {:?}",
                                file_path,
                                errors
                            );
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "Embedded file {} is not valid UTF-8: {}",
                            file_path,
                            e
                        );
                    }
                }
            }
        }

        if resources.is_empty() {
            tracing::debug!("No embedded translations for locale: {}", locale_id);
        } else {
            tracing::info!(
                "Loaded {} embedded translation file(s) for {}",
                resources.len(),
                locale_id
            );
        }

        resources
    }

    /// Load all .ftl files from a filesystem directory
    fn load_ftl_files_from_dir(dir: &Path) -> anyhow::Result<Vec<Arc<FluentResource>>> {
        let mut resources = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    == Some("ftl")
            })
            .collect();

        // Sort entries alphabetically for consistent loading order
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            let content = std::fs::read_to_string(&path)?;
            match FluentResource::try_new(content) {
                Ok(resource) => {
                    tracing::debug!("Loaded translation file: {}", path.display());
                    resources.push(Arc::new(resource));
                }
                Err((_, errors)) => {
                    tracing::error!(
                        "Failed to parse {}: {:?}",
                        path.display(),
                        errors
                    );
                }
            }
        }

        Ok(resources)
    }

    /// Get a translated string by key
    ///
    /// # Arguments
    /// * `key` - Translation key (e.g., "help-title")
    ///
    /// # Returns
    /// * Translated string, or fallback string if key not found
    pub fn get(&self, key: &str) -> String {
        self.get_with_args(key, None)
    }

    /// Get a translated string with arguments
    ///
    /// # Arguments
    /// * `key` - Translation key
    /// * `args` - Optional arguments for parameterized translations
    ///
    /// # Returns
    /// * Translated string with substituted arguments
    pub fn get_with_args(&self, key: &str, args: Option<&FluentArgs>) -> String {
        // Try current locale first
        if let Some(message) = self.bundle.get_message(key) {
            if let Some(pattern) = message.value() {
                let mut errors = vec![];
                let value = self.bundle.format_pattern(pattern, args, &mut errors);

                if !errors.is_empty() {
                    tracing::warn!(
                        "Translation errors for key '{}': {:?}",
                        key,
                        errors
                    );
                }

                return value.to_string();
            }
        }

        // Try fallback locale
        if let Some(fallback) = &self.fallback_bundle {
            if let Some(message) = fallback.get_message(key) {
                if let Some(pattern) = message.value() {
                    let mut errors = vec![];
                    let value = fallback.format_pattern(pattern, args, &mut errors);

                    if !errors.is_empty() {
                        tracing::warn!(
                            "Translation errors for fallback key '{}': {:?}",
                            key,
                            errors
                        );
                    }

                    tracing::debug!(
                        "Using fallback translation for key: {}",
                        key
                    );
                    return value.to_string();
                }
            }
        }

        // No translation found
        tracing::warn!("Missing translation key: {}", key);
        format!("[missing: {}]", key)
    }

    /// Get the current locale code
    pub fn current_locale(&self) -> &str {
        &self.current_locale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_en_us_loads() {
        // en-US is embedded — should always succeed regardless of filesystem
        let manager = LocalizationManager::new("en").expect("Failed to load embedded en-US");
        assert_eq!(manager.current_locale(), "en");
    }

    #[test]
    fn test_embedded_en_us_has_translations() {
        let manager = LocalizationManager::new("en").expect("Failed to load embedded en-US");
        let title = manager.get("app-title");
        assert!(!title.contains("[missing:"), "app-title should be present in embedded en-US");
    }

    #[test]
    fn test_missing_key_returns_placeholder() {
        let manager = LocalizationManager::new("en").expect("Failed to load locale");
        let result = manager.get("nonexistent-key-that-should-not-exist");
        assert!(result.contains("[missing:"));
    }

    #[test]
    fn test_locale_mapping() {
        // "en" maps to "en-US", "ja" maps to "ja-JP"
        let manager = LocalizationManager::new("en").expect("Failed to load en locale");
        assert_eq!(manager.current_locale(), "en");
    }

    #[test]
    fn test_embedded_locales_contain_en_us() {
        // Verify that the rust-embed asset list includes en-US files
        let en_files: Vec<_> = EmbeddedLocales::iter()
            .filter(|p| p.starts_with("en-US/"))
            .collect();
        assert!(!en_files.is_empty(), "en-US should be embedded");
    }
}
