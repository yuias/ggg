#![cfg(windows)]
//! Integration test for the daemon control pipe transport: bind, serve,
//! and probe over a real named pipe.
//!
//! Every wait is bounded (`tokio::time::timeout`), and every pipe name is
//! unique (a v4 UUID suffix) so a parallel test run, or an actual daemon
//! running on this machine, can never collide with these tests and hang
//! them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use ggg::ipc::control::client::{self, DaemonProbe};
use ggg::ipc::control::owner;
use ggg::ipc::control::protocol::{
    ControlErrorCode, ControlRequest, ControlResponse, CONTROL_PROTOCOL_VERSION,
};
use ggg::ipc::control::server::{self, BindError, ControlHandler};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use tokio::sync::Notify;

/// Bounds every wait in this file so a server bug produces a test failure
/// instead of a hang.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn unique_pipe_name() -> String {
    format!(r"\\.\pipe\ggg-daemon-test-{}", uuid::Uuid::new_v4())
}

/// Fires `notify_one` on the wrapped `Notify` when dropped, so a test can
/// tell when a pending future was actually cancelled (not just that the
/// connection loop moved on).
struct NotifyOnDrop<'a>(&'a Notify);

impl Drop for NotifyOnDrop<'_> {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

/// Minimal handler with no `AppState`/`DownloadManager` dependency, as the
/// design intends `serve` to allow for this transport-only test.
struct TestHandler {
    after_response_notify: Notify,
    after_response_count: AtomicUsize,
    /// Notified once the `Wait` arm below starts running, so a test can wait
    /// for the server to have entered the pending future before disconnecting.
    wait_started: Notify,
    /// Notified when the `Wait` arm's future is dropped (cancelled), which is
    /// the outcome under test for an abandoned wait.
    wait_dropped: Notify,
}

impl TestHandler {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            after_response_notify: Notify::new(),
            after_response_count: AtomicUsize::new(0),
            wait_started: Notify::new(),
            wait_dropped: Notify::new(),
        })
    }
}

impl ControlHandler for TestHandler {
    fn handle(&self, req: ControlRequest) -> BoxFuture<'_, ControlResponse> {
        async move {
            match req {
                ControlRequest::Ping => ControlResponse::Pong {
                    protocol: CONTROL_PROTOCOL_VERSION,
                    pid: std::process::id(),
                    version: "test".to_string(),
                },
                ControlRequest::Shutdown => ControlResponse::Ok { message: "bye".to_string() },
                ControlRequest::Wait { .. } => {
                    self.wait_started.notify_one();
                    let _guard = NotifyOnDrop(&self.wait_dropped);
                    // Never resolves on its own; only cancellation (dropping
                    // this future) ends it, which is what this test exercises.
                    std::future::pending::<()>().await;
                    unreachable!("wait future must be cancelled, not resolved");
                }
                _ => ControlResponse::Error {
                    code: ControlErrorCode::Unsupported,
                    message: "not handled by the test handler".to_string(),
                },
            }
        }
        .boxed()
    }

    fn after_response(&self, _req: &ControlRequest, _resp: &ControlResponse) -> BoxFuture<'_, ()> {
        async move {
            self.after_response_count.fetch_add(1, Ordering::SeqCst);
            self.after_response_notify.notify_one();
        }
        .boxed()
    }
}

/// Opens `pipe_name` (retrying while busy), sends one line, and reads one
/// line back. Bounded by `TEST_TIMEOUT` so a stuck server fails the test
/// instead of hanging it.
async fn send_line(pipe_name: &str, line: &str) -> String {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let client = loop {
            match ClientOptions::new().open(pipe_name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("failed to open {pipe_name}: {e}"),
            }
        };

        let (reader, mut writer) = tokio::io::split(client);
        let mut reader = BufReader::new(reader);

        writer.write_all(line.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();
        buf
    })
    .await
    .expect("send_line timed out")
}

#[tokio::test]
async fn ping_over_pipe() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let reply = send_line(&pipe_name, r#"{"type":"ping"}"#).await;
    let parsed: ControlResponse = serde_json::from_str(reply.trim()).expect("valid json reply");
    match parsed {
        ControlResponse::Pong { protocol, .. } => assert_eq!(protocol, CONTROL_PROTOCOL_VERSION),
        other => panic!("expected pong, got {other:?}"),
    }
}

#[tokio::test]
async fn second_bind_reports_already_running() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let err = tokio::time::timeout(TEST_TIMEOUT, server::bind(&pipe_name))
        .await
        .expect("second bind timed out")
        .expect_err("second bind should fail while the first is serving");
    match err {
        BindError::AlreadyRunning { pid: Some(_) } => {}
        other => panic!("expected AlreadyRunning{{pid: Some(_)}}, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_json_gets_error_response() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let reply = send_line(&pipe_name, "not json").await;
    let parsed: ControlResponse = serde_json::from_str(reply.trim()).expect("valid json reply");
    match parsed {
        ControlResponse::Error { code: ControlErrorCode::InvalidInput, .. } => {}
        other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
    }
}

#[tokio::test]
async fn after_response_runs_after_reply() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let reply = send_line(&pipe_name, r#"{"type":"shutdown"}"#).await;
    let parsed: ControlResponse = serde_json::from_str(reply.trim()).expect("valid json reply");
    assert!(matches!(parsed, ControlResponse::Ok { .. }));

    tokio::time::timeout(TEST_TIMEOUT, handler.after_response_notify.notified())
        .await
        .expect("after_response did not run in time");
    assert_eq!(handler.after_response_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn probe_running_and_not_running() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let running = tokio::time::timeout(TEST_TIMEOUT, client::probe(&pipe_name))
        .await
        .expect("probe timed out");
    match running {
        DaemonProbe::Running { protocol, .. } => assert_eq!(protocol, CONTROL_PROTOCOL_VERSION),
        other => panic!("expected Running, got {other:?}"),
    }

    let unused_pipe_name = unique_pipe_name();
    let not_running = tokio::time::timeout(TEST_TIMEOUT, client::probe(&unused_pipe_name))
        .await
        .expect("probe timed out");
    assert!(matches!(not_running, DaemonProbe::NotRunning));
}

#[tokio::test]
async fn bind_reports_already_running_for_non_responding_holder() {
    let pipe_name = unique_pipe_name();
    // A raw instance that holds the name but is never served: nothing calls
    // `connect()` or reads from it, so a probe's ping can open the pipe but
    // never gets a reply. `bind` must treat this as AlreadyRunning with an
    // unknown pid, not free the name for its own first instance.
    let _holder = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .expect("create raw pipe instance");

    // Expected latency is the probe's ~2s ping timeout plus bind's 200ms
    // retry delay (~2.2s); this bound just guards against a real hang.
    let err = tokio::time::timeout(Duration::from_secs(10), server::bind(&pipe_name))
        .await
        .expect("bind timed out")
        .expect_err("bind should fail while the raw instance holds the name");
    match err {
        BindError::AlreadyRunning { pid: None } => {}
        other => panic!("expected AlreadyRunning{{pid: None}}, got {other:?}"),
    }
}

#[tokio::test]
async fn server_owner_check_passes_for_same_user() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let client = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match ClientOptions::new().open(&pipe_name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("failed to open {pipe_name}: {e}"),
            }
        }
    })
    .await
    .expect("open timed out");

    // This test process both created the pipe and opened this client handle
    // to it, so the pipe's owner SID is this process's own TokenUser SID and
    // the check must pass.
    owner::check_server_owner(&client).expect("server owner check should pass for same user");
}

#[tokio::test]
async fn bind_rejects_invalid_pipe_name() {
    let start = std::time::Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(1), server::bind("not-a-pipe-name"))
        .await
        .expect("bind timed out")
        .expect_err("bind should reject an invalid pipe name");
    assert!(start.elapsed() < Duration::from_secs(1), "bind should fail without probing or retrying");
    match err {
        BindError::InvalidName(_) => {}
        other => panic!("expected InvalidName, got {other:?}"),
    }
}

#[tokio::test]
async fn probe_invalid_name_is_unreachable() {
    let result = tokio::time::timeout(TEST_TIMEOUT, client::probe("not-a-pipe-name"))
        .await
        .expect("probe timed out");
    match result {
        DaemonProbe::Unreachable(msg) => assert!(msg.contains("invalid pipe name"), "unexpected message: {msg}"),
        other => panic!("expected Unreachable, got {other:?}"),
    }
}

#[tokio::test]
async fn abandoned_wait_is_cancelled_on_client_disconnect() {
    let pipe_name = unique_pipe_name();
    let bound = server::bind(&pipe_name).await.expect("bind");
    let handler = TestHandler::new();
    let _serve = server::serve(bound, handler.clone());

    let mut client = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match ClientOptions::new().open(&pipe_name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("failed to open {pipe_name}: {e}"),
            }
        }
    })
    .await
    .expect("open timed out");

    let req = ControlRequest::Wait { id: uuid::Uuid::nil(), timeout_secs: None };
    let line = serde_json::to_string(&req).unwrap();
    client.write_all(line.as_bytes()).await.unwrap();
    client.write_all(b"\n").await.unwrap();
    client.flush().await.unwrap();

    tokio::time::timeout(TEST_TIMEOUT, handler.wait_started.notified())
        .await
        .expect("wait did not start in time");

    // Drop the client without reading a reply, simulating Ctrl+C. The server
    // should observe EOF and cancel the pending Wait instead of hanging on it.
    drop(client);

    tokio::time::timeout(TEST_TIMEOUT, handler.wait_dropped.notified())
        .await
        .expect("abandoned wait was not cancelled");

    assert_eq!(handler.after_response_count.load(Ordering::SeqCst), 0);
}
