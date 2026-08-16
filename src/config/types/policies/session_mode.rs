//! Session usage mode — the user-facing dial over the tool *presentation*
//! surface: **chat** / **work** / **code**.
//!
//! The third twin of [`super::exec_tier::ExecTier`] (autonomy) and
//! `ThinkLevel` (reasoning depth): same session-metadata carrier, same
//! request > session > global resolution, a different — fully orthogonal —
//! axis. A mode never grants or denies anything: safety stays with the exec
//! tier / `[policies.tool_permissions]` / sandbox. What a mode changes is
//! **resource allocation** of the tool surface, through the two presentation
//! mechanisms that already exist:
//!
//! * which tools keep a **full schema** in every request (the `[tools] core`
//!   set, consumed by `ProgressiveDisclosureRewriter`; everything else is
//!   collapsed and loaded on demand via `get_tool_schema`), and
//! * which tool families are **deferred** out of the model's initial tool
//!   list entirely (`DeferredTools`, discoverable + promotable via
//!   `tool_search`).
//!
//! Every tool therefore stays reachable in every mode — the model can always
//! pull a deferred tool with `tool_search` (R7 sovereignty). The partition is
//! static and content-blind: it keys on the user's declared mode and the
//! tool's registry name, never on what the message says. That is exactly the
//! progressive-disclosure shape R10 blesses, in the tool-presentation layer,
//! with zero `src/harness/` growth.
//!
//! Survey grounding (2026-07-21 spec): every studied product implements a
//! mode primarily as tool availability (Zed profiles, Copilot custom agents,
//! Continue mode policies, Roo groups), keeps autonomy an independent dial
//! (codex sandbox × approval), and puts the switcher in the composer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Identity-metadata custom key under which a session's mode override is
/// persisted (written through `sessions.patch` or stamped from a
/// request-carried value, read per turn by the execution engine). Third twin
/// of `EXEC_TIER_SESSION_KEY` / `THINK_LEVEL_SESSION_KEY`.
pub const MODE_SESSION_KEY: &str = "session_mode";

/// Dev-focused tools *subtracted* from the schema-resident core set in Chat
/// mode. They stay listed (name + description) and callable; only their full
/// schema moves behind a `get_tool_schema` round-trip. (`code_check` is not
/// here — chat *defers* it entirely, see `CHAT_DEFER_FAMILIES`, so a core
/// subtraction would be dead.) `subagent` and `get_tool_schema` are never
/// subtracted — see `default_core_tools`'s snapshot-exemption invariant.
const CHAT_CORE_SUBTRACT: &[&str] = &["bash", "code_exec", "file_write", "file_edit", "file_ops"];

/// Dev tools *added* to the schema-resident core set in Code mode (on top of
/// whatever `[tools] core` configures).
const CODE_CORE_ADD: &[&str] = &["apply_patch", "ctx_search"];

/// Tool *families* deferred out of the initial tool list in Chat mode. An
/// entry matches at an `_` word boundary: `desktop` catches `desktop` and
/// every `desktop_*` tool but never `desktops`; `loop` catches `loop` and
/// `loop_graph`. Everything here remains discoverable + promotable via
/// `tool_search`. MCP-qualified names (`{server}__{tool}`) are exempt from
/// these tables — see `defers_tool`.
///
/// Deliberate keeps (audited 2026-07-21): `media_understand` stays listed
/// (users paste images into chat), the `agent_*` management family stays
/// (R8 conversational management), and `system`/`pim` stay (their desktop
/// dependency is an implementation detail, not their register).
///
/// Added to that list 2026-08-10: `workspace_manage`. It reads like the
/// `cron_manage` / `skill_manage` register — an admin surface — but its common
/// use is the one-shot conversational question its group-mates already stay
/// resident for ("which workspaces do I have?", "rename this one"), so it is
/// kept listed deliberately.
///
/// The *reason* first recorded here was wrong, and the correction prices every
/// future entry: deferring does not "buy only the schema". A deferred tool is
/// dropped from the model's tool array outright — `ScopedToolService` `retain`s
/// on `DeferredTools` — so its name, its description and the 118-byte "call
/// get_tool_schema first" sentence the collapse rewriter appends all leave the
/// wire. `definitions.rs::CATALOG_DESCRIPTION_CEILING_BYTES` bounds the
/// *constants* the binary carries, not what a request sends. Deferral is the
/// only mechanism here that makes a tool cost zero bytes per request;
/// collapsing merely makes it cost less.
const CHAT_DEFER_FAMILIES: &[&str] = &[
    "desktop",
    "browser",
    "team",
    "task",
    "node",
    "image_generate",
    "video_generate",
    "audio_generate",
    "speech_generate",
    "pdf_generate",
    "cron_manage",
    // Same register as `cron_manage` / `skill_manage`: an admin surface you
    // reach for deliberately ("why isn't my hook firing?"), not something a
    // casual chat turn needs resident. `tool_search` promotes it on demand.
    "hooks",
    "automation",
    "heartbeat",
    "goal",
    "loop",
    "workflow",
    "google_meet",
    "hub",
    "skill_install",
    "skill_manage",
    "a2a",
    "acp",
    "gateway_route",
    "apply_patch",
    "code_check",
    "strategy",
    "vault_store",
    "session_collaborate",
    "session_turn",
    // The whiteboard tool: a deliberate editing surface, not a casual chat
    // need. The `_` word boundary keeps a future `canvas_export` deferred
    // with it; work / code keep it listed. `tool_search` promotes on demand.
    "canvas",
];

/// Exact tool names deferred in Chat mode — entries whose *family* must not
/// be caught wholesale: `media`/`media_send` are deferred but
/// `media_understand` (multimodal analysis of user-pasted images/audio)
/// must stay listed, so a `media` family entry would over-catch.
const CHAT_DEFER_EXACT: &[&str] = &["media", "media_send"];

/// Tool families deferred in Code mode: the desktop / generation / meeting
/// families, which have no place in a development session's initial surface.
/// The browser *core* stays resident (dev-server preview) while its satellites
/// defer in every mode — see [`BROWSER_RESIDENT_CORE`]. Team/task tools
/// (multi-agent development) and `media_understand` (screenshot debugging) stay
/// listed whole.
const CODE_DEFER_FAMILIES: &[&str] = &[
    "desktop",
    "image_generate",
    "video_generate",
    "audio_generate",
    "speech_generate",
    "google_meet",
];

/// Exact tool names deferred in Code mode (same `media_understand` carve-out
/// as chat).
const CODE_DEFER_EXACT: &[&str] = &["media", "media_send"];

/// The `browser_*` tools that stay in the model's initial list wherever the
/// family is offered at all. Everything else in the family — hover, drag,
/// cookies, emulate, pdf, network, dialog, … — is deferred, and `tool_search`
/// promotes it the moment the model reaches for it.
///
/// These three are what a browsing session cannot *start* without: open a page,
/// read its refs, then act on several refs in one batched call. Every satellite
/// verb is a variation on the third.
///
/// Why the family needs a partition of its own when collapsing already exists:
/// a listed-but-collapsed tool still ships its name, its whole description, a
/// 45-byte placeholder schema and the 118-byte collapse sentence on **every**
/// request. Measured 2026-08-12, all 26 browser tools cost 9,536 B per request
/// in work and code — for a capability the overwhelming majority of turns never
/// touch — of which this core is 1,590 B, so the partition returns 7,946 B a
/// request. `browser_family_deferral_is_measured` re-measures both numbers from
/// the live catalog and prints them, rather than trusting this sentence — which
/// is the point: a description edit moves these figures, so read the test
/// output, not the prose, when the exact number matters.
const BROWSER_RESIDENT_CORE: &[&str] = &["browser_open", "browser_snapshot", "browser_exec"];

/// Meta / lifeline tools that must never be deferred in any mode: deferring
/// the discovery mechanism itself (or the human channel, or the R8 tool that
/// leaves the mode) would strand the model.
const NEVER_DEFER: &[&str] = &[
    "tool_search",
    "get_tool_schema",
    "ask_user",
    "session_set_mode",
    "self_config",
];

/// Whether family `entry` matches tool `name` at an `_` word boundary:
/// equal, or `name` continues past the entry with an underscore (`desktop`
/// matches `desktop` and `desktop_som`, never `desktops`).
fn matches_family(entry: &str, name: &str) -> bool {
    name.strip_prefix(entry)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
}

/// Whether `name` is a browser *satellite*: a `browser_*` tool outside
/// [`BROWSER_RESIDENT_CORE`]. True in every mode — chat defers the whole family
/// through [`CHAT_DEFER_FAMILIES`] anyway, and work / code keep only the core
/// listed.
fn is_browser_satellite(name: &str) -> bool {
    matches_family("browser", name) && !BROWSER_RESIDENT_CORE.contains(&name)
}

/// Session usage mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Lightweight conversation: minimal schema-resident core, heavy tool
    /// families deferred (still `tool_search`-reachable).
    Chat,
    /// Multi-step productivity work. The default, and the identity partition
    /// for every family but one: the `browser_*` satellites defer here too
    /// (see [`BROWSER_RESIDENT_CORE`]), because 26 listed browser tools charge
    /// every request ~8.9 KB for a capability most turns never touch. Nothing
    /// else about an unconfigured install's surface changed.
    #[default]
    Work,
    /// Software development: dev tools schema-resident, desktop/media
    /// families deferred.
    Code,
}

impl SessionMode {
    /// Parse a mode from its serialized id (`"chat"` / `"work"` / `"code"`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "chat" => Some(Self::Chat),
            "work" => Some(Self::Work),
            "code" => Some(Self::Code),
            _ => None,
        }
    }

    /// Serialized id of this mode.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Work => "work",
            Self::Code => "code",
        }
    }

    /// The schema-resident core set for this mode, derived from the
    /// configured `[tools] core` list. Work returns it unchanged (the
    /// backward-compatible default); Chat subtracts the dev-focused names;
    /// Code adds the dev extras.
    ///
    /// The progressive-disclosure escape hatch is respected: an empty or
    /// `["*"]` configured core means collapsing is disabled, and the mode
    /// must not re-enable it.
    #[must_use]
    pub fn effective_core_tools(self, configured: &[String]) -> Vec<String> {
        let disabled = configured.is_empty() || configured.iter().any(|c| c == "*");
        if disabled {
            return configured.to_vec();
        }
        match self {
            Self::Work => configured.to_vec(),
            Self::Chat => {
                let subtracted: Vec<String> = configured
                    .iter()
                    .filter(|c| !CHAT_CORE_SUBTRACT.contains(&c.as_str()))
                    .cloned()
                    .collect();
                // A configured core that is a subset of the subtraction list
                // would drain to empty — which downstream reads as the
                // "disclosure disabled" sentinel, giving chat MORE resident
                // schema than work. The escape hatch must stay
                // operator-explicit: fall back to the configured set.
                if subtracted.is_empty() {
                    configured.to_vec()
                } else {
                    subtracted
                }
            }
            Self::Code => {
                let mut core = configured.to_vec();
                for extra in CODE_CORE_ADD {
                    if !core.iter().any(|c| c == extra) {
                        core.push((*extra).to_string());
                    }
                }
                core
            }
        }
    }

    /// Whether this mode defers `name` out of the model's initial tool list.
    /// Static, content-blind, name-keyed — and always overridable by the
    /// model via `tool_search` promotion.
    ///
    /// Family entries match at an `_` word boundary (never mid-word), and
    /// MCP-qualified names (`{server}__{tool}`) are exempt entirely: an
    /// operator's server id (`goal_tracker`, `media_kit`, …) must not collide
    /// with a builtin family word. MCP deferral has its own dedicated knob
    /// (`[tools] defer_mcp_tools`).
    ///
    /// One rule is mode-independent: the browser satellites
    /// ([`is_browser_satellite`]) defer everywhere. It is stated ahead of the
    /// per-mode tables instead of being copied into each of them, so the
    /// family's resident core is named in exactly one place.
    #[must_use]
    pub fn defers_tool(self, name: &str) -> bool {
        if NEVER_DEFER.contains(&name) || name.contains("__") {
            return false;
        }
        if is_browser_satellite(name) {
            return true;
        }
        let (families, exact) = match self {
            Self::Chat => (CHAT_DEFER_FAMILIES, CHAT_DEFER_EXACT),
            Self::Work => return false,
            Self::Code => (CODE_DEFER_FAMILIES, CODE_DEFER_EXACT),
        };
        exact.contains(&name) || families.iter().any(|f| matches_family(f, name))
    }

    /// One model-facing line describing this mode's register and surface, for
    /// the system prompt (rendered by `SecurityLayer`, beside the exec tier's
    /// approval line). The copy lives next to the partition tables — the
    /// single source of what each mode actually changes — so rule and
    /// description cannot drift (R9). Mentions `session_set_mode` so the
    /// switch stays model-drivable (R8).
    #[must_use]
    pub const fn prompt_line(self) -> &'static str {
        match self {
            Self::Chat => {
                "Usage mode: chat — a lightweight conversation session. Heavy tool \
                 families (desktop, browser, media, teams, automation) are deferred from \
                 your tool list but discoverable via `tool_search` when genuinely needed. \
                 Stay conversational; don't start long autonomous jobs unprompted. The user \
                 picks the mode; call `session_set_mode` only when they ask to switch."
            }
            Self::Work => {
                "Usage mode: work — multi-step productivity work (documents, research, \
                 channels, scheduling, media). The standard tool surface is available; \
                 plan visibly, aim for a concrete deliverable, and prefer finished outputs \
                 in plain language over technical process detail. The user picks the mode; \
                 call `session_set_mode` only when they ask to switch."
            }
            Self::Code => {
                "Usage mode: code — a software development session. Dev tools (bash, \
                 code_exec, file editing, apply_patch) carry full schemas; desktop/media/\
                 browser tool families are deferred but discoverable via `tool_search`. \
                 Verify your \
                 changes with checks or tests where practical; technical detail (diffs, \
                 commands, logs) is welcome in replies. The user picks the mode; \
                 call `session_set_mode` only when they ask to switch."
            }
        }
    }

    /// Shortened mode line for SUBAGENT prompts: a spawned child inherits the
    /// parent's partitioned tool surface (same core set, same deferred
    /// families), so it must be told the register and that `tool_search`
    /// promotes — but not the user-switching contract (`session_set_mode`
    /// belongs to the parent conversation, not an ephemeral child session).
    /// Lives beside [`Self::prompt_line`] so the two cannot drift (R9).
    /// Callers skip the weld for `Work` (`SpawnRequest::session_mode`): a child
    /// of a work run inherits the surface its parent already runs under,
    /// browser subtraction included, so the `Work` line below is kept in step
    /// with [`Self::prompt_line`] rather than rendered.
    #[must_use]
    pub const fn subagent_prompt_line(self) -> &'static str {
        match self {
            Self::Chat => {
                "Usage mode: chat — a lightweight conversation session. Heavy tool \
                 families (desktop, browser, media, teams, automation) are deferred from \
                 your tool list but discoverable via `tool_search` when genuinely needed."
            }
            Self::Work => {
                "Usage mode: work — multi-step productivity work. The standard tool \
                 surface is available."
            }
            Self::Code => {
                "Usage mode: code — a software development session. Dev tools carry \
                 full schemas; desktop/media/browser tool families are deferred but \
                 discoverable via `tool_search`. Verify your changes with checks or \
                 tests where practical."
            }
        }
    }
}

/// A mode as offered to a user surface (Panel / CLI / bot). Core owns the
/// IDENTITY (id set + order + every partition rule behind it), never the
/// copy — same contract as [`super::exec_tier::TierPreset`] (R4/R6).
pub type ModePreset = super::DialPreset;

/// The three built-in modes, in display order.
#[must_use]
pub const fn builtin_modes() -> &'static [ModePreset] {
    &[
        ModePreset { id: "chat" },
        ModePreset { id: "work" },
        ModePreset { id: "code" },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn mode_id_roundtrip() {
        for mode in [SessionMode::Chat, SessionMode::Work, SessionMode::Code] {
            assert_eq!(SessionMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(SessionMode::from_id("nonsense"), None);
        assert_eq!(SessionMode::from_id(""), None);
    }

    #[test]
    fn default_mode_is_work() {
        assert_eq!(SessionMode::default(), SessionMode::Work);
    }

    /// Work is the compatibility mode everywhere except the browser family: it
    /// must not touch the core set at all, and must defer nothing but the
    /// browser satellites, so an unconfigured install behaves as it did before
    /// modes existed for every other tool it owns.
    #[test]
    fn work_mode_changes_nothing_but_the_browser_satellites() {
        let core = crate::config::types::tools::default_core_tools();
        assert_eq!(SessionMode::Work.effective_core_tools(&core), core);
        for name in ["bash", "desktop_som", "team_create", "image_generate"] {
            assert!(!SessionMode::Work.defers_tool(name));
        }
        assert!(
            SessionMode::Work.defers_tool("browser_hover"),
            "the one subtraction work makes must actually be made"
        );
    }

    #[test]
    fn chat_mode_subtracts_dev_tools_from_core() {
        let core = crate::config::types::tools::default_core_tools();
        let chat = SessionMode::Chat.effective_core_tools(&core);
        for gone in CHAT_CORE_SUBTRACT {
            assert!(!chat.iter().any(|c| c == gone), "`{gone}` must leave core");
        }
        // Conversation essentials stay.
        for kept in ["search", "web_fetch", "memory_search", "ask_user"] {
            assert!(chat.iter().any(|c| c == kept), "`{kept}` must stay core");
        }
        // The snapshot-exemption invariant survives the subtraction.
        for kept in ["subagent", "get_tool_schema"] {
            assert!(chat.iter().any(|c| c == kept), "`{kept}` must stay core");
        }
    }

    #[test]
    fn code_mode_adds_dev_extras_without_duplicates() {
        let core = crate::config::types::tools::default_core_tools();
        let code = SessionMode::Code.effective_core_tools(&core);
        for extra in CODE_CORE_ADD {
            assert_eq!(code.iter().filter(|c| c.as_str() == *extra).count(), 1);
        }
        // Superset of the configured core.
        for name in &core {
            assert!(code.iter().any(|c| c == name));
        }
    }

    /// Empty / `["*"]` core = the operator disabled schema collapsing; no
    /// mode may re-enable it behind their back.
    #[test]
    fn escape_hatch_is_respected_by_every_mode() {
        for mode in [SessionMode::Chat, SessionMode::Work, SessionMode::Code] {
            assert!(mode.effective_core_tools(&[]).is_empty());
            assert_eq!(
                mode.effective_core_tools(&cfg(&["*"])),
                cfg(&["*"]),
                "{mode:?} must not disturb the wildcard escape hatch"
            );
        }
    }

    /// The inverse guard: a configured core that happens to be a subset of
    /// CHAT_CORE_SUBTRACT must not drain to empty — empty is the "disclosure
    /// disabled" sentinel downstream, which would give chat MORE resident
    /// schema than work (the exact opposite of its intent).
    #[test]
    fn chat_subtraction_never_drains_core_to_empty() {
        let configured = cfg(&["bash", "file_edit"]);
        assert_eq!(
            SessionMode::Chat.effective_core_tools(&configured),
            configured,
            "a fully-subtracted core must fall back to the configured set"
        );
    }

    #[test]
    fn chat_mode_defers_heavy_families() {
        for name in [
            "desktop_som",
            "desktop",
            "browser_navigate",
            "team_create",
            "task_create",
            "node_invoke",
            "media",
            "media_send",
            "image_generate",
            "cron_manage",
            "goal",
            "loop_graph",
            "workflow",
            "apply_patch",
            "code_check",
            "session_collaborate",
            "session_turn",
        ] {
            assert!(
                SessionMode::Chat.defers_tool(name),
                "chat must defer `{name}`"
            );
        }
        // Conversation surface stays resident.
        for name in [
            "search",
            "web_fetch",
            "memory_search",
            "remember",
            "note_manage",
            "file_read",
            "session_send",
            "skill_read",
            "bash",             // collapsed out of core, but still listed
            "media_understand", // users paste images into chat — carve-out
            // Handing the user a finished document is a conversational outcome
            // as much as a working one ("write this up as a report"), and the
            // schema is one small object. Resident in every mode on purpose.
            "artifact_publish",
        ] {
            assert!(
                !SessionMode::Chat.defers_tool(name),
                "chat must keep `{name}` listed"
            );
        }
    }

    #[test]
    fn code_mode_defers_desktop_media_and_the_browser_satellites() {
        for name in [
            "desktop_som",
            "media_send",
            "image_generate",
            "google_meet",
            // Dev-server preview starts from the browser core; the satellites
            // ride `tool_search` like every other deferred verb.
            "browser_navigate",
            "browser_console",
        ] {
            assert!(SessionMode::Code.defers_tool(name));
        }
        for name in [
            "browser_open", // the family's entry point stays one call away
            "team_create",
            "bash",
            "apply_patch",
            "media_understand", // screenshot debugging — carve-out
            "artifact_publish", // design docs and analyses are code-mode output
        ] {
            assert!(
                !SessionMode::Code.defers_tool(name),
                "code must keep `{name}` listed"
            );
        }
    }

    /// Family entries match only at an `_` word boundary: `goal` must not
    /// catch `goals_list`, `desktop` must not catch `desktops`.
    #[test]
    fn family_matching_stops_at_word_boundary() {
        assert!(SessionMode::Chat.defers_tool("goal"));
        assert!(SessionMode::Chat.defers_tool("goal_review"));
        assert!(!SessionMode::Chat.defers_tool("goals_list"));
        assert!(!SessionMode::Chat.defers_tool("desktops"));
        assert!(!SessionMode::Chat.defers_tool("automations_helper"));
    }

    /// MCP-qualified names (`{server}__{tool}`) are exempt from the builtin
    /// tables — an operator's server id must not collide with a family word.
    /// MCP deferral has its own knob (`[tools] defer_mcp_tools`).
    #[test]
    fn mcp_qualified_names_are_exempt() {
        for mode in [SessionMode::Chat, SessionMode::Code] {
            for name in [
                "goal_tracker__list",
                "desktop__click",
                "media_kit__render",
                "workflow__run",
            ] {
                assert!(
                    !mode.defers_tool(name),
                    "{mode:?} must not defer MCP tool `{name}`"
                );
            }
        }
    }

    /// The discovery mechanism, the human channel, and the R8 tools that can
    /// leave the mode must never be deferred — deferring them would strand
    /// the model inside the partition.
    #[test]
    fn lifeline_tools_are_never_deferred() {
        for mode in [SessionMode::Chat, SessionMode::Work, SessionMode::Code] {
            for name in NEVER_DEFER {
                assert!(
                    !mode.defers_tool(name),
                    "{mode:?} must never defer `{name}`"
                );
            }
        }
    }

    #[test]
    fn prompt_line_is_distinct_and_names_the_mode() {
        let chat = SessionMode::Chat.prompt_line();
        let work = SessionMode::Work.prompt_line();
        let code = SessionMode::Code.prompt_line();
        assert!(chat.contains("Usage mode: chat"));
        assert!(work.contains("Usage mode: work"));
        assert!(code.contains("Usage mode: code"));
        assert_ne!(chat, work);
        assert_ne!(work, code);
        assert_ne!(chat, code);
        // Model-drivable switching (R8) must be named in every line.
        for line in [chat, work, code] {
            assert!(line.contains("session_set_mode"));
        }
    }

    #[test]
    fn builtin_modes_cover_every_variant() {
        let ids: Vec<&str> = builtin_modes().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["chat", "work", "code"]);
        assert!(builtin_modes()
            .iter()
            .all(|p| SessionMode::from_id(p.id).is_some()));
    }

    /// The resident core must be *derived*, not a wish list. A `browser_*`
    /// rename that forgets this list would empty the core silently: the family
    /// would defer whole, and browsing would still work — through a
    /// `tool_search` round-trip nobody asked for — so nothing else would fail.
    /// Fail by name instead.
    #[test]
    fn every_browser_core_name_exists_in_the_builtin_catalog() {
        for name in BROWSER_RESIDENT_CORE {
            assert!(
                matches_family("browser", name),
                "`{name}` is not a browser tool"
            );
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS
                    .iter()
                    .any(|d| d.name == *name),
                "`{name}` is in BROWSER_RESIDENT_CORE but not in BUILTIN_TOOL_DEFINITIONS — \
                 if it was renamed, rename it here too"
            );
        }
    }

    /// The partition itself: outside chat the core is listed and everything
    /// else in the family is not. Chat still defers the family whole, so the
    /// core is not a chat carve-out by accident.
    #[test]
    fn work_and_code_list_the_browser_core_and_defer_the_rest() {
        for mode in [SessionMode::Work, SessionMode::Code] {
            for name in BROWSER_RESIDENT_CORE {
                assert!(
                    !mode.defers_tool(name),
                    "{mode:?} must keep `{name}` listed"
                );
            }
            for name in [
                "browser_hover",
                "browser_cookies",
                "browser_pdf",
                "browser_drag",
            ] {
                assert!(mode.defers_tool(name), "{mode:?} must defer `{name}`");
            }
        }
        for name in BROWSER_RESIDENT_CORE {
            assert!(
                SessionMode::Chat.defers_tool(name),
                "chat defers the whole family, core included"
            );
        }
    }

    /// Bytes one *listed* browser tool costs on the wire — measured, not
    /// modelled. Browser tools are not in `[tools] core`, so what a request
    /// carries is the definition after the real `ProgressiveDisclosureRewriter`
    /// has collapsed it; run it through that rewriter and weigh the result.
    ///
    /// `ENVELOPE_BYTES` is the only term that cannot be read off the definition:
    /// the JSON scaffolding around one tool object,
    /// `{"name":"","description":"","input_schema":}`.
    fn resident_wire_bytes(name: &str, description: &str) -> usize {
        use crate::tools::scoped::{ProgressiveDisclosureRewriter, ToolDefinitionRewriter};
        use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};

        const ENVELOPE_BYTES: usize = 44;

        let mut def = ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        };
        ProgressiveDisclosureRewriter::new(std::collections::BTreeSet::new(), false)
            .rewrite(&mut def);
        ENVELOPE_BYTES
            + def.name.len()
            + def.description.len()
            + serde_json::to_string(&def.input_schema)
                .expect("a collapsed schema serializes")
                .len()
    }

    /// What the partition is for, in bytes, re-measured from the live catalog
    /// on every run — the numbers quoted in [`BROWSER_RESIDENT_CORE`]'s doc
    /// come from this test's output, so a description edit moves both.
    ///
    /// The assertion is a ratio rather than a ceiling: pinning a byte count
    /// would red on every wording change, while "the satellites are the bulk of
    /// what the family costs, and they are gone" is the property that must hold.
    #[test]
    fn browser_family_deferral_is_measured() {
        let family: Vec<(&str, &str)> = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .filter(|d| matches_family("browser", d.name))
            .map(|d| (d.name, d.description))
            .collect();
        assert!(!family.is_empty(), "the browser family must be registered");

        let cost = |mode: Option<SessionMode>| -> usize {
            family
                .iter()
                .filter(|(name, _)| mode.is_none_or(|m| !m.defers_tool(name)))
                .map(|(name, desc)| resident_wire_bytes(name, desc))
                .sum()
        };

        let all_listed = cost(None);
        let work = cost(Some(SessionMode::Work));
        let code = cost(Some(SessionMode::Code));
        let chat = cost(Some(SessionMode::Chat));
        eprintln!(
            "browser family per-request wire cost ({} tools): all-listed {all_listed} B, \
             work {work} B, code {code} B, chat {chat} B",
            family.len()
        );

        assert_eq!(chat, 0, "chat defers the family whole");
        assert_eq!(work, code, "the partition is mode-independent outside chat");
        assert!(
            work * 4 < all_listed,
            "deferral must remove the bulk of the family's footprint: \
             {work} B resident of {all_listed} B listed"
        );
    }

    #[test]
    fn deserializes_from_policies_toml() {
        let toml_str = r#"mode = "chat""#;
        let cfg: super::super::PoliciesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.mode, SessionMode::Chat);
        // Unconfigured installs stay on today's behavior.
        let empty: super::super::PoliciesConfig = toml::from_str("").unwrap();
        assert_eq!(empty.mode, SessionMode::Work);
    }
}
