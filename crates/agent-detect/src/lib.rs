//! Agent state detection via terminal screen pattern matching.
//!
//! Ported from herdr (Apache-2.0) — see NOTICE. Type names are kept
//! identical to upstream so fixes can be carried across by diff.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDetection {
    pub state: AgentState,
    pub skip_state_update: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
}

/// Screen snapshot plus OSC-derived strings.
///
/// Empty `osc_title` / `osc_progress` mean "not available" and make the
/// engine behave exactly as the pre-OSC version. They never mean
/// "the title is empty" (judgment §8).
#[derive(Debug, Clone, Copy)]
pub struct DetectionInput<'a> {
    pub screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}

/// Which agent we detected running in a pane.
///
/// Copied verbatim from herdr `src/detect/mod.rs` (upstream lines 43-67,
/// herdr 0.8.2) — variant list, order and names unchanged so upstream
/// additions can be carried across by diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    Mastracode,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
    Qwen,
    Maki,
    Muse,
}

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
///
/// Task 1 stub: always returns `None`. The real label-matching table
/// (upstream `parse_agent_label` / `lookup_agent`, herdr `src/detect/mod.rs`)
/// is ported in Task 2.
#[must_use]
pub fn identify_agent(_process_name: &str) -> Option<Agent> {
    None
}

/// Detect an agent's state from a screen snapshot.
///
/// Task 1 stub. Upstream's `detect_agent_with_osc` starts with an early
/// return for `agent: None`:
///
/// ```text
/// let Some(agent) = agent else { return ...AgentState::Unknown...; };
/// ```
///
/// That early return is CORRECT and PERMANENT, not a placeholder — with no
/// identified agent there is nothing to match screen text against, so the
/// state is `Unknown` regardless of screen content ("I don't know" and "it's
/// idle" are different things, judgment §8). This stub reproduces exactly
/// that return value; it does not yet reach the manifest-driven engine for
/// `Some(agent)`, since that engine does not exist in this crate until
/// Task 2 lands it.
#[must_use]
pub fn detect(_agent: Option<Agent>, _input: DetectionInput<'_>) -> AgentDetection {
    AgentDetection {
        state: AgentState::Unknown,
        skip_state_update: false,
        visible_idle: false,
        visible_blocker: false,
        visible_working: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空屏幕不认识任何 agent，必须是 Unknown——不是 Idle。
    /// 「我不知道」和「它闲着」是两件事（判据 §8）。
    #[test]
    fn an_empty_screen_is_unknown_not_idle() {
        let out = detect(None, DetectionInput { screen: "", osc_title: "", osc_progress: "" });
        assert_eq!(out.state, AgentState::Unknown);
        assert!(!out.visible_idle);
    }

    /// Task 1 only ships a stub `detect()` that always returns `Unknown`
    /// (see its doc comment), so this is ignored for now. Task 2 ports the
    /// real manifest-driven engine into this crate — at that point remove
    /// `#[ignore]` and this must pass.
    ///
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
    #[test]
    #[ignore]
    fn a_known_agent_on_its_idle_chrome_is_not_unknown() {
        let out = detect(
            identify_agent("claude"),
            DetectionInput { screen: "", osc_title: "✳ Claude Code", osc_progress: "" },
        );
        assert_ne!(out.state, AgentState::Unknown);
    }
}
