//! The `2026-07-28` ("modern") MCP wire shape.
//!
//! Revision `2026-07-28` made MCP stateless: it removed the
//! `initialize`/`notifications/initialized` handshake, protocol-level sessions
//! (and the `Mcp-Session-Id` header), `ping`, `logging/setLevel`, SSE stream
//! resumability, and server-initiated requests. Every request is now
//! self-contained and carries its own protocol version, client identity, and
//! client capabilities in `_meta`.
//!
//! Servers from every earlier revision still speak the handshake, so Aleph is a
//! *dual-era* client: it detects which era a server implements once, caches that
//! for the life of the connection, and speaks the matching dialect. This module
//! owns everything specific to the modern shape; the legacy path is untouched
//! and still lives in [`crate::mcp::protocol`] (`InitializeParams`) plus the
//! session handling in [`crate::mcp::transport::http`].
//!
//! Submodules:
//! - [`discover`] — the `server/discover` RPC, used both to learn a server's
//!   capabilities and as the era probe.
//! - [`mrtr`] — Multi Round-Trip Requests, which replace server-initiated
//!   sampling / elicitation / roots.
//! - [`headers`] — the HTTP request-metadata headers a client must mirror from
//!   the request body, plus `x-mcp-header` tool-parameter mirroring.

pub mod cache;
pub mod discover;
pub mod headers;
pub mod mrtr;

use serde_json::{Map, Value};

use crate::mcp::jsonrpc::JsonRpcError;
use crate::mcp::protocol::{ClientCapabilities, ClientInfo};

/// The modern protocol revision Aleph speaks.
///
/// Sent in every request's `_meta` and, on Streamable HTTP, mirrored into the
/// `MCP-Protocol-Version` header. Unlike the legacy revision this is *not*
/// negotiated down by a handshake: a server that does not implement it answers
/// with [`error_codes::UNSUPPORTED_PROTOCOL_VERSION`] and lists what it does
/// support, and the client retries with a mutually supported version.
pub const MCP_MODERN_PROTOCOL_VERSION: &str = "2026-07-28";

/// `_meta` key carrying the protocol version of an individual request.
pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta` key carrying the client's name and version.
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
/// `_meta` key carrying the capabilities the client can service.
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
/// `_meta` key under which a server reports its own identity in each result.
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// The `_meta` member name itself.
pub const META_FIELD: &str = "_meta";

/// Error codes the MCP specification reserves for itself.
///
/// `2026-07-28` partitioned the JSON-RPC server-error range: `-32000..=-32019`
/// stays implementation-defined, `-32020..=-32099` belongs to the spec. Codes in
/// the reserved range are therefore proof that the peer speaks a modern
/// revision — which is exactly how the Streamable HTTP fallback tells a modern
/// `400` apart from a legacy one (see [`is_modern_error`]).
pub mod error_codes {
    /// HTTP headers disagree with the request body, or a required header is
    /// missing or malformed.
    pub const HEADER_MISMATCH: i32 = -32020;
    /// The server needs a client capability the client did not declare.
    pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i32 = -32021;
    /// The server does not implement the requested protocol version.
    pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;

    /// Inclusive bounds of the range reserved for the specification.
    pub const SPEC_RESERVED_RANGE: std::ops::RangeInclusive<i32> = -32099..=-32020;
}

/// Whether a JSON-RPC error was minted by the specification itself.
///
/// Used to discriminate eras: a `400 Bad Request` whose body carries a
/// spec-reserved code comes from a modern server that rejected *this request*
/// (wrong version, header mismatch, undeclared capability), not from a legacy
/// server that failed to understand a handshake-less request. The former means
/// "correct the request and retry"; the latter means "fall back to
/// `initialize`".
#[must_use]
pub const fn is_modern_error(code: i32) -> bool {
    // `RangeInclusive::contains` is not const, so compare the bounds directly.
    code <= -32020 && code >= -32099
}

/// A server's answer to a protocol version it does not implement.
///
/// Carried as the `data` member of an [`error_codes::UNSUPPORTED_PROTOCOL_VERSION`]
/// error. The client is expected to pick a mutually supported version from
/// `supported` and retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVersion {
    /// Versions the server does implement, in the server's preference order.
    pub supported: Vec<String>,
    /// The version the client asked for, echoed back when the server provides it.
    pub requested: Option<String>,
}

impl UnsupportedVersion {
    /// Extract the version-negotiation payload from a JSON-RPC error, if this is
    /// one. Returns `None` for every other error, including a
    /// `-32022` whose `data` is missing or malformed — a server that says
    /// "unsupported" without saying what it *does* support leaves the client
    /// nothing to retry with.
    #[must_use]
    pub fn from_error(error: &JsonRpcError) -> Option<Self> {
        if error.code != error_codes::UNSUPPORTED_PROTOCOL_VERSION {
            return None;
        }
        let data = error.data.as_ref()?;
        let supported: Vec<String> = data
            .get("supported")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if supported.is_empty() {
            return None;
        }
        Some(Self {
            supported,
            requested: data
                .get("requested")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// Which era a server speaks. Determined once per server and cached for the
/// life of the connection — the spec makes this a property of the server, not
/// of an individual request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpDialect {
    /// `2026-07-28` and later: no handshake, per-request `_meta`, no sessions.
    Modern {
        /// The revision agreed on for this connection. Normally
        /// [`MCP_MODERN_PROTOCOL_VERSION`], or a later one the server named
        /// after an [`UnsupportedVersion`] retry.
        version: String,
    },
    /// `2025-11-25` and earlier: an `initialize` handshake establishes a
    /// session, and the negotiated revision applies to every later request.
    Legacy {
        /// The revision the server chose during `initialize`.
        version: String,
    },
}

impl McpDialect {
    /// Whether this connection speaks the stateless per-request-metadata shape.
    #[must_use]
    pub const fn is_modern(&self) -> bool {
        matches!(self, Self::Modern { .. })
    }

    /// The protocol revision in force on this connection.
    #[must_use]
    pub fn version(&self) -> &str {
        match self {
            Self::Modern { version } | Self::Legacy { version } => version,
        }
    }
}

/// The `_meta` block a modern client attaches to every request.
///
/// Built once per connection because all three members are constant for its
/// lifetime; attaching is then a small map clone rather than three
/// serializations per request.
#[derive(Debug, Clone)]
pub struct RequestMeta {
    meta: Map<String, Value>,
}

impl RequestMeta {
    /// Build the per-request metadata for a connection.
    ///
    /// `capabilities` is a promise: a server **must not** ask (via
    /// [`mrtr`]) for input the client did not declare it can produce, so
    /// whatever is advertised here must have a live handler behind it.
    #[must_use]
    pub fn new(version: &str, client: &ClientInfo, capabilities: &ClientCapabilities) -> Self {
        let mut meta = Map::new();
        meta.insert(
            META_PROTOCOL_VERSION.to_string(),
            Value::String(version.to_string()),
        );
        // `ClientInfo` and `ClientCapabilities` are plain structs of owned
        // fields; serialization cannot fail. Fall back to a null-free empty
        // object rather than panicking if that ever stops being true.
        meta.insert(
            META_CLIENT_INFO.to_string(),
            serde_json::to_value(client).unwrap_or_else(|_| Value::Object(Map::new())),
        );
        meta.insert(
            META_CLIENT_CAPABILITIES.to_string(),
            serde_json::to_value(capabilities).unwrap_or_else(|_| Value::Object(Map::new())),
        );
        Self { meta }
    }

    /// Attach this metadata to a request's params, preserving any `_meta`
    /// members the caller already set (the spec's `_meta` is a shared,
    /// prefix-namespaced map — progress tokens and trace context live there
    /// too).
    ///
    /// Params absent or `null` become a fresh object holding only `_meta`;
    /// every core MCP method takes an object, so a non-object is a caller bug
    /// and is returned untouched rather than silently rewritten.
    #[must_use]
    pub fn attach(&self, params: Option<Value>) -> Value {
        let mut params = match params {
            Some(Value::Object(map)) => map,
            None | Some(Value::Null) => Map::new(),
            Some(other) => {
                debug_assert!(false, "MCP request params must be a JSON object");
                tracing::error!(
                    "MCP request params were not a JSON object; \
                     required _meta could not be attached"
                );
                return other;
            }
        };

        match params.get_mut(META_FIELD) {
            Some(Value::Object(existing)) => {
                for (key, value) in self.meta.clone() {
                    existing.insert(key, value);
                }
            }
            _ => {
                params.insert(META_FIELD.to_string(), Value::Object(self.meta.clone()));
            }
        }
        Value::Object(params)
    }
}

/// What a server said about the completeness of a result.
///
/// Every modern result carries `resultType`. Results from earlier revisions
/// omit it, and the spec requires clients to read that absence as
/// [`ResultKind::Complete`] — so a legacy server never accidentally looks like
/// it is asking for more input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    /// An ordinary, final result.
    Complete,
    /// The server needs more input before it can finish; see [`mrtr`].
    InputRequired,
}

/// `resultType` value for an ordinary result.
pub const RESULT_TYPE_COMPLETE: &str = "complete";
/// `resultType` value for a Multi Round-Trip interim result.
pub const RESULT_TYPE_INPUT_REQUIRED: &str = "input_required";

/// Classify a raw JSON-RPC result.
///
/// An unrecognized `resultType` is treated as [`ResultKind::Complete`]: a
/// future revision may add kinds, and passing the payload through unchanged
/// degrades better than failing a call the server considers finished.
#[must_use]
pub fn result_kind(result: &Value) -> ResultKind {
    match result.get("resultType").and_then(Value::as_str) {
        Some(RESULT_TYPE_INPUT_REQUIRED) => ResultKind::InputRequired,
        _ => ResultKind::Complete,
    }
}

/// The identity a modern client reports in every request.
#[must_use]
pub fn aleph_client_info() -> ClientInfo {
    ClientInfo {
        name: "Aleph".to_string(),
        version: env!("ALEPH_VERSION").to_string(),
    }
}

/// The capabilities Aleph declares to modern servers.
///
/// Deliberately narrower than the set of things a server *may* ask for, and
/// narrower still when the connection cannot service them. A server must not
/// send an [`mrtr`] input request for an undeclared capability, so this is the
/// lever that keeps a server from asking for something that would come back
/// empty — every declared capability here has to have a live handler behind it.
///
/// `sampling` is declared only when the connection actually carries a
/// [`crate::mcp::sampling::SamplingHandler`]; connections without one (agent-scoped
/// inline servers spawned outside `McpClient`) declare nothing and are simply
/// never asked.
///
/// Two capabilities are deliberately never declared:
///
/// - `elicitation` would need the clarification subsystem
///   (`crate::clarification`) threaded down into the MCP connection layer,
///   which no current seam provides.
/// - `roots` is deprecated in this revision and Aleph exposes no root set.
#[must_use]
pub fn aleph_client_capabilities(can_sample: bool) -> ClientCapabilities {
    ClientCapabilities {
        sampling: can_sample.then_some(crate::mcp::protocol::SamplingCapability {}),
        experimental: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta() -> RequestMeta {
        RequestMeta::new(
            MCP_MODERN_PROTOCOL_VERSION,
            &aleph_client_info(),
            &aleph_client_capabilities(true),
        )
    }

    #[test]
    fn attaches_meta_to_object_params() {
        let params = meta().attach(Some(json!({"name": "get_weather"})));

        assert_eq!(params["name"], "get_weather");
        assert_eq!(
            params[META_FIELD][META_PROTOCOL_VERSION],
            MCP_MODERN_PROTOCOL_VERSION
        );
        assert_eq!(params[META_FIELD][META_CLIENT_INFO]["name"], "Aleph");
        assert!(params[META_FIELD][META_CLIENT_CAPABILITIES]
            .get("sampling")
            .is_some());
    }

    #[test]
    fn attaches_meta_when_params_absent() {
        let params = meta().attach(None);

        assert_eq!(
            params[META_FIELD][META_PROTOCOL_VERSION],
            MCP_MODERN_PROTOCOL_VERSION
        );
    }

    #[test]
    fn preserves_caller_supplied_meta_members() {
        let params = meta().attach(Some(json!({
            "name": "t",
            "_meta": {"progressToken": 7}
        })));

        // Both the caller's key and the required protocol keys survive.
        assert_eq!(params[META_FIELD]["progressToken"], 7);
        assert_eq!(
            params[META_FIELD][META_PROTOCOL_VERSION],
            MCP_MODERN_PROTOCOL_VERSION
        );
    }

    #[test]
    fn declared_capabilities_are_only_those_with_handlers() {
        let caps = serde_json::to_value(aleph_client_capabilities(true)).unwrap();

        // Advertising a capability Aleph cannot service would let a server ask
        // for input that can never be produced.
        assert!(caps.get("sampling").is_some());
        assert!(caps.get("elicitation").is_none());
        assert!(caps.get("roots").is_none());
    }

    #[test]
    fn a_connection_without_a_sampler_declares_none() {
        // A server must not send an input request for an undeclared capability,
        // so withholding the claim is what keeps such a connection from being
        // asked for something it could only answer with an error.
        let caps = serde_json::to_value(aleph_client_capabilities(false)).unwrap();

        assert!(caps.get("sampling").is_none());
    }

    #[test]
    fn header_version_and_body_version_share_one_source() {
        // The transport mirrors the dialect's version into MCP-Protocol-Version
        // while the body carries the `_meta` copy; a server rejects the request
        // if they differ, so both must resolve to the same revision.
        let params = meta().attach(None);

        assert_eq!(
            params[META_FIELD][META_PROTOCOL_VERSION],
            MCP_MODERN_PROTOCOL_VERSION
        );
    }

    #[test]
    fn missing_result_type_reads_as_complete() {
        // Required by the spec so results from earlier revisions stay usable.
        assert_eq!(result_kind(&json!({"content": []})), ResultKind::Complete);
        assert_eq!(
            result_kind(&json!({"resultType": "complete"})),
            ResultKind::Complete
        );
        assert_eq!(
            result_kind(&json!({"resultType": "input_required"})),
            ResultKind::InputRequired
        );
    }

    #[test]
    fn unknown_result_type_reads_as_complete() {
        assert_eq!(
            result_kind(&json!({"resultType": "invented_by_a_later_revision"})),
            ResultKind::Complete
        );
    }

    #[test]
    fn spec_reserved_codes_identify_a_modern_peer() {
        assert!(is_modern_error(error_codes::HEADER_MISMATCH));
        assert!(is_modern_error(
            error_codes::MISSING_REQUIRED_CLIENT_CAPABILITY
        ));
        assert!(is_modern_error(error_codes::UNSUPPORTED_PROTOCOL_VERSION));
        assert!(is_modern_error(-32099));

        // Implementation-defined range: says nothing about the peer's era.
        assert!(!is_modern_error(-32000));
        assert!(!is_modern_error(-32019));
        // Standard JSON-RPC codes are era-agnostic too.
        assert!(!is_modern_error(-32601));
        assert!(!is_modern_error(-32602));
    }

    #[test]
    fn parses_unsupported_version_payload() {
        let error = JsonRpcError {
            code: error_codes::UNSUPPORTED_PROTOCOL_VERSION,
            message: "Unsupported protocol version".to_string(),
            data: Some(json!({
                "supported": ["2026-07-28", "2025-11-25"],
                "requested": "1900-01-01"
            })),
        };

        let parsed = UnsupportedVersion::from_error(&error).unwrap();
        assert_eq!(parsed.supported, vec!["2026-07-28", "2025-11-25"]);
        assert_eq!(parsed.requested.as_deref(), Some("1900-01-01"));
    }

    #[test]
    fn unsupported_version_without_alternatives_is_not_actionable() {
        // Nothing to retry with, so the caller must surface the error instead
        // of looping.
        let error = JsonRpcError {
            code: error_codes::UNSUPPORTED_PROTOCOL_VERSION,
            message: "no".to_string(),
            data: Some(json!({"supported": []})),
        };
        assert!(UnsupportedVersion::from_error(&error).is_none());

        let error = JsonRpcError {
            code: error_codes::UNSUPPORTED_PROTOCOL_VERSION,
            message: "no".to_string(),
            data: None,
        };
        assert!(UnsupportedVersion::from_error(&error).is_none());
    }

    #[test]
    fn other_errors_are_not_version_negotiation() {
        let error = JsonRpcError {
            code: -32601,
            message: "Method not found".to_string(),
            data: Some(json!({"supported": ["2026-07-28"]})),
        };
        assert!(UnsupportedVersion::from_error(&error).is_none());
    }

    #[test]
    fn dialect_reports_its_revision() {
        let modern = McpDialect::Modern {
            version: MCP_MODERN_PROTOCOL_VERSION.to_string(),
        };
        let legacy = McpDialect::Legacy {
            version: "2025-03-26".to_string(),
        };

        assert!(modern.is_modern());
        assert_eq!(modern.version(), MCP_MODERN_PROTOCOL_VERSION);
        assert!(!legacy.is_modern());
        assert_eq!(legacy.version(), "2025-03-26");
    }
}
