//! `McpHandler` — forwards to MCP tools/call via `McpClient::call_tool`. Task 4.
//!
//! Each handler instance wraps one discovered MCP tool. The handler:
//! - Holds an `Arc<McpClient>` (shared across all tools from the same manager)
//! - Pins its originating `server_id` into `ToolSource::Mcp { server_id }`
//! - Maps transport-like `AlephError` variants to `ToolError::Transport` /
//!   `ToolError::Timeout` and everything else to `ToolError::Execution`
//!   (design §6).
//!
//! The qualified tool name injected into the registry is `{server_id}__{tool}`
//! to avoid collisions across servers — see design §4.3.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::AlephError;
use crate::mcp::McpClient;
use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};

pub struct McpHandler {
    client: Arc<McpClient>,
    server_id: String,
    tool_name: String,
    description: String,
    input_schema: Value,
    /// Scheduling/approval flags derived from the server's `ToolAnnotations`
    /// (see `McpTool`): `concurrent_safe` ← readOnlyHint, `idempotent` ←
    /// idempotentHint, `requires_approval` ← destructiveHint. Defaults are
    /// all-false (whole-world exclusive, no auto-retry, no confirmation).
    ///
    /// `max_duration_ms` is seeded from the MCP client's own request timeout
    /// (see [`Self::with_timeout_seconds`]) — never `None`, so the harness
    /// cannot mistake an MCP tool for an unbudgeted one and abort the run on
    /// a slow call.
    metadata: ToolDefinitionMetadata,
}

impl McpHandler {
    pub fn new(
        client: Arc<McpClient>,
        server_id: String,
        tool_name: String,
        description: String,
        input_schema: Value,
    ) -> Self {
        Self {
            client,
            server_id,
            tool_name,
            description,
            input_schema,
            metadata: ToolDefinitionMetadata {
                max_duration_ms: Some(crate::tools::budget::mcp_tool_budget_ms(None)),
                ..ToolDefinitionMetadata::default()
            },
        }
    }

    /// Attach annotation-derived scheduling/approval flags.
    #[must_use]
    pub const fn with_flags(
        mut self,
        read_only: bool,
        idempotent: bool,
        requires_approval: bool,
    ) -> Self {
        self.metadata.concurrent_safe = read_only;
        self.metadata.idempotent = idempotent;
        self.metadata.requires_approval = requires_approval;
        self
    }

    /// Declare the owning server's configured request timeout (`None` = the
    /// client's own remote default) as this tool's wall-clock budget, plus
    /// headroom.
    ///
    /// The two clocks must stay ordered: the MCP client has to be the one that
    /// gives up, so the model receives a real `Timeout` tool error it can act
    /// on. Before this, MCP tools declared no budget at all and the harness
    /// killed the run at its own fallback (120s) — well before the client's
    /// 300s default could ever return gracefully.
    #[must_use]
    pub fn with_timeout_seconds(mut self, timeout_seconds: Option<u64>) -> Self {
        self.metadata.max_duration_ms =
            Some(crate::tools::budget::mcp_tool_budget_ms(timeout_seconds));
        self
    }

    /// The provider-safe qualified name used in the registry:
    /// `{server_id}__{short_tool_name}`, restricted to `[A-Za-z0-9_-]` and
    /// 64 chars (OpenAI-compatible function-name charset; Anthropic and
    /// Gemini accept the same alphabet).
    ///
    /// `tool_name` arrives in the manager's namespaced form
    /// (`{server}:{tool}` — see `McpServerConnection::refresh_tools`); the
    /// redundant prefix is stripped before composing so the LLM never sees
    /// `server__server:tool`, and any residual unsafe character (`:`, `.`,
    /// unicode) is mapped to `_`.
    #[must_use]
    pub fn qualified_name(&self) -> String {
        let short = self
            .tool_name
            .strip_prefix(&format!("{}:", self.server_id))
            .unwrap_or(&self.tool_name);
        sanitize_tool_name(&format!("{}__{}", self.server_id, short))
    }
}

/// Whether `name` is a registry key [`McpHandler::qualified_name`] can
/// produce for some tool of `server_id`: the same composition
/// (`{server_id}__{tool}` through [`sanitize_tool_name`]) decides, so the two
/// cannot drift. A glob, an over-long name or a bare `{server}__` is no key.
/// A server id so long that its own `__` prefix fills the 64-character key
/// leaves no room for a tool part, and owns nothing here.
pub(crate) fn is_tool_key_of_server(server_id: &str, name: &str) -> bool {
    let prefix = sanitize_tool_name(&format!("{server_id}__"));
    name.len() > prefix.len() && name.starts_with(&prefix) && sanitize_tool_name(name) == name
}

/// Map a candidate registry name onto the provider-safe alphabet
/// `[A-Za-z0-9_-]`, truncating to 64 characters. Strict-schema providers
/// (`OpenAI` function calling) reject names outside this set with a request-
/// level 400, which would poison the whole turn for every tool.
pub(crate) fn sanitize_tool_name(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

#[async_trait]
impl ToolHandler for McpHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let qualified = self.qualified_name();
        // The underlying McpClient resolves tools by their *inner* name (the
        // short name reported by the server), not the qualified form.
        match self.client.call_tool(&self.tool_name, input).await {
            Ok(result) => {
                if result.success {
                    Ok(ToolOutput {
                        value: result.content,
                        metadata: ToolOutputMetadata::default(),
                    })
                } else {
                    let error_msg = result
                        .error
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "MCP tool returned failure without message".to_string());
                    Err(ToolError::Execution {
                        name: qualified,
                        cause: crate::mcp::redact_mcp_error(&error_msg),
                    })
                }
            }
            Err(e) => Err(map_mcp_error(qualified, e)),
        }
    }

    fn concurrency_claim(&self, _input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        // with_flags binds these to this MCP tool's own annotations. Retry
        // safety alone is not read-only, and concurrency safety alone must not
        // become authorization to bypass Plan/side-question gates.
        if self.metadata.idempotent && self.metadata.concurrent_safe {
            crate::tools::concurrency::ConcurrencyClaim::Shared
        } else {
            crate::tools::concurrency::ConcurrencyClaim::global()
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.qualified_name(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            source: ToolSource::Mcp {
                server_id: self.server_id.clone(),
            },
            metadata: self.metadata.clone(),
        }
    }
}

/// Map an `AlephError` from a tools/call roundtrip into a `ToolError`.
///
/// Transport-like variants (network, I/O, timeouts) are marked retryable via
/// `ToolError::Transport` / `ToolError::Timeout`. Everything else — including
/// MCP protocol errors — is an `Execution` failure.
///
/// Free-text `cause` strings are passed through [`crate::mcp::redact_mcp_error`]
/// so a server that echoes back a secret-bearing argument or URL cannot leak
/// it into conversation history.
fn map_mcp_error(name: String, err: AlephError) -> ToolError {
    match err {
        AlephError::NetworkError { message, .. } => ToolError::Transport {
            name,
            cause: crate::mcp::redact_mcp_error(&message),
        },
        AlephError::IoError(msg) => ToolError::Transport {
            name,
            cause: crate::mcp::redact_mcp_error(&msg),
        },
        AlephError::McpTimeout => ToolError::Timeout {
            name,
            elapsed_ms: 0, // concrete latency not surfaced by this variant
        },
        AlephError::Timeout { .. } => ToolError::Timeout {
            name,
            elapsed_ms: 0,
        },
        AlephError::McpToolNotFound(tool) => ToolError::NotFound { name: tool },
        // Surface schema-violation prose (whether ours or relayed from the
        // MCP server) as a fixable validation failure so the LLM can rewrite
        // and retry on its next turn.
        AlephError::Validation(cause) => ToolError::ValidationFailed {
            name,
            cause: crate::mcp::redact_mcp_error(&cause),
        },
        other => ToolError::Execution {
            name,
            cause: crate::mcp::redact_mcp_error(&other.to_string()),
        },
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The ownership test is derived from the handler's own key: whatever
    /// `qualified_name` produces for a server, that server owns — and nothing
    /// that no handler of it can produce.
    #[test]
    fn a_tool_key_belongs_to_the_server_whose_handler_made_it() {
        let server = crate::extension::mcp_config::plugin_server_id("my-plug", "my_srv");
        let key = McpHandler::new(
            Arc::new(McpClient::new()),
            server.clone(),
            "do_thing".into(),
            String::new(),
            json!({"type": "object"}),
        )
        .qualified_name();
        assert!(is_tool_key_of_server(&server, &key), "{key}");
        let other = crate::extension::mcp_config::plugin_server_id("my-plug", "other");
        assert!(!is_tool_key_of_server(&other, &key), "another server");
        assert!(
            !is_tool_key_of_server(&server, "plugin_my-plug_my_srv__"),
            "no tool part"
        );
        assert!(
            !is_tool_key_of_server(&server, "plugin_my-plug_my_srv__*"),
            "a glob is not a key any handler makes"
        );
    }

    #[test]
    fn qualified_name_uses_double_underscore() {
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(
            client,
            "time_server".into(),
            "get_time".into(),
            "returns current time".into(),
            json!({"type": "object"}),
        );
        assert_eq!(h.qualified_name(), "time_server__get_time");
    }

    #[test]
    fn definition_projects_source_and_schema() {
        let client = Arc::new(McpClient::new());
        let schema = json!({"type": "object", "properties": {"tz": {"type": "string"}}});
        let h = McpHandler::new(
            client,
            "time_server".into(),
            "get_time".into(),
            "desc".into(),
            schema.clone(),
        );
        let def = h.definition();
        assert_eq!(def.name, "time_server__get_time");
        assert_eq!(def.description, "desc");
        assert_eq!(def.input_schema, schema);
        assert_eq!(
            def.source,
            ToolSource::Mcp {
                server_id: "time_server".into()
            }
        );
    }

    #[test]
    fn qualified_name_strips_redundant_server_prefix() {
        // Production registration receives the manager's namespaced name
        // ("server:tool"); the qualified form must not double-prefix or
        // leak the provider-invalid colon.
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(
            client,
            "github".into(),
            "github:create_issue".into(),
            "d".into(),
            json!({"type": "object"}),
        );
        assert_eq!(h.qualified_name(), "github__create_issue");
    }

    #[test]
    fn qualified_name_sanitizes_unsafe_chars_and_truncates() {
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(
            client,
            "svr".into(),
            "other:tool.v2".into(), // foreign prefix is NOT stripped, only sanitized
            "d".into(),
            json!({}),
        );
        assert_eq!(h.qualified_name(), "svr__other_tool_v2");

        let long = "x".repeat(100);
        assert_eq!(sanitize_tool_name(&long).len(), 64);
    }

    #[test]
    fn with_flags_projects_into_definition_metadata() {
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(
            client,
            "svr".into(),
            "read_thing".into(),
            "d".into(),
            json!({}),
        )
        .with_flags(true, true, false);
        let def = h.definition();
        assert!(def.metadata.concurrent_safe);
        assert!(def.metadata.idempotent);
        assert!(!def.metadata.requires_approval);

        let client = Arc::new(McpClient::new());
        let destructive = McpHandler::new(
            client,
            "svr".into(),
            "drop_db".into(),
            "d".into(),
            json!({}),
        )
        .with_flags(false, false, true);
        assert!(destructive.definition().metadata.requires_approval);
        assert!(!destructive.definition().metadata.concurrent_safe);
    }

    #[test]
    fn shared_claim_requires_both_intrinsic_flags() {
        use crate::tools::concurrency::ConcurrencyClaim;
        for (idempotent, read_only) in [(false, false), (true, false), (false, true), (true, true)]
        {
            let handler = McpHandler::new(
                Arc::new(McpClient::new()),
                "svr".into(),
                "claim_fixture".into(),
                "d".into(),
                json!({}),
            )
            .with_flags(idempotent, read_only, false);
            let expected = if idempotent && read_only {
                ConcurrencyClaim::Shared
            } else {
                ConcurrencyClaim::global()
            };
            assert_eq!(handler.concurrency_claim(&json!({})), expected);
        }
    }

    #[test]
    fn map_mcp_error_network_is_transport() {
        let err = AlephError::NetworkError {
            message: "connection reset".into(),
            suggestion: None,
        };
        match map_mcp_error("s__t".into(), err) {
            ToolError::Transport { name, cause } => {
                assert_eq!(name, "s__t");
                assert!(cause.contains("connection reset"));
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[test]
    fn map_mcp_error_io_is_transport() {
        let err = AlephError::IoError("stream closed".into());
        assert!(matches!(
            map_mcp_error("s__t".into(), err),
            ToolError::Transport { .. }
        ));
    }

    #[test]
    fn map_mcp_error_timeout_is_timeout() {
        assert!(matches!(
            map_mcp_error("s__t".into(), AlephError::McpTimeout),
            ToolError::Timeout { .. }
        ));
    }

    #[test]
    fn map_mcp_error_tool_not_found_is_not_found() {
        let err = AlephError::McpToolNotFound("missing".into());
        match map_mcp_error("s__t".into(), err) {
            ToolError::NotFound { name } => assert_eq!(name, "missing"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn map_mcp_error_other_is_execution() {
        let err = AlephError::Other {
            message: "boom".into(),
            suggestion: None,
        };
        match map_mcp_error("s__t".into(), err) {
            ToolError::Execution { name, cause } => {
                assert_eq!(name, "s__t");
                assert!(cause.contains("boom"));
            }
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_tool_not_registered_returns_execution_mapped_from_not_found() {
        // Empty McpClient: call_tool on unknown tool yields
        // AlephError::McpToolNotFound, which map_mcp_error routes to NotFound.
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(client, "svr".into(), "ghost".into(), "d".into(), json!({}));
        let err = h.invoke(json!({})).await.expect_err("should fail");
        match err {
            ToolError::NotFound { name } => assert_eq!(name, "ghost"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
