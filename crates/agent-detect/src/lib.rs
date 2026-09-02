//! Agent state detection via terminal screen pattern matching.
//!
//! Ported from herdr (Apache-2.0) — see NOTICE. Type names are kept
//! identical to upstream so fixes can be carried across by diff.
//!
//! Module map, and where each came from in herdr 0.8.2:
//!
//! | module | upstream |
//! |---|---|
//! | [`engine`] | `src/detect/mod.rs` |
//! | [`manifest`] | `src/detect/manifest.rs` + `src/detect/manifests/*.toml` |
//! | [`screen_rules`] | `src/pane/agent_detection.rs` (one fn of ten; see its header) |
//!
//! `src/detect/manifest_update.rs` is deliberately absent: every function in
//! it was the remote-download path, which this phase does not ship. The one
//! item the engine actually needs — `ManifestVersion` — was salvaged into
//! [`manifest`].

pub mod engine;
pub mod manifest;
pub mod screen_rules;

pub use engine::{agent_label, Agent, AgentDetection, AgentState};
pub use manifest::{manifest_version, DetectionInput, ManifestSource};

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
///
/// Delegates to upstream's [`engine::parse_agent_label`], which matches the
/// canonical label first and then the alias table.
#[must_use]
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    engine::identify_agent(process_name)
}

/// Detect an agent's state from a screen snapshot.
///
/// Upstream's `detect_agent_with_osc` starts with an early return for
/// `agent: None`:
///
/// ```text
/// let Some(agent) = agent else { return ...AgentState::Unknown...; };
/// ```
///
/// That early return is CORRECT and PERMANENT, not a placeholder — with no
/// identified agent there is nothing to match screen text against, so the
/// state is `Unknown` regardless of screen content ("I don't know" and "it's
/// idle" are different things, judgment §8).
///
/// This is a thin adapter, not a second engine: it exists only because Aleph
/// passes the screen and the two OSC strings as one [`DetectionInput`] while
/// upstream passes them as three arguments. All behaviour lives in
/// [`engine::detect_agent_with_osc`].
#[must_use]
pub fn detect(agent: Option<Agent>, input: DetectionInput<'_>) -> AgentDetection {
    engine::detect_agent_with_osc(agent, input.screen, input.osc_title, input.osc_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空屏幕不认识任何 agent，必须是 Unknown——不是 Idle。
    /// 「我不知道」和「它闲着」是两件事（判据 §8）。
    #[test]
    fn an_empty_screen_is_unknown_not_idle() {
        let out = detect(
            None,
            DetectionInput {
                screen: "",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(out.state, AgentState::Unknown);
        assert!(!out.visible_idle);
    }

    /// The idle-chrome signal below is copied verbatim, not invented: it is
    /// herdr's own `claude_osc_title_static_prefix_is_idle` test (herdr
    /// 0.8.2, `src/detect/manifest/tests.rs:689-696`), which asserts
    /// `osc_explain(Agent::Claude, "", "✳ Claude Code", "")` is
    /// `AgentState::Idle` via rule `osc_title_idle`. No screen-only (no-OSC)
    /// idle fixture for Claude exists anywhere in herdr's committed test
    /// suite — confirmed by grepping `src/detect/`, `src/pane.rs` and
    /// `tests/` for `Agent::Claude` and for the box-drawing glyph (`❯`) used
    /// by Claude's screen-based idle rule (`live_prompt_box` in
    /// `src/detect/manifests/claude.toml`) — so this test drives the
    /// `osc_title` field instead of `screen`, deviating from the literal
    /// call shape in the task instructions on that one point.
    ///
    /// Shipped `#[ignore]`d in Task 1 against the stub `detect()`. Task 2
    /// replaced the stub with the ported engine and removed the attribute:
    /// this now exercises `identify_agent` -> the bundled `claude.toml` ->
    /// the `osc_title` region, i.e. the whole path end to end.
    ///
    /// `visible_idle` carries the discrimination. `assert_ne!(.., Unknown)`
    /// alone is close to a恒真 predicate here (判据 §2): ANY known agent that
    /// matches no rule at all still lands on `Idle` via
    /// `DEFAULT_KNOWN_AGENT_IDLE_FALLBACK`, so that assertion survives the
    /// `osc_title` region being wired to nothing --- verified by mutation.
    /// The fallback sets `visible_idle: false`; only a rule match sets it, so
    /// the pair separates "recognized the idle chrome" from "gave up and
    /// guessed idle". This matches what upstream asserts on the same fixture
    /// (`claude_osc_title_static_prefix_is_idle`, herdr
    /// `src/detect/manifest/tests.rs:689-696`).
    #[test]
    fn a_known_agent_on_its_idle_chrome_is_not_unknown() {
        let out = detect(
            identify_agent("claude"),
            DetectionInput {
                screen: "",
                osc_title: "✳ Claude Code",
                osc_progress: "",
            },
        );
        assert_ne!(out.state, AgentState::Unknown);
        assert!(
            out.visible_idle,
            "matched the idle rule, not the idle fallback"
        );
    }
}
