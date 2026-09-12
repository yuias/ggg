/// Script hook system for download events
///
/// Provides JavaScript/TypeScript runtime for user scripts to customize
/// download behavior through event hooks.
///
/// # Architecture
///
/// - ScriptManager: Public API, coordinates script lifecycle
/// - ScriptEngine: Wraps deno_core JsRuntime (V8), executes handlers
/// - ScriptLoader: Loads scripts from filesystem
/// - events: Event types and context structures
/// - api: JavaScript API bindings (ggg.*)
/// - error: Error types
///
/// # Usage
///
/// The `ScriptManager` API is synchronous (the V8 runtime is single-threaded
/// and !Send; it runs on a dedicated executor thread — see `message`/`executor`).
///
/// ```rust,ignore
/// use crate::script::{ScriptManager, events::*};
/// use crate::app::config::ScriptConfig;
///
/// // Initialize
/// let mut manager = ScriptManager::new(&config.scripts)?;
/// manager.load_all_scripts()?;
///
/// // Trigger events (effective_script_files merges app + folder overrides)
/// let mut ctx = BeforeRequestContext { /* ... */ };
/// manager.trigger_before_request(&mut ctx, &effective_script_files)?;
/// ```

pub mod api;
pub mod engine;
pub mod error;
pub mod events;
pub mod executor;
pub mod loader;
pub mod message;
pub mod sender;

use crate::app::config::ScriptConfig;
use crate::script::engine::ScriptEngine;
use crate::script::error::ScriptResult;
use crate::script::events::{
    AuthRequiredContext, BeforeRequestContext, CompletedContext, ErrorContext,
    HeadersReceivedContext, HookEvent, ProgressContext,
};
use crate::script::loader::ScriptLoader;
use std::time::Duration;

/// Main script manager - coordinates script system
pub struct ScriptManager {
    engine: ScriptEngine,
    loader: ScriptLoader,
    _config: ScriptConfig,
}

impl ScriptManager {
    /// Create new script manager from configuration
    pub fn new(config: &ScriptConfig) -> ScriptResult<Self> {
        let timeout = Duration::from_secs(config.timeout);
        let loader = ScriptLoader::new(&config.directory);
        let engine = ScriptEngine::new(timeout)?;

        Ok(Self {
            engine,
            loader,
            _config: config.clone(),
        })
    }

    /// Load all scripts from scripts directory
    /// Loads all .js files regardless of config (filtering happens at execution time)
    /// Clears existing handlers before loading
    pub fn load_all_scripts(&mut self) -> ScriptResult<()> {
        // Clear existing handlers before reloading
        self.engine.clear_handlers();

        let scripts = self.loader.list_scripts()?;

        for script_path in scripts {
            if let Err(e) = self.engine.load_script(&script_path) {
                tracing::error!("Failed to load script {:?}: {}", script_path, e);
                // Continue loading other scripts even if one fails
            }
        }

        Ok(())
    }

    /// Run pending V8 foreground tasks (see `ScriptEngine::pump_v8_tasks`)
    pub fn pump_v8_tasks(&mut self) {
        self.engine.pump_v8_tasks();
    }

    /// Trigger beforeRequest hook
    ///
    /// # Parameters
    /// - `ctx`: Event context
    /// - `effective_script_files`: Effective script_files config (Application + Folder override)
    pub fn trigger_before_request(
        &mut self,
        ctx: &mut BeforeRequestContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        self.engine
            .execute_handlers(HookEvent::BeforeRequest, ctx, effective_script_files)?;
        Ok(())
    }

    /// Trigger headersReceived hook
    pub fn trigger_headers_received(
        &mut self,
        ctx: &HeadersReceivedContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        let mut ctx = ctx.clone();
        self.engine
            .execute_handlers(HookEvent::HeadersReceived, &mut ctx, effective_script_files)?;
        Ok(())
    }

    /// Trigger authRequired hook
    pub fn trigger_auth_required(
        &mut self,
        ctx: &mut AuthRequiredContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        self.engine
            .execute_handlers(HookEvent::AuthRequired, ctx, effective_script_files)?;
        Ok(())
    }

    /// Trigger completed hook
    pub fn trigger_completed(
        &mut self,
        ctx: &mut CompletedContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        self.engine.execute_handlers(HookEvent::Completed, ctx, effective_script_files)?;
        Ok(())
    }

    /// Trigger error hook (fire-and-forget)
    pub fn trigger_error(
        &mut self,
        ctx: &ErrorContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        let mut ctx = ctx.clone();
        self.engine
            .execute_handlers(HookEvent::ErrorOccurred, &mut ctx, effective_script_files)?;
        Ok(())
    }

    /// Trigger progress hook (fire-and-forget)
    pub fn trigger_progress(
        &mut self,
        ctx: &ProgressContext,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<()> {
        let mut ctx = ctx.clone();
        self.engine.execute_handlers(HookEvent::Progress, &mut ctx, effective_script_files)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_module_structure() {
        // Verify module structure compiles
        let config = ScriptConfig {
            enabled: true,
            directory: PathBuf::from("./scripts"),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };
        assert_eq!(config.timeout, 30);
    }

    #[test]
    fn test_script_manager_creation() {
        let temp_dir = std::env::temp_dir().join("ggg_test_manager_create");
        fs::create_dir_all(&temp_dir).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let manager = ScriptManager::new(&config);
        assert!(manager.is_ok());

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_multiple_scripts_in_order() {
        let temp_dir = std::env::temp_dir().join("ggg_test_multi_scripts");
        fs::create_dir_all(&temp_dir).unwrap();

        // Create scripts with alphabetical names
        let script1 = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Script-1'] = 'loaded';
                return true;
            });
        "#;

        let script2 = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Script-2'] = 'loaded';
                return true;
            });
        "#;

        let script3 = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Script-3'] = 'loaded';
                return true;
            });
        "#;

        fs::write(temp_dir.join("a_first.js"), script1).unwrap();
        fs::write(temp_dir.join("b_second.js"), script2).unwrap();
        fs::write(temp_dir.join("c_third.js"), script3).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let mut manager = ScriptManager::new(&config).unwrap();
        manager.load_all_scripts().unwrap();

        // Execute handlers and verify all scripts ran
        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = HashMap::new(); // All scripts enabled by default
        manager.trigger_before_request(&mut ctx, &script_files).unwrap();

        // All three scripts should have set their headers
        assert_eq!(ctx.headers.get("X-Script-1"), Some(&"loaded".to_string()));
        assert_eq!(ctx.headers.get("X-Script-2"), Some(&"loaded".to_string()));
        assert_eq!(ctx.headers.get("X-Script-3"), Some(&"loaded".to_string()));

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_script_execution_order() {
        let temp_dir = std::env::temp_dir().join("ggg_test_exec_order");
        fs::create_dir_all(&temp_dir).unwrap();

        // Scripts that append to URL to verify execution order
        let script1 = r#"
            ggg.on('beforeRequest', function(e) {
                e.url = e.url + '?script1';
                return true;
            });
        "#;

        let script2 = r#"
            ggg.on('beforeRequest', function(e) {
                e.url = e.url + '&script2';
                return true;
            });
        "#;

        fs::write(temp_dir.join("01_first.js"), script1).unwrap();
        fs::write(temp_dir.join("02_second.js"), script2).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let mut manager = ScriptManager::new(&config).unwrap();
        manager.load_all_scripts().unwrap();

        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = HashMap::new(); // All scripts enabled by default
        manager.trigger_before_request(&mut ctx, &script_files).unwrap();

        // Verify execution order by URL modification
        assert_eq!(
            ctx.url,
            "https://example.com/file.zip?script1&script2"
        );

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_script_stops_propagation() {
        let temp_dir = std::env::temp_dir().join("ggg_test_stop_prop");
        fs::create_dir_all(&temp_dir).unwrap();

        let script1 = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-First'] = 'yes';
                return false; // Stop here
            });
        "#;

        let script2 = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Second'] = 'yes';
                return true;
            });
        "#;

        fs::write(temp_dir.join("first.js"), script1).unwrap();
        fs::write(temp_dir.join("second.js"), script2).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let mut manager = ScriptManager::new(&config).unwrap();
        manager.load_all_scripts().unwrap();

        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = HashMap::new(); // All scripts enabled by default
        manager.trigger_before_request(&mut ctx, &script_files).unwrap();

        // First script ran, second didn't
        assert_eq!(ctx.headers.get("X-First"), Some(&"yes".to_string()));
        assert_eq!(ctx.headers.get("X-Second"), None);

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_empty_directory() {
        let temp_dir = std::env::temp_dir().join("ggg_test_empty_load");
        fs::create_dir_all(&temp_dir).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let mut manager = ScriptManager::new(&config).unwrap();
        let result = manager.load_all_scripts();

        // Should succeed with empty directory
        assert!(result.is_ok());

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_script_error_doesnt_break_loading() {
        let temp_dir = std::env::temp_dir().join("ggg_test_error_load");
        fs::create_dir_all(&temp_dir).unwrap();

        let good_script = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Good'] = 'yes';
                return true;
            });
        "#;

        let bad_script = r#"
            // Syntax error
            ggg.on('beforeRequest' function(e) { // Missing comma
                e.headers['X-Bad'] = 'yes';
                return true;
            });
        "#;

        let another_good_script = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Another-Good'] = 'yes';
                return true;
            });
        "#;

        fs::write(temp_dir.join("a_good.js"), good_script).unwrap();
        fs::write(temp_dir.join("b_bad.js"), bad_script).unwrap();
        fs::write(temp_dir.join("c_good.js"), another_good_script).unwrap();

        let config = ScriptConfig {
            enabled: true,
            directory: temp_dir.clone(),
            timeout: 30,
            script_files: std::collections::HashMap::new(),
        };

        let mut manager = ScriptManager::new(&config).unwrap();
        // Load should succeed even with bad script
        let result = manager.load_all_scripts();
        assert!(result.is_ok());

        // Good scripts should still work
        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = HashMap::new(); // All scripts enabled by default
        manager.trigger_before_request(&mut ctx, &script_files).unwrap();

        // Both good scripts should have run
        assert_eq!(ctx.headers.get("X-Good"), Some(&"yes".to_string()));
        assert_eq!(ctx.headers.get("X-Another-Good"), Some(&"yes".to_string()));

        fs::remove_dir_all(&temp_dir).ok();
    }
}
