use super::config::Config;
use crate::script::{executor, message::ScriptRequest};
use crate::util::i18n::LocalizationManager;
use anyhow::Result;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    /// Shared internationalization manager
    pub i18n: Arc<LocalizationManager>,
    /// Channel sender for script execution requests
    ///
    /// When Some, scripts are enabled and requests are sent to the executor thread.
    /// The executor thread runs in a separate OS thread with its own ScriptManager.
    pub script_sender: Option<mpsc::Sender<ScriptRequest>>,
}

impl AppState {
    /// Create LocalizationManager with fallback to English
    fn create_i18n(language: &str) -> Arc<LocalizationManager> {
        Arc::new(
            LocalizationManager::new(language).unwrap_or_else(|e| {
                tracing::error!("Failed to load translations for '{}': {}", language, e);
                tracing::info!("Falling back to English");
                LocalizationManager::new("en").unwrap_or_else(|e2| {
                    // Bundled English should always load; if it somehow can't,
                    // start with a placeholder manager rather than panicking.
                    tracing::error!("Failed to load fallback English locale: {}", e2);
                    LocalizationManager::fallback()
                })
            }),
        )
    }

    pub fn new(config: Config, language: &str) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            i18n: Self::create_i18n(language),
            script_sender: None,
        }
    }

    pub async fn new_with_scripts(config: Config, language: &str) -> Result<Self> {
        // Spawn script executor thread if scripts enabled
        let script_sender = if config.scripts.enabled {
            let (tx, rx) = std::sync::mpsc::channel();
            // The thread reports whether ScriptManager initialized so we can
            // drop the sender on failure — otherwise script_sender would stay
            // Some and every later send would fail silently (a no-op).
            let (init_tx, init_rx) = std::sync::mpsc::channel();

            let script_config = config.scripts.clone();

            // deno_core captures the current tokio handle when the JsRuntime is
            // created and uses it to schedule V8 delayed tasks (e.g. GC). Without
            // one, the first delayed task aborts the whole process.
            let tokio_handle = tokio::runtime::Handle::current();

            // Spawn in a dedicated OS thread since ScriptManager (!Send) cannot cross thread boundaries
            std::thread::spawn(move || {
                let _rt_guard = tokio_handle.enter();

                // Create ScriptManager
                let mut script_manager = match crate::script::ScriptManager::new(&script_config) {
                    Ok(sm) => {
                        tracing::info!("ScriptManager created successfully");
                        let _ = init_tx.send(true);
                        sm
                    }
                    Err(e) => {
                        tracing::error!("Failed to create ScriptManager: {}", e);
                        tracing::warn!("Script executor thread exiting due to initialization failure");
                        let _ = init_tx.send(false);
                        return;
                    }
                };

                // Load all scripts
                if let Err(e) = script_manager.load_all_scripts() {
                    tracing::error!("Failed to load scripts: {}", e);
                } else {
                    tracing::info!("Scripts loaded successfully");
                }

                // Run executor loop (the runtime guard above stays entered)
                executor::script_executor_loop(rx, script_manager);
            });

            // Wait for the init result (ScriptManager::new is quick). If it
            // failed or the thread died, disable scripting cleanly.
            match init_rx.recv() {
                Ok(true) => {
                    tracing::info!("Script executor thread ready");
                    Some(tx)
                }
                _ => {
                    tracing::warn!("Scripting disabled: executor failed to initialize");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            i18n: Self::create_i18n(language),
            script_sender,
        })
    }

    /// Get translated string by key
    pub fn t(&self, key: &str) -> String {
        self.i18n.get(key)
    }

    /// Get translated string with arguments
    pub fn t_with_args(&self, key: &str, args: Option<&fluent_bundle::FluentArgs>) -> String {
        self.i18n.get_with_args(key, args)
    }

    /// Reload all scripts from disk
    pub async fn reload_scripts(&self) -> Result<()> {
        if let Some(ref sender) = self.script_sender {
            let (response_tx, response_rx) = std::sync::mpsc::channel();
            let sender = sender.clone();

            // Send request and receive response in blocking task
            tokio::task::spawn_blocking(move || {
                sender
                    .send(ScriptRequest::Reload {
                        response: response_tx,
                    })
                    .map_err(|e| anyhow::anyhow!("Failed to send reload request: {}", e))?;

                response_rx
                    .recv()
                    .map_err(|e| anyhow::anyhow!("Failed to receive reload response: {}", e))?
                    // No extra prefix here: `ScriptError`'s own `Display` is
                    // already the full detail, and the caller (`dispatch_reload_scripts`)
                    // adds its own framing around it.
                    ?;

                Ok::<(), anyhow::Error>(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("Blocking task failed: {}", e))??;

            tracing::info!("Scripts reloaded successfully");
            Ok(())
        } else {
            Err(anyhow::anyhow!("Scripts are not enabled"))
        }
    }

    /// Wait for every script request queued before this call to finish
    /// processing.
    ///
    /// Returns immediately when scripts are disabled (no executor thread).
    /// On timeout, logs a warning and returns without waiting further.
    pub async fn flush_scripts(&self, timeout: Duration) {
        let Some(sender) = self.script_sender.clone() else {
            return;
        };

        let flushed = tokio::task::spawn_blocking(move || {
            let (response_tx, response_rx) = mpsc::channel();
            if sender.send(ScriptRequest::Flush { response: response_tx }).is_err() {
                // Executor thread is gone; nothing to wait for.
                return true;
            }
            response_rx.recv_timeout(timeout).is_ok()
        })
        .await
        .unwrap_or(false);

        if !flushed {
            tracing::warn!("Timed out waiting for script executor to flush");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppState>();
        assert_send_sync::<LocalizationManager>();
    }

    #[tokio::test]
    async fn flush_scripts_without_executor_returns_immediately() {
        let state = AppState::new(Config::default(), "en");
        assert!(state.script_sender.is_none());

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        state.flush_scripts(Duration::from_secs(5)).await;
        assert!(std::time::Instant::now() < deadline);
    }
}
