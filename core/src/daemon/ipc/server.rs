use crate::daemon::{DaemonError, DaemonStatus, Result};
use crate::daemon::ipc::protocol::*;
use serde_json::Value;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info};

pub struct IpcServer {
    socket_path: String,
}

impl IpcServer {
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Start the IPC server
    pub async fn start(&self) -> Result<()> {
        // Remove existing socket file if it exists
        if Path::new(&self.socket_path).exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        // Bind to Unix Domain Socket
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| DaemonError::IpcError(format!("Failed to bind socket: {}", e)))?;

        info!("IPC server listening on {}", self.socket_path);

        // Accept connections
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream).await {
                            error!("Error handling connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a single client connection
    async fn handle_connection(stream: UnixStream) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;

            if bytes_read == 0 {
                // Connection closed
                break;
            }

            debug!("Received request: {}", line.trim());

            // Parse JSON-RPC request
            let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => Self::handle_request(request).await,
                Err(e) => {
                    let error = JsonRpcError::new(
                        Value::Null,
                        PARSE_ERROR,
                        format!("Parse error: {}", e),
                    );
                    serde_json::to_string(&error).unwrap_or_else(|e| {
                        format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"Serialization error: {}"}},"id":null}}"#, e)
                    })
                }
            };

            // Send response
            writer.write_all(response.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }

        Ok(())
    }

    /// Handle a single JSON-RPC request
    async fn handle_request(request: JsonRpcRequest) -> String {
        let result = match request.method.as_str() {
            "daemon.status" => Self::handle_status(request.id).await,
            "daemon.ping" => Self::handle_ping(request.id).await,
            "daemon.shutdown" => Self::handle_shutdown(request.id).await,
            _ => {
                let error = JsonRpcError::new(
                    request.id,
                    METHOD_NOT_FOUND,
                    format!("Method not found: {}", request.method),
                );
                serde_json::to_string(&error).unwrap_or_else(|e| {
                        format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"Serialization error: {}"}},"id":null}}"#, e)
                    })
            }
        };

        result
    }

    async fn handle_status(id: Value) -> String {
        let status = DaemonStatus::Running; // Always running if we can respond
        let result = serde_json::json!({
            "status": status,
            "uptime": 0, // TODO: Track actual uptime
        });

        let response = JsonRpcResponse::new(id, result);
        serde_json::to_string(&response).unwrap_or_else(|e| {
                format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"Serialization error: {}"}},"id":null}}"#, e)
            })
    }

    async fn handle_ping(id: Value) -> String {
        let response = JsonRpcResponse::new(id, serde_json::json!({"pong": true}));
        serde_json::to_string(&response).unwrap_or_else(|e| {
                format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"Serialization error: {}"}},"id":null}}"#, e)
            })
    }

    async fn handle_shutdown(id: Value) -> String {
        // TODO: Implement graceful shutdown
        let response = JsonRpcResponse::new(id, serde_json::json!({"shutting_down": true}));
        serde_json::to_string(&response).unwrap_or_else(|e| {
                format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"Serialization error: {}"}},"id":null}}"#, e)
            })
    }
}
