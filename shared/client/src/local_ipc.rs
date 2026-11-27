//! Length-prefixed local IPC independent of WebSocket framing.

use std::io;

use aleph_protocol::framing::{
    decode_header, encode_header, DEFAULT_MAX_PAYLOAD_BYTES, HEADER_BYTES,
};
use aleph_protocol::panel_voice::{MAX_WAV_BASE64_BYTES, VOICE_TURN_TRANSCRIBE_METHOD};
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(any(unix, windows))]
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(windows)]
pub mod windows;

// Do not expose the Windows endpoint until its native security gates pass.
pub const AVAILABLE: bool = cfg!(unix);

pub const MAX_FRAME_BYTES: u32 = DEFAULT_MAX_PAYLOAD_BYTES;
pub const MAX_INFLIGHT: u32 = 8;
pub const HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const MAX_DEPTH: usize = 64;
const MAX_ITEMS: usize = 4096;
/// Maximum UTF-8 byte length of ordinary strings in a v1 JSON payload.
pub const MAX_STRING_BYTES: usize = 65_536;

/// EOF before a new header is clean; EOF within a frame is an error.
/// If cancelled, discard the stream rather than restarting at a partial frame.
pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<String>> {
    let Some(len) = read_frame_length(reader).await? else {
        return Ok(None);
    };
    read_body(reader, len).await.map(Some)
}

/// Reserve wire bytes after validating the header and before allocating the body.
#[cfg(any(unix, windows))]
pub async fn read_frame_budgeted(
    reader: &mut (impl AsyncRead + Unpin),
    budget: &std::sync::Arc<Semaphore>,
) -> io::Result<Option<(String, OwnedSemaphorePermit)>> {
    let Some(len) = read_frame_length(reader).await? else {
        return Ok(None);
    };
    let permit = budget
        .clone()
        .acquire_many_owned(len)
        .await
        .map_err(invalid_data)?;
    Ok(Some((read_body(reader, len).await?, permit)))
}

async fn read_frame_length(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<u32>> {
    let mut header = [0; HEADER_BYTES];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    decode_header(header, MAX_FRAME_BYTES)
        .map(Some)
        .map_err(|_| invalid_data("invalid IPC frame length"))
}

async fn read_body(reader: &mut (impl AsyncRead + Unpin), len: u32) -> io::Result<String> {
    let mut body = vec![0; len as usize];
    reader.read_exact(&mut body).await?;
    String::from_utf8(body).map_err(invalid_data)
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
    parse_bounded_payload(text, None)
}

pub fn parse_request_payload(text: &str) -> io::Result<Value> {
    let value = parse_bounded_payload(text, Some(("params", MAX_WAV_BASE64_BYTES)))?;
    if has_long_voice_audio(&value, "params")
        && value.get("method").and_then(Value::as_str) != Some(VOICE_TURN_TRANSCRIBE_METHOD)
    {
        return Err(invalid_data("IPC string limit exceeded"));
    }
    Ok(value)
}

/// `accepts_synthesis_id` must match responses to a dispatched or pending synthesis call.
pub fn parse_voice_response_payload(
    text: &str,
    frame_budget: usize,
    accepts_synthesis_id: impl FnOnce(&Value) -> bool,
) -> io::Result<Value> {
    if text.len() > frame_budget {
        return Err(invalid_data("IPC frame length exceeded"));
    }
    let value = parse_bounded_payload(text, Some(("result", frame_budget)))?;
    if has_long_voice_audio(&value, "result")
        && (value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || value.get("method").is_some()
            || value.get("error").is_some()
            || !value.get("id").is_some_and(accepts_synthesis_id))
    {
        return Err(invalid_data("IPC string limit exceeded"));
    }
    Ok(value)
}

fn has_long_voice_audio(value: &Value, parent: &str) -> bool {
    value
        .get(parent)
        .and_then(|parent| parent.get("audio_base64"))
        .and_then(Value::as_str)
        .is_some_and(|audio| audio.len() > MAX_STRING_BYTES)
}

fn parse_bounded_payload(
    text: &str,
    voice_audio: Option<(&'static str, usize)>,
) -> io::Result<Value> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = JsonValue {
        depth: 0,
        position: voice_audio.map_or(JsonPosition::Ordinary, |(parent, max)| {
            JsonPosition::Root(parent, max)
        }),
    }
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
#[derive(Clone, Copy)]
enum JsonPosition {
    Ordinary,
    Root(&'static str, usize),
    VoiceAudioParent(usize),
    VoiceAudio(usize),
}

struct JsonValue {
    depth: usize,
    position: JsonPosition,
}

impl JsonValue {
    fn child(&self, position: JsonPosition) -> Self {
        Self {
            depth: self.depth + 1,
            position,
        }
    }
}

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
        let max = match self.position {
            JsonPosition::VoiceAudio(max) => max,
            _ => MAX_STRING_BYTES,
        };
        if value.len() > max {
            return Err(E::custom("IPC string limit exceeded"));
        }
        Ok(value.into())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(A::Error::custom("IPC JSON depth limit exceeded"));
        }
        let mut items = Vec::new();
        while let Some(value) = seq.next_element_seed(self.child(JsonPosition::Ordinary))? {
            if items.len() == MAX_ITEMS {
                return Err(A::Error::custom("IPC array limit exceeded"));
            }
            items.push(value);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
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
            let position = match self.position {
                JsonPosition::Root(parent, max) if key == parent => {
                    JsonPosition::VoiceAudioParent(max)
                }
                JsonPosition::VoiceAudioParent(max) if key == "audio_base64" => {
                    JsonPosition::VoiceAudio(max)
                }
                _ => JsonPosition::Ordinary,
            };
            let value = map.next_value_seed(self.child(position))?;
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
        path::{Path, PathBuf},
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
        let endpoint = private_endpoint(endpoint)?;
        let socket = fs::symlink_metadata(&endpoint)?;
        if !socket.file_type().is_socket() || socket.uid() != effective_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC endpoint is not an owned socket",
            ));
        }
        let stream = UnixStream::connect(endpoint).await?;
        require_same_user(&stream)?;
        Ok(stream)
    }

    pub fn private_endpoint(endpoint: &Path) -> io::Result<PathBuf> {
        let name = endpoint
            .file_name()
            .filter(|_| endpoint.is_absolute())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "IPC endpoint must be an absolute socket path",
                )
            })?;
        let parent = endpoint.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let uid = effective_uid();
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC endpoint requires an owned mode-0700 parent",
            ));
        }
        let canonical = fs::canonicalize(parent)?;
        if !fs::symlink_metadata(&canonical)
            .is_ok_and(|live| live.dev() == metadata.dev() && live.ino() == metadata.ino())
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC parent changed",
            ));
        }
        // Use the checked path, not a caller-provided ancestor alias. A root-owned
        // sticky directory (e.g. /private/tmp) can contain an owned private dir.
        for ancestor in canonical.ancestors().skip(1) {
            let entry = fs::symlink_metadata(ancestor)?;
            let trusted_owner = entry.uid() == 0 || entry.uid() == uid;
            let root_sticky = entry.uid() == 0 && entry.mode() & 0o1000 != 0;
            let writable_by_others = entry.mode() & 0o022 != 0;
            if !entry.is_dir() || !trusted_owner || (writable_by_others && !root_sticky) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "IPC parent has an untrusted ancestor",
                ));
            }
        }
        Ok(canonical.join(name))
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
    fn voice_turn_request_parser_only_admits_exact_audio_path() {
        let max = "A".repeat(MAX_WAV_BASE64_BYTES);
        let before_method = format!(
            r#"{{"params":{{"audio_base64":"{max}"}},"method":"{VOICE_TURN_TRANSCRIBE_METHOD}"}}"#
        );
        assert!(parse_request_payload(&before_method).is_ok());
        assert!(parse_payload(&before_method).is_err());

        let over = format!(
            r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"audio_base64":"{max}A"}}}}"#
        );
        assert!(parse_request_payload(&over).is_err());
        let long = "A".repeat(MAX_STRING_BYTES + 1);
        for bad in [
            format!(r#"{{"method":"voice.transcribe","params":{{"audio_base64":"{long}"}}}}"#),
            format!(
                r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":[{{"audio_base64":"{long}"}}]}}"#
            ),
            format!(
                r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"nested":{{"audio_base64":"{long}"}}}}}}"#
            ),
            format!(
                r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"audio_base64":["{long}"]}}}}"#
            ),
            format!(
                r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"text":"{long}"}}}}"#
            ),
            format!(
                r#"{{"method":"voice.transcribe","method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"audio_base64":"{long}"}}}}"#
            ),
            format!(
                r#"{{"method":"{VOICE_TURN_TRANSCRIBE_METHOD}","params":{{"audio_base64":"{long}","audio_base64":"A"}}}}"#
            ),
        ] {
            assert!(parse_request_payload(&bad).is_err());
        }
    }

    #[test]
    fn voice_turn_response_parser_requires_correlated_synthesis() {
        let long = "A".repeat(MAX_STRING_BYTES + 1);
        let exact = format!(r#"{{"result":{{"audio_base64":"{long}"}},"id":1,"jsonrpc":"2.0"}}"#);
        let accepts = |id: &Value| id.as_i64() == Some(1);
        assert!(parse_voice_response_payload(&exact, exact.len(), accepts).is_ok());
        assert!(parse_voice_response_payload(&exact, exact.len() - 1, accepts).is_err());
        assert!(parse_voice_response_payload(&exact, exact.len(), |_| false).is_err());
        assert!(parse_payload(&exact).is_err());
        for bad in [
            format!(r#"{{"jsonrpc":"2.0","result":{{"audio_base64":"{long}"}}}}"#),
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"notice","result":{{"audio_base64":"{long}"}}}}"#
            ),
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"nested":{{"audio_base64":"{long}"}}}}}}"#
            ),
            format!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"text":"{long}"}}}}"#),
            format!(r#"{{"jsonrpc":"2.0","id":1,"id":2,"result":{{"audio_base64":"{long}"}}}}"#),
        ] {
            assert!(parse_voice_response_payload(&bad, bad.len(), accepts).is_err());
        }
    }

    #[test]
    fn json_shape_limits() {
        assert!(parse_payload(&format!("{}0{}", "[".repeat(64), "]".repeat(64))).is_ok());
        assert!(parse_payload(&format!("{}0{}", "[".repeat(65), "]".repeat(65))).is_err());
        assert!(parse_payload(&serde_json::to_string(&vec![0; 4097]).unwrap()).is_err());
        assert!(parse_payload(&serde_json::to_string(&"a".repeat(65_537)).unwrap()).is_err());
        assert!(parse_payload(r#"{"id":"one","id":"two"}"#).is_err());
    }
}
