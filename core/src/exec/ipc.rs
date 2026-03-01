//! IPC protocol for exec approvals via Unix socket.
//!
//! Provides secure communication between Gateway and macOS App for approval requests.
//! Uses HMAC-SHA256 challenge-response authentication.

use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;

use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use super::manager::ExecApprovalManager;
use super::socket::{ApprovalDecisionType, ApprovalRequestPayload};

type HmacSha256 = Hmac<Sha256>;

/// IPC errors
#[derive(Debug, Error)]
pub enum IpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Timeout")]
    Timeout,

    #[error("Connection closed")]
    ConnectionClosed,
}

/// IPC message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage {
    /// Challenge from server (32-byte nonce)
    Challenge { nonce: String },

    /// Response to challenge (HMAC of nonce)
    ChallengeResponse { response: String },

    /// Authentication result
    AuthResult { success: bool, error: Option<String> },

    /// Approval request (server -> client)
    ApprovalRequest {
        id: String,
        request: ApprovalRequestPayload,
        timeout_ms: u64,
    },

    /// Approval decision (client -> server)
    ApprovalDecision {
        id: String,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    },

    /// Get pending approvals request
    GetPending,

    /// Pending approvals response
    PendingList {
        pending: Vec<PendingInfo>,
    },

    /// Error message
    Error { message: String },

    /// Ping/Pong for keepalive
    Ping,
    Pong,
}

/// Simplified pending approval info for IPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInfo {
    pub id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub agent_id: String,
    pub remaining_ms: u64,
}

/// IPC Server (Gateway side)
pub struct IpcServer {
    socket_path: PathBuf,
    token: Vec<u8>,
    manager: Arc<ExecApprovalManager>,
}

impl IpcServer {
    /// Create a new IPC server
    pub fn new(
        socket_path: impl AsRef<Path>,
        token: impl AsRef<[u8]>,
        manager: Arc<ExecApprovalManager>,
    ) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            token: token.as_ref().to_vec(),
            manager,
        }
    }

    /// Get default socket path
    pub fn default_socket_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("exec-approvals.sock")
    }

    /// Start the IPC server
    pub async fn start(&self) -> Result<(), IpcError> {
        // Remove existing socket if present
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        // Ensure parent directory exists
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Set permissions to 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.socket_path, perms)?;
        }

        info!(path = %self.socket_path.display(), "IPC server started");

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let token = self.token.clone();
                    let manager = self.manager.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, &token, manager).await {
                            debug!("IPC connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept IPC connection: {}", e);
                }
            }
        }
    }

    /// Handle a single connection
    async fn handle_connection(
        stream: UnixStream,
        token: &[u8],
        manager: Arc<ExecApprovalManager>,
    ) -> Result<(), IpcError> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Step 1: Send challenge
        let mut nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce);
        let nonce_hex = hex::encode(nonce);

        let challenge = IpcMessage::Challenge {
            nonce: nonce_hex.clone(),
        };
        Self::send_message(&mut writer, &challenge).await?;

        // Step 2: Receive response
        let response_msg = Self::recv_message(&mut reader).await?;
        let response_hex = match response_msg {
            IpcMessage::ChallengeResponse { response } => response,
            _ => {
                let err = IpcMessage::AuthResult {
                    success: false,
                    error: Some("Expected challenge response".to_string()),
                };
                Self::send_message(&mut writer, &err).await?;
                return Err(IpcError::AuthFailed("Expected challenge response".to_string()));
            }
        };

        // Step 3: Verify HMAC
        let expected = Self::compute_hmac(token, &nonce);
        let response_bytes = hex::decode(&response_hex).map_err(|e| {
            IpcError::AuthFailed(format!("Invalid response format: {}", e))
        })?;

        if expected != response_bytes {
            let err = IpcMessage::AuthResult {
                success: false,
                error: Some("Invalid HMAC".to_string()),
            };
            Self::send_message(&mut writer, &err).await?;
            return Err(IpcError::AuthFailed("Invalid HMAC".to_string()));
        }

        // Step 4: Send success
        let success = IpcMessage::AuthResult {
            success: true,
            error: None,
        };
        Self::send_message(&mut writer, &success).await?;

        debug!("IPC client authenticated");

        // Step 5: Handle messages
        loop {
            let msg = match Self::recv_message(&mut reader).await {
                Ok(m) => m,
                Err(IpcError::ConnectionClosed) => break,
                Err(e) => return Err(e),
            };

            let response = Self::handle_message(msg, &manager);
            Self::send_message(&mut writer, &response).await?;
        }

        Ok(())
    }

    /// Handle an incoming message
    fn handle_message(msg: IpcMessage, manager: &ExecApprovalManager) -> IpcMessage {
        match msg {
            IpcMessage::ApprovalDecision {
                id,
                decision,
                resolved_by,
            } => {
                let resolved = manager.resolve(&id, decision, resolved_by);
                if resolved {
                    IpcMessage::AuthResult {
                        success: true,
                        error: None,
                    }
                } else {
                    IpcMessage::Error {
                        message: "Approval not found or already resolved".to_string(),
                    }
                }
            }
            IpcMessage::GetPending => {
                let pending = manager
                    .list_pending()
                    .into_iter()
                    .map(|p| PendingInfo {
                        id: p.record.id,
                        command: p.record.command,
                        cwd: p.record.cwd,
                        agent_id: p.record.agent_id,
                        remaining_ms: p.remaining_ms,
                    })
                    .collect();
                IpcMessage::PendingList { pending }
            }
            IpcMessage::Ping => IpcMessage::Pong,
            _ => IpcMessage::Error {
                message: "Unexpected message type".to_string(),
            },
        }
    }

    /// Compute HMAC-SHA256
    fn compute_hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key size issue");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    /// Send a message
    async fn send_message(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        msg: &IpcMessage,
    ) -> Result<(), IpcError> {
        let json = serde_json::to_string(msg)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    /// Receive a message
    async fn recv_message(
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> Result<IpcMessage, IpcError> {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(IpcError::ConnectionClosed);
        }
        let msg: IpcMessage = serde_json::from_str(&line)?;
        Ok(msg)
    }
}

/// IPC Client (macOS App side)
pub struct IpcClient {
    socket_path: PathBuf,
    token: Vec<u8>,
}

impl IpcClient {
    /// Create a new IPC client
    pub fn new(socket_path: impl AsRef<Path>, token: impl AsRef<[u8]>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            token: token.as_ref().to_vec(),
        }
    }

    /// Connect and authenticate
    pub async fn connect(&self) -> Result<IpcConnection, IpcError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Receive challenge
        let challenge_msg = Self::recv_message(&mut reader).await?;
        let nonce_hex = match challenge_msg {
            IpcMessage::Challenge { nonce } => nonce,
            _ => {
                return Err(IpcError::AuthFailed("Expected challenge".to_string()));
            }
        };

        // Compute response
        let nonce = hex::decode(&nonce_hex)
            .map_err(|e| IpcError::AuthFailed(format!("Invalid nonce: {}", e)))?;
        let hmac = IpcServer::compute_hmac(&self.token, &nonce);
        let response_hex = hex::encode(&hmac);

        // Send response
        let response = IpcMessage::ChallengeResponse {
            response: response_hex,
        };
        Self::send_message(&mut writer, &response).await?;

        // Receive auth result
        let result_msg = Self::recv_message(&mut reader).await?;
        match result_msg {
            IpcMessage::AuthResult { success: true, .. } => {}
            IpcMessage::AuthResult {
                success: false,
                error,
            } => {
                return Err(IpcError::AuthFailed(
                    error.unwrap_or_else(|| "Unknown error".to_string()),
                ));
            }
            _ => {
                return Err(IpcError::AuthFailed("Unexpected response".to_string()));
            }
        }

        Ok(IpcConnection { reader, writer })
    }

    /// Send a message (static helper)
    async fn send_message(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        msg: &IpcMessage,
    ) -> Result<(), IpcError> {
        let json = serde_json::to_string(msg)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    /// Receive a message (static helper)
    async fn recv_message(
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> Result<IpcMessage, IpcError> {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(IpcError::ConnectionClosed);
        }
        let msg: IpcMessage = serde_json::from_str(&line)?;
        Ok(msg)
    }
}

/// An authenticated IPC connection
pub struct IpcConnection {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl IpcConnection {
    /// Send an approval decision
    pub async fn send_decision(
        &mut self,
        id: &str,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    ) -> Result<bool, IpcError> {
        let msg = IpcMessage::ApprovalDecision {
            id: id.to_string(),
            decision,
            resolved_by,
        };
        IpcClient::send_message(&mut self.writer, &msg).await?;

        let response = IpcClient::recv_message(&mut self.reader).await?;
        match response {
            IpcMessage::AuthResult { success, .. } => Ok(success),
            IpcMessage::Error { message } => {
                warn!("Decision error: {}", message);
                Ok(false)
            }
            _ => Err(IpcError::InvalidMessage("Unexpected response".to_string())),
        }
    }

    /// Get pending approvals
    pub async fn get_pending(&mut self) -> Result<Vec<PendingInfo>, IpcError> {
        let msg = IpcMessage::GetPending;
        IpcClient::send_message(&mut self.writer, &msg).await?;

        let response = IpcClient::recv_message(&mut self.reader).await?;
        match response {
            IpcMessage::PendingList { pending } => Ok(pending),
            _ => Err(IpcError::InvalidMessage("Expected pending list".to_string())),
        }
    }

    /// Send ping and wait for pong
    pub async fn ping(&mut self) -> Result<(), IpcError> {
        IpcClient::send_message(&mut self.writer, &IpcMessage::Ping).await?;
        let response = IpcClient::recv_message(&mut self.reader).await?;
        match response {
            IpcMessage::Pong => Ok(()),
            _ => Err(IpcError::InvalidMessage("Expected pong".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hmac() {
        let key = b"secret";
        let data = b"hello";
        let hmac1 = IpcServer::compute_hmac(key, data);
        let hmac2 = IpcServer::compute_hmac(key, data);
        assert_eq!(hmac1, hmac2);

        let hmac3 = IpcServer::compute_hmac(key, b"world");
        assert_ne!(hmac1, hmac3);
    }

    #[test]
    fn test_message_serialization() {
        let msg = IpcMessage::Challenge {
            nonce: "abc123".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("challenge"));
        assert!(json.contains("abc123"));

        let parsed: IpcMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            IpcMessage::Challenge { nonce } => assert_eq!(nonce, "abc123"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decision_message() {
        let msg = IpcMessage::ApprovalDecision {
            id: "req-123".to_string(),
            decision: ApprovalDecisionType::AllowOnce,
            resolved_by: Some("alice".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("approval_decision"));
        assert!(json.contains("allow-once"));
    }
}
