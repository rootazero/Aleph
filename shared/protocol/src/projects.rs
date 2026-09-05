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

impl BindingPeerKind {
    /// Every variant.
    ///
    /// A variant added to the enum must be added here too. The exhaustiveness
    /// tripwire in `every_peer_kind_spells_one_word_everywhere` is what makes
    /// that a compile error in this file rather than a silently unasserted
    /// variant — Rust cannot enumerate variants on its own, so the alternative
    /// to this pair is a list that rots without saying so.
    pub const ALL: [Self; 2] = [Self::Group, Self::Thread];

    /// The wire spelling.
    ///
    /// **The only place in the tree these two words are typed** for this
    /// field. Before 2026-08-29 there were three authors of them — serde's
    /// `rename_all` here (the authoritative one, since it is what actually
    /// goes on the wire), plus a hand-written `match` in each direction in
    /// `alephcore`'s `projects::binding`. Those two agreed with serde by
    /// coincidence of review rather than by construction, and `aleph-cli`
    /// could reach neither of them: it must not depend on `alephcore`, so a
    /// fourth copy was about to be written in the CLI, where a mismatch shows
    /// up as `INVALID_PARAMS` on a command that has never once worked.
    ///
    /// Pinned against serde — the genuinely independent author, since it
    /// derives the spelling from the variant identifier — by
    /// `every_peer_kind_spells_one_word_everywhere`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Thread => "thread",
        }
    }
}

impl core::fmt::Display for BindingPeerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `peer_kind` spelling no variant answers to.
///
/// Carries the offending text so a CLI can quote it back, and derives the
/// accepted list from [`BindingPeerKind::ALL`] rather than restating it — the
/// error message is the fourth place that list would otherwise be written, and
/// an error message that lists the wrong options is worse than none.
///
/// 一个不属于任何变体的 `peer_kind` 拼法。可接受拼法从 [`BindingPeerKind::ALL`]
/// 派生，而不是在错误信息里再抄一份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPeerKind(pub String);

impl core::fmt::Display for UnknownPeerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "unknown peer kind {:?}; expected one of: ", self.0)?;
        for (i, kind) in BindingPeerKind::ALL.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(kind.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for UnknownPeerKind {}

impl core::str::FromStr for BindingPeerKind {
    type Err = UnknownPeerKind;

    /// Exactly the spellings the wire accepts, and no others.
    ///
    /// Case-sensitive on purpose: `"Group"` is rejected here for the same
    /// reason serde rejects it (see
    /// `an_uppercase_peer_kind_is_rejected_at_the_parse_boundary`). A
    /// case-insensitive parse in a client would let `--peer-kind Group`
    /// through the client and mint a second, never-matched primary key on the
    /// server — the exact failure the typed field exists to make impossible.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| UnknownPeerKind(s.to_string()))
    }
}

/// One project room, as `projects.list` / `projects.get` send it.
///
/// Here rather than in `alephcore` because `aleph projects list` needs it and
/// `aleph-cli` must not depend on `alephcore` — the same reason the
/// `projects.channel.*` shapes are here. The server **constructs** this type
/// (`gateway::handlers::projects::render_project`) rather than parsing into
/// it, which is what makes the CLI's column reconciliation mean something: a
/// test that only parses a response proves the client's fields are a SUBSET of
/// what was sent, never that they are the same set.
///
/// `workspace_path` is `None` for a room bound to no folder, and when set it
/// has already been through `utils::paths::display_string` — the stored value
/// comes out of `canonicalize`, which on Windows carries the `\\?\`
/// extended-length prefix. That is right for the filesystem layer and wrong in
/// anything a person reads.
///
/// 一个项目房间的线上形态。放在协议 crate 而不是 `alephcore`：CLI 不允许依赖
/// `alephcore`，而服务端**构造**这个类型（不是解析成它），这样客户端的列对账
/// 才证明得了"两边字段相等"而不只是"客户端是子集"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub owner_user_id: Option<String>,
    pub workspace_path: Option<String>,
    pub status: String,
    pub member_ids: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: i64,
}

/// `projects.list` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectListResult {
    pub projects: Vec<ProjectRow>,
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
    /// No session row was found for that conversation — or every row it found
    /// had vanished by the time it was reached.
    ///
    /// Worded as what the server **observed**, not as what it would infer.
    /// "Nobody has spoken in that conversation yet" is the usual explanation
    /// and is true today, but it is an interpretation layered on top of "I
    /// found no row" — and an interpretation becomes a lie in the hands of
    /// whoever next narrows the search. The value only ever means the search
    /// produced nothing to move.
    ///
    /// The second clause covers a narrow race the server can genuinely be in:
    /// the scan lists rows, then rescopes each one, and a row deleted between
    /// those two steps answers "no such row". Reporting that as
    /// [`Self::Unknown`] would render a benign race as an alarming receipt, so
    /// it lands here — but the doc says so rather than letting the value mean
    /// something narrower than the code does.
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

/// The sentence every surface prints after a successful `unbind`.
///
/// **Deliberately a `const`, not a field on [`ChannelUnbindResult`]** — those
/// are two different questions and only the first was settled by Ruling AI.
///
/// *Should it go on the wire?* No. The statement is unconditionally true — it
/// holds whether or not a transcript exists — so the server would be sending a
/// constant, which is client copy rather than wire state.
///
/// *Does that mean it needs no owner?* Also no, and that half does not follow
/// from the first. Copy that must be **identical** on three surfaces (CLI,
/// Panel, and any future one) otherwise acquires three authors, and this
/// repo's most-recorded defect is one fact with two statements where only one
/// gets updated. A `const` here is the same remedy `ADMIN_REQUIRED_MESSAGE`
/// already uses for cross-surface wording.
///
/// Why the sentence exists at all: `unbind` does not move the transcript back
/// out of the room, and there is no correct destination to move it to (the
/// previous scope was never recorded, and reverting to `personal:<somebody>`
/// means picking a person, where picking wrong is worse than not reverting).
/// An operator who is not told will reasonably assume a symmetry that does not
/// exist.
///
/// 每个客户端面在 unbind 成功后都要打印这句话。刻意是 `const` 而不是 wire 字段：
/// 它无条件为真（所以不该上 wire），但**不上 wire ≠ 不需要所有者**——必须在三个
/// 面上逐字一致的文案，否则就是三个作者。
pub const UNBIND_KEEPS_TRANSCRIPT_NOTICE: &str = "The conversation's existing transcript \
    stays with the room — unbinding stops future turns from joining it, it does not move \
    history back.";

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

    /// Serde, `Display`/`as_str` and `FromStr` must spell every variant the
    /// same single word.
    ///
    /// This is the assertion that ties author #1 (serde's `rename_all`) to
    /// author #2 ([`BindingPeerKind::as_str`], the one hand-written `match`
    /// left). Before this existed there were three authors across two crates
    /// and no test compared any two of them; they agreed because a reviewer
    /// looked, which is not a mechanism.
    ///
    /// **Serde is the oracle here, and that choice is the whole reason this
    /// test is worth its lines.** `rename_all` derives each spelling from the
    /// variant *identifier*, so it is a genuinely different author from the
    /// thing under test — nothing hand-typed feeds it. A round-trip test whose
    /// oracle shares an author with its subject proves only that one function
    /// is self-consistent, which is true of every function and worth nothing.
    /// If someone later "simplifies" this by comparing `as_str` against a
    /// literal, or by having `as_str` delegate to serde, the assertion becomes
    /// exactly that kind of tautology and this file goes back to having no
    /// mechanism holding its two authors together.
    ///
    /// Written to enumerate variants rather than to compare two literals. A
    /// two-literal test passes forever on the day somebody adds a third
    /// variant — which is the only day it would have mattered.
    #[test]
    fn every_peer_kind_spells_one_word_everywhere() {
        use std::str::FromStr as _;

        let mut seen = std::collections::BTreeSet::new();
        for kind in BindingPeerKind::ALL {
            // Exhaustiveness tripwire. A new variant makes this match
            // non-exhaustive, and the compile error lands in the same file as
            // `ALL` — which is what stops a variant being added to the enum
            // and not to the list this test iterates.
            match kind {
                BindingPeerKind::Group | BindingPeerKind::Thread => {}
            }

            let wire = serde_json::to_value(kind).unwrap();
            let spelled = wire
                .as_str()
                .expect("a unit variant is a JSON string")
                .to_string();
            let spelled = spelled.as_str();

            assert_eq!(
                spelled,
                kind.as_str(),
                "serde sends {spelled:?} for {kind:?} while as_str/Display say {:?} — a \
                 client that formats one and sends the other addresses a row that does \
                 not exist",
                kind.as_str()
            );
            assert_eq!(spelled, kind.to_string(), "Display must be as_str");
            assert_eq!(
                BindingPeerKind::from_str(spelled).expect("FromStr accepts the wire spelling"),
                kind,
                "FromStr must accept exactly what serde emits, or a CLI argument that \
                 parses locally still fails at the server's parse boundary"
            );
            assert_eq!(
                serde_json::from_value::<BindingPeerKind>(wire).unwrap(),
                kind,
                "round trip"
            );
            assert!(
                seen.insert(spelled.to_string()),
                "two variants share the wire spelling {spelled:?}"
            );
        }
        assert_eq!(seen.len(), BindingPeerKind::ALL.len());
    }

    /// `FromStr` must be no wider than the wire, and must say what it wants.
    ///
    /// The width half is the point: a client that accepted `"Group"` would
    /// convert it to a `BindingPeerKind` and send the lowercase form, which
    /// looks like it works — until somebody wonders why the CLI accepts a
    /// spelling the JSON API rejects and "fixes" one of them.
    #[test]
    fn from_str_refuses_what_the_wire_refuses_and_names_the_alternatives() {
        use std::str::FromStr as _;

        let err = BindingPeerKind::from_str("Group").expect_err("case must match the wire");
        let text = err.to_string();
        assert!(text.contains("Group"), "quote what was typed: {text}");
        for kind in BindingPeerKind::ALL {
            assert!(
                text.contains(kind.as_str()),
                "the error must list {:?} as an option: {text}",
                kind.as_str()
            );
        }
        assert!(
            BindingPeerKind::from_str("").is_err(),
            "empty is not a kind"
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
