//! P7: Real server RPC probe tests for ACP handlers.
//!
//! Spawns a real Aleph server with ACP enabled and exercises the
//! acp.* JSON-RPC methods over WebSocket.

use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::OnceCell;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

// =============================================================================
// Test server harness (ACP-enabled)
// =============================================================================

struct AcpTestServer {
    child: Child,
    port: u16,
    ws_url: String,
    _config_dir: TempDir,
}

static SERVER: OnceCell<AcpTestServer> = OnceCell::const_new();

async fn get_server() -> &'static AcpTestServer {
    SERVER
        .get_or_init(|| async { AcpTestServer::start().await })
        .await
}

impl AcpTestServer {
    fn find_free_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
        listener.local_addr().unwrap().port()
    }

    /// Write gateway config (used via --config) and app config (used via HOME/.aleph/config.toml).
    ///
    /// The Aleph server loads gateway settings from --config but loads the
    /// application config (providers, agents, ACP, etc.) from ~/.aleph/config.toml.
    /// We set HOME to the temp dir so the server picks up our app config.
    fn write_configs(dir: &std::path::Path) {
        // Gateway config (loaded via --config flag)
        // Must include agents to pass FullGatewayConfig validation.
        let gateway_config_path = dir.join("gateway.toml");
        let gateway_content = r#"
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
"#;
        std::fs::write(&gateway_config_path, gateway_content)
            .expect("Failed to write gateway config");

        // App config (loaded via Config::load() from ~/.aleph/config.toml)
        let aleph_dir = dir.join(".aleph");
        std::fs::create_dir_all(&aleph_dir).expect("Failed to create .aleph dir");
        let app_config_path = aleph_dir.join("config.toml");
        let app_content = r#"
[agents.main]
model = "test-model"

[providers.test]
protocol = "openai"
models = ["test-model"]
base_url = "http://localhost:1/v1"
enabled = true

[acp]
enabled = true
"#;
        std::fs::write(&app_config_path, app_content).expect("Failed to write app config.toml");
    }

    async fn start() -> Self {
        let config_dir = TempDir::new().expect("Failed to create temp dir");
        Self::write_configs(config_dir.path());

        let port = Self::find_free_port();
        let gateway_config_path = config_dir.path().join("gateway.toml");

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = std::path::Path::new(manifest_dir)
            .parent()
            .expect("Cannot find workspace root");
        let binary_path = workspace_root.join("target/debug/aleph-server");

        assert!(
            binary_path.exists(),
            "Aleph binary not found at {:?}. Run `cargo build -p alephcore --bin aleph-server` first.",
            binary_path
        );

        let child = Command::new(&binary_path)
            .args([
                "--config",
                gateway_config_path.to_str().unwrap(),
                "--port",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
            ])
            // Set HOME so Config::load() reads from our temp dir's .aleph/config.toml
            .env("HOME", config_dir.path())
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

        server.wait_for_ready(Duration::from_secs(60)).await;
        server
    }

    async fn wait_for_ready(&mut self, max_wait: Duration) {
        let start = Instant::now();
        let addr = format!("127.0.0.1:{}", self.port);

        loop {
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

            if TcpStream::connect(&addr).is_ok() {
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

    async fn rpc_call(&self, method: &str, params: Value) -> Value {
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
                    Ok(WsMessage::Close(_)) => panic!("WebSocket closed unexpectedly"),
                    Err(e) => panic!("WebSocket error: {}", e),
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

    async fn rpc_ok(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(
            response.get("error").is_none(),
            "Expected success for '{}', got error: {}",
            method,
            serde_json::to_string_pretty(&response).unwrap()
        );
        response.get("result").cloned().unwrap_or(Value::Null)
    }

    async fn rpc_err(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(
            response.get("error").is_some(),
            "Expected error for '{}', got success: {}",
            method,
            serde_json::to_string_pretty(&response).unwrap()
        );
        response.get("error").cloned().unwrap()
    }
}

impl Drop for AcpTestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
#[serial]
async fn p7_01_list_returns_presets() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.list", json!({})).await;
    let list = result.as_array().unwrap();
    assert!(list.len() >= 3, "expected >= 3 presets, got {}", list.len());
    for entry in list {
        assert!(entry.get("id").is_some());
        assert!(entry.get("display_name").is_some());
        assert!(entry.get("mode").is_some());
        assert!(entry.get("enabled").is_some());
        assert!(entry.get("available").is_some());
    }
}

#[tokio::test]
#[serial]
async fn p7_02_get_preset() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.get", json!({"id": "codex"})).await;
    assert_eq!(result["id"], "codex");
    assert_eq!(result["display_name"], "Codex");
    assert_eq!(result["mode"], "oneshot");
}

#[tokio::test]
#[serial]
async fn p7_03_get_nonexistent() {
    let server = get_server().await;
    let err = server
        .rpc_err("acp.get", json!({"id": "nonexistent-xyz"}))
        .await;
    assert!(err["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
#[serial]
async fn p7_04_create_update_delete_cycle() {
    let server = get_server().await;
    let test_id = "probe-test-cli";

    // Create
    let created = server
        .rpc_ok(
            "acp.create",
            json!({
                "id": test_id,
                "config": {
                    "display_name": "Probe Test CLI",
                    "executable": "/usr/bin/echo",
                    "mode": "oneshot",
                    "timeout_seconds": 60,
                    "enabled": true
                }
            }),
        )
        .await;
    assert_eq!(created["id"].as_str(), Some(test_id));

    // Get — verify exists
    let got = server.rpc_ok("acp.get", json!({"id": test_id})).await;
    assert_eq!(got["display_name"].as_str(), Some("Probe Test CLI"));

    // Update
    let updated = server
        .rpc_ok(
            "acp.update",
            json!({
                "id": test_id,
                "config": {
                    "display_name": "Updated CLI",
                    "executable": "/usr/bin/echo",
                    "mode": "oneshot",
                    "timeout_seconds": 120,
                    "enabled": true
                }
            }),
        )
        .await;
    assert_eq!(updated["display_name"].as_str(), Some("Updated CLI"));

    // Delete
    server.rpc_ok("acp.delete", json!({"id": test_id})).await;

    // Verify gone
    let err = server.rpc_err("acp.get", json!({"id": test_id})).await;
    assert!(err["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
#[serial]
async fn p7_05_create_invalid_id() {
    let server = get_server().await;
    let err = server
        .rpc_err(
            "acp.create",
            json!({
                "id": "Invalid!ID",
                "config": {"display_name": "Bad", "enabled": true}
            }),
        )
        .await;
    assert!(err["message"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
#[serial]
async fn p7_06_delete_preset_rejected() {
    let server = get_server().await;
    let err = server.rpc_err("acp.delete", json!({"id": "gemini"})).await;
    let msg = err["message"].as_str().unwrap();
    assert!(
        msg.contains("preset") || msg.contains("Cannot delete"),
        "unexpected error message: {}",
        msg
    );
}

#[tokio::test]
#[serial]
async fn p7_07_presets_returns_defaults() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.presets", json!({})).await;
    let presets = result.as_array().unwrap();
    assert_eq!(presets.len(), 3);
    let ids: Vec<&str> = presets.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"claude-code"));
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"gemini"));
}

#[tokio::test]
#[serial]
async fn p7_08_set_enabled_toggle() {
    let server = get_server().await;

    // Disable codex
    server
        .rpc_ok("acp.set_enabled", json!({"id": "codex", "enabled": false}))
        .await;

    // Verify disabled in list
    let list = server.rpc_ok("acp.list", json!({})).await;
    let codex = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "codex")
        .unwrap();
    assert_eq!(codex["enabled"], false);

    // Re-enable
    server
        .rpc_ok("acp.set_enabled", json!({"id": "codex", "enabled": true}))
        .await;

    let list2 = server.rpc_ok("acp.list", json!({})).await;
    let codex2 = list2
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "codex")
        .unwrap();
    assert_eq!(codex2["enabled"], true);
}

#[tokio::test]
#[serial]
async fn p7_09_test_harness_returns_result() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.test", json!({"id": "codex"})).await;
    assert!(result.get("success").is_some());
    assert!(result.get("message").is_some());
    assert!(result.get("duration_ms").is_some());
    assert!(
        result["duration_ms"].as_u64().unwrap_or(0) > 0
            || result["duration_ms"].as_f64().unwrap_or(0.0) > 0.0
    );
}

#[tokio::test]
#[serial]
async fn p7_10_config_persistence() {
    let server = get_server().await;
    let test_id = "persist-test";

    // Create
    server
        .rpc_ok(
            "acp.create",
            json!({
                "id": test_id,
                "config": {
                    "display_name": "Persist Test",
                    "executable": "/usr/bin/true",
                    "enabled": true
                }
            }),
        )
        .await;

    // Verify via get
    let got = server.rpc_ok("acp.get", json!({"id": test_id})).await;
    assert_eq!(got["id"].as_str(), Some(test_id));
    assert_eq!(got["display_name"].as_str(), Some("Persist Test"));

    // Cleanup
    server.rpc_ok("acp.delete", json!({"id": test_id})).await;
}
