//! Length-prefixed local IPC independent of WebSocket framing.

use std::io;

use aleph_protocol::framing::{
    decode_header, encode_header, DEFAULT_MAX_PAYLOAD_BYTES, HEADER_BYTES,
};
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: u32 = DEFAULT_MAX_PAYLOAD_BYTES;
pub const MAX_INFLIGHT: u32 = 8;
pub const HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const MAX_DEPTH: usize = 64;
const MAX_ITEMS: usize = 4096;
const MAX_STRING_BYTES: usize = 65_536;

/// EOF before a new header is clean; EOF within a frame is an error.
/// If cancelled, discard the stream rather than restarting at a partial frame.
pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<String>> {
    let mut header = [0; HEADER_BYTES];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    let len = decode_header(header, MAX_FRAME_BYTES)
        .map_err(|_| invalid_data("invalid IPC frame length"))?;
    let mut body = vec![0; len as usize];
    reader.read_exact(&mut body).await?;
    String::from_utf8(body).map(Some).map_err(invalid_data)
}

/// If cancelled, discard the stream rather than writing behind a partial frame.
pub async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), text: &str) -> io::Result<()> {
    let header = encode_header(text.len(), MAX_FRAME_BYTES)
        .map_err(|_| invalid_data("invalid IPC frame length"))?;
    writer.write_all(&header).await?;
    writer.write_all(text.as_bytes()).await?;
    writer.flush().await
}

pub fn parse_payload(text: &str) -> io::Result<Value> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = JsonValue(0)
        .deserialize(&mut decoder)
        .map_err(invalid_data)?;
    decoder.end().map_err(invalid_data)?;
    Ok(value)
}

fn invalid_data(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

// Check the shape during deserialization: counting the final map would miss
// repeated keys and their discarded values.
struct JsonValue(usize);

impl<'de> DeserializeSeed<'de> for JsonValue {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for JsonValue {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a bounded JSON value")
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        if value.len() > MAX_STRING_BYTES {
            return Err(E::custom("IPC string limit exceeded"));
        }
        Ok(value.into())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if self.0 >= MAX_DEPTH {
            return Err(A::Error::custom("IPC JSON depth limit exceeded"));
        }
        let mut items = Vec::new();
        while let Some(value) = seq.next_element_seed(JsonValue(self.0 + 1))? {
            if items.len() == MAX_ITEMS {
                return Err(A::Error::custom("IPC array limit exceeded"));
            }
            items.push(value);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        if self.0 >= MAX_DEPTH {
            return Err(A::Error::custom("IPC JSON depth limit exceeded"));
        }
        let mut members = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if members.len() == MAX_ITEMS
                || key.len() > MAX_STRING_BYTES
                || members.contains_key(&key)
            {
                return Err(A::Error::custom(
                    "IPC object limit exceeded or duplicate member",
                ));
            }
            let value = map.next_value_seed(JsonValue(self.0 + 1))?;
            members.insert(key, value);
        }
        Ok(Value::Object(members))
    }
}

#[cfg(unix)]
pub mod unix {
    use std::{
        fs,
        os::unix::fs::{FileTypeExt, MetadataExt},
        path::Path,
    };

    use tokio::net::UnixStream;

    use super::io;

    pub fn effective_uid() -> u32 {
        // SAFETY: geteuid has no preconditions and does not touch memory.
        unsafe { libc::geteuid() }
    }

    pub fn require_same_user(stream: &UnixStream) -> io::Result<()> {
        if stream.peer_cred()?.uid() != effective_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC peer account mismatch",
            ));
        }
        Ok(())
    }

    pub async fn connect(endpoint: &Path) -> io::Result<UnixStream> {
        private_parent(endpoint)?;
        let socket = fs::symlink_metadata(endpoint)?;
        if !socket.file_type().is_socket()
            || socket.uid() != effective_uid()
            || socket.mode() & 0o7777 != 0o600
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC endpoint is not an owned mode-0600 socket",
            ));
        }
        let stream = UnixStream::connect(endpoint).await?;
        require_same_user(&stream)?;
        Ok(stream)
    }

    pub fn private_parent(endpoint: &Path) -> io::Result<fs::Metadata> {
        if !endpoint.is_absolute() || endpoint.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPC endpoint must be an absolute socket path",
            ));
        }
        let parent = endpoint.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC endpoint requires an owned mode-0700 parent",
            ));
        }
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn frames_split_and_coalesce() {
        let (mut tx, mut rx) = tokio::io::duplex(3);
        let send = async {
            write_frame(&mut tx, "{}").await.unwrap();
            write_frame(&mut tx, "[]").await.unwrap();
            drop(tx);
        };
        let receive = async {
            assert_eq!(read_frame(&mut rx).await.unwrap().as_deref(), Some("{}"));
            assert_eq!(read_frame(&mut rx).await.unwrap().as_deref(), Some("[]"));
            assert_eq!(read_frame(&mut rx).await.unwrap(), None);
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(send, receive);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn frames_refuse_invalid_before_body() {
        for header in [0_u32, 8_388_609, u32::MAX] {
            let (mut tx, mut rx) = tokio::io::duplex(4);
            tx.write_all(&header.to_be_bytes()).await.unwrap();
            let error =
                tokio::time::timeout(std::time::Duration::from_secs(2), read_frame(&mut rx))
                    .await
                    .unwrap()
                    .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        for truncated in [&b"\0\0"[..], &b"\0\0\0\x02{"[..]] {
            let mut rx = truncated;
            assert_eq!(
                read_frame(&mut rx).await.unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        let mut non_utf8 = &b"\0\0\0\x01\xff"[..];
        assert_eq!(
            read_frame(&mut non_utf8).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn json_shape_limits() {
        assert!(parse_payload(&format!("{}0{}", "[".repeat(64), "]".repeat(64))).is_ok());
        assert!(parse_payload(&format!("{}0{}", "[".repeat(65), "]".repeat(65))).is_err());
        assert!(parse_payload(&serde_json::to_string(&vec![0; 4097]).unwrap()).is_err());
        assert!(parse_payload(&serde_json::to_string(&"a".repeat(65_537)).unwrap()).is_err());
        assert!(parse_payload(r#"{"id":"one","id":"two"}"#).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_client_uses_hello_and_correlates_out_of_order() {
        use crate::{AlephClient, CliConfig, CliError};
        use aleph_protocol::{JsonRpcRequest, JsonRpcResponse};
        use serde_json::json;
        use std::os::unix::fs::PermissionsExt;
        use tokio::{net::UnixListener, sync::oneshot, time::timeout};

        let dir = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = dir.path().join("ipc");
        let listener = UnixListener::bind(&endpoint).unwrap();
        std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (received, observed) = oneshot::channel();
        let peer = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            unix::require_same_user(&stream).unwrap();
            let hello: JsonRpcRequest =
                serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            assert_eq!(hello.method, "system.hello");
            let response = JsonRpcResponse::success(
                hello.id.unwrap(),
                json!({
                    "protocol": {"major": 1, "minor": 0},
                    "server": {"name": "fixture", "version": "1"},
                    "connection_id": "fixture-connection",
                    "features": [],
                    "limits": {"max_message_bytes": MAX_FRAME_BYTES, "max_inflight_requests": 2},
                }),
            );
            write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
                .await
                .unwrap();
            let first: JsonRpcRequest =
                serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            let second: JsonRpcRequest =
                serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            assert_ne!(first.id, second.id);
            for request in [second, first] {
                let response = JsonRpcResponse::success(request.id.unwrap(), json!(request.method));
                write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
                    .await
                    .unwrap();
            }
            assert!(read_frame(&mut stream).await.unwrap().is_some());
            received.send(()).unwrap();
            assert_eq!(read_frame(&mut stream).await.unwrap(), None);
        };
        let caller = async {
            let hello = serde_json::from_value(json!({
                "protocol": {"major": 1, "minor": 0},
                "client": {"name": "fixture", "version": "1", "platform": "unix", "ui_runtime": "test"},
                "features": [],
            })).unwrap();
            let (client, _events) = AlephClient::connect_local(&endpoint, &hello).await.unwrap();
            let (first, second) = tokio::join!(
                client.call::<(), String>("first", None),
                client.call::<(), String>("second", None)
            );
            assert_eq!(first.unwrap(), "first");
            assert_eq!(second.unwrap(), "second");
            assert!(client.reconnect(&CliConfig::default()).await.is_err());
            let (unanswered, closed) =
                tokio::join!(client.call::<(), Value>("unanswered", None), async {
                    observed.await.unwrap();
                    client.close().await
                });
            closed.unwrap();
            assert!(matches!(unanswered, Err(CliError::Disconnected(_))));
        };
        timeout(HELLO_TIMEOUT, async {
            tokio::join!(peer, caller);
        })
        .await
        .unwrap();
    }
}
