//! Loop-graph governance vocabulary — node kinds, the six-verb closed edge
//! set, and provenance. The graph holds POWER STRUCTURE only (who watches
//! whom, what is frozen, where the human-supplied root references live); all
//! semantic judgment on top of it belongs to the LLM (R7). See
//! docs/reference/GRAPH_LAYER.md.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a graph node stands for. Loop kinds are *bindings* to existing
/// entities (goal/cron/heartbeat/daemon) — never copies of their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A standing goal (`goal:<session_id>`).
    LoopGoal,
    /// A scheduled cron job (`cron:<job_id>`).
    LoopCron,
    /// A heartbeat monitoring task (`heartbeat:<task_id>`).
    LoopHeartbeat,
    /// A well-known background daemon (`daemon:<name>`, e.g. `daemon:dreaming`).
    Daemon,
    /// An explicitly governed multi-agent team (`team:<team_id>`). Teams enter
    /// the graph only by explicit pairing — never automatically; fast-loop
    /// coord tasks never become nodes.
    Team,
    /// An irrefutable measurement (`anchor:<slug>`) — body declares `{probe, truth}`.
    Anchor,
    /// A rule no optimizing loop may tune (`frozen:<slug>`) — body points at
    /// the enforcement site; the graph registers, it does not enforce.
    Frozen,
    /// A human-supplied definition of "better" (`root:<slug>`) — origin MUST
    /// be `human` (store invariant).
    Root,
}

impl NodeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoopGoal => "loop_goal",
            Self::LoopCron => "loop_cron",
            Self::LoopHeartbeat => "loop_heartbeat",
            Self::Daemon => "daemon",
            Self::Team => "team",
            Self::Anchor => "anchor",
            Self::Frozen => "frozen",
            Self::Root => "root",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "loop_goal" => Some(Self::LoopGoal),
            "loop_cron" => Some(Self::LoopCron),
            "loop_heartbeat" => Some(Self::LoopHeartbeat),
            "daemon" => Some(Self::Daemon),
            "team" => Some(Self::Team),
            "anchor" => Some(Self::Anchor),
            "frozen" => Some(Self::Frozen),
            "root" => Some(Self::Root),
            _ => None,
        }
    }

    /// Loops that optimize something and therefore need watching. Anchors,
    /// frozen rules and roots are ground, not optimizers.
    #[must_use]
    pub const fn is_optimization_loop(self) -> bool {
        matches!(
            self,
            Self::LoopGoal | Self::LoopCron | Self::LoopHeartbeat | Self::Daemon | Self::Team
        )
    }
}

/// The six-verb closed edge vocabulary. Deliberately NOT free text (unlike
/// memory-note relations): every verb here has a structural code consumer —
/// `Feeds` being the one acknowledged documentation-only exception — and the
/// set only grows when a new verb brings its consumer with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Watcher loop → optimizing loop (Goodhart pairing: counter-metric view).
    Watches,
    /// Governing loop → child loop: owns the child's reference/objective.
    OwnsReference,
    /// Arbiter loop → each conflicting loop (≥2 edges).
    Arbitrates,
    /// Audit loop → any node: periodically verifies its numbers still touch reality.
    Audits,
    /// Loop → anchor node grounding its verdicts.
    AnchoredBy,
    /// Upstream loop → downstream loop (data flow; documentation-only edge).
    Feeds,
}

impl EdgeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Watches => "watches",
            Self::OwnsReference => "owns_reference",
            Self::Arbitrates => "arbitrates",
            Self::Audits => "audits",
            Self::AnchoredBy => "anchored_by",
            Self::Feeds => "feeds",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "watches" => Some(Self::Watches),
            "owns_reference" => Some(Self::OwnsReference),
            "arbitrates" => Some(Self::Arbitrates),
            "audits" => Some(Self::Audits),
            "anchored_by" => Some(Self::AnchoredBy),
            "feeds" => Some(Self::Feeds),
            _ => None,
        }
    }
}

/// First-class provenance: who put this node/edge in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Human,
    Llm,
}

impl Origin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Llm => "llm",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(Self::Human),
            "llm" => Some(Self::Llm),
            _ => None,
        }
    }
}

/// A governance-graph node. Immutable-update style: `with_*` return copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub agent_id: String,
    /// Prefixed id binding to the underlying entity, e.g. `goal:<session_id>`,
    /// `cron:<job_id>`, `daemon:dreaming`, `anchor:user-corrections`.
    pub id: String,
    pub kind: NodeKind,
    /// One-line human-readable name.
    pub label: String,
    /// Anchor: `{probe, truth}` declaration. Frozen: rule text + enforcement
    /// pointer. Root: the human-authored reference text. Loops: optional
    /// description of the controlled variable / reference.
    pub body: Option<String>,
    /// Declared pace class ("per_turn" | "hourly" | "nightly" | "weekly" |
    /// "monthly" | free text). Used by lint to flag fast loops owning slow
    /// loops' references; free text simply opts out of that check.
    pub cadence: Option<String>,
    pub origin: Origin,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl GraphNode {
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        id: impl Into<String>,
        kind: NodeKind,
        label: impl Into<String>,
        origin: Origin,
    ) -> Self {
        let now = now_ms();
        Self {
            agent_id: agent_id.into(),
            id: id.into(),
            kind,
            label: label.into(),
            body: None,
            cadence: None,
            origin,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    #[must_use]
    pub fn with_cadence(mut self, cadence: impl Into<String>) -> Self {
        self.cadence = Some(cadence.into());
        self
    }
}

/// A governance edge. PK = (agent_id, from_id, to_id, kind).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub agent_id: String,
    pub from_id: String,
    pub to_id: String,
    pub kind: EdgeKind,
    /// One-line rationale, prose — code never parses it.
    pub note: Option<String>,
    pub origin: Origin,
    pub created_at_ms: i64,
}

impl GraphEdge {
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        kind: EdgeKind,
        origin: Origin,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            from_id: from_id.into(),
            to_id: to_id.into(),
            kind,
            note: None,
            origin,
            created_at_ms: now_ms(),
        }
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Ordered pace rank for the known cadence classes; `None` for free text
/// (which opts out of the fast-owns-slow lint).
#[must_use]
pub fn cadence_rank(cadence: &str) -> Option<u8> {
    match cadence {
        "per_turn" => Some(0),
        "hourly" => Some(1),
        "nightly" | "daily" => Some(2),
        "weekly" => Some(3),
        "monthly" => Some(4),
        _ => None,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_roundtrip_through_strings() {
        for k in [
            NodeKind::LoopGoal,
            NodeKind::LoopCron,
            NodeKind::LoopHeartbeat,
            NodeKind::Daemon,
            NodeKind::Team,
            NodeKind::Anchor,
            NodeKind::Frozen,
            NodeKind::Root,
        ] {
            assert_eq!(NodeKind::parse(k.as_str()), Some(k));
        }
        for e in [
            EdgeKind::Watches,
            EdgeKind::OwnsReference,
            EdgeKind::Arbitrates,
            EdgeKind::Audits,
            EdgeKind::AnchoredBy,
            EdgeKind::Feeds,
        ] {
            assert_eq!(EdgeKind::parse(e.as_str()), Some(e));
        }
        assert_eq!(NodeKind::parse("bogus"), None);
        assert_eq!(EdgeKind::parse("likes"), None, "vocabulary is closed");
    }

    #[test]
    fn optimization_loop_classification() {
        assert!(NodeKind::Daemon.is_optimization_loop());
        assert!(NodeKind::Team.is_optimization_loop());
        assert!(!NodeKind::Anchor.is_optimization_loop());
        assert!(!NodeKind::Root.is_optimization_loop());
    }

    #[test]
    fn cadence_ranks_order_known_classes() {
        assert!(cadence_rank("per_turn") < cadence_rank("weekly"));
        assert_eq!(cadence_rank("every so often"), None);
    }
}
