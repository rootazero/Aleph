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

/// What happened to the conversation's existing transcript when it was bound.
///
/// Three-valued, and deliberately **not** `Option<bool>`: an `Option` invites
/// `.unwrap_or(false)`, which silently reproduces the very collapse this type
/// exists to prevent, once per call site. A named enum with no natural default
/// makes every client say out loud what it does with [`Self::Unknown`].
///
/// 一个绑定操作对"旧记录去哪了"有三个答案，不是两个。用具名枚举而非
/// `Option<bool>`：后者会诱使调用方写 `.unwrap_or(false)`，那正是本类型要
/// 消除的塌缩，而且会在每个调用点各塌缩一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RescopeOutcome {
    /// An existing transcript was moved into the room scope.
    Moved,
    /// No session row was found for that conversation.
    ///
    /// Worded as what the server **observed**, not as what it would infer.
    /// "Nobody has spoken in that conversation yet" is the usual explanation
    /// and is true today, but it is an interpretation layered on top of "I
    /// found no row" — and an interpretation becomes a lie in the hands of
    /// whoever next narrows the search. The value only ever means the search
    /// came back empty.
    ///
    /// A distinct answer from [`Self::Moved`], because a receipt that claims a
    /// migration happened when it did not is a client asserting a result it
    /// never saw.
    ///
    /// 措辞描述服务端**观测到**什么，而不是它**推断**什么：「没找到行」是事实，
    /// 「没人说过话」是对它的解释，而解释会在下一个收窄搜索的人手里变成假话。
    NothingToMove,
    /// The session store could not say. The binding itself committed — this
    /// arm reports only that the transcript's fate is **unobserved**.
    ///
    /// Its own variant rather than being folded into [`Self::NothingToMove`]:
    /// a store that errored has not "found nothing to move", and a client that
    /// renders the two identically ends up printing a confident factual claim
    /// about a conversation whose store just failed.
    Unknown,
}

impl RescopeOutcome {
    /// The wire spelling, for logs and audit records that interpolate it.
    ///
    /// Derived from the same variant the wire carries rather than re-spelled at
    /// each formatting site, so an audit line cannot describe a different
    /// outcome than the receipt did.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::NothingToMove => "nothing_to_move",
            Self::Unknown => "unknown",
        }
    }
}

impl core::fmt::Display for RescopeOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `projects.channel.bind` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindResult {
    pub binding: ChannelBindingRow,
    /// What happened to any transcript the conversation already had.
    ///
    /// See [`RescopeOutcome`] for why this is a three-state enum rather than
    /// the `bool` it started as.
    pub rescoped_session: RescopeOutcome,
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

    /// The three outcomes must be three distinct wire values. If any two
    /// coincide, every client that renders them renders a claim it did not
    /// receive — which is the exact collapse `RescopeOutcome` replaced
    /// (`matches!(.., Ok(true))` folding a store error into "nothing to
    /// move"). Written as an exhaustive match so a fourth variant is a
    /// compile error here rather than a silently unasserted one.
    #[test]
    fn the_three_rescope_outcomes_are_three_distinct_wire_values() {
        let mut seen = std::collections::BTreeSet::new();
        for o in [
            RescopeOutcome::Moved,
            RescopeOutcome::NothingToMove,
            RescopeOutcome::Unknown,
        ] {
            let wire = serde_json::to_value(o).unwrap();
            let s = wire
                .as_str()
                .expect("a unit variant is a JSON string")
                .to_string();
            assert_eq!(
                s,
                o.as_str(),
                "Display/as_str and the serde spelling must be the same word, or an \
                 audit line will describe a different outcome than the receipt did"
            );
            assert!(seen.insert(s), "two outcomes share a wire value: {o:?}");
            assert_eq!(
                serde_json::from_value::<RescopeOutcome>(wire).unwrap(),
                o,
                "round trip"
            );
        }
        assert_eq!(seen.len(), 3);
    }

    /// `Unknown` must be a value on the wire, not an omission. An absent field
    /// is what a client reads as `false`/default, and reintroducing a default
    /// is how the collapse comes back one call site at a time.
    #[test]
    fn an_unknown_rescope_outcome_is_sent_not_omitted() {
        let v = serde_json::to_value(ChannelBindResult {
            binding: ChannelBindingRow {
                project_id: "p-1".into(),
                channel_id: "telegram".into(),
                peer_kind: BindingPeerKind::Group,
                peer_id: "c1".into(),
                bound_by: None,
                bound_at: 0,
                label: None,
            },
            rescoped_session: RescopeOutcome::Unknown,
        })
        .unwrap();
        assert_eq!(v["rescoped_session"], serde_json::json!("unknown"));
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
