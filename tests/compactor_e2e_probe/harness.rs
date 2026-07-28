//! E2E probe harness for session compactor testing.
//!
//! Manages a child Aleph server process with aggressive session compactor
//! settings, providing helpers for chat interaction and memory queries.

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
// CompactorE2eHarness
// =============================================================================

/// A child-process Aleph server configured for aggressive session compaction.
///
/// Based on the `AlephTestServer` pattern from `provider_rpc_probe`.
/// Key differences:
/// - Config includes aggressive session_compactor settings for fast compression
/// - Provides chat-oriented helpers (`send_message`, `wait_for_completion`)
/// - Provides memory query helpers (`query_memory_facts`, `search_memory`)
pub struct CompactorE2eHarness {
    child: Child,
    pub port: u16,
    pub ws_url: String,
    _config_dir: TempDir,
}

/// Shared server instance (initialized once across all tests).
static SERVER: OnceCell<CompactorE2eHarness> = OnceCell::const_new();

/// Get or initialize the shared test server.
///
/// The server is started exactly once per test binary run with aggressive
/// compaction settings. All tests share the same server but should use
/// unique session keys to avoid interference.
pub async fn get_server() -> &'static CompactorE2eHarness {
    SERVER
        .get_or_init(|| async { CompactorE2eHarness::start().await })
        .await
}

impl CompactorE2eHarness {
    /// Find a random available port by binding to :0.
    fn find_free_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
        listener.local_addr().unwrap().port()
    }

    /// Write config.toml with aggressive session compactor settings.
    ///
    /// Key settings:
    /// - `fresh_tail_count = 5` — keep only 5 recent messages uncompressed
    /// - `leaf_chunk_tokens = 300` — small chunks for fast d0 creation
    /// - `d1_min_fanout = 3` — condense after just 3 d0 summaries
    /// - `d2_min_fanout = 3` — condense after just 3 d1 summaries
    fn write_config(dir: &std::path::Path) {
        let config_path = dir.join("config.toml");
        let config_content = r#"
[gateway]
port = 18790

[gateway.auth]
mode = "none"

[agents.main]
model = "test-model"

[providers.test]
protocol = "openai"
models = ["test-model"]
base_url = "http://localhost:1/v1"
enabled = true

[memory.session_compactor]
enabled = true
fresh_tail_count = 5
leaf_chunk_tokens = 300
d1_min_fanout = 3
d2_min_fanout = 3
max_summary_depth = 2
"#;
        std::fs::write(&config_path, config_content).expect("Failed to write test config.toml");
    }

    /// Start the server with aggressive compaction config.
    pub async fn start() -> Self {
        let config_dir = TempDir::new().expect("Failed to create temp dir");
        Self::write_config(config_dir.path());

        // HARD ISOLATION GUARD — same contract as `provider_rpc_probe`. `--config`
        // only redirects config.toml; data, vault, singleton lock and logs still
        // resolve from `ALEPH_HOME` (see alephcore::utils::paths::get_config_dir),
        // so a probe without it runs a real server against the developer's real
        // ~/.aleph/data. Refuse to start if the directory we are about to hand it
        // is somehow not under the system temp dir.
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

        // Locate the built binary. alephcore is the workspace root package, so
        // CARGO_MANIFEST_DIR is the workspace root and `target/` lives directly
        // under it (no `.parent()`).
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = std::path::Path::new(manifest_dir);
        let binary_path = workspace_root.join("target/debug/aleph-server");

        assert!(
            binary_path.exists(),
            "Aleph binary not found at {:?}. Run `cargo build -p alephcore --bin aleph-server` first.",
            binary_path
        );

        // Spawn the pre-built binary directly (no cargo overhead).
        //
        // `ALEPH_HOME` is the single authoritative knob: config, data, vault,
        // singleton lock and logs all resolve under it, so each probe server is
        // self-contained and CANNOT touch the real ~/.aleph. `HOME` is set too as
        // a belt-and-suspenders fallback for any path that still consults it.
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
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to spawn aleph server process");

        let ws_url = format!("ws://127.0.0.1:{}/ws", port);

        let mut server = Self {
            child,
            port,
            ws_url,
            _config_dir: config_dir,
        };

        // Wait for the server to be ready
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

    // =========================================================================
    // Low-level JSON-RPC
    // =========================================================================

    /// Send a JSON-RPC request over a fresh WebSocket connection and return the response.
    ///
    /// Each call opens a new WS connection (stateless probe pattern).
    /// Times out after 10 seconds.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Value {
        let rpc_request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        let (ws_stream, _) = timeout(Duration::from_secs(10), connect_async(&self.ws_url))
            .await
            .expect("WebSocket connect timed out")
            .expect("WebSocket connect failed");

        let (mut write, mut read) = ws_stream.split();

        let request_text = serde_json::to_string(&rpc_request).unwrap();
        write
            .send(WsMessage::Text(request_text.into()))
            .await
            .expect("Failed to send WS message");

        // Read response (skip event broadcasts, wait for JSON-RPC response with "id")
        let response = timeout(Duration::from_secs(10), async {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            if val.get("id").is_some() {
                                return val;
                            }
                        }
                    }
                    Ok(WsMessage::Close(_)) => {
                        panic!("WebSocket closed unexpectedly");
                    }
                    Err(e) => {
                        panic!("WebSocket error: {}", e);
                    }
                    _ => {}
                }
            }
            panic!("WebSocket stream ended without a response");
        })
        .await
        .expect("RPC call timed out waiting for response");

        let _ = write.send(WsMessage::Close(None)).await;

        response
    }

    /// Send an RPC call and assert success. Returns the "result" value.
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

    // =========================================================================
    // Chat Helpers
    // =========================================================================

    /// Send a chat message and wait for the agent run to complete.
    ///
    /// Uses `chat.send` with `stream: false` for simplicity, then polls
    /// `chat.history` until the assistant response appears.
    ///
    /// Returns the assistant's reply text.
    pub async fn send_message(&self, text: &str, session_key: &str) -> String {
        // Send the message (non-streaming for simpler response handling)
        let send_result = self
            .rpc_ok(
                "chat.send",
                json!({
                    "message": text,
                    "session_key": session_key,
                    "stream": false
                }),
            )
            .await;

        let _run_id = send_result["run_id"]
            .as_str()
            .expect("chat.send should return run_id")
            .to_string();

        // Wait for the assistant reply to appear in history.
        // The agent run is async, so we poll until we see the response.
        let reply = timeout(Duration::from_secs(120), async {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;

                let history = self
                    .rpc_ok(
                        "chat.history",
                        json!({
                            "session_key": session_key
                        }),
                    )
                    .await;

                let messages = history["messages"].as_array();
                if let Some(msgs) = messages {
                    // Find the last assistant message
                    if let Some(last_assistant) = msgs
                        .iter()
                        .rev()
                        .find(|m| m["role"].as_str() == Some("assistant"))
                    {
                        let content = last_assistant["content"].as_str().unwrap_or("").to_string();
                        if !content.is_empty() {
                            // Count user messages to make sure this reply is for our message
                            let user_count = msgs
                                .iter()
                                .filter(|m| m["role"].as_str() == Some("user"))
                                .count();
                            // Simple heuristic: if we have at least as many assistant
                            // replies as expected, the latest one is ours
                            let assistant_count = msgs
                                .iter()
                                .filter(|m| m["role"].as_str() == Some("assistant"))
                                .count();
                            if assistant_count >= user_count {
                                return content;
                            }
                        }
                    }
                }
            }
        })
        .await
        .expect("Timed out waiting for assistant reply");

        reply
    }

    /// Send multiple messages sequentially and return all assistant replies.
    pub async fn send_messages(&self, messages: &[&str], session_key: &str) -> Vec<String> {
        let mut replies = Vec::new();
        for msg in messages {
            let reply = self.send_message(msg, session_key).await;
            replies.push(reply);
            // Small delay between messages to let compression kick in
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        replies
    }

    // =========================================================================
    // Memory Query Helpers
    // =========================================================================

    /// Query all memory facts via `memory.list_facts`.
    ///
    /// Returns the raw facts array from the RPC response.
    pub async fn query_memory_facts(&self, agent_id: Option<&str>) -> Vec<Value> {
        let mut params = json!({ "limit": 200, "include_invalid": false });
        if let Some(id) = agent_id {
            params["agent_id"] = json!(id);
        }

        let result = self.rpc_ok("memory.list_facts", params).await;
        result["facts"].as_array().cloned().unwrap_or_default()
    }

    /// Search memory via `memory.search`.
    ///
    /// Returns the raw memories array from the RPC response.
    pub async fn search_memory(&self, query: Option<&str>) -> Vec<Value> {
        let mut params = json!({ "limit": 50 });
        if let Some(q) = query {
            params["query"] = json!(q);
        }

        let result = self.rpc_ok("memory.search", params).await;
        result["memories"].as_array().cloned().unwrap_or_default()
    }

    /// Get memory statistics via `memory.stats`.
    pub async fn memory_stats(&self) -> Value {
        self.rpc_ok("memory.stats", json!({})).await
    }

    /// Wait for session compression facts to appear, polling at intervals.
    ///
    /// Returns `true` if at least one fact with a `SessionCompressed` or
    /// session-scoped path appears within the timeout, `false` otherwise.
    pub async fn wait_for_compression(&self, timeout_secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        while Instant::now() < deadline {
            let facts = self.query_memory_facts(None).await;

            // Look for facts that indicate session compression occurred.
            // Session compactor creates facts with paths like
            // "aleph://session/{session_id}/d{depth}/{seq}"
            let has_session_facts = facts.iter().any(|f| {
                let path = f["path"].as_str().unwrap_or("");
                path.contains("aleph://session/") && path.contains("/d0/")
            });

            if has_session_facts {
                return true;
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        false
    }

    /// Count facts at a specific depth level for any session.
    pub async fn count_facts_at_depth(&self, depth: u32) -> usize {
        let facts = self.query_memory_facts(None).await;
        let depth_segment = format!("/d{}/", depth);
        facts
            .iter()
            .filter(|f| {
                let path = f["path"].as_str().unwrap_or("");
                path.contains("aleph://session/") && path.contains(&depth_segment)
            })
            .count()
    }

    /// Generate a unique session key for test isolation.
    pub fn unique_session_key(test_name: &str) -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("agent:main:e2e-{}-{}", test_name, ts)
    }
}

impl Drop for CompactorE2eHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
