//! GUI string catalog on top of the core localization manager.

use fluent_bundle::FluentArgs;
use ggg::util::i18n::LocalizationManager;
use std::sync::Arc;

/// Cheap-to-clone handle to the translations for the configured language.
#[derive(Clone)]
pub struct Tr(Arc<LocalizationManager>);

impl Tr {
    /// Loads `language`, falling back to English and then to a placeholder
    /// manager, so a bad locale never keeps the window from opening.
    pub fn load(language: &str) -> Self {
        Self(Arc::new(LocalizationManager::new(language).unwrap_or_else(
            |e| {
                tracing::error!("Failed to load translations for '{language}': {e}");
                tracing::info!("Falling back to English");
                LocalizationManager::new("en").unwrap_or_else(|e2| {
                    tracing::error!("Failed to load fallback English locale: {e2}");
                    LocalizationManager::fallback()
                })
            },
        )))
    }

    pub fn t(&self, key: &str) -> String {
        self.0.get(key)
    }

    /// Localized text for a command failure, shared with the other front ends.
    pub fn command_error(&self, err: &ggg::ui::commands::CommandError) -> String {
        ggg::ui::command_i18n::localize_command_error(&self.0, err)
    }

    pub fn t_args(&self, key: &str, args: &FluentArgs) -> String {
        self.0.get_with_args(key, Some(args))
    }
}
