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
    // ── Session shorthand (pre-existing) ──────────────────────────────────
    // Canonical tool name is `session_rename` (the LLM-facing name in
    // `BUILTIN_TOOL_DEFINITIONS`); its impl struct is `SessionSetTopicTool`
    // and it reads a `topic` arg, but the registered/dispatched name is
    // `session_rename`, so that is what the alias and the arg-mapper key on.
    ("rename", "session_rename"),
    // ── Generation shorthands (pre-existing) ──────────────────────────────
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
    // path runs it deterministically on every surface. The TUI/CLI reach the
    // SAME `context::compact::manual::compact_session` through the
    // `session.compact` RPC, so every surface compacts identically (R6).
    // Trailing free text becomes the summary directive — see
    // `slash_command::build_tool_arguments`'s `session_compact` arm, without
    // which the generic `{input,query,args}` fallback silently drops it.
    ("compact", "session_compact"),
    ("compress", "session_compact"),
];

/// Shorthand targets that are **not** in `BUILTIN_TOOL_DEFINITIONS`.
///
/// These are provider-gated optional tools (registered at boot in
/// `optional_tools.rs`, gated on a live generation provider): they have an
/// `execute_tool` dispatch arm but no static def. The catalog's defs-driven
/// alias-seeding loop only iterates `BUILTIN_TOOL_DEFINITIONS`, so without a
/// dedicated pass their shorthand aliases (`/video`, `/audio`, `/speech`) would
/// resolve ONLY on the Panel/CLI fast path (`resolve_shorthand` → runtime
/// `get_tool`) and stay invisible to channels and `/help` — breaking this
/// module's invariant that "a row in `SHORTHAND_ALIASES` surfaces the shortcut
/// in every layer". `tool_catalog_init` iterates this list to seed a discovery
/// entry per target (mirroring how `/image` is surfaced from the in-defs
/// `image_generate`), and the executability guard test treats these as
/// legitimately backed. The description is what `/help` shows for the entry.
///
/// Every entry MUST be a `SHORTHAND_ALIASES` target and MUST NOT be in
/// `BUILTIN_TOOL_DEFINITIONS` — both directions are asserted by tests here.
pub const RUNTIME_ONLY_ALIAS_TARGETS: &[(&str, &str)] = &[
    (
        "video_generate",
        "Generate a short video from a text prompt",
    ),
    (
        "audio_generate",
        "Generate audio or music from a text prompt",
    ),
    (
        "speech_generate",
        "Convert text to spoken audio (text-to-speech)",
    ),
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
        assert_eq!(
            shorthand_aliases_for("agent_switch"),
            vec!["agent", "agents"]
        );
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

    // ── Cross-table executability guard (round-3) ─────────────────────────
    // These couple the alias table to the real tool set at compile-time. They
    // exist because two silent severed wires shipped without them: `/rename`
    // pointed at a phantom `session_set_topic`, and curated `generate_image`/
    // `switch` had no dispatch arm. The self-consistency tests above cannot
    // catch a wrong-but-well-formed target; only a check against the executor's
    // tool set can. A mis-targeted alias now fails the test suite.

    /// Every `SHORTHAND_ALIASES` target must be executable: either a static
    /// builtin (in `BUILTIN_TOOL_DEFINITIONS`) or a declared runtime-only
    /// provider-gated tool. This is what would have caught F1 (`/rename` →
    /// `session_set_topic`, which is in neither set).
    #[test]
    fn every_shorthand_target_is_executable() {
        use crate::executor::BUILTIN_TOOL_DEFINITIONS;
        for (alias, target) in SHORTHAND_ALIASES {
            let in_defs = BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == *target);
            let runtime_only = RUNTIME_ONLY_ALIAS_TARGETS.iter().any(|(t, _)| t == target);
            assert!(
                in_defs || runtime_only,
                "/{alias} → `{target}` has no executable backing: it is not in \
                 BUILTIN_TOOL_DEFINITIONS and not a declared runtime-only target. \
                 Fix the alias target, or (if it is a provider-gated runtime tool) \
                 add it to RUNTIME_ONLY_ALIAS_TARGETS."
            );
        }
    }

    /// Every runtime-only entry must actually be seeded (a shorthand row points
    /// at it) and must not duplicate a defs entry (which the defs loop already
    /// seeds). Keeps the two-list contract honest in both directions.
    #[test]
    fn runtime_only_targets_are_shorthand_targets_not_in_defs() {
        use crate::executor::BUILTIN_TOOL_DEFINITIONS;
        for (target, description) in RUNTIME_ONLY_ALIAS_TARGETS {
            assert!(
                SHORTHAND_ALIASES.iter().any(|(_, t)| t == target),
                "`{target}` is declared runtime-only but no SHORTHAND_ALIASES row \
                 points to it — its catalog entry would never be seeded."
            );
            assert!(
                !BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == *target),
                "`{target}` is in BUILTIN_TOOL_DEFINITIONS, so the defs loop already \
                 seeds its alias — remove it from RUNTIME_ONLY_ALIAS_TARGETS to avoid \
                 a duplicate catalog entry."
            );
            assert!(
                !description.is_empty(),
                "`{target}` needs a /help description"
            );
        }
    }

    /// F1 regression: `/rename` must resolve to the real `session_rename` tool
    /// (canonical name in defs), not the phantom `session_set_topic`.
    #[test]
    fn rename_alias_targets_real_session_rename_tool() {
        use crate::executor::BUILTIN_TOOL_DEFINITIONS;
        assert_eq!(resolve_shorthand("rename"), Some("session_rename"));
        assert!(BUILTIN_TOOL_DEFINITIONS
            .iter()
            .any(|d| d.name == "session_rename"));
        assert!(!BUILTIN_TOOL_DEFINITIONS
            .iter()
            .any(|d| d.name == "session_set_topic"));
    }
}
