//! RPC integration probe test harness.
//!
//! Manages a child Aleph server process with a temporary config directory.
//! Provides helpers for JSON-RPC calls over WebSocket.

#![allow(dead_code)]

use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::OnceCell;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

// =============================================================================
// AlephTestServer
// =============================================================================

/// A child-process Aleph server for integration testing.
///
/// Writes a minimal config.toml to a TempDir, spawns the server binary
/// on a random port, and provides WebSocket JSON-RPC helpers.
pub struct AlephTestServer {
    child: Child,
    pub port: u16,
    pub ws_url: String,
    _config_dir: TempDir,
}

/// Shared server instance (initialized once across all tests).
static SERVER: OnceCell<AlephTestServer> = OnceCell::const_new();

/// Get or initialize the shared test server.
///
/// Uses `tokio::sync::OnceCell` so the server is started exactly once
/// per test binary run, safely callable from async context.
/// Each test should call `clean_providers()` before mutating state.
pub async fn get_server() -> &'static AlephTestServer {
    SERVER
        .get_or_init(|| async { AlephTestServer::start().await })
        .await
}

impl AlephTestServer {
    /// Find a random available port by binding to :0 and reading the assigned port.
    fn find_free_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
        listener.local_addr().unwrap().port()
    }

    /// Write a minimal config.toml with a "test" provider. LAN-trust has no
    /// auth knobs; connections are trusted and `connect` is a plain
    /// session-init handshake.
    fn write_config(dir: &std::path::Path, extra_toml: &str) {
        let config_path = dir.join("config.toml");
        let config_content = format!(
            r#"
[gateway]
port = 18790

[agents.main]
model = "test-model"

[providers.test]
protocol = "openai"
models = ["test-model"]
base_url = "http://localhost:1/v1"
enabled = true

{extra_toml}
"#
        );
        std::fs::write(&config_path, config_content).expect("Failed to write test config.toml");
    }

    /// Start the server with default config.
    pub async fn start() -> Self {
        Self::start_with_config("").await
    }

    /// Start the server with extra TOML appended to the default config.
    pub async fn start_with_config(extra_toml: &str) -> Self {
        let config_dir = TempDir::new().expect("Failed to create temp dir");
        Self::write_config(config_dir.path(), extra_toml);

        // HARD ISOLATION GUARD. The spawned server resolves ALL state — config,
        // data, vault, singleton lock, logs — from `ALEPH_HOME` (see
        // alephcore::utils::paths::get_config_dir). We point it at this per-test
        // TempDir. Refuse to run if that is somehow NOT under the system temp
        // dir: a regression that let this resolve to the real ~/.aleph once
        // ran `clean_providers()` against a user's live config and deleted it.
        let aleph_home = config_dir.path().to_path_buf();
        let tmp_root = std::env::temp_dir();
        let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        assert!(
            canon(&aleph_home).starts_with(canon(&tmp_root)),
            "REFUSING to start probe server: ALEPH_HOME={:?} is not under the \
             system temp dir {:?}. Aborting to avoid mutating real user data.",
            aleph_home,
            tmp_root,
        );

        let port = Self::find_free_port();
        let config_path = config_dir.path().join("config.toml");

        // Locate the built binary via cargo's canonical mechanism: cargo
        // builds the package's binaries for integration tests and exports
        // their absolute paths. Unlike a hand-rolled
        // `<CARGO_MANIFEST_DIR>/target/debug/aleph-server` path, this stays
        // correct when a machine-local `.cargo/config.toml` pins a shared
        // `target-dir` outside the workspace root. Same pattern as
        // `tests/spec_c_cli_no_server.rs`.
        let binary_path = std::path::PathBuf::from(env!("CARGO_BIN_EXE_aleph-server"));

        // Spawn the pre-built binary directly (no cargo overhead).
        //
        // Isolate via `ALEPH_HOME` (= the per-test TempDir, treated as the
        // `.aleph` data dir). This is the single authoritative knob: config,
        // data, vault, singleton lock and logs all resolve under it, so each
        // probe server is fully self-contained and CANNOT touch the real
        // ~/.aleph. `HOME` is set too as a belt-and-suspenders fallback for any
        // path that still consults it.
        let child = Command::new(&binary_path)
            .env("ALEPH_HOME", config_dir.path())
            .env("HOME", config_dir.path())
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "--port",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
            ])
            // Pipe stderr for debugging, suppress stdout
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to spawn aleph server process");

        // The harness lives in a `static OnceCell`, which never drops — so the
        // `impl Drop` below never runs for the shared instance and this child
        // would outlive the test binary. Register it with the process-exit
        // reaper, which fires on both a normal return and `process::exit`.
        #[cfg(unix)]
        alephcore::utils::scratch::reap_on_exit(child.id(), config_dir.path().to_path_buf());

        let ws_url = format!("ws://127.0.0.1:{}/ws", port);

        let mut server = Self {
            child,
            port,
            ws_url,
            _config_dir: config_dir,
        };

        // Wait for the server to be ready (up to 60 seconds for compilation)
        server.wait_for_ready(Duration::from_secs(60)).await;

        server
    }

    /// Poll until a TCP connection to the server port succeeds.
    async fn wait_for_ready(&mut self, max_wait: Duration) {
        let start = Instant::now();
        let addr = format!("127.0.0.1:{}", self.port);

        loop {
            // Check if child has exited unexpectedly
            if let Ok(Some(status)) = self.child.try_wait() {
                // Try to capture stderr for debugging
                let stderr_output = if let Some(stderr) = self.child.stderr.take() {
                    use std::io::Read;
                    let mut buf = String::new();
                    let mut stderr = stderr;
                    let _ = stderr.read_to_string(&mut buf);
                    buf
                } else {
                    String::new()
                };
                panic!(
                    "Aleph server exited unexpectedly with status: {}\nstderr: {}",
                    status, stderr_output
                );
            }

            // Try TCP connect
            if TcpStream::connect(&addr).is_ok() {
                // Give the WebSocket handler a moment to initialize
                tokio::time::sleep(Duration::from_millis(200)).await;
                return;
            }

            if start.elapsed() > max_wait {
                // Kill the child before panicking
                let _ = self.child.kill();
                panic!(
                    "Aleph server did not become ready within {}s on port {}",
                    max_wait.as_secs(),
                    self.port
                );
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Send a JSON-RPC request over a fresh WebSocket connection and return the response.
    ///
    /// Each call opens a new WS connection (stateless probe pattern).
    /// LAN-trust session-init invariant: the first frame on every connection
    /// must be `connect` (a credential-less handshake that bootstraps
    /// per-connection session state), so each fresh connection performs that
    /// handshake before the actual probe request. Times out after 10 seconds.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Value {
        let (ws_stream, _) = timeout(Duration::from_secs(10), connect_async(&self.ws_url))
            .await
            .expect("WebSocket connect timed out")
            .expect("WebSocket connect failed");

        let (mut write, mut read) = ws_stream.split();

        // 1. `connect` handshake (id 0) — required first frame under LAN-trust.
        let connect_request = json!({
            "jsonrpc": "2.0",
            "method": "connect",
            "params": { "device_name": "rpc-probe" },
            "id": 0
        });
        write
            .send(WsMessage::Text(
                serde_json::to_string(&connect_request).unwrap().into(),
            ))
            .await
            .expect("Failed to send connect handshake");
        let _hello = Self::read_response(&mut read, &json!(0)).await;

        // 2. The actual probe request (id 1).
        let rpc_request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });
        write
            .send(WsMessage::Text(
                serde_json::to_string(&rpc_request).unwrap().into(),
            ))
            .await
            .expect("Failed to send WS message");

        let response = Self::read_response(&mut read, &json!(1)).await;

        // Close gracefully
        let _ = write.send(WsMessage::Close(None)).await;

        response
    }

    /// Read frames until the JSON-RPC response carrying `expected_id`
    /// arrives, skipping event broadcasts (no `id`) and unrelated responses.
    /// Times out after 10 seconds.
    async fn read_response<S>(read: &mut S, expected_id: &Value) -> Value
    where
        S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        timeout(Duration::from_secs(10), async {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            if val.get("id") == Some(expected_id) {
                                return val;
                            }
                            // Event broadcast or unrelated response — skip it
                        }
                    }
                    Ok(WsMessage::Close(_)) => {
                        panic!("WebSocket closed unexpectedly");
                    }
                    Err(e) => {
                        panic!("WebSocket error: {}", e);
                    }
                    _ => {} // Ignore ping/pong/binary
                }
            }
            panic!("WebSocket stream ended without a response");
        })
        .await
        .expect("RPC call timed out waiting for response")
    }

    /// Send an RPC call and assert success (no error field). Returns the "result" value.
    pub async fn rpc_ok(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(
            response.get("error").is_none(),
            "Expected success for '{}', got error: {}",
            method,
            serde_json::to_string_pretty(&response).unwrap()
        );
        response.get("result").cloned().unwrap_or(Value::Null)
    }

    /// Send an RPC call and assert error. Returns the "error" value.
    pub async fn rpc_err(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(
            response.get("error").is_some(),
            "Expected error for '{}', got success: {}",
            method,
            serde_json::to_string_pretty(&response).unwrap()
        );
        response.get("error").cloned().unwrap()
    }

    /// Remove all providers except "test" to restore clean state between tests.
    pub async fn clean_providers(&self) {
        let result = self.rpc_ok("providers.list", json!({})).await;
        let providers = result["providers"].as_array().cloned().unwrap_or_default();

        for provider in providers {
            let name = provider["name"].as_str().unwrap_or("");
            if name != "test" {
                // Ignore errors (e.g., trying to delete the default provider)
                let _ = self
                    .rpc_call("providers.delete", json!({ "name": name }))
                    .await;
            }
        }
    }
}

impl Drop for AlephTestServer {
    fn drop(&mut self) {
        // Kill the child server process
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
