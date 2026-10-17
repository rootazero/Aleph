//! The single tool an isolated public-telecom run may be offered:
//! `request_private_lookup { query }`.
//!
//! It is a *signal*, not a capability (R7: the model decides whether the
//! caller's request needs the owner's private records). Executing it has no
//! external effect: it records the bounded query into this run's slot and
//! returns a fixed text. Whether anything is ever read is decided afterwards
//! by the trusted telecom driver and the operator (approval A). Every other
//! tool name is refused exactly like [`crate::tools::null::NullToolService`].

use crate::sync_primitives::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::session::events::ToolOutput;
use crate::tool_metadata::ToolDefinition as MetadataToolDefinition;
use crate::tools::service::{
    to_metadata_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

/// Name of the only tool a lookup-enabled public run is offered.
pub const PRIVATE_LOOKUP_TOOL: &str = "request_private_lookup";

/// Upper bound of a recorded query, in bytes.
pub const MAX_PRIVATE_LOOKUP_QUERY_BYTES: usize = 512;

/// Fixed tool result. Never echoes the query.
pub const PRIVATE_LOOKUP_RECORDED: &str = "Lookup request recorded; the operator must approve \
it. Tell the caller you need to check and will reply.";

/// One-tool service of a lookup-enabled public run. The recorded query is
/// read back by the run once the harness finished.
#[derive(Default)]
pub(super) struct PrivateLookupToolService {
    slot: Mutex<Option<String>>,
}

impl PrivateLookupToolService {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The query recorded during the run, if any.
    pub(super) fn take_query(&self) -> Option<String> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: PRIVATE_LOOKUP_TOOL.to_string(),
            description: "Ask the owner's operator for permission to search the owner's \
private notes for this caller's request. Does not return any private data; the \
operator decides later. Use only when the caller's request cannot be answered without \
the owner's private records."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Short search query describing what to look up (max 512 bytes)."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                max_duration_ms: Some(1_000),
                ..Default::default()
            },
        }
    }
}

impl std::fmt::Debug for PrivateLookupToolService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The recorded query may describe private matters; never print it.
        f.debug_struct("PrivateLookupToolService")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolService for PrivateLookupToolService {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if name != PRIVATE_LOOKUP_TOOL {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if query.is_empty() {
            return Err(ToolError::Other("query must be a non-empty string".into()));
        }
        if query.len() > MAX_PRIVATE_LOOKUP_QUERY_BYTES {
            return Err(ToolError::Other(format!(
                "query exceeds {MAX_PRIVATE_LOOKUP_QUERY_BYTES} bytes"
            )));
        }
        {
            let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            // First request wins: one lookup per public turn.
            if slot.is_none() {
                *slot = Some(query.to_string());
            }
        }
        Ok(ToolOutput {
            value: Value::String(PRIVATE_LOOKUP_RECORDED.to_string()),
            metadata: Default::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        vec![Self::definition()]
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        (name == PRIVATE_LOOKUP_TOOL).then(Self::definition)
    }

    fn metadata_schema(&self) -> Arc<[MetadataToolDefinition]> {
        to_metadata_form(&[Self::definition()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offers_exactly_one_tool() {
        let svc = PrivateLookupToolService::new();
        let names: Vec<_> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert_eq!(names, [PRIVATE_LOOKUP_TOOL]);
        assert_eq!(svc.metadata_schema().len(), 1);
        assert!(svc.describe("memory_search").await.is_none());
    }

    #[tokio::test]
    async fn records_query_only_and_returns_fixed_text() {
        let svc = PrivateLookupToolService::new();
        let out = svc
            .execute(
                PRIVATE_LOOKUP_TOOL,
                json!({"query": "  dentist appointment  "}),
            )
            .await
            .unwrap();
        assert_eq!(out.value, Value::String(PRIVATE_LOOKUP_RECORDED.into()));
        // A second call cannot replace the first request.
        svc.execute(PRIVATE_LOOKUP_TOOL, json!({"query": "other"}))
            .await
            .unwrap();
        assert_eq!(svc.take_query().as_deref(), Some("dentist appointment"));
        assert_eq!(svc.take_query(), None);
    }

    #[tokio::test]
    async fn empty_missing_and_oversize_queries_are_refused() {
        let svc = PrivateLookupToolService::new();
        for input in [
            json!({}),
            json!({"query": ""}),
            json!({"query": "   "}),
            json!({"query": 7}),
            json!({"query": "x".repeat(MAX_PRIVATE_LOOKUP_QUERY_BYTES + 1)}),
        ] {
            assert!(svc.execute(PRIVATE_LOOKUP_TOOL, input).await.is_err());
        }
        assert_eq!(svc.take_query(), None);
        // Exactly at the bound is accepted.
        svc.execute(
            PRIVATE_LOOKUP_TOOL,
            json!({"query": "y".repeat(MAX_PRIVATE_LOOKUP_QUERY_BYTES)}),
        )
        .await
        .unwrap();
        assert!(svc.take_query().is_some());
    }

    #[tokio::test]
    async fn any_other_tool_is_not_found_and_records_nothing() {
        let svc = PrivateLookupToolService::new();
        for name in [
            "exec",
            "memory_search",
            "owner_mcp__read_secret",
            "channel_outbox",
        ] {
            let err = svc
                .execute(name, json!({"query": "secret"}))
                .await
                .unwrap_err();
            assert!(matches!(err, ToolError::NotFound { name: ref n } if n == name));
        }
        assert_eq!(svc.take_query(), None);
    }
}
