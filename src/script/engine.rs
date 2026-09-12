use crate::script::error::{ScriptError, ScriptResult};
use crate::script::events::{EventContext, HookEvent};
use deno_core::{v8, JsRuntime, RuntimeOptions};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// JavaScript engine wrapper around deno_core JsRuntime
///
/// Manages:
/// - V8 runtime initialization via deno_core
/// - Event handler registry
/// - Handler execution with timeout enforcement
/// - URL filtering
pub struct ScriptEngine {
    runtime: JsRuntime,
    // Per-event handlers are stored behind an `Arc` so dispatch can clone the
    // list pointer (O(1)) to release the lock, instead of deep-cloning the
    // vector (and its compiled `Regex` filters) on every event.
    handlers: Arc<Mutex<HashMap<HookEvent, Arc<Vec<EventHandler>>>>>,
    timeout: Duration,
}

/// Registered event handler
#[derive(Debug, Clone)]
struct EventHandler {
    /// Function name in JavaScript (generated callback ID)
    callback_id: String,
    /// URL filter pattern (optional)
    filter: Option<UrlFilter>,
    /// Source script path for error reporting
    script_path: PathBuf,
}

/// URL filter for conditional handler execution
#[derive(Debug, Clone)]
struct UrlFilter {
    #[allow(dead_code)]
    pattern: String,
    regex: Regex,
}

/// Validate a generated callback identifier (`__callback_<digits>`) before it
/// is interpolated into JavaScript source as a bare identifier.
fn is_valid_callback_id(id: &str) -> bool {
    id.strip_prefix("__callback_")
        .map(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or(false)
}

impl UrlFilter {
    /// Create new URL filter from a pattern string.
    ///
    /// Patterns containing `*`, `^`, or `$` are treated as regular expressions;
    /// anything else is matched as an escaped substring. (The `regex` crate is
    /// linear-time, so there is no catastrophic-backtracking risk; the size
    /// limit below only bounds a pathologically large compiled program.)
    fn new(pattern: String, script: &Path) -> ScriptResult<Self> {
        let regex_pattern = if pattern.contains('*') || pattern.contains('^') || pattern.contains('$')
        {
            // Already looks like regex
            pattern.clone()
        } else {
            // Simple substring match - escape and wrap
            regex::escape(&pattern)
        };

        let regex = regex::RegexBuilder::new(&regex_pattern)
            .size_limit(1 << 20) // 1 MiB compiled-program cap
            .build()
            .map_err(|_| ScriptError::InvalidFilter {
                script: script.display().to_string(),
                pattern: pattern.clone(),
            })?;

        Ok(Self { pattern, regex })
    }

    /// Check if URL matches this filter
    fn matches(&self, url: &str) -> bool {
        self.regex.is_match(url)
    }
}

impl ScriptEngine {
    /// Deserialize a V8 global value into a Rust type via serde_v8
    fn deserialize_v8<T: for<'de> Deserialize<'de>>(
        &mut self,
        global: v8::Global<v8::Value>,
    ) -> ScriptResult<T> {
        deno_core::scope!(scope, self.runtime);
        let local = v8::Local::new(scope, global);
        deno_core::serde_v8::from_v8(scope, local)
            .map_err(|e| ScriptError::InternalError(format!("V8 deserialization error: {}", e)))
    }

    /// Execute JavaScript with timeout enforcement.
    /// Returns the raw v8::Global<v8::Value> on success.
    fn execute_with_timeout(
        &mut self,
        name: &'static str,
        code: String,
    ) -> ScriptResult<v8::Global<v8::Value>> {
        let handle = self.runtime.v8_isolate().thread_safe_handle();
        let timeout = self.timeout;

        // Signal-based watchdog: the worker waits for either a completion
        // signal or the timeout. Using a channel (instead of sleep + flag)
        // avoids the race where a fixed sleep fires *after* the script already
        // returned and terminates the *next* execution. The watchdog also exits
        // immediately on completion rather than sleeping the full timeout, so
        // threads do not accumulate under load.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            match rx.recv_timeout(timeout) {
                Err(RecvTimeoutError::Timeout) => {
                    handle.terminate_execution();
                    true // terminated due to timeout
                }
                _ => false, // completed in time (signalled or sender dropped)
            }
        });

        let result = self.runtime.execute_script(name, code);

        // Tell the watchdog we're done and wait for it to settle. After it has
        // joined we unconditionally clear any pending termination so a late or
        // spurious terminate cannot poison the next execute_script call.
        let _ = tx.send(());
        let timed_out = watchdog.join().unwrap_or(false);
        self.runtime.v8_isolate().cancel_terminate_execution();

        match result {
            Ok(global) => Ok(global),
            Err(e) => {
                if timed_out {
                    Err(ScriptError::Timeout {
                        script: name.to_string(),
                        timeout_ms: timeout.as_millis() as u64,
                    })
                } else {
                    // A genuine JS/V8 exception (not a timeout) — preserve it as
                    // a structured execution error rather than a flat internal
                    // error, keeping the script name for diagnostics.
                    Err(ScriptError::ExecutionError {
                        script: name.to_string(),
                        message: e.to_string(),
                    })
                }
            }
        }
    }

    /// Create new script engine with timeout
    /// Clear all registered handlers (used when reloading scripts)
    pub fn clear_handlers(&mut self) {
        let mut handlers = self.handlers.lock().unwrap_or_else(|e| e.into_inner());
        handlers.clear();
        drop(handlers);

        // Also clear JavaScript-side handler registry and reset callback ID.
        // Delete the `globalThis.__callback_*` functions too, otherwise stale
        // callbacks (and their captured closures) leak for the runtime's
        // lifetime when a reload registers fewer handlers. Routed through the
        // timeout wrapper since a loaded script could override these globals.
        if let Err(e) = self.execute_with_timeout(
            "<ggg:clear>",
            "for (const k of Object.keys(globalThis)) { \
                 if (k.startsWith('__callback_')) delete globalThis[k]; \
             } \
             ggg._handlers.clear(); ggg._nextCallbackId = 0;"
                .to_string(),
        ) {
            tracing::warn!("Failed to clear JavaScript handlers: {}", e);
        }

        tracing::debug!("Cleared all script handlers");
    }

    pub fn new(timeout: Duration) -> ScriptResult<Self> {
        // Cap the V8 heap so a runaway script (e.g. `while(true) a.push(x)`)
        // cannot exhaust process memory. The near-heap-limit callback
        // terminates execution and raises the limit just enough to let the
        // termination unwind instead of letting V8 hard-abort the process.
        const MAX_HEAP_BYTES: usize = 256 * 1024 * 1024;
        let create_params = v8::CreateParams::default().heap_limits(0, MAX_HEAP_BYTES);
        let mut runtime = JsRuntime::new(RuntimeOptions {
            create_params: Some(create_params),
            ..Default::default()
        });

        let isolate_handle = runtime.v8_isolate().thread_safe_handle();
        runtime.add_near_heap_limit_callback(move |current, _initial| {
            tracing::error!(
                "Script exceeded the V8 heap limit (~{} bytes); terminating execution",
                current
            );
            isolate_handle.terminate_execution();
            // Grow the limit so V8 can unwind via termination rather than abort.
            current.saturating_mul(2)
        });

        let handlers = Arc::new(Mutex::new(HashMap::new()));

        // Register global `ggg` object with API functions
        let register_code = r#"
            globalThis.ggg = {
                // Handler storage (populated from Rust)
                _handlers: new Map(),
                _nextCallbackId: 0,

                // Register event handler
                on: function(eventName, callback, filter) {
                    if (typeof callback !== 'function') {
                        throw new Error('Callback must be a function');
                    }

                    // Generate unique callback ID
                    const callbackId = `__callback_${this._nextCallbackId++}`;
                    globalThis[callbackId] = callback;

                    // Store handler info for Rust to retrieve
                    if (!this._handlers.has(eventName)) {
                        this._handlers.set(eventName, []);
                    }
                    this._handlers.get(eventName).push({
                        callbackId: callbackId,
                        filter: filter || null
                    });

                    return true;
                },

                // Logging function (buffered, flushed to tracing by Rust).
                // Bounded so a script logging in a tight loop can't grow the
                // buffer until it hits the heap limit.
                _logBuffer: [],
                _maxLogBuffer: 1000,
                _pushLog: function(message) {
                    if (ggg._logBuffer.length < ggg._maxLogBuffer) {
                        ggg._logBuffer.push(message);
                    }
                },
                log: function(message) {
                    ggg._pushLog(String(message));
                },

                // Config access (stub for now)
                config: {
                    get: function(key) {
                        return undefined;
                    }
                }
            };

            // Override console methods to redirect output to ggg._logBuffer
            // Prevents Deno core console from writing directly to stdout
            globalThis.console = {
                log: function(...args) {
                    ggg._pushLog(args.map(String).join(' '));
                },
                warn: function(...args) {
                    ggg._pushLog('[WARN] ' + args.map(String).join(' '));
                },
                error: function(...args) {
                    ggg._pushLog('[ERROR] ' + args.map(String).join(' '));
                },
                info: function(...args) {
                    ggg._pushLog(args.map(String).join(' '));
                },
                debug: function(...args) {
                    ggg._pushLog('[DEBUG] ' + args.map(String).join(' '));
                },
            };
        "#;

        runtime
            .execute_script("<ggg:init>", register_code.to_string())
            .map_err(|e| {
                ScriptError::RuntimeInitError(format!("Failed to register ggg API: {}", e))
            })?;

        Ok(Self {
            runtime,
            handlers,
            timeout,
        })
    }

    /// Load and compile a script file
    pub fn load_script(&mut self, path: &Path) -> ScriptResult<()> {
        // Cap script size before reading: scripts are read fully into memory
        // and executed, so an oversized file is rejected rather than risking
        // an out-of-memory read of an untrusted file.
        const MAX_SCRIPT_BYTES: u64 = 1024 * 1024; // 1 MiB
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > MAX_SCRIPT_BYTES {
                return Err(ScriptError::CompilationError {
                    path: path.to_owned(),
                    message: format!(
                        "Script exceeds the {} byte size limit ({} bytes)",
                        MAX_SCRIPT_BYTES,
                        meta.len()
                    ),
                });
            }
        }

        // Read script file
        let script_content = std::fs::read_to_string(path).map_err(|e| ScriptError::FileReadError {
            path: path.to_owned(),
            source: e,
        })?;

        // Execute script to register handlers (with timeout)
        self.execute_with_timeout("<ggg:load>", script_content)
            .map_err(|e| ScriptError::CompilationError {
                path: path.to_owned(),
                message: e.to_string(),
            })?;

        // Extract registered handlers from JavaScript. Bounded by the timeout:
        // a script could install a malicious `toJSON`/getter that loops during
        // `JSON.stringify`, which would otherwise hang the executor thread.
        let global = self
            .execute_with_timeout(
                "<ggg:handlers>",
                "JSON.stringify(Array.from(ggg._handlers.entries()))".to_string(),
            )
            .map_err(|e| ScriptError::InternalError(format!("Failed to get handlers: {}", e)))?;
        let handlers_json: String = self.deserialize_v8(global)?;

        // Parse handlers and store in registry
        let handlers_data: Vec<(String, Vec<serde_json::Value>)> =
            serde_json::from_str(&handlers_json)
                .map_err(|e| ScriptError::InternalError(format!("Failed to parse handlers: {}", e)))?;

        let mut registry = self.handlers.lock().unwrap_or_else(|e| e.into_inner());

        for (event_name, handlers_list) in handlers_data {
            let event = HookEvent::from_str(&event_name).ok_or_else(|| {
                ScriptError::InvalidEventName(event_name.clone())
            })?;

            let event_handlers =
                Arc::make_mut(registry.entry(event).or_insert_with(|| Arc::new(Vec::new())));

            for handler_data in handlers_list {
                let callback_id = handler_data["callbackId"]
                    .as_str()
                    .ok_or_else(|| ScriptError::InternalError("Missing callbackId".to_string()))?
                    .to_string();

                let filter = if let Some(filter_str) = handler_data["filter"].as_str() {
                    Some(UrlFilter::new(filter_str.to_string(), path)?)
                } else {
                    None
                };

                event_handlers.push(EventHandler {
                    callback_id,
                    filter,
                    script_path: path.to_owned(),
                });
            }
        }
        drop(registry); // Release the handlers lock before re-entering the runtime

        // Clear JavaScript handlers map for next script
        // (Callbacks remain in globalThis, handlers map is just for registration)
        self.execute_with_timeout("<ggg:clear_map>", "ggg._handlers.clear()".to_string())
            .map_err(|e| {
                ScriptError::InternalError(format!("Failed to clear handlers map: {}", e))
            })?;

        tracing::info!("Loaded script: {:?}", path);
        Ok(())
    }

    /// Execute handlers for a specific event
    pub fn execute_handlers<C: EventContext>(
        &mut self,
        event: HookEvent,
        ctx: &mut C,
        effective_script_files: &std::collections::HashMap<String, bool>,
    ) -> ScriptResult<bool> {
        let handlers = self.handlers.lock().unwrap_or_else(|e| e.into_inner());
        let event_handlers = match handlers.get(&event) {
            Some(h) if !h.is_empty() => Arc::clone(h),
            _ => return Ok(true), // No handlers, continue
        };
        drop(handlers); // Release lock (cheap Arc clone above keeps the list)

        // Handler contract:
        // - handlers run in registration order;
        // - each handler's context mutations are applied before its return
        //   value is inspected, so a handler that mutates `ctx` and then returns
        //   `false` (stop) still has its mutation applied;
        // - a handler that throws is logged and skipped (treated as a no-op),
        //   so a broken script does not abort the remaining handlers.
        for handler in event_handlers.iter() {
            // Check if the script file is enabled. Keys come from config, which
            // stores enable/disable by file name; the loader only loads
            // top-level `.js` files (no subdirectories), so basenames are
            // unique and cannot collide. An entry absent from the map means the
            // user set no preference, so it defaults to enabled (new scripts are
            // on by default).
            let filename = handler
                .script_path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("");
            let is_enabled = effective_script_files.get(filename).copied().unwrap_or(true);

            if !is_enabled {
                tracing::debug!(
                    script = ?handler.script_path,
                    "Skipping disabled script: {}",
                    filename
                );
                continue;
            }

            // Serialize current context to JSON (updated after each handler)
            let ctx_json = ctx.to_json()?;

            // Check URL filter if present
            if let Some(ref filter) = handler.filter {
                if let Some(url) = ctx_json.get("url").and_then(|v| v.as_str()) {
                    if !filter.matches(url) {
                        continue; // Skip this handler
                    }
                }
            }

            // Defense-in-depth: `callback_id` is generated as `__callback_<n>`
            // and round-tripped through JSON. Validate it before splicing it
            // into JS source as a bare identifier so a tampered handler entry
            // cannot construct arbitrary code at this call site.
            if !is_valid_callback_id(&handler.callback_id) {
                tracing::error!(
                    script = ?handler.script_path,
                    "Rejecting handler with invalid callback id: {:?}",
                    handler.callback_id
                );
                continue;
            }

            // Execute handler with timeout
            let callback_code = format!(
                "(function() {{
                    const ctx = {};
                    const result = {}(ctx);
                    return {{ result: result, ctx: ctx }};
                }})()",
                serde_json::to_string(&ctx_json)?,
                handler.callback_id
            );

            let exec_result = self.execute_with_timeout("<ggg:callback>", callback_code);

            let result: serde_json::Value = match exec_result {
                Ok(global) => match self.deserialize_v8(global) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            script = ?handler.script_path,
                            "Script deserialization error: {}",
                            e
                        );
                        self.flush_log_buffer(&handler.script_path);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::error!(
                        script = ?handler.script_path,
                        "Script execution error: {}",
                        e
                    );
                    self.flush_log_buffer(&handler.script_path);
                    continue; // Continue to next handler on error
                }
            };

            // Flush ggg.log() messages to tracing
            self.flush_log_buffer(&handler.script_path);

            // Update context from modified JavaScript object
            if let Some(modified_ctx) = result.get("ctx") {
                *ctx = C::from_json(modified_ctx.clone())?;
            }

            // Check if handler returned false (stop propagation). Context
            // mutations above are applied first, then the stop is honored.
            if let Some(false) = result.get("result").and_then(|v| v.as_bool()) {
                tracing::debug!(
                    event = ?event,
                    script = ?handler.script_path,
                    "Handler stopped propagation"
                );
                return Ok(false); // Stop processing
            }
        }

        Ok(true) // Continue processing
    }

    /// Flush buffered ggg.log() messages to tracing
    fn flush_log_buffer(&mut self, script_path: &Path) {
        let global = match self
            .execute_with_timeout("<ggg:log>", "ggg._logBuffer.splice(0)".to_string())
        {
            Ok(g) => g,
            Err(_) => return,
        };
        let messages: Vec<String> = self.deserialize_v8(global).unwrap_or_default();
        for msg in messages {
            tracing::info!(script = ?script_path, "[Script] {}", msg);
        }
    }

    /// Run V8 foreground tasks queued by the platform (GC and similar).
    ///
    /// deno_core drains these only inside the event loop, which this engine
    /// never runs otherwise, so they would pile up without this.
    pub fn pump_v8_tasks(&mut self) {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        if let std::task::Poll::Ready(Err(e)) =
            self.runtime.poll_event_loop(&mut cx, Default::default())
        {
            tracing::warn!("V8 event loop tick failed: {}", e);
        }
    }

    /// Get handler count for an event (for testing)
    #[cfg(test)]
    pub fn handler_count(&self, event: HookEvent) -> usize {
        self.handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&event)
            .map(|h| h.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::events::BeforeRequestContext;
    use std::collections::HashMap;

    #[test]
    fn test_engine_creation() {
        let engine = ScriptEngine::new(Duration::from_secs(30));
        assert!(engine.is_ok());
    }

    #[test]
    fn test_is_valid_callback_id() {
        assert!(is_valid_callback_id("__callback_0"));
        assert!(is_valid_callback_id("__callback_12345"));
        assert!(!is_valid_callback_id("__callback_"));
        assert!(!is_valid_callback_id("__callback_1a"));
        assert!(!is_valid_callback_id("evil(); //"));
        assert!(!is_valid_callback_id("globalThis.x"));
        assert!(!is_valid_callback_id(""));
    }

    #[test]
    fn test_url_filter_simple_match() {
        let filter = UrlFilter::new("pbs.twimg.com".to_string(), Path::new("test.js")).unwrap();
        assert!(filter.matches("https://pbs.twimg.com/media/image.jpg"));
        assert!(!filter.matches("https://example.com/file.zip"));
    }

    #[test]
    fn test_url_filter_regex_match() {
        let filter = UrlFilter::new("^https://.*\\.twimg\\.com".to_string(), Path::new("test.js")).unwrap();
        assert!(filter.matches("https://pbs.twimg.com/media/image.jpg"));
        assert!(filter.matches("https://video.twimg.com/video.mp4"));
        assert!(!filter.matches("http://pbs.twimg.com/image.jpg"));
    }

    #[test]
    fn test_load_simple_script() {
        let mut engine = ScriptEngine::new(Duration::from_secs(30)).unwrap();

        // Create a test script
        let test_script = r#"
            ggg.on('beforeRequest', function(e) {
                ggg.log('Handler called');
                return true;
            });
        "#;

        // Write to temp file
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("test_script.js");
        std::fs::write(&script_path, test_script).unwrap();

        // Load script
        let result = engine.load_script(&script_path);
        assert!(result.is_ok(), "Failed to load script: {:?}", result);

        // Verify handler was registered
        assert_eq!(engine.handler_count(HookEvent::BeforeRequest), 1);

        // Cleanup
        std::fs::remove_file(script_path).ok();
    }

    #[test]
    fn test_execute_handler_modifies_context() {
        let mut engine = ScriptEngine::new(Duration::from_secs(30)).unwrap();

        let test_script = r#"
            ggg.on('beforeRequest', function(e) {
                e.url = 'https://modified.com/file.zip';
                e.headers['X-Custom'] = 'test';
                return true;
            });
        "#;

        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("test_modify.js");
        std::fs::write(&script_path, test_script).unwrap();

        engine.load_script(&script_path).unwrap();

        // Create context
        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        // Execute handlers
        let script_files = std::collections::HashMap::new();
        let result = engine.execute_handlers(HookEvent::BeforeRequest, &mut ctx, &script_files);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue

        // Verify modifications
        assert_eq!(ctx.url, "https://modified.com/file.zip");
        assert_eq!(ctx.headers.get("X-Custom"), Some(&"test".to_string()));

        std::fs::remove_file(script_path).ok();
    }

    #[test]
    fn test_handler_stop_propagation() {
        let mut engine = ScriptEngine::new(Duration::from_secs(30)).unwrap();

        let test_script = r#"
            ggg.on('beforeRequest', function(e) {
                ggg.log('First handler');
                return false; // Stop propagation
            });

            ggg.on('beforeRequest', function(e) {
                ggg.log('Second handler - should not run');
                e.url = 'https://modified.com';
                return true;
            });
        "#;

        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("test_stop.js");
        std::fs::write(&script_path, test_script).unwrap();

        engine.load_script(&script_path).unwrap();

        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = std::collections::HashMap::new();
        let result = engine.execute_handlers(HookEvent::BeforeRequest, &mut ctx, &script_files);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should stop

        // URL should NOT be modified
        assert_eq!(ctx.url, "https://example.com/file.zip");

        std::fs::remove_file(script_path).ok();
    }

    #[test]
    fn test_timeout_terminates_and_runtime_recovers() {
        // A handler that loops forever must be terminated by the watchdog, and
        // crucially the runtime must remain usable afterwards (the watchdog
        // race previously could leave a pending termination that poisoned the
        // next execution).
        let mut engine = ScriptEngine::new(Duration::from_millis(150)).unwrap();

        let temp_dir = std::env::temp_dir();
        let loop_path = temp_dir.join("test_timeout_loop.js");
        std::fs::write(
            &loop_path,
            "ggg.on('beforeRequest', function(e){ while(true){} });",
        )
        .unwrap();
        engine.load_script(&loop_path).unwrap();

        let script_files = HashMap::new();
        let mut ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };
        // Times out internally; execute_handlers logs and continues.
        let result = engine.execute_handlers(HookEvent::BeforeRequest, &mut ctx, &script_files);
        assert!(result.is_ok());
        std::fs::remove_file(&loop_path).ok();

        // The same engine must still run a normal script correctly.
        engine.clear_handlers();
        let ok_path = temp_dir.join("test_timeout_recover.js");
        std::fs::write(
            &ok_path,
            "ggg.on('beforeRequest', function(e){ e.url = 'https://recovered.com'; return true; });",
        )
        .unwrap();
        engine.load_script(&ok_path).unwrap();

        let mut ctx2 = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };
        engine
            .execute_handlers(HookEvent::BeforeRequest, &mut ctx2, &script_files)
            .unwrap();
        assert_eq!(ctx2.url, "https://recovered.com");
        std::fs::remove_file(&ok_path).ok();
    }

    #[test]
    fn test_url_filter_conditional_execution() {
        let mut engine = ScriptEngine::new(Duration::from_secs(30)).unwrap();

        let test_script = r#"
            ggg.on('beforeRequest', function(e) {
                e.headers['X-Twitter'] = 'yes';
                return true;
            }, 'twimg.com');
        "#;

        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("test_filter.js");
        std::fs::write(&script_path, test_script).unwrap();

        engine.load_script(&script_path).unwrap();

        // Test with matching URL
        let mut ctx1 = BeforeRequestContext {
            url: "https://pbs.twimg.com/image.jpg".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let script_files = HashMap::new();
        engine
            .execute_handlers(HookEvent::BeforeRequest, &mut ctx1, &script_files)
            .unwrap();
        assert_eq!(ctx1.headers.get("X-Twitter"), Some(&"yes".to_string()));

        // Test with non-matching URL
        let mut ctx2 = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        engine
            .execute_handlers(HookEvent::BeforeRequest, &mut ctx2, &script_files)
            .unwrap();
        assert_eq!(ctx2.headers.get("X-Twitter"), None); // Should not be set

        std::fs::remove_file(script_path).ok();
    }
}
