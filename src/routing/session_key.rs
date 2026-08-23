//! Enhanced session key with full context encoding.
//!
//! Session keys encode agent identity, channel, peer, and scope information
//! into a single hierarchical key for session lookup and persistence.

use serde::{Deserialize, Serialize};
use std::fmt;

/// DM session isolation strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum DmScope {
    /// All DMs share main session
    Main,
    /// Per-user isolation (cross-channel)
    #[default]
    PerPeer,
    /// Per-channel per-user isolation
    PerChannelPeer,
}

/// Peer type for group sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerKind {
    Group,
    Thread,
}

/// Enhanced session key with full context encoding
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKey {
    /// Main session (cross-channel shared)
    Main {
        agent_id: String,
        #[serde(default = "default_main_key")]
        main_key: String,
        #[serde(default)]
        epoch: u32,
    },

    /// Direct message session with scope strategy
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        #[serde(default)]
        dm_scope: DmScope,
        #[serde(default)]
        epoch: u32,
    },

    /// Group/channel session
    Group {
        agent_id: String,
        channel: String,
        peer_kind: PeerKind,
        peer_id: String,
    },

    /// Task session (cron, webhook, scheduled)
    Task {
        agent_id: String,
        task_type: String,
        task_id: String,
    },

    /// Subagent session (nested under parent)
    Subagent {
        parent_key: Box<Self>,
        subagent_id: String,
    },

    /// Ephemeral session: not part of any conversation's addressable history,
    /// but **stored like every other session**. Both backends give it a real
    /// row — `session_store::file_backend` writes it with
    /// `session_type = "ephemeral"`, and the SQLite backend INSERTs the same
    /// string via `gateway::session_manager::session_type_str`.
    ///
    /// The name once said "no persistence", which was never true and was the
    /// reason nothing ever cleaned these up: every reader concluded there was
    /// nothing to clean. Whoever mints one owns retiring it — see
    /// [`crate::gateway::continuation_lifecycle::retire_side_session`] for the
    /// `/btw` side session's retirement.
    ///
    /// # Retirement must stay targeted — a sweep would delete live work
    ///
    /// Do not retire these by *variant*, or by any prefix broader than one
    /// derived key. This variant is the catch-all for "a session that is not a
    /// conversation", and its production inhabitants are load-bearing:
    ///
    /// * background sub-agent children — `agents::subagent_spawner`'s
    ///   `ephemeral_for`, which is running work at the moment you would sweep;
    /// * the OpenAI-compatible completions face —
    ///   `gateway::openai_api::completions::agent`;
    /// * the orchestrator's parse fallback —
    ///   `orchestrator::harness_bridge::runner_impl`, which mints one for any
    ///   session string that is not a serialized key;
    /// * the `aleph-server` CLI's `sandbox-debug` and `node` subcommands.
    ///
    /// Every entry above is production code, checked outside its file's
    /// `cfg(test)` block. Do not trust an `Ephemeral` construction you find in
    /// a test module as evidence about either direction: an earlier draft of
    /// this list named the steering rescue path, whose only `Ephemeral` is a
    /// test fixture — the rescue path in fact *reuses* a key rather than
    /// minting one, which is why `btw::is_side_key` had to be made idempotent.
    ///
    /// The `/btw` retirement is safe precisely because it is not a sweep: it
    /// touches exactly `gateway::btw::side_key_for(<the key being retired>)`
    /// and nothing else.
    ///
    /// # A side session's row is load-bearing, not just residue
    ///
    /// It carries the incremental seed cursor
    /// (`identity_meta.custom["btw_seed_cursor"]`, see `gateway::btw::seed`),
    /// so deleting the row also discards how far the side thread has been
    /// seeded, forcing a fresh cold seed on the next `/btw`.
    ///
    /// Which retirements must reach it, and why:
    ///
    /// * **Epoch bump** (`/new`, `sessions.new`, the `session_new` tool, a
    ///   compaction split) — the derived key changes, so the old side session
    ///   becomes unaddressable. Retiring it is the only thing that keeps it
    ///   from being permanent residue.
    /// * **Delete** (`sessions.delete`) — the conversation is gone.
    /// * **Content wipe with the key unchanged** (`chat.clear`,
    ///   `sessions.reset`) — this one is not about residue. The side session
    ///   holds a *copied* prefix of the main transcript in its own event log,
    ///   so a clear that spares it leaves the user's next `/btw` able to quote
    ///   back the conversation they just wiped, out of the same conversation
    ///   they wiped it from. The stale cursor is a second, weaker reason: it
    ///   would keep the warm arm from ever re-seeding.
    ///
    /// What would be wrong is retiring one on a **timer**, or for any reason
    /// other than the conversation it derives from being rolled, deleted or
    /// cleared: that silently re-seeds a live side thread from scratch. This
    /// is deliberately narrower than "wrong at any other time", which an
    /// earlier draft of this comment said — that sentence foreclosed the
    /// `clear`/`reset` case above, in the same doc whose previous over-wide
    /// negation ("no persistence") is the reason any of this was written.
    Ephemeral {
        agent_id: String,
        ephemeral_id: String,
    },
}

fn default_main_key() -> String {
    "main".to_string()
}

/// Default agent ID constant
pub const DEFAULT_AGENT_ID: &str = "main";
/// Default main key constant
pub const DEFAULT_MAIN_KEY: &str = "main";

impl SessionKey {
    /// Create a main session key
    pub fn main(agent_id: impl Into<String>) -> Self {
        Self::Main {
            agent_id: normalize_agent_id(&agent_id.into()),
            main_key: DEFAULT_MAIN_KEY.to_string(),
            epoch: 0,
        }
    }

    /// Create the canonical session key for a P2 project room.
    ///
    /// A `Main` variant with the project id as its `main_key`, following the
    /// same shape `gateway::agent_env` uses for its `agent-env-*` keys. Two
    /// properties are load-bearing:
    ///
    /// - [`Self::is_interactive`] is `true`. A room chat has humans in it; the
    ///   `Task` variant (whose rendering `agent:{id}:room:{project}` would also
    ///   have been available) is the automated-origin family and would suppress
    ///   the strategic planner on every member's first turn.
    /// - `agent:{agent}:p-…` cannot collide with the agent's personal
    ///   `agent:{agent}:main`, so a room never lands in a member's own session
    ///   and never inherits its epoch sequence.
    ///
    /// The caller persists the rendered string once per room
    /// (`ProjectStore::claim_session_key`); it is not re-derived per member,
    /// because members may resolve different default agents.
    pub fn project_room(agent_id: impl Into<String>, project_id: &str) -> Self {
        Self::Main {
            agent_id: normalize_agent_id(&agent_id.into()),
            main_key: sanitize_component(project_id),
            epoch: 0,
        }
    }

    /// Create a per-peer DM session key (legacy compatibility alias).
    pub fn peer(agent_id: impl Into<String>, peer_id: impl Into<String>) -> Self {
        Self::dm(agent_id, "", peer_id, DmScope::PerPeer)
    }

    /// Create a DM session key with scope strategy
    ///
    /// If `dm_scope` is Main, returns a Main session key (DMs collapse into main).
    pub fn dm(
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        peer_id: impl Into<String>,
        dm_scope: DmScope,
    ) -> Self {
        let agent_id = normalize_agent_id(&agent_id.into());
        match dm_scope {
            DmScope::Main => Self::Main {
                agent_id,
                main_key: DEFAULT_MAIN_KEY.to_string(),
                epoch: 0,
            },
            _ => Self::DirectMessage {
                agent_id,
                channel: sanitize_component(&channel.into()),
                peer_id: sanitize_component(&peer_id.into()),
                dm_scope,
                epoch: 0,
            },
        }
    }

    /// Create a group session key
    pub fn group(
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        peer_kind: PeerKind,
        peer_id: impl Into<String>,
    ) -> Self {
        Self::Group {
            agent_id: normalize_agent_id(&agent_id.into()),
            channel: sanitize_component(&channel.into()),
            peer_kind,
            peer_id: sanitize_component(&peer_id.into()),
        }
    }

    /// Create a task session key. `task_type` is normalized and must not be a
    /// reserved routing marker (`peer`, `dm`, `ephemeral`) to avoid serializing
    /// an ambiguous key that would parse as a DM/Ephemeral instead.
    pub fn task(
        agent_id: impl Into<String>,
        task_type: impl Into<String>,
        task_id: impl Into<String>,
    ) -> Self {
        let task_type = normalize_agent_id(&task_type.into());
        // On the `if`, not on the `panic!`: an attribute applied directly to a
        // macro invocation is ignored ("the built-in attribute `expect` will be
        // ignored"), so writing it there suppresses nothing while looking like
        // it does.
        #[expect(
            clippy::panic,
            reason = "unreachable by construction; see the comment below and the \
                      parse-ordering test that pins it"
        )]
        if matches!(task_type.as_str(), "peer" | "dm" | "ephemeral") {
            // Unreachable, and deliberately still a panic. Every production
            // caller passes a compile-time constant ("cron", "heartbeat",
            // "a2a", TEAM_TASK_TASK_TYPE, TEAM_CHAT_TASK_TYPE); the one caller
            // that passes a runtime value (`builtin_tools::sessions::send_tool`)
            // rebuilds a key that came out of `parse`, and `parse_rest` matches
            // the reserved markers *before* its `[task_type, task_id]`
            // catch-all, so a parsed Task cannot carry one. That ordering is
            // the load-bearing fact, and it is pinned by
            // `parse_never_yields_a_task_whose_type_is_a_reserved_marker` below
            // — reorder those arms and the test goes red, rather than a crafted
            // session key reaching here.
            //
            // Returning `Result` instead would put a `?` on ~15 infallible call
            // sites to describe a state none of them can produce; sanitizing
            // the value instead would ship the ambiguity silently, which is the
            // failure this exists to prevent.
            //
            // `expect` rather than `allow` on purpose: it retires itself. If
            // the panic ever goes away, `unfulfilled_lint_expectations` fails
            // the Lint job instead of leaving a permit behind.
            // rust-doctor-disable-next-line panic-in-library
            panic!("reserved task_type `{task_type}` would produce an ambiguous session key");
        }
        Self::Task {
            agent_id: normalize_agent_id(&agent_id.into()),
            task_type,
            task_id: normalize_agent_id(&task_id.into()),
        }
    }

    /// Create an ephemeral session key
    pub fn ephemeral(agent_id: impl Into<String>) -> Self {
        Self::Ephemeral {
            agent_id: normalize_agent_id(&agent_id.into()),
            ephemeral_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn subagent(parent_key: Self, subagent_id: impl Into<String>) -> Self {
        Self::Subagent {
            parent_key: Box::new(parent_key),
            subagent_id: sanitize_component(&subagent_id.into()),
        }
    }

    /// Format the base string for a DM session key (shared between `to_key_string` and `base_key_pattern`).
    fn format_dm_base(agent_id: &str, channel: &str, peer_id: &str, dm_scope: &DmScope) -> String {
        match dm_scope {
            DmScope::Main => format!("agent:{agent_id}:main"),
            DmScope::PerPeer if channel.is_empty() => {
                format!("agent:{agent_id}:peer:{peer_id}")
            }
            DmScope::PerPeer => format!("agent:{agent_id}:dm:{peer_id}"),
            DmScope::PerChannelPeer => {
                format!("agent:{agent_id}:{channel}:dm:{peer_id}")
            }
        }
    }

    /// Append epoch suffix if non-zero.
    fn append_epoch(base: String, epoch: u32) -> String {
        if epoch > 0 {
            format!("{base}:s{epoch}")
        } else {
            base
        }
    }

    /// Get the agent ID from this session key
    #[must_use]
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Main { agent_id, .. } => agent_id,
            Self::DirectMessage { agent_id, .. } => agent_id,
            Self::Group { agent_id, .. } => agent_id,
            Self::Task { agent_id, .. } => agent_id,
            Self::Subagent { parent_key, .. } => parent_key.agent_id(),
            Self::Ephemeral { agent_id, .. } => agent_id,
        }
    }

    /// True for genuine human-interactive session variants (`Main`,
    /// `DirectMessage`, `Group`). False for automated/internal origins (`Task`
    /// = cron/webhook/team_chat, `Subagent`, `Ephemeral`). Used by the naked
    /// agent-loop strategic-planner gate so a cron job / group-chat member /
    /// subagent's first turn never trips the planner (R7: an origin fact, not a
    /// message-content heuristic). Fail-closed: any future internal variant
    /// defaults to non-interactive.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Self::Main { .. } | Self::DirectMessage { .. } | Self::Group { .. }
        )
    }

    /// Get the epoch of this session key (only Main and `DirectMessage` have epochs)
    #[must_use]
    pub const fn epoch(&self) -> u32 {
        match self {
            Self::Main { epoch, .. } | Self::DirectMessage { epoch, .. } => *epoch,
            _ => 0,
        }
    }

    /// Return a clone with the given epoch (legacy compatibility alias).
    #[must_use]
    pub fn with_epoch(&self, epoch: u32) -> Self {
        let mut cloned = self.clone();
        match cloned {
            Self::Main {
                epoch: ref mut e, ..
            } => *e = epoch,
            Self::DirectMessage {
                epoch: ref mut e, ..
            } => *e = epoch,
            _ => {}
        }
        cloned
    }

    /// Return a clone with epoch incremented by 1.
    /// For non-epoch types (Group, Task, Subagent, Ephemeral), returns clone unchanged.
    #[must_use]
    pub fn with_next_epoch(&self) -> Self {
        let mut cloned = self.clone();
        match cloned {
            Self::Main { ref mut epoch, .. } => {
                *epoch = epoch.saturating_add(1);
            }
            Self::DirectMessage { ref mut epoch, .. } => {
                *epoch = epoch.saturating_add(1);
            }
            _ => {}
        }
        cloned
    }

    /// Return the base key string WITHOUT epoch suffix (for SQL LIKE queries).
    #[must_use]
    pub fn base_key_pattern(&self) -> String {
        match self {
            Self::Main {
                agent_id, main_key, ..
            } => {
                format!("agent:{agent_id}:{main_key}")
            }
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                ..
            } => Self::format_dm_base(agent_id, channel, peer_id, dm_scope),
            // Non-epoch types: base_key_pattern == to_key_string
            _ => self.to_key_string(),
        }
    }

    /// Serialize to string key for storage/lookup
    #[must_use]
    pub fn to_key_string(&self) -> String {
        match self {
            Self::Main {
                agent_id,
                main_key,
                epoch,
            } => {
                let base = format!("agent:{agent_id}:{main_key}");
                Self::append_epoch(base, *epoch)
            }
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                epoch,
            } => {
                let base = Self::format_dm_base(agent_id, channel, peer_id, dm_scope);
                Self::append_epoch(base, *epoch)
            }
            Self::Group {
                agent_id,
                channel,
                peer_kind,
                peer_id,
            } => {
                let kind = match peer_kind {
                    PeerKind::Group => "group",
                    PeerKind::Thread => "thread",
                };
                format!("agent:{agent_id}:{channel}:{kind}:{peer_id}")
            }
            Self::Task {
                agent_id,
                task_type,
                task_id,
            } => {
                format!("agent:{agent_id}:{task_type}:{task_id}")
            }
            Self::Subagent {
                parent_key,
                subagent_id,
            } => {
                format!("{}:subagent:{}", parent_key.to_key_string(), subagent_id)
            }
            Self::Ephemeral {
                agent_id,
                ephemeral_id,
            } => {
                format!("agent:{agent_id}:ephemeral:{ephemeral_id}")
            }
        }
    }

    /// Parse a session key from a string.
    ///
    /// Epoch suffixes (`:sN`) are only recognised after the standard key segments so
    /// that IDs which happen to be formatted like `"s1"` are not mis‑interpreted.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || !parts[0].eq_ignore_ascii_case("agent") {
            return None;
        }

        // Handle Subagent keys: agent:{parent_key}:subagent:{subagent_id}
        // Must check before parse_rest because subagent_id is not bounded.
        //
        // Use the LAST `subagent` marker so nested subagents round-trip:
        // `...:subagent:level1:subagent:level2` must parse the outer layer
        // (level2) with the rest recursed as the parent. Using the first marker
        // would silently drop every layer above the innermost subagent, collapsing
        // a sub-subagent session into its grandparent's session.
        if let Some(pos) = parts.iter().rposition(|&p| p == "subagent") {
            if pos >= 2 && pos + 1 < parts.len() {
                let parent_str = parts[..pos].join(":");
                let subagent_id = parts[pos + 1].to_string();
                if let Some(parent_key) = Self::parse(&parent_str) {
                    return Some(Self::Subagent {
                        parent_key: Box::new(parent_key),
                        subagent_id,
                    });
                }
            }
        }

        let agent_id = normalize_agent_id(parts[1]);
        if agent_id.is_empty() {
            return None;
        }

        let rest = &parts[2..];

        if let Some((rest_no_epoch, epoch)) = Self::strip_epoch(rest) {
            if let Some(key) = Self::parse_rest(&agent_id, rest_no_epoch, epoch) {
                // Epochs are only ever serialized onto Main (2-segment rest) and
                // DirectMessage keys. For 3+ segment keys, a trailing `:sN` must
                // NOT be stripped as an epoch when the non-epoch parse already
                // yields a concrete non-Main variant — otherwise a Group/DM whose
                // peer_id happens to look like `s7` (e.g. `agent:a:discord:group:s7`)
                // is mis-parsed as a shorter key, breaking the round-trip. The
                // 2-segment case keeps epoch priority so `agent:a:main:s7` stays Main.
                let prefer_direct = rest.len() >= 3
                    && Self::parse_rest(&agent_id, rest, 0)
                        .as_ref()
                        .is_some_and(|d| !matches!(d, Self::Main { .. }));
                if !prefer_direct {
                    return Some(key);
                }
            }
        }

        Self::parse_rest(&agent_id, rest, 0)
    }

    fn strip_epoch<'a>(rest: &'a [&'a str]) -> Option<(&'a [&'a str], u32)> {
        let last = rest.last()?;
        let n_str = last.strip_prefix('s')?;
        let n = n_str.parse::<u32>().ok()?;
        // Accept epoch 0 (s0) so that keys like agent:id:main:s0 are
        // parsed as Main with epoch 0 rather than falling through to
        // the [task_type, task_id] catch-all and becoming a Task.
        if rest.len() > 1 {
            // Only treat a trailing `:sN` as an epoch when the stripped rest
            // is a known epoch-bearing shape. For 2-segment rests, the first
            // segment must be `main` (Main) or a DM marker; otherwise a Task
            // whose `task_id` happens to match `s[0-9]+` (e.g. `cron:s7`)
            // would round-trip as Main and break routing.
            if rest.len() == 2 && !matches!(rest[0], "main" | "peer" | "dm") {
                return None;
            }
            Some((&rest[..rest.len() - 1], n))
        } else {
            None
        }
    }

    fn parse_rest(agent_id: &str, rest: &[&str], epoch: u32) -> Option<Self> {
        match rest {
            // agent:id:peer:peer_id (legacy per-peer DM format)
            ["peer", peer_id] => Some(Self::DirectMessage {
                agent_id: agent_id.to_string(),
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
                epoch,
            }),

            // agent:id:dm:peer (per-peer DM)
            ["dm", peer_id] => Some(Self::DirectMessage {
                agent_id: agent_id.to_string(),
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
                epoch,
            }),

            // agent:id:channel:dm:peer (per-channel-peer DM)
            [channel, "dm", peer_id] => Some(Self::DirectMessage {
                agent_id: agent_id.to_string(),
                channel: channel.to_string(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerChannelPeer,
                epoch,
            }),

            // agent:id:channel:group:peer
            [channel, "group", peer_id] => Some(Self::Group {
                agent_id: agent_id.to_string(),
                channel: channel.to_string(),
                peer_kind: PeerKind::Group,
                peer_id: peer_id.to_string(),
            }),

            // agent:id:channel:thread:peer
            [channel, "thread", peer_id] => Some(Self::Group {
                agent_id: agent_id.to_string(),
                channel: channel.to_string(),
                peer_kind: PeerKind::Thread,
                peer_id: peer_id.to_string(),
            }),

            // agent:id:ephemeral:uuid
            ["ephemeral", ephemeral_id] => Some(Self::Ephemeral {
                agent_id: agent_id.to_string(),
                ephemeral_id: ephemeral_id.to_string(),
            }),

            // agent:id:main (or any single token as main_key)
            // Must come before the catch-all task pattern so that "main" is
            // not misinterpreted as a task_type.
            //
            // Exclude the structural markers "peer"/"dm"/"ephemeral": these are
            // the leading tokens of two-segment DM/ephemeral keys. Without this
            // guard, parsing `agent:id:dm:s1` (a DM with peer_id "s1") would
            // strip "s1" as an epoch and collapse the leading "dm" into a
            // Main{main_key:"dm"} — leaking that DM into the agent's main
            // session. Rejecting them here forces the no-epoch fall-through in
            // `parse`, which matches the correct `["dm", peer_id]` arm.
            [task_type, task_id] if false => Some(Self::Task {
                agent_id: agent_id.to_string(),
                task_type: task_type.to_string(),
                task_id: task_id.to_string(),
            }),
            [main_key] if !matches!(*main_key, "peer" | "dm" | "ephemeral") => Some(Self::Main {
                agent_id: agent_id.to_string(),
                main_key: main_key.to_string(),
                epoch,
            }),

            [task_type, task_id] => Some(Self::Task {
                agent_id: agent_id.to_string(),
                task_type: task_type.to_string(),
                task_id: task_id.to_string(),
            }),

            _ => None,
        }
    }

    /// Alias for `parse` with legacy fallback to match gateway/router.rs behavior.
    #[must_use]
    pub fn from_key_string(s: &str) -> Option<Self> {
        Self::parse(s).or_else(|| Self::from_legacy(s))
    }

    /// Parse legacy format from gateway/router.rs for backward compatibility
    #[must_use]
    pub fn from_legacy(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || !parts[0].eq_ignore_ascii_case("agent") {
            return None;
        }

        let agent_id = normalize_agent_id(parts[1]);

        match parts.get(2..) {
            Some(&["peer", ref rest @ ..]) if !rest.is_empty() => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: rest.join(":"),
                dm_scope: DmScope::PerPeer,
                epoch: 0,
            }),
            Some(&["ephemeral", ephemeral_id]) => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),
            _ => None,
        }
    }
}

/// Normalize agent ID: lowercase, alphanumeric + dash/underscore, max 64 chars
#[must_use]
pub fn normalize_agent_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return DEFAULT_AGENT_ID.to_string();
    }

    let normalized: String = trimmed
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let result = normalized
        .trim_start_matches('-')
        .trim_end_matches('-')
        .to_string();

    if result.is_empty() {
        DEFAULT_AGENT_ID.to_string()
    } else if result.len() > 64 {
        // Safe: all chars are ASCII after the filter above, so byte[64] is always a char boundary.
        // Using chars().take() for defensive correctness against future filter changes.
        result.chars().take(64).collect()
    } else {
        result
    }
}

fn sanitize_component(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized: String = trimmed
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Cap component length to prevent unbounded session key growth.
    if normalized.len() > 64 {
        normalized.chars().take(64).collect()
    } else {
        normalized
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_key_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_constructor() {
        let key = SessionKey::main("main");
        assert_eq!(key.agent_id(), "main");
    }

    /// The room key must round-trip through the wire form the Panel sends
    /// back on every turn, stay interactive, and never collide with the
    /// agent's own main session.
    #[test]
    fn a_project_room_key_round_trips_and_stays_interactive() {
        let key = SessionKey::project_room("main", "p-7f3a9c");
        let wire = key.to_key_string();
        assert_eq!(wire, "agent:main:p-7f3a9c");
        assert_eq!(SessionKey::from_key_string(&wire), Some(key.clone()));
        assert!(
            key.is_interactive(),
            "a room chat has humans in it; the planner gate keys on this"
        );
        assert_ne!(wire, SessionKey::main("main").to_key_string());
        assert_ne!(
            key.base_key_pattern(),
            SessionKey::main("main").to_key_string()
        );
    }

    /// `SessionKey::task` panics on a reserved `task_type`, and the one caller
    /// that feeds it a runtime value rebuilds a key that came out of `parse`.
    /// That round-trip is safe only because `parse_rest` matches the reserved
    /// markers before its `[task_type, task_id]` catch-all — an ordering
    /// nothing asserted until now. Move those arms below the catch-all and this
    /// goes red, instead of a crafted session key turning into a panic.
    #[test]
    fn parse_never_yields_a_task_whose_type_is_a_reserved_marker() {
        const RESERVED: [&str; 3] = ["peer", "dm", "ephemeral"];
        let mut checked = 0;
        for marker in RESERVED {
            for key in [
                // Two-segment rest: the shape the catch-all would swallow.
                format!("agent:main:{marker}:x"),
                // ...and with a trailing `sN`, which `strip_epoch` rewrites.
                format!("agent:main:{marker}:s7"),
                // Three-segment rest, reached via the channel-qualified arms.
                format!("agent:main:chan:{marker}:x"),
            ] {
                for (via, parsed) in [
                    ("parse", SessionKey::parse(&key)),
                    // `from_key_string` adds the legacy fallback; same invariant.
                    ("from_key_string", SessionKey::from_key_string(&key)),
                ] {
                    checked += 1;
                    let offending = match &parsed {
                        Some(SessionKey::Task { task_type, .. })
                            if RESERVED.contains(&task_type.as_str()) =>
                        {
                            Some(task_type.clone())
                        }
                        _ => None,
                    };
                    assert!(
                        offending.is_none(),
                        "{via}({key:?}) produced a Task with reserved task_type {offending:?}; \
                         round-tripping it through SessionKey::task would panic"
                    );
                }
            }
        }
        assert_eq!(
            checked, 18,
            "expected 3 markers x 3 shapes x 2 entry points"
        );
    }

    #[test]
    fn test_dm_per_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::DirectMessage { .. }));
    }

    #[test]
    fn test_dm_main_scope_returns_main() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::Main);
        assert!(matches!(key, SessionKey::Main { .. }));
    }

    #[test]
    fn test_group_constructor() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::Group { .. }));
    }

    #[test]
    fn test_task_constructor() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(key.agent_id(), "main");
    }

    #[test]
    fn test_ephemeral_constructor() {
        let key = SessionKey::ephemeral("main");
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::Ephemeral { .. }));
    }

    #[test]
    fn test_subagent_agent_id_delegates_to_parent() {
        let parent = SessionKey::main("main");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        assert_eq!(key.agent_id(), "main");
    }

    // --- Serialization tests ---

    #[test]
    fn test_to_key_string_main() {
        let key = SessionKey::main("main");
        assert_eq!(key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_to_key_string_dm_per_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        assert_eq!(key.to_key_string(), "agent:main:dm:user123");
    }

    #[test]
    fn test_to_key_string_peer() {
        let key = SessionKey::peer("main", "user123");
        assert_eq!(key.to_key_string(), "agent:main:peer:user123");
    }

    #[test]
    fn test_to_key_string_dm_per_channel_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerChannelPeer);
        assert_eq!(key.to_key_string(), "agent:main:telegram:dm:user123");
    }

    #[test]
    fn test_to_key_string_group() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(key.to_key_string(), "agent:main:discord:group:guild456");
    }

    #[test]
    fn test_to_key_string_task() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(key.to_key_string(), "agent:main:cron:daily-summary");
    }

    #[test]
    fn test_to_key_string_subagent() {
        let parent = SessionKey::main("main");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        assert_eq!(key.to_key_string(), "agent:main:main:subagent:coding");
    }

    // --- Parse tests ---

    #[test]
    fn test_parse_main() {
        let key = SessionKey::parse("agent:main:main").unwrap();
        assert!(
            matches!(key, SessionKey::Main { agent_id, main_key, .. } if agent_id == "main" && main_key == "main")
        );
    }

    #[test]
    fn test_parse_dm_per_peer() {
        let key = SessionKey::parse("agent:main:dm:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, .. } if peer_id == "user123")
        );
    }

    #[test]
    fn test_parse_peer_legacy() {
        let key = SessionKey::parse("agent:main:peer:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, channel, .. } if peer_id == "user123" && channel.is_empty())
        );
    }

    #[test]
    fn test_parse_dm_per_channel_peer() {
        let key = SessionKey::parse("agent:main:telegram:dm:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { channel, peer_id, dm_scope: DmScope::PerChannelPeer, .. } if channel == "telegram" && peer_id == "user123")
        );
    }

    #[test]
    fn test_parse_group() {
        let key = SessionKey::parse("agent:main:discord:group:guild456").unwrap();
        assert!(
            matches!(key, SessionKey::Group { channel, peer_kind: PeerKind::Group, peer_id, .. } if channel == "discord" && peer_id == "guild456")
        );
    }

    #[test]
    fn test_parse_task() {
        let key = SessionKey::parse("agent:main:cron:daily").unwrap();
        assert!(
            matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "cron" && task_id == "daily")
        );
    }

    #[test]
    fn test_parse_ephemeral() {
        let key = SessionKey::parse("agent:main:ephemeral:abc-123").unwrap();
        assert!(
            matches!(key, SessionKey::Ephemeral { ephemeral_id, .. } if ephemeral_id == "abc-123")
        );
    }

    #[test]
    fn test_parse_subagent() {
        let key = SessionKey::parse("agent:main:main:subagent:coding").unwrap();
        assert!(matches!(key, SessionKey::Subagent { subagent_id, .. } if subagent_id == "coding"));
    }

    #[test]
    fn test_parse_task_team() {
        let key = SessionKey::parse("agent:main:team:task-1").unwrap();
        assert!(
            matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "team" && task_id == "task-1")
        );
    }

    #[test]
    fn test_parse_task_heartbeat() {
        let key = SessionKey::parse("agent:main:heartbeat:check-1").unwrap();
        assert!(
            matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "heartbeat" && task_id == "check-1")
        );
    }

    #[test]
    fn test_parse_task_a2a() {
        let key = SessionKey::parse("agent:main:a2a:req-1").unwrap();
        assert!(
            matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "a2a" && task_id == "req-1")
        );
    }

    #[test]
    fn test_parse_invalid() {
        assert!(SessionKey::parse("invalid").is_none());
        assert!(SessionKey::parse("agent:").is_none());
        assert!(SessionKey::parse("").is_none());
    }

    #[test]
    fn test_roundtrip() {
        let keys = vec![
            SessionKey::main("work"),
            SessionKey::peer("main", "user1"),
            SessionKey::dm("main", "discord", "user2", DmScope::PerChannelPeer),
            SessionKey::task("main", "webhook", "hook-1"),
            SessionKey::Subagent {
                parent_key: Box::new(SessionKey::main("main")),
                subagent_id: "coding".to_string(),
            },
        ];
        for key in keys {
            let s = key.to_key_string();
            let parsed = SessionKey::parse(&s).unwrap_or_else(|| panic!("Failed to parse: {}", s));
            assert_eq!(parsed.to_key_string(), s, "Roundtrip failed for: {}", s);
        }
    }

    // --- Epoch tests ---

    #[test]
    fn test_main_with_epoch() {
        let key = SessionKey::Main {
            agent_id: "main".to_string(),
            main_key: "main".to_string(),
            epoch: 2,
        };
        assert_eq!(key.to_key_string(), "agent:main:main:s2");
        assert_eq!(key.epoch(), 2);
    }

    #[test]
    fn test_main_epoch_zero_no_suffix() {
        let key = SessionKey::main("main");
        assert_eq!(key.to_key_string(), "agent:main:main");
        assert_eq!(key.epoch(), 0);
    }

    #[test]
    fn test_parse_with_epoch() {
        let key = SessionKey::parse("agent:main:main:s3").unwrap();
        assert_eq!(key.epoch(), 3);
    }

    #[test]
    fn test_parse_without_epoch_defaults_zero() {
        let key = SessionKey::parse("agent:main:main").unwrap();
        assert_eq!(key.epoch(), 0);
    }

    #[test]
    fn test_dm_with_epoch_roundtrip() {
        let key = SessionKey::DirectMessage {
            agent_id: "main".to_string(),
            channel: String::new(),
            peer_id: "user123".to_string(),
            dm_scope: DmScope::PerPeer,
            epoch: 1,
        };
        assert_eq!(key.to_key_string(), "agent:main:peer:user123:s1");
        let parsed = SessionKey::parse(&key.to_key_string()).unwrap();
        assert_eq!(parsed.epoch(), 1);
    }

    #[test]
    fn test_next_epoch() {
        let key = SessionKey::main("main");
        let next = key.with_next_epoch();
        assert_eq!(next.epoch(), 1);
        assert_eq!(next.to_key_string(), "agent:main:main:s1");
        let next2 = next.with_next_epoch();
        assert_eq!(next2.epoch(), 2);
    }

    #[test]
    fn test_base_key_pattern() {
        let key = SessionKey::Main {
            agent_id: "main".to_string(),
            main_key: "main".to_string(),
            epoch: 3,
        };
        assert_eq!(key.base_key_pattern(), "agent:main:main");
    }

    #[test]
    fn test_epoch_roundtrip_all_types() {
        let keys_with_epoch = vec![
            SessionKey::Main {
                agent_id: "work".to_string(),
                main_key: "main".to_string(),
                epoch: 5,
            },
            SessionKey::DirectMessage {
                agent_id: "main".to_string(),
                channel: "telegram".to_string(),
                peer_id: "u1".to_string(),
                dm_scope: DmScope::PerChannelPeer,
                epoch: 2,
            },
        ];
        for key in keys_with_epoch {
            let s = key.to_key_string();
            let parsed = SessionKey::parse(&s).unwrap_or_else(|| panic!("Failed to parse: {}", s));
            assert_eq!(
                parsed.epoch(),
                key.epoch(),
                "Epoch roundtrip failed for: {}",
                s
            );
        }
    }

    #[test]
    fn test_parse_dm_peer_id_looks_like_epoch() {
        // A DM whose peer_id is "s1" must parse as a DM, not collapse into a Main
        // session (which would leak the DM into the agent main thread). The key
        // guarantee is that the trailing "s1" is NOT stripped as an epoch.
        //
        // Note: a `dm:` key drops the channel for PerPeer scope, so the parsed
        // form has an empty channel and canonically re-serializes via the legacy
        // `peer:` spelling — `dm:`/`peer:` are intentionally distinct outputs for
        // PerPeer (see `test_to_key_string_dm_per_peer` / `test_to_key_string_peer`).
        // We therefore assert the parsed *shape* (PerPeer DM, peer=s1), not a
        // byte-identical round-trip, which the canonical-form asymmetry precludes.
        let key = SessionKey::dm("main", "telegram", "s1", DmScope::PerPeer);
        assert_eq!(key.to_key_string(), "agent:main:dm:s1");
        let parsed = SessionKey::parse("agent:main:dm:s1").expect("must parse");
        assert!(
            matches!(
                &parsed,
                SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, .. }
                    if peer_id == "s1"
            ),
            "expected per-peer DirectMessage(peer=s1), got {parsed:?}"
        );
    }

    #[test]
    fn test_parse_legacy_peer_id_looks_like_epoch() {
        let parsed = SessionKey::parse("agent:main:peer:s42").expect("must parse");
        assert!(
            matches!(&parsed, SessionKey::DirectMessage { peer_id, .. } if peer_id == "s42"),
            "expected DirectMessage(peer=s42), got {parsed:?}"
        );
    }

    #[test]
    fn test_parse_ephemeral_id_looks_like_epoch() {
        let parsed = SessionKey::parse("agent:main:ephemeral:s7").expect("must parse");
        assert!(
            matches!(&parsed, SessionKey::Ephemeral { ephemeral_id, .. } if ephemeral_id == "s7"),
            "expected Ephemeral(id=s7), got {parsed:?}"
        );
    }

    #[test]
    fn test_roundtrip_with_special_chars() {
        let key = SessionKey::dm("main", "ch:annel", "us:er", DmScope::PerChannelPeer);
        let s = key.to_key_string();
        let parsed = SessionKey::parse(&s).expect("must parse sanitized key");
        assert_eq!(parsed.to_key_string(), s);
        assert_eq!(
            parsed,
            SessionKey::dm("main", "ch-annel", "us-er", DmScope::PerChannelPeer)
        );
    }

    #[test]
    fn test_task_roundtrip_arbitrary_type() {
        let key = SessionKey::task("main", "custom_type", "id-1");
        let s = key.to_key_string();
        let parsed = SessionKey::parse(&s).expect("must parse arbitrary task type");
        assert_eq!(parsed.to_key_string(), s);
    }

    #[test]
    fn test_nested_subagent_roundtrip() {
        // A sub-subagent must round-trip without collapsing into its
        // grandparent's session (parse must use the OUTERMOST subagent marker).
        let grandparent = SessionKey::dm("main", "telegram", "user", DmScope::PerChannelPeer);
        let parent = SessionKey::subagent(grandparent, "level1");
        let key = SessionKey::subagent(parent, "level2");
        let s = key.to_key_string();
        let parsed = SessionKey::parse(&s).expect("must parse nested subagent");
        assert_eq!(
            parsed.to_key_string(),
            s,
            "nested subagent roundtrip failed"
        );
        // The outer layer (level2) must survive parsing.
        match parsed {
            SessionKey::Subagent {
                parent_key,
                subagent_id,
            } => {
                assert_eq!(subagent_id, "level2");
                assert!(matches!(*parent_key, SessionKey::Subagent { .. }));
            }
            other => panic!("expected nested Subagent, got {other:?}"),
        }
    }

    #[test]
    fn test_group_peer_id_resembling_epoch_roundtrip() {
        // A group peer_id that looks like an epoch suffix (`s7`) must not be
        // mis-stripped into a shorter key.
        let key = SessionKey::group("main", "discord", PeerKind::Group, "s7");
        let s = key.to_key_string();
        assert_eq!(s, "agent:main:discord:group:s7");
        let parsed = SessionKey::parse(&s).expect("must parse group with s7 peer");
        assert_eq!(parsed.to_key_string(), s);
        assert!(matches!(
            parsed,
            SessionKey::Group { peer_id, .. } if peer_id == "s7"
        ));
    }

    #[test]
    fn test_subagent_constructor() {
        let parent = SessionKey::main("main");
        let key = SessionKey::subagent(parent, "coding:agent");
        assert_eq!(key.agent_id(), "main");
        let s = key.to_key_string();
        let parsed = SessionKey::parse(&s).expect("must parse subagent");
        assert_eq!(parsed.to_key_string(), s);
    }

    #[test]
    fn is_interactive_true_for_human_variants() {
        // Main + DirectMessage (via the peer alias) are genuine human sessions.
        assert!(SessionKey::main("a").is_interactive());
        assert!(SessionKey::peer("a", "peer-1").is_interactive());
    }

    #[test]
    fn is_interactive_false_for_automated_variants() {
        // cron + group-chat member runs use Task keys; subagents/ephemerals too.
        // These must never trip the naked-loop planner gate.
        assert!(!SessionKey::task("a", "cron", "job-1").is_interactive());
        assert!(!SessionKey::task("a", "team_chat", "team-1").is_interactive());
        assert!(!SessionKey::ephemeral("a").is_interactive());
        assert!(!SessionKey::subagent(SessionKey::main("a"), "sub-1").is_interactive());
    }
}
