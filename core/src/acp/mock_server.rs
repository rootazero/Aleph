//! Mock ACP server for integration testing.
//!
//! Provides a simple inline mock that reads NDJSON from a `BufRead` source
//! and writes JSON-RPC 2.0 responses to a `Write` sink.

#[cfg(test)]
pub mod mock {
    use serde_json::Value;
    use std::io::{BufRead, Write};

    /// Run a mock ACP server that processes NDJSON requests from `stdin`
    /// and writes responses to `stdout`.
    ///
    /// Supported methods:
    /// - `initialize` — returns server info
    /// - `prompt` — echoes back with "[mock] Processed: " prefix
    /// - `cancel` — returns `{ "cancelled": true }`
    /// - Unknown method — returns JSON-RPC error -32601
    pub fn run_mock_inline(stdin: impl BufRead, mut stdout: impl Write) {
        for line in stdin.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let req: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let id = req.get("id").cloned();
            let method = req
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("");

            let response = match method {
                "initialize" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "serverInfo": {
                            "name": "mock-acp-server",
                            "version": "0.1.0"
                        }
                    }
                }),

                "prompt" => {
                    let text = req
                        .get("params")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let reply = format!("[mock] Processed: {}", text);
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": reply
                        }
                    })
                }

                "cancel" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "cancelled": true
                    }
                }),

                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {}", method)
                    }
                }),
            };

            let mut out = serde_json::to_string(&response).unwrap();
            out.push('\n');
            let _ = stdout.write_all(out.as_bytes());
            let _ = stdout.flush();
        }
    }
}
