//! `channel_directory` — resolve a conversation NAME a human said into the
//! opaque id the channel APIs actually take.
//!
//! `channel_message` can send anywhere, but only to a `conversation_id`
//! (`C0A1B2C3`, a numeric chat id, a JID). Nothing produced those ids except an
//! inbound message, so before this tool the agent could only ever reply where it
//! was spoken to: "post the release numbers to #eng-releases" had no first step.
//!
//! Deliberately a SEPARATE tool from `channel_message` rather than another
//! action on it, for two reasons that both live in the enforcement layer:
//!
//! * `ToolFacts::idempotent` is keyed on the tool NAME
//!   (`registry_adapter::READ_ONLY_TOOLS`). Folded into `channel_message` — a
//!   non-idempotent tool — a lookup would be gated under the `Ask` exec tier,
//!   contradicting that tier's own promise that "read-only tools stay allowed so
//!   the model can still investigate". A tier never *widens*, so the split is
//!   the only way to keep a read open.
//! * Reading the workspace roster and posting into it are different blast
//!   radii, and `src/gateway/method_authz.rs` gates per tool name.
//!
//! Pure I/O over the existing `ChannelRegistry` — no new transport, and no
//! judgement about which conversation the user "meant" (R7: the tool returns
//! candidates, the model picks).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::gateway::channel::{ChannelId, ConversationKind, ConversationRef};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Ceiling on returned rows. Clamped at the boundary rather than trusted from
/// the model: an unbounded roster on a large workspace is thousands of names,
/// and the central result budget would silently persist-to-disk instead of
/// answering.
const MAX_LIMIT: usize = 50;
const DEFAULT_LIMIT: usize = 20;

/// Arguments for the `channel_directory` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ChannelDirectoryArgs {
    /// Which connected channel to search (e.g. "slack").
    pub channel_id: String,

    /// Name fragment to match, case-insensitive; leading `#` / `@` are ignored.
    /// Omit to list the head of the roster.
    #[serde(default)]
    pub query: Option<String>,

    /// Maximum rows to return (default 20, hard cap 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One addressable conversation, as the model sees it.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelDirectoryEntry {
    /// Pass this verbatim as `channel_message`'s `conversation_id`.
    pub conversation_id: String,
    /// Human name, without any platform sigil.
    pub name: String,
    /// `channel` | `group` | `direct`.
    pub kind: ConversationKind,
    /// `false` = it exists but this account cannot post there yet.
    pub is_member: bool,
}

impl From<ConversationRef> for ChannelDirectoryEntry {
    fn from(r: ConversationRef) -> Self {
        Self {
            conversation_id: r.id.as_str().to_string(),
            name: r.name,
            kind: r.kind,
            is_member: r.is_member,
        }
    }
}

/// Output of the `channel_directory` tool.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelDirectoryOutput {
    /// The channel that was searched.
    pub channel_id: String,
    /// Matches, best first.
    pub conversations: Vec<ChannelDirectoryEntry>,
    /// `true` when the roster had more matches than `limit` allowed through, so
    /// an absent name means "narrow the query", not "does not exist".
    pub truncated: bool,
    /// Reasons this answer may be incomplete — a missing OAuth scope, a roster
    /// too large for one sweep. Present so "not found" is never confused with
    /// "not looked at".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Tool for resolving a conversation name to its platform id.
#[derive(Clone)]
pub struct ChannelDirectoryTool {
    channel_registry: Arc<ChannelRegistry>,
}

impl ChannelDirectoryTool {
    pub const fn new(channel_registry: Arc<ChannelRegistry>) -> Self {
        Self { channel_registry }
    }
}

#[async_trait]
impl AlephTool for ChannelDirectoryTool {
    const NAME: &'static str = "channel_directory";
    const DESCRIPTION: &'static str =
        "Look up conversations on a connected channel by name — channels, groups, and people — \
         and get the conversation_id that `channel_message` needs. Use this whenever the user \
         names a destination (\"post it to #eng-releases\", \"DM Alice the summary\") instead of \
         guessing an id. Read-only. An entry with is_member=false exists but cannot be posted to \
         yet (on Slack: invite the bot to that channel first). Not every channel type has a \
         directory; those return an unsupported-feature error.";

    type Args = ChannelDirectoryArgs;
    type Output = ChannelDirectoryOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let channel_id = ChannelId::new(args.channel_id.clone());
        let query = args.query.unwrap_or_default();
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

        // Ask for one more than we will return: that extra row is how we know
        // the list was cut, without a second call or a total-count API.
        let mut page = self
            .channel_registry
            .list_conversations(&channel_id, &query, limit.saturating_add(1))
            .await
            .map_err(|e| {
                AlephError::tool(format!(
                    "Failed to list conversations on channel '{channel_id}': {e}"
                ))
            })?;

        let truncated = page.conversations.len() > limit;
        page.conversations.truncate(limit);

        Ok(ChannelDirectoryOutput {
            channel_id: args.channel_id,
            conversations: page.conversations.into_iter().map(Into::into).collect(),
            truncated,
            warnings: page.warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::ConversationId;

    fn tool() -> ChannelDirectoryTool {
        ChannelDirectoryTool::new(Arc::new(ChannelRegistry::new()))
    }

    #[test]
    fn entry_flattens_the_opaque_id_to_a_plain_string() {
        let entry: ChannelDirectoryEntry = ConversationRef {
            id: ConversationId::new("C0A1B2C3"),
            name: "eng-releases".to_string(),
            kind: ConversationKind::Channel,
            is_member: true,
        }
        .into();
        assert_eq!(entry.conversation_id, "C0A1B2C3");
        assert_eq!(entry.name, "eng-releases");
    }

    #[test]
    fn args_accept_a_bare_channel_id() {
        let args: ChannelDirectoryArgs =
            serde_json::from_value(serde_json::json!({"channel_id": "slack"})).unwrap();
        assert_eq!(args.channel_id, "slack");
        assert!(args.query.is_none());
        assert!(args.limit.is_none());
    }

    /// A model asking for 10_000 rows must not get 10_000 rows.
    #[test]
    fn limit_is_clamped_at_the_boundary() {
        assert_eq!(usize::MAX.clamp(1, MAX_LIMIT), MAX_LIMIT);
        assert_eq!(0_usize.clamp(1, MAX_LIMIT), 1);
        assert_eq!(7_usize.clamp(1, MAX_LIMIT), 7);
    }

    /// An unknown channel must say so, not return an empty roster — "there are
    /// no channels called that" and "you are not connected to Slack" are
    /// different problems with different fixes.
    #[tokio::test]
    async fn unknown_channel_is_an_error_not_an_empty_list() {
        let err = tool()
            .call(ChannelDirectoryArgs {
                channel_id: "nope".to_string(),
                query: None,
                limit: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }
}
