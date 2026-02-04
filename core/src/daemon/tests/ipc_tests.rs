#[cfg(test)]
mod tests {
    use crate::daemon::*;
    use crate::daemon::ipc::*;
    use tokio::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_ipc_server_creation() {
        let socket_path = "/tmp/aether-test.sock";
        let _ = std::fs::remove_file(socket_path); // Clean up if exists

        let server = IpcServer::new(socket_path.to_string());
        assert_eq!(server.socket_path(), socket_path);
    }

    #[tokio::test]
    async fn test_json_rpc_request_parsing() {
        let request_json = r#"{"jsonrpc":"2.0","method":"daemon.status","id":1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "daemon.status");
        assert_eq!(request.id, serde_json::json!(1));
    }
}
