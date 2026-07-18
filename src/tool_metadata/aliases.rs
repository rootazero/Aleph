//! Slash-command shorthand aliases — the single source of truth.
//!
//! One `const` drives **both** layers a slash alias must touch:
//!
//! - **Execution fast-path** (`gateway::execution_engine::slash_command`) maps a
//!   typed alias (`/model`) to its canonical tool name (`select_model`) for
//!   direct dispatch on the Panel/CLI path via [`resolve_shorthand`].
//! - **Discovery / resolution**: the `ToolCatalog` builder seeds each matching
//!   `UnifiedTool`'s `aliases` from [`shorthand_aliases_for`], so the same
//!   shortcut appears in `commands.list`, resolves via `find_best_match`
//!   (Tier-2 alias) on channels, and powers "did you mean?" suggestions.
//!
//! Previously the two layers kept **disjoint** alias tables that silently
//! drifted: `SHORTHAND_ALIASES` lived in the gateway execution engine and only
//! the fast path read it (so `/image` executed but was undiscoverable), while
//! `UnifiedTool::with_aliases` was used once for `/new` (discoverable but not in
//! the execution table). Collapsing the alias facts into this one const — owned
//! by the tool-metadata layer both consumers already depend on — removes that
//! drift: adding a row here surfaces the shortcut in every layer at once.
//!
//! Aliases here must target a **fast-path-executable** builtin tool — one the
//! execution fast path can run via `ToolRegistry::execute_tool`. That is either
//! a static tool with a `create_tool_boxed` arm (`select_model`, `doctor`, …)
//! OR a live-registered tool that has an `execute_tool` dispatch arm even though
//! `create_tool_boxed` returns `None` (a `SessionManager`-dependent tool built
//! at boot — e.g. `session_compact`). The fast path dispatches through
//! `execute_tool` (see `execution_engine::slash_command::execute_direct_tool`),
//! NOT `create_tool_boxed`, so the dispatch arm is what actually matters.
//!
//! What does NOT belong here: shortcuts for commands the **router intercepts**
//! before the fast path and handles via a bespoke lifecycle path rather than the
//! tool (e.g. `/new`, `/clear` → `handle_new_session`, which regenerates a topic
//! and terminates continuations). `session_new` is a live tool too, but its
//! `/new`/`/clear` names stay discovery-only (`with_aliases` on the curated
//! entry) because the router owns its execution; a SHORTHAND row would race the
//! router intercept. `session_compact` has no such router path — the tool IS the
//! whole operation — so it is fast-pathed here.

/// Shorthand slash alias → canonical tool name.
///
/// The canonical name is the tool's `BUILTIN_TOOL_DEFINITIONS` name, i.e. the
/// key the execution registry's `get_tool` and the catalog's bare registration
/// both use.
pub const SHORTHAND_ALIASES: &[(&str, &str)] = &[
    // ── Generation shorthands (pre-existing) ──────────────────────────────
    ("rename", "session_set_topic"),
    ("video", "video_generate"),
    ("image", "image_generate"),
    ("audio", "audio_generate"),
    ("speech", "speech_generate"),
    // ── Cross-tool-standard names surfaced from existing builtin tools ─────
    // Each targets a tool already present in BUILTIN_TOOL_DEFINITIONS (so it is
    // already bare-resolvable as `/select_model` etc.); the alias just adds the
    // familiar name every reference CLI (codex/openclaw/hermes/kimi) uses.
    ("model", "select_model"),
    ("config", "self_config"),
    ("status", "doctor"),
    ("memories", "memory_search"),
    ("agent", "agent_switch"),
    ("agents", "agent_switch"),
    // ── Session compaction (codex `/compact`, hermes `/compress` parity) ───
    // `session_compact` is a live `SessionManager`-dependent tool (`None` in
    // `create_tool_boxed`) but has an `execute_tool` dispatch arm, so the fast
    // path runs it deterministically on every surface. The TUI/CLI already
    // reach the same `SessionStore::compact` via the `session.compact` RPC.
    ("compact", "session_compact"),
    ("compress", "session_compact"),
];

/// Reverse lookup: the aliases pointing at `canonical` (empty if none).
///
/// Used by the `ToolCatalog` builder to seed a tool's discoverable `aliases`
/// from the same source the fast path executes against.
#[must_use]
pub fn shorthand_aliases_for(canonical: &str) -> Vec<&'static str> {
    SHORTHAND_ALIASES
        .iter()
        .filter(|(_, target)| *target == canonical)
        .map(|(alias, _)| *alias)
        .collect()
}

/// Resolve a typed shorthand to its canonical tool name, if any.
#[must_use]
pub fn resolve_shorthand(name: &str) -> Option<&'static str> {
    SHORTHAND_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, canonical)| *canonical)
}

/// Returns `true` if `name` is a shorthand slash alias (e.g. `image`, `model`).
#[must_use]
pub fn is_shorthand_alias(name: &str) -> bool {
    SHORTHAND_ALIASES.iter().any(|(alias, _)| *alias == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_lookup_matches_flat_table() {
        assert_eq!(shorthand_aliases_for("select_model"), vec!["model"]);
        assert_eq!(shorthand_aliases_for("agent_switch"), vec!["agent", "agents"]);
        assert_eq!(
            shorthand_aliases_for("session_compact"),
            vec!["compact", "compress"]
        );
        assert!(shorthand_aliases_for("nonexistent").is_empty());
    }

    #[test]
    fn compact_aliases_resolve_to_session_compact() {
        assert_eq!(resolve_shorthand("compact"), Some("session_compact"));
        assert_eq!(resolve_shorthand("compress"), Some("session_compact"));
    }

    #[test]
    fn resolve_and_is_alias_agree() {
        for (alias, canonical) in SHORTHAND_ALIASES {
            assert!(is_shorthand_alias(alias));
            assert_eq!(resolve_shorthand(alias), Some(*canonical));
        }
        assert!(!is_shorthand_alias("definitely_not_an_alias"));
        assert_eq!(resolve_shorthand("definitely_not_an_alias"), None);
    }
}
