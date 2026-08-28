//! Wire contract for `projects.channel.*`.
//!
//! The CLI cannot depend on `alephcore`, so a hand-written `json!({...})` on
//! one side and a `#[derive(Deserialize)]` on the other have no way to
//! disagree until a user reports that a command has never worked. Three
//! families have shipped that defect here (`aleph workspace create`, the TUI's
//! `agent.run`, `aleph providers list/get/add`), so the shape lives in the
//! crate both halves already depend on, and each half reconciles against it.

use serde::{Deserialize, Serialize};

/// Which kind of group conversation a binding names.
///
/// A typed wire field, not a `String` with a doc comment: `peer_kind` is part of
/// the binding's PRIMARY KEY, so `"Group"` and `"group"` would be two different
/// rows — the already-bound conflict guard would not catch the typo, every
/// surface would report the room as bound, and `project_for_conversation` (which
/// derives its string from `PeerKind` and therefore always asks for `"group"`)
/// would never match it. Typing it here makes a bad value a parse-boundary
/// rejection on every client at once, instead of a validator each new call site
/// has to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingPeerKind {
    Group,
    Thread,
}

/// `projects.channel.bind`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindParams {
    pub project_id: String,
    pub channel_id: String,
    pub peer_kind: BindingPeerKind,
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// `projects.channel.unbind`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelUnbindParams {
    pub channel_id: String,
    pub peer_kind: BindingPeerKind,
    pub peer_id: String,
}

/// `projects.channel.list`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelListParams {
    pub project_id: String,
}

/// One bound conversation, as every surface renders it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindingRow {
    pub project_id: String,
    pub channel_id: String,
    pub peer_kind: BindingPeerKind,
    pub peer_id: String,
    pub bound_by: Option<String>,
    pub bound_at: i64,
    pub label: Option<String>,
}

/// `projects.channel.bind` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindResult {
    pub binding: ChannelBindingRow,
    /// Whether an existing session row was moved into the room scope.
    ///
    /// `false` means nobody had spoken in that conversation yet — a distinct
    /// answer from "I moved it", because a receipt that claims a migration
    /// happened when it did not is a client asserting a result it never saw.
    pub rescoped_session: bool,
}

/// `projects.channel.unbind` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelUnbindResult {
    /// `false` means nothing was bound.
    pub unbound: bool,
}

/// `projects.channel.list` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelListResult {
    pub bindings: Vec<ChannelBindingRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The envelope is a wire key too, and it is usually the last hand-copied
    /// part. Serialising the contract type is how a client learns the key
    /// rather than guessing it.
    #[test]
    fn the_list_result_envelope_is_named_bindings() {
        let v = serde_json::to_value(ChannelListResult { bindings: vec![] }).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["bindings"]);
    }

    #[test]
    fn an_absent_label_is_not_sent() {
        let v = serde_json::to_value(ChannelBindParams {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: BindingPeerKind::Group,
            peer_id: "C1".into(),
            label: None,
        })
        .unwrap();
        assert!(
            !v.as_object().unwrap().contains_key("label"),
            "an omitted optional must be omitted, not sent as null"
        );
    }

    /// The single assertion that makes `BindingPeerKind` worth adding: a typo
    /// in the wire value must be a parse-boundary rejection, not a value that
    /// silently becomes a second, never-matched primary key. `"Group"` and
    /// `"group"` must not both parse.
    #[test]
    fn an_uppercase_peer_kind_is_rejected_at_the_parse_boundary() {
        let v = serde_json::json!({
            "project_id": "p-1",
            "channel_id": "telegram",
            "peer_kind": "Group",
            "peer_id": "C1",
        });
        let result = serde_json::from_value::<ChannelBindParams>(v);
        assert!(
            result.is_err(),
            "\"Group\" must not parse as a BindingPeerKind: \
             #[serde(rename_all = \"lowercase\")] only accepts \"group\", and a \
             value that slips through here becomes a second, never-matched row \
             under the (channel_id, peer_kind, peer_id) primary key"
        );
    }
}
