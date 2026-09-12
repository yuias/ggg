/// Dedicated script executor thread
///
/// Runs in a separate OS thread to handle script execution requests.
/// This avoids Send/Sync issues with deno_core::JsRuntime.

use super::message::ScriptRequest;
use super::ScriptManager;
use std::sync::mpsc;
use std::time::Duration;

/// Script executor loop
///
/// Processes script execution requests from the channel using a pre-created ScriptManager.
///
/// # Arguments
///
/// * `rx` - Receiver for script execution requests
/// * `script_manager` - Pre-initialized ScriptManager
///
/// # Lifecycle
///
/// Runs until the channel is closed (all senders dropped).
pub fn script_executor_loop(
    rx: mpsc::Receiver<ScriptRequest>,
    mut script_manager: ScriptManager,
) {
    tracing::info!("Starting script executor loop");

    // Delayed V8 tasks (e.g. GC memory reducer) also arrive while idle, so the
    // loop wakes up periodically to run them.
    const IDLE_PUMP_INTERVAL: Duration = Duration::from_secs(5);

    // Process requests
    let mut request_count = 0;
    loop {
        let request = match rx.recv_timeout(IDLE_PUMP_INTERVAL) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                script_manager.pump_v8_tasks();
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        request_count += 1;
        tracing::debug!("Processing script request #{}: {:?}", request_count, request);

        match request {
            ScriptRequest::BeforeRequest { mut ctx, effective_script_files, response } => {
                let result = script_manager.trigger_before_request(&mut ctx, &effective_script_files);
                if let Err(e) = &result {
                    tracing::error!("beforeRequest hook error: {}", e);
                }
                let _ = response.send((ctx, result));
            }

            ScriptRequest::HeadersReceived { ctx, effective_script_files, response } => {
                let result = script_manager.trigger_headers_received(&ctx, &effective_script_files);
                if let Err(e) = &result {
                    tracing::error!("headersReceived hook error: {}", e);
                }
                let _ = response.send(result);
            }

            ScriptRequest::Completed { mut ctx, effective_script_files, response } => {
                let result = script_manager.trigger_completed(&mut ctx, &effective_script_files);
                if let Err(e) = &result {
                    tracing::error!("completed hook error: {}", e);
                }
                let _ = response.send((ctx, result));
            }

            ScriptRequest::Error { ctx, effective_script_files } => {
                // Fire-and-forget
                if let Err(e) = script_manager.trigger_error(&ctx, &effective_script_files) {
                    tracing::error!("error hook error: {}", e);
                }
            }

            ScriptRequest::Progress { ctx, effective_script_files } => {
                // Fire-and-forget
                if let Err(e) = script_manager.trigger_progress(&ctx, &effective_script_files) {
                    tracing::error!("progress hook error: {}", e);
                }
            }

            ScriptRequest::AuthRequired { mut ctx, effective_script_files, response } => {
                let result = script_manager.trigger_auth_required(&mut ctx, &effective_script_files);
                if let Err(e) = &result {
                    tracing::error!("authRequired hook error: {}", e);
                }
                let _ = response.send((ctx, result));
            }

            ScriptRequest::Reload { response } => {
                tracing::info!("Reloading scripts from disk...");

                // Reload all scripts using the existing ScriptManager
                let result = script_manager.load_all_scripts();

                if let Ok(_) = &result {
                    tracing::info!("Scripts reloaded successfully");
                } else {
                    tracing::error!("Failed to reload scripts: {:?}", result);
                }

                let _ = response.send(result);
            }
        }

        script_manager.pump_v8_tasks();
    }

    tracing::info!(
        "Script executor loop shutting down (processed {} requests)",
        request_count
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::ScriptConfig;
    use crate::script::events::BeforeRequestContext;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn test_executor_loop_initialization() {
        let (tx, rx) = mpsc::channel();
        let config = ScriptConfig {
            enabled: true,
            directory: PathBuf::from("./scripts"),
            timeout: 30,
            script_files: HashMap::new(),
        };

        // Spawn executor thread (create ScriptManager inside to avoid Send issues)
        let handle = std::thread::spawn(move || {
            // Create ScriptManager inside the thread
            let script_manager = ScriptManager::new(&config).unwrap();
            script_executor_loop(rx, script_manager);
        });

        // Drop sender to close channel
        drop(tx);

        // Thread should complete
        handle.join().unwrap();
    }

    #[test]
    fn test_executor_processes_request() {
        let (tx, rx) = mpsc::channel();
        let config = ScriptConfig {
            enabled: true,
            directory: PathBuf::from("./nonexistent_test_dir"),
            timeout: 30,
            script_files: HashMap::new(),
        };

        // Spawn executor thread (create ScriptManager inside to avoid Send issues)
        std::thread::spawn(move || {
            // Create ScriptManager inside the thread
            let script_manager = ScriptManager::new(&config).unwrap();
            script_executor_loop(rx, script_manager);
        });

        // Send a request
        let ctx = BeforeRequestContext {
            url: "https://example.com".to_string(),
            headers: HashMap::new(),
            user_agent: None,
            download_id: None,
        };

        let (response_tx, response_rx) = std::sync::mpsc::channel();
        let script_files = HashMap::new();
        tx.send(ScriptRequest::BeforeRequest {
            ctx,
            effective_script_files: script_files,
            response: response_tx,
        })
        .unwrap();

        // Should receive response
        let (ctx, result) = response_rx.recv().unwrap();
        assert!(result.is_ok());
        assert_eq!(ctx.url, "https://example.com");
    }
}
