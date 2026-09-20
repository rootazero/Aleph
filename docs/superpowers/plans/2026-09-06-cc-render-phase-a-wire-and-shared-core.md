# CC-Style Transcript Rendering — Phase A (Wire + Shared Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put every medium-independent piece of Claude-Code-style transcript rendering in place — typed `FileChange` diffs riding a UI side-channel from the file tools to the wire and the trace, a `trace.tool_output` RPC for untruncated output, a `context.breakdown` RPC backed by a per-session measurement of the real prompt, and the `shared-ui-logic::transcript` pure-function cluster (view model, summarizer, fold, group, diff view, markdown enhance, context reconcile, turn summary, affordance, theme tokens) — so Phase B (TUI) and Phase C (Panel) are painting only.

**Architecture:** Tools emit a `_presentation` key inside their normal JSON output; `apply_layer_two` hoists it into `ToolOutputMetadata.presentation` before the value is flattened to model text (the same hoist-before-truncate pattern `images` already uses). The harness passes the metadata to the callback, the orchestrator carries it on `FlowStreamEvent::ToolCallDone`, the drain sets it on the wire `ToolResult.presentation`, and the trace mirror stores it on `AgentTraceToolCallEnd.presentation` so `trace.by_runs` replays it. All shapes live once in `aleph-protocol`. Rendering decisions live once in `shared-ui-logic::transcript` as `data → data` functions with no leptos/ratatui dependency.

**Tech Stack:** Rust 1.96 (MSRV 1.95), serde, `similar` 2 (new alephcore dep, `default-features=false, features=["text"]`), `unicode-width` 0.2 (already a dep), tokio, rusqlite (existing), schemars.

**Spec:** `docs/superpowers/specs/2026-09-06-cc-style-transcript-rendering-design.md`

## Global Constraints

- Branch `worktree-cc-render-r1` only; never touch `main`. Commit messages: English `<scope>: <description>`, trailer `Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP`.
- R1/R3: `similar` is the only new runtime dep, in alephcore `[dependencies]` (root `Cargo.toml:158-313`) with the house comment naming `cargo tree -p alephcore -i similar`. No other new deps.
- R9: the model-facing tool text must be byte-identical before and after this phase (`aleph-server prompt-size` and the "presentation never reaches model text" test).
- R10: `src/harness/` is ratcheted by `src/harness/tests/budget.rs::CEILING` (5239 at branch start). This phase touches it in exactly one task (Task 15), designed as a zero-line delta. If the measured count still exceeds CEILING, raise it by the measured delta and answer the three R10 questions in that commit body.
- Wire contract (判据 §10): every new wire shape is defined in `shared/protocol` and CONSTRUCTED by the server from that type; clients parse it. No hand-written `json!` response for a new method.
- Fail-closed (判据 §8): `Err` / `None` mean "unknown", never "empty" or "healthy".
- `shared-ui-logic::transcript` must compile with `default-features = false` (TUI build) — no feature gates, no leptos, no web-sys.
- Verification for alephcore: use `CARGO_TARGET_DIR=D:/Workspace/Aleph/target` for `check` / `--lib` / clippy; run `cargo test -p alephcore --lib <filter>` per module (the full `--lib` build is ~13 min and must be detached — see memory `alephcore-build-memory`). Compare failure NAMES against `scratchpad/baseline/alephcore-lib-branch-start.out.txt`, not counts.
- Run from the worktree root `D:\Workspace\Aleph\.claude\worktrees\cc-render-r1`. Bash tool paths: `/d/Workspace/Aleph/.claude/worktrees/cc-render-r1`.

---

## File Structure

**Create**
- `shared/protocol/src/file_change.rs` — `FileChange`, `FileChangeKind`, `Hunk`, `HunkLine`, `LineTag`, `Unavailable`, `Presentation`, `MAX_HUNK_LINES`.
- `shared/protocol/src/context_breakdown.rs` — `ContextBreakdown`, `LayerSizeView`, `ToolSchemaSize`, `UsageTokens`, `ToolOutputPage`, `ToolOutputSource`.
- `src/builtin_tools/file_ops/diff.rs` — `compute_file_change(path, before, after) -> FileChange` (the only `similar` call site).
- `src/gateway/handlers/tool_output.rs` — `handle_tool_output` (`trace.tool_output`).
- `src/gateway/handlers/context_breakdown.rs` — `handle_context_breakdown` (`context.breakdown`).
- `src/thinker/prompt_size_registry.rs` — `PromptSizeRegistry` (latest measured layout per session) + its `CapabilitySlot`.
- `shared/ui_logic/src/transcript/mod.rs` + `view_model.rs`, `summarize.rs`, `fold.rs`, `group.rs`, `diff_view.rs`, `md_enhance.rs`, `context.rs`, `turn_summary.rs`, `affordance.rs`, `theme_tokens.rs`.

**Modify**
- `shared/protocol/src/lib.rs` — two `pub mod` + re-exports.
- `shared/protocol/src/events.rs` — `ToolResult.presentation` (and CUT the never-written `ToolResult.metadata`), `RunSummary.context_tokens/context_window`, `ModelInfo.context_window`, `AgentTraceToolCallEnd.presentation`.
- `src/session/events.rs` — `ToolOutputMetadata.presentation`.
- `src/tools/result_processing.rs` — `hoist_presentation`.
- `src/tools/scoped/dispatch.rs:1447-1458` — call the hoist.
- `src/builtin_tools/file_ops/{mod.rs,edit.rs,write.rs,ops.rs,apply_patch.rs}` — attach `_presentation`.
- `src/tools/traits.rs` — `AlephTool::mutates_file_content()` default `false`.
- `src/harness/callback.rs`, `src/harness/agent/act.rs` — pass `presentation` to `on_tool_call_done` (Task 9, ratchet).
- `src/orchestrator/dispatch.rs`, `src/orchestrator/harness_bridge/callback.rs` — `ToolCallDone.presentation`.
- `src/gateway/execution_engine/event_drain.rs` — set `ToolResult.presentation`; fill `AgentTraceToolCallEnd.presentation`.
- `src/gateway/event_emitter/types.rs` — `ToolResult` becomes `pub use aleph_protocol::ToolResult`; `RunSummary` twin stays but a parity test pins the key set.
- `src/thinker/prompt_pipeline.rs` — `execute_measured` (single pass returning sizes).
- `src/gateway/method_census.rs`, `method_visibility.rs`, `method_admin.rs`, `src/bin/aleph-server/commands/start/builder/**` — register the two methods.
- `shared/ui_logic/src/lib.rs`, `Cargo.toml`, `connection/mod.rs`, `connection/failure.rs`, `safety/*` — CUT dead items; `interfaces/webchat/Cargo.toml:124` — drop the `leptos` feature request.
- `plugins/diff-viewer/` — delete (zero references).

**Test**
- Each new file carries `#[cfg(test)] mod tests`. Cross-crate guards: `shared/protocol/src/file_change.rs` round-trip; `src/tools/scoped/tests.rs` (or new `src/tools/presentation_census.rs`) census; `src/gateway/handlers/tool_output.rs` NotFound; `src/gateway/handlers/context_breakdown.rs` contract keys.

---

## Part 1 — `shared-ui-logic::transcript` (no server dependency; can run in parallel with Part 2)

### Task 1: `transcript` module skeleton + `theme_tokens`

**Files:**
- Create: `shared/ui_logic/src/transcript/mod.rs`, `shared/ui_logic/src/transcript/theme_tokens.rs`
- Modify: `shared/ui_logic/src/lib.rs`

**Interfaces:**
- Produces: `shared_ui_logic::transcript::{SemanticColor, ALL_SEMANTIC_COLORS}`; `SemanticColor::css_var(self) -> &'static str`.

- [ ] **Step 1: Write the failing test**

Create `shared/ui_logic/src/transcript/theme_tokens.rs`:

```rust
//! The one roster of semantic colour roles both surfaces paint from.
//!
//! Panel maps a role to a CSS custom property name; TUI maps it to a
//! `ratatui::style::Color`. Neither side may invent a role the other cannot
//! see — that is why the enum lives here and `ALL_SEMANTIC_COLORS` exists:
//! a mapping table on either side is asserted complete against it.

/// A colour ROLE, not a colour. Values are assigned by the surface's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticColor {
    Fg,
    Dim,
    Accent,
    Prompt,
    UserBar,
    ToolPending,
    ToolRunning,
    ToolOk,
    ToolErr,
    ToolRail,
    DiffAdd,
    DiffDel,
    DiffCtx,
    DiffGutter,
    AdmonitionNote,
    AdmonitionTip,
    AdmonitionImportant,
    AdmonitionWarning,
    AdmonitionCaution,
    CodeBorder,
    Link,
    ContextOk,
    ContextWarn,
    ContextDanger,
    Cost,
    Spinner,
}

/// Every variant, in declaration order. A surface's mapping test iterates
/// this and must resolve each — a new role added here goes red on both
/// sides until it is painted.
pub const ALL_SEMANTIC_COLORS: &[SemanticColor] = &[
    SemanticColor::Fg,
    SemanticColor::Dim,
    SemanticColor::Accent,
    SemanticColor::Prompt,
    SemanticColor::UserBar,
    SemanticColor::ToolPending,
    SemanticColor::ToolRunning,
    SemanticColor::ToolOk,
    SemanticColor::ToolErr,
    SemanticColor::ToolRail,
    SemanticColor::DiffAdd,
    SemanticColor::DiffDel,
    SemanticColor::DiffCtx,
    SemanticColor::DiffGutter,
    SemanticColor::AdmonitionNote,
    SemanticColor::AdmonitionTip,
    SemanticColor::AdmonitionImportant,
    SemanticColor::AdmonitionWarning,
    SemanticColor::AdmonitionCaution,
    SemanticColor::CodeBorder,
    SemanticColor::Link,
    SemanticColor::ContextOk,
    SemanticColor::ContextWarn,
    SemanticColor::ContextDanger,
    SemanticColor::Cost,
    SemanticColor::Spinner,
];

impl SemanticColor {
    /// CSS custom property the Panel reads for this role (without `var()`).
    #[must_use]
    pub const fn css_var(self) -> &'static str {
        match self {
            Self::Fg => "--tr-fg",
            Self::Dim => "--tr-dim",
            Self::Accent => "--tr-accent",
            Self::Prompt => "--tr-prompt",
            Self::UserBar => "--tr-user-bar",
            Self::ToolPending => "--tr-tool-pending",
            Self::ToolRunning => "--tr-tool-running",
            Self::ToolOk => "--tr-tool-ok",
            Self::ToolErr => "--tr-tool-err",
            Self::ToolRail => "--tr-tool-rail",
            Self::DiffAdd => "--tr-diff-add",
            Self::DiffDel => "--tr-diff-del",
            Self::DiffCtx => "--tr-diff-ctx",
            Self::DiffGutter => "--tr-diff-gutter",
            Self::AdmonitionNote => "--tr-adm-note",
            Self::AdmonitionTip => "--tr-adm-tip",
            Self::AdmonitionImportant => "--tr-adm-important",
            Self::AdmonitionWarning => "--tr-adm-warning",
            Self::AdmonitionCaution => "--tr-adm-caution",
            Self::CodeBorder => "--tr-code-border",
            Self::Link => "--tr-link",
            Self::ContextOk => "--tr-ctx-ok",
            Self::ContextWarn => "--tr-ctx-warn",
            Self::ContextDanger => "--tr-ctx-danger",
            Self::Cost => "--tr-cost",
            Self::Spinner => "--tr-spinner",
        }
    }
}

/// Linear RGB mix: `t = 0` is `a`, `t = 1` is `b`. Used for diff row (12%)
/// and inline-emphasis (26%) backgrounds derived from the theme's own diff
/// colours, so a user theme stays coherent (pi-cc-extensions `diff-palette`).
#[must_use]
pub fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| -> u8 {
        let v = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

/// Row-background mix ratio for added/removed diff lines.
pub const DIFF_ROW_MIX: f32 = 0.12;
/// Inline-emphasis (changed word span) mix ratio.
pub const DIFF_EMPHASIS_MIX: f32 = 0.26;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_role_has_a_distinct_css_var() {
        let vars: HashSet<&str> = ALL_SEMANTIC_COLORS.iter().map(|c| c.css_var()).collect();
        assert_eq!(vars.len(), ALL_SEMANTIC_COLORS.len(), "two roles share a CSS var");
        assert!(vars.iter().all(|v| v.starts_with("--tr-")));
    }

    #[test]
    fn the_roster_is_complete() {
        // If a variant is added without being listed, this count is off and
        // both surfaces' mapping tests silently stop covering it.
        let mut seen = HashSet::new();
        for c in ALL_SEMANTIC_COLORS {
            assert!(seen.insert(*c), "duplicate in ALL_SEMANTIC_COLORS: {c:?}");
        }
        assert_eq!(seen.len(), 26);
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        assert_eq!(mix_rgb((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(mix_rgb((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(mix_rgb((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
        // Out-of-range t is clamped, never wraps.
        assert_eq!(mix_rgb((10, 10, 10), (20, 20, 20), 7.0), (20, 20, 20));
    }
}
```

Create `shared/ui_logic/src/transcript/mod.rs`:

```rust
//! Transcript presentation logic shared by the Panel (Leptos) and the TUI
//! (ratatui). Everything here is `data → data`: no rendering, no signals, no
//! terminal. Each surface paints what these functions decide, so the two
//! can never disagree about a fold, a summary, a hunk or a colour role.
//!
//! Compiles under `default-features = false` (the TUI build) — nothing in
//! this module may be feature-gated or reach for leptos / web-sys.

mod theme_tokens;

pub use theme_tokens::{
    mix_rgb, SemanticColor, ALL_SEMANTIC_COLORS, DIFF_EMPHASIS_MIX, DIFF_ROW_MIX,
};
```

Modify `shared/ui_logic/src/lib.rs` — add after `pub mod state;`:

```rust
pub mod transcript;
```

- [ ] **Step 2: Run the tests to verify they compile and pass**

Run: `cargo test -p shared-ui-logic transcript::theme_tokens`
Expected: 3 passed.

Also run: `cargo check -p shared-ui-logic --no-default-features`
Expected: clean (the TUI shape).

- [ ] **Step 3: Commit**

```bash
git add shared/ui_logic/src/lib.rs shared/ui_logic/src/transcript/
git commit -m "ui-logic: add transcript module with the shared semantic colour roster

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 2: `fold` — physical-row folding with a logical safety net

**Files:**
- Create: `shared/ui_logic/src/transcript/fold.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`, `shared/ui_logic/Cargo.toml` (add `unicode-width = "0.2"`)

**Interfaces:**
- Produces: `fold(lines: &[&str], width: u16, policy: FoldPolicy) -> Folded`; `FoldPolicy { collapsed_rows: u16, anchor: FoldAnchor, body: FoldBody }`; `FoldAnchor::{Head, Tail}`; `FoldBody::{Show, Hide}`; `Folded { rows: Vec<String>, hidden_rows: usize, hidden_lines: usize, truncated: bool }`; `wrap_physical(line: &str, width: u16) -> Vec<String>`; `DEFAULT_COLLAPSED_ROWS: u16 = 2`; `FALLBACK_WIDTH: u16 = 80`.

- [ ] **Step 1: Add the dependency**

In `shared/ui_logic/Cargo.toml` under `# Core` add:

```toml
# Same 0.2 copy alephcore/tui/webchat already pin — `cargo tree -i unicode-width`
unicode-width = "0.2"
```

- [ ] **Step 2: Write the failing tests**

Create `shared/ui_logic/src/transcript/fold.rs`:

```rust
//! Fold a tool result body to a few PHYSICAL rows.
//!
//! The rule that matters (pi-claude-code-tui `MAX_RESULT_ROWS`, the #1
//! "most likely to be got wrong" item): wrap FIRST, then count. One minified
//! JSON line is sixty terminal rows; counting logical lines would show all
//! sixty. The logical line count participates only as a safety net — if the
//! body has more logical lines than the limit it is truncated regardless of
//! how the wrap measured.

use unicode_width::UnicodeWidthChar;

/// Rows shown when collapsed (excluding the `… +N lines` hint row).
pub const DEFAULT_COLLAPSED_ROWS: u16 = 2;
/// Width assumed when the surface cannot measure yet (first frame, hidden
/// container). Unknown must still fold — never "skip the fold".
pub const FALLBACK_WIDTH: u16 = 80;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldAnchor {
    /// Keep the first rows (default).
    Head,
    /// Keep the last rows — shell output, where the error is at the end.
    Tail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldBody {
    /// Show `collapsed_rows` of body when collapsed.
    Show,
    /// Show no body when collapsed (a `file_read` shows only `Read 120 lines`).
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldPolicy {
    pub collapsed_rows: u16,
    pub anchor: FoldAnchor,
    pub body: FoldBody,
}

impl Default for FoldPolicy {
    fn default() -> Self {
        Self {
            collapsed_rows: DEFAULT_COLLAPSED_ROWS,
            anchor: FoldAnchor::Head,
            body: FoldBody::Show,
        }
    }
}

impl FoldPolicy {
    /// Per-tool overrides (spec §5 `fold` row). Keyed by the DISPLAY name
    /// produced by `summarize::display_name`, so MCP/unknown tools get the
    /// default and no name list has to be kept in two places.
    #[must_use]
    pub fn for_display_name(display_name: &str) -> Self {
        match display_name {
            "Read" => Self {
                body: FoldBody::Hide,
                ..Self::default()
            },
            "Bash" => Self {
                anchor: FoldAnchor::Tail,
                ..Self::default()
            },
            _ => Self::default(),
        }
    }
}

/// Result of a fold. `rows` are physical (already wrapped to `width`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Folded {
    pub rows: Vec<String>,
    /// Physical rows not shown. What the `… +N lines` hint reports.
    pub hidden_rows: usize,
    /// Logical lines not fully shown (for the "N lines" wording when a
    /// surface prefers logical units, e.g. `Read 120 lines`).
    pub hidden_lines: usize,
    pub truncated: bool,
}

/// Wrap one logical line into physical rows of at most `width` columns,
/// measuring with `unicode-width` (CJK = 2 columns, combining = 0).
/// Never returns an empty Vec: an empty line is one empty row.
#[must_use]
pub fn wrap_physical(line: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0);
        if cur_w + w > width && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    rows.push(cur);
    rows
}

/// Fold `lines` (logical) to the policy's collapsed height at `width`.
/// `width == 0` is treated as unknown → [`FALLBACK_WIDTH`].
#[must_use]
pub fn fold(lines: &[&str], width: u16, policy: FoldPolicy) -> Folded {
    let width = if width == 0 { FALLBACK_WIDTH } else { width };
    let limit = usize::from(policy.collapsed_rows);
    let total_lines = lines.len();

    if policy.body == FoldBody::Hide {
        let hidden_rows: usize = lines.iter().map(|l| wrap_physical(l, width).len()).sum();
        return Folded {
            rows: Vec::new(),
            hidden_rows,
            hidden_lines: total_lines,
            truncated: total_lines > 0,
        };
    }

    // Logical safety net: more logical lines than rows → truncates for sure,
    // even if a wrap measurement disagreed.
    let logical_truncates = total_lines > limit;

    // Wrap everything. Bodies are capped upstream (TUI keeps ≤ 64 KB per row
    // in memory; larger output is fetched on expand), so a full wrap is a
    // few hundred microseconds — no pre-scan shortcut is worth its edge cases.
    let mut physical: Vec<(usize, String)> = Vec::new(); // (logical index, row)
    for (i, line) in lines.iter().enumerate() {
        for row in wrap_physical(line, width) {
            physical.push((i, row));
        }
    }

    let total_rows = physical.len();
    let truncated = logical_truncates || total_rows > limit;
    if !truncated {
        return Folded {
            rows: physical.into_iter().map(|(_, r)| r).collect(),
            hidden_rows: 0,
            hidden_lines: 0,
            truncated: false,
        };
    }

    let (shown, first_shown_line, last_shown_line) = match policy.anchor {
        FoldAnchor::Head => {
            let s: Vec<&(usize, String)> = physical.iter().take(limit).collect();
            let first = s.first().map_or(0, |(i, _)| *i);
            let last = s.last().map_or(0, |(i, _)| *i);
            (s, first, last)
        }
        FoldAnchor::Tail => {
            let skip = total_rows.saturating_sub(limit);
            let s: Vec<&(usize, String)> = physical.iter().skip(skip).collect();
            let first = s.first().map_or(0, |(i, _)| *i);
            let last = s.last().map_or(0, |(i, _)| *i);
            (s, first, last)
        }
    };
    let shown_rows: Vec<String> = shown.iter().map(|(_, r)| (*r).clone()).collect();
    let hidden_rows = total_rows - shown_rows.len();
    // A logical line counts as hidden unless EVERY one of its rows is shown.
    let rows_of = |li: usize| physical.iter().filter(|(i, _)| *i == li).count();
    let shown_rows_of = |li: usize| shown.iter().filter(|(i, _)| *i == li).count();
    let fully_shown = (first_shown_line..=last_shown_line)
        .filter(|li| shown_rows_of(*li) == rows_of(*li))
        .count();
    let hidden_lines = total_lines.saturating_sub(fully_shown);

    Folded {
        rows: shown_rows,
        hidden_rows,
        hidden_lines,
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_body_is_not_folded() {
        let f = fold(&["a", "b"], 80, FoldPolicy::default());
        assert!(!f.truncated);
        assert_eq!(f.rows, vec!["a", "b"]);
        assert_eq!(f.hidden_rows, 0);
    }

    #[test]
    fn head_keeps_the_first_two_rows_and_counts_the_rest() {
        let f = fold(&["l1", "l2", "l3", "l4", "l5"], 80, FoldPolicy::default());
        assert!(f.truncated);
        assert_eq!(f.rows, vec!["l1", "l2"]);
        assert_eq!(f.hidden_rows, 3);
        assert_eq!(f.hidden_lines, 3);
    }

    #[test]
    fn tail_keeps_the_last_two_rows() {
        let p = FoldPolicy::for_display_name("Bash");
        let f = fold(&["l1", "l2", "l3", "l4", "l5"], 80, p);
        assert_eq!(f.rows, vec!["l4", "l5"]);
        assert_eq!(f.hidden_rows, 3);
    }

    #[test]
    fn one_minified_json_line_folds_by_physical_rows_not_logical_lines() {
        // The regression this module exists for: raw newline count is 0, yet
        // at width 80 this is dozens of rows and must fold.
        let blob = "x".repeat(6 * 1024);
        let f = fold(&[blob.as_str()], 80, FoldPolicy::default());
        assert!(f.truncated, "a 6 KB single line must fold");
        assert_eq!(f.rows.len(), 2);
        assert!(f.rows.iter().all(|r| r.chars().count() == 80));
        assert!(f.hidden_rows > 20, "hidden rows must follow wrap width, got {}", f.hidden_rows);
    }

    #[test]
    fn cjk_is_two_columns_wide() {
        // 41 CJK chars = 82 columns → 2 rows at width 80.
        let s = "中".repeat(41);
        let rows = wrap_physical(&s, 80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chars().count(), 40);
        assert_eq!(rows[1].chars().count(), 1);
    }

    #[test]
    fn hide_body_shows_nothing_but_still_counts() {
        let p = FoldPolicy::for_display_name("Read");
        let f = fold(&["a", "b", "c"], 80, p);
        assert!(f.rows.is_empty());
        assert_eq!(f.hidden_lines, 3);
        assert!(f.truncated);
    }

    #[test]
    fn unknown_width_falls_back_to_eighty_columns_rather_than_skipping() {
        let blob = "y".repeat(500);
        let f = fold(&[blob.as_str()], 0, FoldPolicy::default());
        assert!(f.truncated);
        assert_eq!(f.rows[0].chars().count(), usize::from(FALLBACK_WIDTH));
    }

    #[test]
    fn exactly_the_limit_is_not_truncated() {
        let f = fold(&["a", "b"], 80, FoldPolicy::default());
        assert!(!f.truncated);
    }

    #[test]
    fn a_thousand_lines_report_exact_hidden_counts() {
        let lines: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let f = fold(&refs, 80, FoldPolicy::default());
        assert_eq!(f.rows, vec!["line 0", "line 1"]);
        assert_eq!(f.hidden_rows, 998);
        assert_eq!(f.hidden_lines, 998);
    }

    #[test]
    fn a_partially_shown_line_counts_as_hidden() {
        // Line 0 wraps to 3 rows at width 4; only 2 rows fit, so line 0 is
        // partially hidden and line 1 entirely: hidden_lines must be 2, not 1.
        let f = fold(&["abcdefghij", "k"], 4, FoldPolicy::default());
        assert_eq!(f.rows, vec!["abcd", "efgh"]);
        assert_eq!(f.hidden_rows, 2);
        assert_eq!(f.hidden_lines, 2);
    }
}
```

Add to `shared/ui_logic/src/transcript/mod.rs`:

```rust
mod fold;
pub use fold::{
    fold, wrap_physical, FoldAnchor, FoldBody, FoldPolicy, Folded, DEFAULT_COLLAPSED_ROWS,
    FALLBACK_WIDTH,
};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p shared-ui-logic transcript::fold`
Expected: 10 passed. The assertions are the contract; if an arithmetic detail disagrees, fix the implementation, not the test.

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic/Cargo.toml shared/ui_logic/src/transcript/
git commit -m "ui-logic: fold tool bodies by physical rows with a logical safety net

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 3: Protocol types — `file_change.rs`, `context_breakdown.rs`, additive fields on `events.rs`

**Files:**
- Create: `shared/protocol/src/file_change.rs`, `shared/protocol/src/context_breakdown.rs`
- Modify: `shared/protocol/src/lib.rs:17-94`, `shared/protocol/src/events.rs` (`ToolResult` :737-770, `AgentTraceToolCallEnd` :380, `RunSummary` :779, `ModelInfo` :268-285)

**Interfaces:**
- Produces: `aleph_protocol::file_change::{FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Unavailable, Presentation, MAX_HUNK_LINES, CONTEXT_LINES}`; `aleph_protocol::context_breakdown::{ContextBreakdown, LayerSizeView, ToolSchemaSize, UsageTokens, ToolOutputPage, ToolOutputSource}`; `ToolResult.presentation: Option<Presentation>`; `ToolResult::with_presentation(self, Option<Presentation>) -> Self`; `AgentTraceToolCallEnd.presentation: Option<Presentation>`; `RunSummary.context_tokens: u32`, `RunSummary.context_window: u32`; `ModelInfo.context_window: Option<u32>`.

- [ ] **Step 1: Write `file_change.rs` with its tests**

```rust
//! Structured file-change presentation attached to a file-mutating tool's
//! result — the UI side-channel, never the model-facing text.
//!
//! Computed server-side by the tool that holds the pre-image (spec R-3),
//! hoisted out of the tool's JSON before the result is flattened for the
//! model, and carried on `ToolResult.presentation` (live) and
//! `AgentTraceToolCallEnd.presentation` (replay). Hunks are a bounded view;
//! the full text is behind `trace.tool_output`.

use serde::{Deserialize, Serialize};

/// Context lines kept on each side of a change (pi `edit-diff.ts`: 4).
pub const CONTEXT_LINES: usize = 4;
/// Total hunk lines carried on the wire before the diff degrades to stats
/// only (`Unavailable::TooLarge`). Bounds the `tool_end` frame.
pub const MAX_HUNK_LINES: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineTag {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkLine {
    pub tag: LineTag,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// 1-based first line of this hunk in the OLD file.
    pub old_start: u32,
    /// 1-based first line of this hunk in the NEW file.
    pub new_start: u32,
    pub lines: Vec<HunkLine>,
}

/// Why no hunks are attached. A closed set: a renderer shows the reason and
/// NEVER guesses (a missing pre-image is not "created").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    /// Either side is not valid UTF-8 text.
    Binary,
    /// The hunks exceeded [`MAX_HUNK_LINES`]; `added` / `removed` are still exact.
    TooLarge,
    /// The tool could not read the file before writing it.
    PreImageUnavailable,
    /// Text decoded but line splitting failed (e.g. lone surrogates).
    Encoding,
    /// The tool itself failed; nothing was written.
    ToolFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
}

impl FileChange {
    /// A change whose hunks could not be produced. Stats may still be known.
    #[must_use]
    pub fn unavailable(path: impl Into<String>, kind: FileChangeKind, why: Unavailable) -> Self {
        Self {
            path: path.into(),
            kind,
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            unavailable: Some(why),
        }
    }

    /// Total hunk lines (for the wire bound).
    #[must_use]
    pub fn hunk_line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// The UI side-channel a tool result can carry. One variant today; a
/// closed enum so a second kind (e.g. a table) is a deliberate addition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Presentation {
    /// One entry per file the call touched (a single edit is a Vec of one).
    FileChanges { changes: Vec<FileChange> },
}

/// The key a tool puts this under inside its own JSON output. The dispatch
/// layer hoists and removes it BEFORE the value is flattened for the model.
pub const PRESENTATION_KEY: &str = "_presentation";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Presentation {
        Presentation::FileChanges {
            changes: vec![FileChange {
                path: "src/a.rs".into(),
                kind: FileChangeKind::Modified,
                hunks: vec![Hunk {
                    old_start: 10,
                    new_start: 10,
                    lines: vec![
                        HunkLine { tag: LineTag::Ctx, text: "fn a() {".into() },
                        HunkLine { tag: LineTag::Del, text: "    1".into() },
                        HunkLine { tag: LineTag::Add, text: "    2".into() },
                    ],
                }],
                added: 1,
                removed: 1,
                unavailable: None,
            }],
        }
    }

    #[test]
    fn presentation_round_trips_and_is_tagged_by_kind() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["kind"], "file_changes");
        assert_eq!(v["changes"][0]["hunks"][0]["lines"][1]["tag"], "del");
        let back: Presentation = serde_json::from_value(v).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn an_unavailable_change_carries_no_hunks_and_says_why() {
        let c = FileChange::unavailable("x.bin", FileChangeKind::Modified, Unavailable::Binary);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("hunks").is_none(), "empty hunks are elided");
        assert_eq!(v["unavailable"], "binary");
        assert_eq!(c.hunk_line_count(), 0);
    }

    #[test]
    fn a_change_from_an_older_server_without_optional_fields_still_parses() {
        let v = serde_json::json!({
            "path": "p", "kind": "created", "added": 3, "removed": 0
        });
        let c: FileChange = serde_json::from_value(v).unwrap();
        assert!(c.hunks.is_empty());
        assert!(c.unavailable.is_none());
    }

    #[test]
    fn wire_key_names_are_locked() {
        let v = serde_json::to_value(sample()).unwrap();
        let change = &v["changes"][0];
        for k in ["path", "kind", "hunks", "added", "removed"] {
            assert!(change.get(k).is_some(), "missing wire key {k}");
        }
        for k in ["old_start", "new_start", "lines"] {
            assert!(change["hunks"][0].get(k).is_some(), "missing hunk key {k}");
        }
    }
}
```

- [ ] **Step 2: Write `context_breakdown.rs` with its tests**

```rust
//! `context.breakdown` and `trace.tool_output` response shapes.
//!
//! Defined here and CONSTRUCTED by the gateway (never a hand-written `json!`)
//! — see `trace_replay.rs` for why the direction matters.

use serde::{Deserialize, Serialize};

/// One prompt layer's measured contribution to the LAST prompt actually
/// sent for the session. Bytes are exact; `tokens` is the server's
/// content-aware estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSizeView {
    pub name: String,
    pub bytes: u64,
    pub tokens: u64,
    /// `"stable"` or `"dynamic"` (prefix-cache zone).
    pub zone: String,
}

/// Bytes of one tool's schema + description as handed to the prompt/tool
/// list (pre provider-adapter transform).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchemaSize {
    pub name: String,
    pub schema_bytes: u64,
    pub description_bytes: u64,
}

/// Provider-reported usage for the same turn, when it exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub session_key: String,
    /// Monotonic turn counter of the measured prompt.
    pub turn: u64,
    pub layers: Vec<LayerSizeView>,
    pub tools: Vec<ToolSchemaSize>,
    /// Estimated tokens of the conversation messages in the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_tokens: Option<u64>,
    /// `None` right after a compaction until a fresh response arrives — a
    /// first-class "unknown", rendered as `?`, never as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reported: Option<UsageTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

impl ContextBreakdown {
    #[must_use]
    pub fn layer_bytes(&self) -> u64 {
        self.layers.iter().map(|l| l.bytes).sum()
    }
    #[must_use]
    pub fn tool_bytes(&self) -> u64 {
        self.tools.iter().map(|t| t.schema_bytes + t.description_bytes).sum()
    }
}

/// Where a `trace.tool_output` page's bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputSource {
    /// The result fit the model budget; the event log holds the whole text.
    Inline,
    /// The result was offloaded to a blob file and that file was read.
    Persisted,
    /// The result was offloaded but the blob has been swept; only the
    /// budgeted text survives. `truncated` is true and cannot be cured.
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutputPage {
    pub tool_call_id: String,
    pub text: String,
    pub offset: u64,
    pub total_bytes: u64,
    /// True when `offset + text.len() < total_bytes` OR the source is
    /// `Expired` — either way the reader does not hold the whole output.
    pub truncated: bool,
    pub source: ToolOutputSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_sums_and_optional_fields_default() {
        let b = ContextBreakdown {
            session_key: "k".into(),
            turn: 3,
            layers: vec![
                LayerSizeView { name: "identity".into(), bytes: 10, tokens: 3, zone: "stable".into() },
                LayerSizeView { name: "tools".into(), bytes: 5, tokens: 2, zone: "stable".into() },
            ],
            tools: vec![ToolSchemaSize { name: "grep".into(), schema_bytes: 100, description_bytes: 20 }],
            messages_tokens: None,
            provider_reported: None,
            context_window: None,
        };
        assert_eq!(b.layer_bytes(), 15);
        assert_eq!(b.tool_bytes(), 120);
        let v = serde_json::to_value(&b).unwrap();
        assert!(v.get("provider_reported").is_none(), "None is elided, not 0");
        let back: ContextBreakdown = serde_json::from_value(v).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn tool_output_source_is_snake_case_on_the_wire() {
        let p = ToolOutputPage {
            tool_call_id: "c1".into(),
            text: "abc".into(),
            offset: 0,
            total_bytes: 3,
            truncated: false,
            source: ToolOutputSource::Persisted,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["source"], "persisted");
        for k in ["tool_call_id", "text", "offset", "total_bytes", "truncated", "source"] {
            assert!(v.get(k).is_some(), "missing key {k}");
        }
    }
}
```

- [ ] **Step 3: Register both modules and re-exports in `lib.rs`**

Add `pub mod context_breakdown;` after `pub mod commands;` and `pub mod file_change;` after `pub mod extension_usage;` (alphabetical). Add re-exports after the `events` re-export block:

```rust
pub use context_breakdown::{
    ContextBreakdown, LayerSizeView, ToolOutputPage, ToolOutputSource, ToolSchemaSize, UsageTokens,
};
pub use file_change::{
    FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Presentation, Unavailable,
    PRESENTATION_KEY,
};
```

- [ ] **Step 4: Additive fields on `events.rs`**

`ToolResult` (`events.rs:737-770`) — CUT the dead `metadata: Option<Value>` (zero readers and zero writers on both copies and all three clients — dossier-wire §11.4; it is `skip_serializing_if`, so no wire byte changes), add the typed field and a builder; keep the two constructors:

```rust
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    /// UI side-channel (structured diff etc.). Never part of the model text.
    /// `default` so an older server's frame still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<crate::file_change::Presentation>,
}

impl ToolResult {
    // ... existing success()/error() lose `metadata: None` and gain `presentation: None`
    #[must_use]
    pub fn with_presentation(mut self, p: Option<crate::file_change::Presentation>) -> Self {
        self.presentation = p;
        self
    }
}
```

The gateway twin at `src/gateway/event_emitter/types.rs:410-437` is replaced by a re-export in Task 15; until then it keeps compiling on its own (it is a separate struct). Any `Value` import that becomes unused in `events.rs` — remove it.

`AgentTraceToolCallEnd` (`events.rs:380`) — add:

```rust
    /// Same side-channel as `ToolResult.presentation`, so `trace.by_runs`
    /// replays the diff a live `tool_end` carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<crate::file_change::Presentation>,
```

(`PartialEq, Eq` derive stays valid — `Presentation` derives both.)

`RunSummary` (`events.rs:779`) — add after `token_breakdown`, matching the gateway twin at `src/gateway/event_emitter/types.rs:449` field-for-field:

```rust
    /// Context-window occupancy after the latest turn (gauge numerator).
    #[serde(default)]
    pub context_tokens: u32,
    /// Per-model context window (gauge denominator). 0 = unknown.
    #[serde(default)]
    pub context_window: u32,
```

`ModelInfo` (`events.rs:268-285`) — add:

```rust
    /// Per-model context window, when the server resolved one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
```

Add a test to `events.rs`'s existing test module:

```rust
    #[test]
    fn run_summary_carries_the_gauge_fields_the_gateway_twin_sends() {
        // Wire-compat: the gateway struct serializes these two; before this
        // field existed the protocol copy silently dropped them.
        let v = serde_json::json!({
            "total_tokens": 1, "tool_calls": 0, "loops": 1,
            "context_tokens": 12_345, "context_window": 200_000
        });
        let s: RunSummary = serde_json::from_value(v).unwrap();
        assert_eq!(s.context_tokens, 12_345);
        assert_eq!(s.context_window, 200_000);
        let legacy: RunSummary = serde_json::from_value(
            serde_json::json!({"total_tokens": 1, "tool_calls": 0, "loops": 1})
        ).unwrap();
        assert_eq!(legacy.context_window, 0);
    }

    #[test]
    fn tool_result_presentation_is_optional_and_elided_when_none() {
        let r = ToolResult::success("ok");
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("presentation").is_none());
        let legacy: ToolResult = serde_json::from_value(
            serde_json::json!({"success": true, "output": "x", "error": null})
        ).unwrap();
        assert!(legacy.presentation.is_none());
    }
```

- [ ] **Step 5: Run the protocol tests**

Run: `cargo test -p aleph-protocol`
Expected: previous 350 + 8 new, all green. Then `cargo check -p alephcore` (the gateway constructs `ToolResult { .. }` literals? grep `ToolResult {` in `src/gateway` — the dossier says both constructors are the only builders; if a struct literal exists, add `presentation: None`).

- [ ] **Step 6: Commit**

```bash
git add shared/protocol/src/
git commit -m "protocol: add FileChange presentation side-channel and context breakdown shapes

ToolResult/AgentTraceToolCallEnd gain an optional typed presentation;
RunSummary gains the gauge fields the gateway twin already sent;
ModelInfo gains context_window. All additive, serde(default).

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 4: `summarize` — display names and the one-line argument summary

**Files:**
- Create: `shared/ui_logic/src/transcript/summarize.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Produces: `display_name(tool: &str) -> String`; `summarize(tool: &str, args: &serde_json::Value) -> CallSummary { display_name: String, args_text: String }`; `humanize(name: &str) -> String`; `PREFERRED_ARG_KEYS: &[&str]`; `DISPLAY_NAMES: &[(&str, &str)]`; `ARGS_CLIP: usize = 100`; `clip_one_line(s: &str, max: usize) -> String`.
- Consumers: Task 5 `ToolRow::new`, `FoldPolicy::for_display_name`, the alephcore census in Task 14.

- [ ] **Step 1: Write the failing tests + implementation**

Create `shared/ui_logic/src/transcript/summarize.rs`:

```rust
//! `⏺ Read(src/x.rs:1-120)` — the tool row headline.
//!
//! Two decisions live here and nowhere else: what a tool is CALLED on the
//! row (Claude Code vocabulary for Aleph's snake_case names, humanised
//! Title Case for everything else, `Server · Tool` for MCP), and which
//! argument is worth one line (a per-tool rule, then a preferred-key
//! fallback in a fixed order — pi-cc-extensions `names.ts`).

use serde_json::Value;

/// Max columns of the argument text (pi `inputClip` default).
pub const ARGS_CLIP: usize = 100;

/// Aleph tool name → row label. Anything not listed is humanised.
/// The alephcore census (`presentation_census.rs`) asserts every registered
/// builtin either appears here or in its explicit fallback list.
pub const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("file_read", "Read"),
    ("file_edit", "Edit"),
    ("file_write", "Write"),
    ("apply_patch", "Patch"),
    ("file_ops", "Files"),
    ("shell_exec", "Bash"),
    ("grep", "Grep"),
    ("find", "Find"),
    ("web_fetch", "Fetch"),
    ("web_search", "Search"),
    ("scratchpad", "Plan"),
    ("tool_search", "Tools"),
    ("terminal", "Terminal"),
    ("memory_search", "Memory"),
    ("note_manage", "Note"),
    ("subagent", "Agent"),
    ("ctx_search", "Context"),
];

/// Fallback argument keys, first present wins. Objects/arrays are skipped.
pub const PREFERRED_ARG_KEYS: &[&str] = &[
    "path", "file_path", "command", "query", "question", "pattern", "url", "name", "id",
    "action", "message",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSummary {
    pub display_name: String,
    pub args_text: String,
}

/// `snake_case` / `camelCase` / `kebab-case` → `Title Case`.
#[must_use]
pub fn humanize(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            spaced.push(' ');
            continue;
        }
        if c.is_ascii_uppercase() && i > 0 && chars[i - 1].is_ascii_alphanumeric() && !chars[i - 1].is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(c);
    }
    spaced
        .split_whitespace()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// MCP tools arrive as `server__tool` (or `mcp__server__tool`); render
/// `Server · Tool`. Returns `None` when the name is not MCP-shaped.
fn mcp_display(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__").unwrap_or(name);
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(format!("{} · {}", humanize(server), humanize(tool)))
}

#[must_use]
pub fn display_name(tool: &str) -> String {
    if let Some((_, label)) = DISPLAY_NAMES.iter().find(|(n, _)| *n == tool) {
        return (*label).to_string();
    }
    if let Some(mcp) = mcp_display(tool) {
        return mcp;
    }
    humanize(tool)
}

/// First line only, clipped to `max` chars with an ellipsis.
#[must_use]
pub fn clip_one_line(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    let n = first.chars().count();
    if n <= max {
        return first.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = first.chars().take(keep).collect();
    out.push('…');
    out
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn read_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_u64);
    let limit = args.get("limit").and_then(Value::as_u64);
    match (offset, limit) {
        (Some(o), Some(l)) => Some(format!("{}-{}", o.max(1), o.max(1) + l.saturating_sub(1))),
        (Some(o), None) => Some(format!("{}-", o.max(1))),
        (None, Some(l)) => Some(format!("1-{l}")),
        (None, None) => None,
    }
}

fn shorten_path(p: &str) -> String {
    // Keep it as given; surfaces may relativise against cwd. Only collapse
    // a Windows drive-absolute or POSIX home prefix if very long.
    if p.chars().count() <= ARGS_CLIP {
        return p.to_string();
    }
    let tail: String = p.chars().rev().take(ARGS_CLIP - 1).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

/// The one-line argument summary for a tool call.
#[must_use]
pub fn summarize(tool: &str, args: &Value) -> CallSummary {
    let display = display_name(tool);
    let text = match tool {
        "file_read" => str_arg(args, "path")
            .or_else(|| str_arg(args, "file_path"))
            .map(|p| match read_range(args) {
                Some(r) => format!("{}:{}", shorten_path(p), r),
                None => shorten_path(p),
            }),
        "file_edit" | "file_write" => str_arg(args, "file_path")
            .or_else(|| str_arg(args, "path"))
            .map(shorten_path),
        "apply_patch" => str_arg(args, "patch").map(|patch| {
            let n = patch.lines().filter(|l| {
                l.starts_with("*** Add File:") || l.starts_with("*** Update File:") || l.starts_with("*** Delete File:")
            }).count();
            format!("{n} file{}", if n == 1 { "" } else { "s" })
        }),
        "grep" | "find" => str_arg(args, "pattern").map(|pat| match str_arg(args, "path") {
            Some(p) => format!("{pat} in {}", shorten_path(p)),
            None => pat.to_string(),
        }),
        "shell_exec" => str_arg(args, "command").map(|c| clip_one_line(c, 80)),
        "subagent" => {
            let who = str_arg(args, "agent").or_else(|| str_arg(args, "name")).unwrap_or("agent");
            let task = str_arg(args, "task").or_else(|| str_arg(args, "prompt")).map(|t| clip_one_line(t, 40));
            Some(match task {
                Some(t) => format!("{who}: {t}"),
                None => who.to_string(),
            })
        }
        _ => PREFERRED_ARG_KEYS.iter().find_map(|k| str_arg(args, k)).map(str::to_string),
    };
    CallSummary {
        display_name: display,
        args_text: clip_one_line(text.as_deref().unwrap_or(""), ARGS_CLIP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_tools_get_claude_code_names() {
        assert_eq!(display_name("file_read"), "Read");
        assert_eq!(display_name("shell_exec"), "Bash");
        assert_eq!(display_name("apply_patch"), "Patch");
    }

    #[test]
    fn unknown_tools_are_humanised_and_mcp_tools_show_server_and_tool() {
        assert_eq!(humanize("get_subagent_result"), "Get Subagent Result");
        assert_eq!(humanize("customTranslate"), "Custom Translate");
        assert_eq!(display_name("mcp__github__search_issues"), "Github · Search Issues");
        assert_eq!(display_name("github__search"), "Github · Search");
        assert_eq!(display_name("weird_tool"), "Weird Tool");
    }

    #[test]
    fn read_shows_path_and_range() {
        let s = summarize("file_read", &json!({"path": "src/a.rs", "offset": 10, "limit": 50}));
        assert_eq!(s.args_text, "src/a.rs:10-59");
        let s = summarize("file_read", &json!({"path": "src/a.rs"}));
        assert_eq!(s.args_text, "src/a.rs");
    }

    #[test]
    fn grep_shows_pattern_in_path_and_bash_shows_the_first_command_line() {
        let s = summarize("grep", &json!({"pattern": "fold|expand", "path": "src"}));
        assert_eq!(s.args_text, "fold|expand in src");
        let s = summarize("shell_exec", &json!({"command": "cargo test -p x\necho done"}));
        assert_eq!(s.args_text, "cargo test -p x");
    }

    #[test]
    fn patch_counts_files_and_subagent_names_agent_and_task() {
        let patch = "*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** Add File: b.rs\n+z\n*** End Patch";
        assert_eq!(summarize("apply_patch", &json!({"patch": patch})).args_text, "2 files");
        let s = summarize("subagent", &json!({"agent": "reviewer", "task": "Review the auth module for injection risks and more"}));
        assert_eq!(s.args_text, "reviewer: Review the auth module for injection ri…");
    }

    #[test]
    fn fallback_walks_preferred_keys_and_skips_objects() {
        let s = summarize("some_tool", &json!({"opts": {"path": "no"}, "url": "https://x.y/z"}));
        assert_eq!(s.args_text, "https://x.y/z");
        let s = summarize("some_tool", &json!({"text": "hi"}));
        assert_eq!(s.args_text, "", "a key outside the preferred list yields no argument");
    }

    #[test]
    fn clip_is_one_line_and_ellipsised() {
        assert_eq!(clip_one_line("a\nb", 10), "a");
        let long = "x".repeat(120);
        let c = clip_one_line(&long, ARGS_CLIP);
        assert_eq!(c.chars().count(), ARGS_CLIP);
        assert!(c.ends_with('…'));
    }
}
```

Add to `transcript/mod.rs`:

```rust
mod summarize;
pub use summarize::{
    clip_one_line, display_name, humanize, summarize, CallSummary, ARGS_CLIP, DISPLAY_NAMES,
    PREFERRED_ARG_KEYS,
};
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p shared-ui-logic transcript::summarize`
Expected: 7 passed.

- [ ] **Step 3: Verify the tool names against the live registry (do not guess)**

Run: `grep -rhoE 'const NAME: &'"'"'static str = "[a-z_]+"' /d/Workspace/Aleph/.claude/worktrees/cc-render-r1/src/builtin_tools | sort -u | head -80`
Compare every name in `DISPLAY_NAMES` against this list. Fix any that differ (the shell tool may be `shell_exec` or `shell`; the plan tool is `scratchpad` per `shared/protocol/src/plan.rs`). A name in `DISPLAY_NAMES` that is not registered is a lie the Task 14 census will catch — fix it here first.

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic/src/transcript/
git commit -m "ui-logic: tool row display names and one-line argument summaries

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 5: `view_model` + `group` — transcript entries, tool rows, resumed-state settling, read-only grouping

**Files:**
- Create: `shared/ui_logic/src/transcript/view_model.rs`, `shared/ui_logic/src/transcript/group.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Produces: `TranscriptEntry` enum; `ToolRow { id, tool, summary: CallSummary, status: RowStatus, body: RowBody, expanded: bool, started_ms: Option<u64>, ended_ms: Option<u64> }`; `RowStatus::{Pending, Running{since_ms}, Ok{duration_ms}, Err{duration_ms, message}}`; `RowBody::{None, Text(String), FileChanges(Vec<FileChange>)}`; `ToolRow::new(id, tool, args) -> ToolRow`; `ToolRow::start(&mut self, now_ms)`; `ToolRow::finish(&mut self, result: &aleph_protocol::ToolResult, duration_ms)`; `ToolRow::settle_resumed(&mut self)`; `ToolRow::is_read_only(&self) -> bool`; `group_entries(entries: Vec<TranscriptEntry>) -> Vec<TranscriptEntry>`; `ToolGroup { rows: Vec<ToolRow>, expanded: bool }`; `ToolGroup::headline(&self) -> String`; `READ_ONLY_DISPLAY_NAMES`.

- [ ] **Step 1: Write `view_model.rs`**

```rust
//! The transcript as data. Entries are chronological; a tool row is its own
//! entry (Claude Code interleaves tools with text), so a surface that used
//! to draw "all tools above the turn's text" now just paints the list.

use aleph_protocol::file_change::{FileChange, Presentation};
use aleph_protocol::ToolResult;

use super::summarize::{summarize, CallSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowStatus {
    /// Requested, not yet started (or restored from a log with no start
    /// event and no result — NOT spinning, see `settle_resumed`).
    Pending,
    Running { since_ms: u64 },
    Ok { duration_ms: u64 },
    Err { duration_ms: u64, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowBody {
    None,
    Text(String),
    FileChanges(Vec<FileChange>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub id: String,
    pub tool: String,
    pub summary: CallSummary,
    pub status: RowStatus,
    pub body: RowBody,
    pub expanded: bool,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

/// Display names whose calls are read-only (they group; Edit/Write/Bash never do).
pub const READ_ONLY_DISPLAY_NAMES: &[&str] = &["Read", "Grep", "Find", "Fetch", "Search", "Files", "Memory", "Context", "Tools"];

impl ToolRow {
    #[must_use]
    pub fn new(id: impl Into<String>, tool: impl Into<String>, args: &serde_json::Value) -> Self {
        let tool = tool.into();
        Self {
            summary: summarize(&tool, args),
            id: id.into(),
            tool,
            status: RowStatus::Pending,
            body: RowBody::None,
            expanded: false,
            started_ms: None,
            ended_ms: None,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.started_ms = Some(now_ms);
        self.status = RowStatus::Running { since_ms: now_ms };
    }

    /// Apply a wire `ToolResult`. A presentation wins over text for the body.
    pub fn finish(&mut self, result: &ToolResult, duration_ms: u64, now_ms: u64) {
        self.ended_ms = Some(now_ms);
        self.body = match &result.presentation {
            Some(Presentation::FileChanges { changes }) => RowBody::FileChanges(changes.clone()),
            None => match result.output.as_deref().filter(|s| !s.is_empty()) {
                Some(text) => RowBody::Text(text.to_string()),
                None => RowBody::None,
            },
        };
        self.status = if result.success {
            RowStatus::Ok { duration_ms }
        } else {
            RowStatus::Err {
                duration_ms,
                message: result.error.clone().unwrap_or_else(|| "failed".into()),
            }
        };
    }

    /// A row restored from a log arrives without a start event. If it has a
    /// result it is settled; if it has none it stays `Pending` — never a
    /// spinner that turns forever (pi `resolveToolVisualState`).
    pub fn settle_resumed(&mut self) {
        if let RowStatus::Running { .. } = self.status {
            if self.ended_ms.is_some() {
                self.status = RowStatus::Ok { duration_ms: 0 };
            } else {
                self.status = RowStatus::Pending;
            }
        }
    }

    #[must_use]
    pub fn is_read_only(&self) -> bool {
        READ_ONLY_DISPLAY_NAMES.contains(&self.summary.display_name.as_str())
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self.status, RowStatus::Ok { .. } | RowStatus::Err { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGroup {
    pub rows: Vec<ToolRow>,
    pub expanded: bool,
}

impl ToolGroup {
    /// `Explored 4 files · 0.8s` — files = distinct paths in the rows' arg
    /// text (for Read/Grep/Find), otherwise the row count.
    #[must_use]
    pub fn headline(&self) -> String {
        let n = self.rows.len();
        let total_ms: u64 = self.rows.iter().map(|r| match &r.status {
            RowStatus::Ok { duration_ms } | RowStatus::Err { duration_ms, .. } => *duration_ms,
            _ => 0,
        }).sum();
        let noun = if n == 1 { "call" } else { "calls" };
        let dur = if total_ms > 0 { format!(" · {}", super::affordance::fmt_duration_ms(total_ms)) } else { String::new() };
        format!("Explored {n} {noun}{dur}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummaryEntry {
    pub commands: u32,
    pub reads: u32,
    pub edits: u32,
    pub writes: u32,
    pub others: u32,
    pub failed: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    UserText { id: String, text: String },
    AssistantText { id: String, markdown: String, streaming: bool },
    Reasoning { id: String, text: String, collapsed: bool },
    Tool(ToolRow),
    ToolGroup(ToolGroup),
    TurnSummary(TurnSummaryEntry),
    SystemNotice { id: String, text: String },
}
```

- [ ] **Step 2: Write `group.rs`**

```rust
//! Merge consecutive read-only tool rows into one `Explored N calls` group.
//!
//! Rules (pi-cc-extensions `grouping.ts`, adjusted for Aleph names): only
//! read-only rows group; Edit/Write/Bash never do and always break a run;
//! up to `MAX_GAP_TEXT` empty/whitespace assistant texts between two
//! read-only rows are tolerated; a group of one dissolves back to a row.

use super::view_model::{ToolGroup, TranscriptEntry};

/// Empty assistant texts tolerated inside a run before it breaks.
pub const MAX_GAP_TEXT: usize = 3;
/// Rows needed for a group to exist.
pub const MIN_GROUP: usize = 2;

fn is_blank_text(e: &TranscriptEntry) -> bool {
    matches!(e, TranscriptEntry::AssistantText { markdown, .. } if markdown.trim().is_empty())
}

#[must_use]
pub fn group_entries(entries: Vec<TranscriptEntry>) -> Vec<TranscriptEntry> {
    let mut out: Vec<TranscriptEntry> = Vec::with_capacity(entries.len());
    let mut run: Vec<super::view_model::ToolRow> = Vec::new();
    let mut gap: Vec<TranscriptEntry> = Vec::new();

    let flush = |run: &mut Vec<super::view_model::ToolRow>, gap: &mut Vec<TranscriptEntry>, out: &mut Vec<TranscriptEntry>| {
        if run.len() >= MIN_GROUP {
            out.push(TranscriptEntry::ToolGroup(ToolGroup { rows: std::mem::take(run), expanded: false }));
        } else {
            for r in run.drain(..) {
                out.push(TranscriptEntry::Tool(r));
            }
        }
        out.append(gap);
    };

    for e in entries {
        match e {
            TranscriptEntry::Tool(row) if row.is_read_only() => {
                // Gap texts between two read-only rows are swallowed into the
                // group's position (they were blank anyway).
                gap.clear();
                run.push(row);
            }
            other if !run.is_empty() && is_blank_text(&other) && gap.len() < MAX_GAP_TEXT => {
                gap.push(other);
            }
            other => {
                flush(&mut run, &mut gap, &mut out);
                out.push(other);
            }
        }
    }
    flush(&mut run, &mut gap, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::super::view_model::{RowStatus, ToolRow};
    use super::*;
    use serde_json::json;

    fn read(id: &str) -> TranscriptEntry {
        let mut r = ToolRow::new(id, "file_read", &json!({"path": format!("{id}.rs")}));
        r.status = RowStatus::Ok { duration_ms: 100 };
        TranscriptEntry::Tool(r)
    }
    fn edit(id: &str) -> TranscriptEntry {
        TranscriptEntry::Tool(ToolRow::new(id, "file_edit", &json!({"file_path": "x.rs"})))
    }
    fn text(s: &str) -> TranscriptEntry {
        TranscriptEntry::AssistantText { id: "t".into(), markdown: s.into(), streaming: false }
    }

    #[test]
    fn consecutive_reads_group_and_edits_break_the_run() {
        let out = group_entries(vec![read("a"), read("b"), edit("c"), read("d")]);
        assert!(matches!(&out[0], TranscriptEntry::ToolGroup(g) if g.rows.len() == 2));
        assert!(matches!(&out[1], TranscriptEntry::Tool(r) if r.tool == "file_edit"));
        // A lone read after the break dissolves back to a plain row.
        assert!(matches!(&out[2], TranscriptEntry::Tool(r) if r.id == "d"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn blank_texts_inside_a_run_are_tolerated_but_real_text_breaks_it() {
        let out = group_entries(vec![read("a"), text("  "), read("b"), text("Found it."), read("c")]);
        assert!(matches!(&out[0], TranscriptEntry::ToolGroup(g) if g.rows.len() == 2));
        assert!(matches!(&out[1], TranscriptEntry::AssistantText { markdown, .. } if markdown == "Found it."));
        assert!(matches!(&out[2], TranscriptEntry::Tool(_)));
    }

    #[test]
    fn a_single_read_never_becomes_a_group() {
        let out = group_entries(vec![read("a"), text("x")]);
        assert!(matches!(&out[0], TranscriptEntry::Tool(_)));
    }

    #[test]
    fn group_headline_counts_calls_and_sums_duration() {
        let out = group_entries(vec![read("a"), read("b")]);
        let TranscriptEntry::ToolGroup(g) = &out[0] else { panic!("group") };
        assert_eq!(g.headline(), "Explored 2 calls · 0.2s");
    }
}
```

Also add to `view_model.rs` a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finish_prefers_a_presentation_body_over_text() {
        let mut r = ToolRow::new("c1", "file_edit", &json!({"file_path": "a.rs"}));
        r.start(1000);
        let res = ToolResult::success("Replaced 1 occurrence").with_presentation(Some(
            Presentation::FileChanges { changes: vec![FileChange::unavailable("a.rs", aleph_protocol::file_change::FileChangeKind::Modified, aleph_protocol::file_change::Unavailable::TooLarge)] },
        ));
        r.finish(&res, 42, 1042);
        assert!(matches!(r.body, RowBody::FileChanges(ref c) if c.len() == 1));
        assert_eq!(r.status, RowStatus::Ok { duration_ms: 42 });
    }

    #[test]
    fn a_resumed_row_without_a_result_is_pending_not_spinning() {
        let mut r = ToolRow::new("c1", "grep", &json!({"pattern": "x"}));
        r.status = RowStatus::Running { since_ms: 5 }; // what a naive replay would set
        r.settle_resumed();
        assert_eq!(r.status, RowStatus::Pending);
    }

    #[test]
    fn an_error_result_carries_its_message() {
        let mut r = ToolRow::new("c1", "shell_exec", &json!({"command": "false"}));
        r.finish(&ToolResult::error("exit 1"), 7, 7);
        assert!(matches!(r.status, RowStatus::Err { ref message, .. } if message == "exit 1"));
        assert!(r.is_terminal());
    }
}
```

Add to `transcript/mod.rs`:

```rust
mod group;
mod view_model;
pub use group::{group_entries, MAX_GAP_TEXT, MIN_GROUP};
pub use view_model::{
    RowBody, RowStatus, ToolGroup, ToolRow, TranscriptEntry, TurnSummaryEntry,
    READ_ONLY_DISPLAY_NAMES,
};
```

`ToolGroup::headline` calls `super::affordance::fmt_duration_ms`, written in Task 8. Until then add a private stub in `view_model.rs`:

```rust
// TEMP until Task 8 lands `affordance::fmt_duration_ms`; Task 8 deletes this.
mod affordance_stub { pub fn fmt_duration_ms(ms: u64) -> String { format!("{:.1}s", ms as f64 / 1000.0) } }
```

and call `affordance_stub::fmt_duration_ms` — Task 8 swaps the path and removes the stub.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p shared-ui-logic transcript::`
Expected: all green (theme 3 + fold 10 + summarize 7 + view_model 3 + group 4).

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic/src/transcript/
git commit -m "ui-logic: transcript view model with chronological tool rows and read-only grouping

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 6: `diff_view` — hunks to gutter rows with word-level emphasis

**Files:**
- Create: `shared/ui_logic/src/transcript/diff_view.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Consumes: `aleph_protocol::file_change::{FileChange, Hunk, HunkLine, LineTag}` (Task 3).
- Produces: `diff_rows(change: &FileChange, expanded: bool) -> DiffRows { rows: Vec<DiffRow>, hidden_rows: usize, hidden_hunks: usize }`; `DiffRow { old_no: Option<u32>, new_no: Option<u32>, tag: LineTag, spans: Vec<Span> }`; `Span { text: String, emphasis: bool }`; `word_spans(del: &str, add: &str) -> (Vec<Span>, Vec<Span>)`; `COLLAPSED_DIFF_ROWS: usize = 2`; `EXPANDED_DIFF_ROWS: usize = 400`; `LCS_CELL_BUDGET_COLLAPSED: usize = 200_000`; `LCS_CELL_BUDGET_EXPANDED: usize = 1_000_000`; `MAX_INLINE_LINE_CHARS: usize = 700`; `stats_label(change: &FileChange) -> String` (`+12 -3` or `diff unavailable: binary`).

- [ ] **Step 1: Write the implementation + tests**

```rust
//! From a wire `FileChange` to paintable rows. Unified layout only (spec
//! R-3 / §5): every row has an old/new line number, a tag, and spans where
//! `emphasis` marks the changed words of a paired Del/Add line (a second
//! LCS over tokens — pi-cc-extensions `diff-inline.ts`), budgeted so a
//! minified file never costs a quadratic table.

use aleph_protocol::file_change::{FileChange, Hunk, LineTag, Unavailable};

pub const COLLAPSED_DIFF_ROWS: usize = 2;
pub const EXPANDED_DIFF_ROWS: usize = 400;
pub const LCS_CELL_BUDGET_COLLAPSED: usize = 200_000;
pub const LCS_CELL_BUDGET_EXPANDED: usize = 1_000_000;
/// Lines longer than this get no inline spans (guards minified files).
pub const MAX_INLINE_LINE_CHARS: usize = 700;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub emphasis: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub tag: LineTag,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffRows {
    pub rows: Vec<DiffRow>,
    pub hidden_rows: usize,
    pub hidden_hunks: usize,
}

/// `+12 -3`, or the unavailable reason when there are no hunks to show.
#[must_use]
pub fn stats_label(change: &FileChange) -> String {
    match change.unavailable {
        Some(Unavailable::TooLarge) | None => format!("+{} -{}", change.added, change.removed),
        Some(Unavailable::Binary) => "diff unavailable: binary".into(),
        Some(Unavailable::PreImageUnavailable) => "diff unavailable: previous content unreadable".into(),
        Some(Unavailable::Encoding) => "diff unavailable: encoding".into(),
        Some(Unavailable::ToolFailed) => "diff unavailable: tool failed".into(),
    }
}

/// Tokens: identifier runs, whitespace runs, single punctuation chars.
fn tokenize(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        let class = if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
        let start = i;
        i += c.len_utf8();
        if class != 2 {
            while i < s.len() {
                let d = s[i..].chars().next().unwrap();
                let dc = if d.is_alphanumeric() || d == '_' { 0 } else if d.is_whitespace() { 1 } else { 2 };
                if dc != class { break; }
                i += d.len_utf8();
            }
        }
        let _ = bytes;
        out.push(&s[start..i]);
    }
    out
}

/// LCS over tokens; returns per-side `changed` flags. `None` when over budget.
fn token_lcs_changed(a: &[&str], b: &[&str], cell_budget: usize) -> Option<(Vec<bool>, Vec<bool>)> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > cell_budget {
        return None;
    }
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut ca = vec![true; n];
    let mut cb = vec![true; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ca[i] = false;
            cb[j] = false;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some((ca, cb))
}

fn spans_from(tokens: &[&str], changed: &[bool]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (t, &c) in tokens.iter().zip(changed) {
        // Whitespace never carries emphasis on its own (pi trims spans).
        let c = c && !t.trim().is_empty();
        match out.last_mut() {
            Some(last) if last.emphasis == c => last.text.push_str(t),
            _ => out.push(Span { text: (*t).to_string(), emphasis: c }),
        }
    }
    out
}

fn plain(text: &str) -> Vec<Span> {
    vec![Span { text: text.to_string(), emphasis: false }]
}

/// Word-level spans for a paired removed/added line. Falls back to plain
/// spans when either line is too long or the token table is over budget.
#[must_use]
pub fn word_spans(del: &str, add: &str, cell_budget: usize) -> (Vec<Span>, Vec<Span>) {
    if del == add || del.chars().count() > MAX_INLINE_LINE_CHARS || add.chars().count() > MAX_INLINE_LINE_CHARS {
        return (plain(del), plain(add));
    }
    let (ta, tb) = (tokenize(del), tokenize(add));
    match token_lcs_changed(&ta, &tb, cell_budget) {
        Some((ca, cb)) => (spans_from(&ta, &ca), spans_from(&tb, &cb)),
        None => (plain(del), plain(add)),
    }
}

fn hunk_rows(h: &Hunk, budget: usize, out: &mut Vec<DiffRow>) {
    let (mut old_no, mut new_no) = (h.old_start, h.new_start);
    let mut i = 0;
    while i < h.lines.len() {
        let l = &h.lines[i];
        match l.tag {
            LineTag::Ctx => {
                out.push(DiffRow { old_no: Some(old_no), new_no: Some(new_no), tag: LineTag::Ctx, spans: plain(&l.text) });
                old_no += 1;
                new_no += 1;
                i += 1;
            }
            LineTag::Del => {
                // Pair a Del run with the Add run that immediately follows it.
                let del_start = i;
                while i < h.lines.len() && h.lines[i].tag == LineTag::Del { i += 1; }
                let add_start = i;
                while i < h.lines.len() && h.lines[i].tag == LineTag::Add { i += 1; }
                let dels = &h.lines[del_start..add_start];
                let adds = &h.lines[add_start..i];
                let pairs = dels.len().max(adds.len());
                let mut pending_adds: Vec<Vec<Span>> = Vec::with_capacity(adds.len());
                for k in 0..pairs {
                    match (dels.get(k), adds.get(k)) {
                        (Some(d), Some(a)) => {
                            let (ds, as_) = word_spans(&d.text, &a.text, budget);
                            out.push(DiffRow { old_no: Some(old_no), new_no: None, tag: LineTag::Del, spans: ds });
                            old_no += 1;
                            pending_adds.push(as_);
                        }
                        (Some(d), None) => {
                            out.push(DiffRow { old_no: Some(old_no), new_no: None, tag: LineTag::Del, spans: plain(&d.text) });
                            old_no += 1;
                        }
                        (None, Some(a)) => pending_adds.push(plain(&a.text)),
                        (None, None) => {}
                    }
                }
                for spans in pending_adds {
                    out.push(DiffRow { old_no: None, new_no: Some(new_no), tag: LineTag::Add, spans });
                    new_no += 1;
                }
            }
            LineTag::Add => {
                out.push(DiffRow { old_no: None, new_no: Some(new_no), tag: LineTag::Add, spans: plain(&l.text) });
                new_no += 1;
                i += 1;
            }
        }
    }
}

/// All rows (expanded) or the first `COLLAPSED_DIFF_ROWS` (collapsed), with
/// exact hidden counts so the hint can say `… +N rows · M hunks`.
#[must_use]
pub fn diff_rows(change: &FileChange, expanded: bool) -> DiffRows {
    let budget = if expanded { LCS_CELL_BUDGET_EXPANDED } else { LCS_CELL_BUDGET_COLLAPSED };
    let limit = if expanded { EXPANDED_DIFF_ROWS } else { COLLAPSED_DIFF_ROWS };
    let mut all: Vec<DiffRow> = Vec::new();
    let mut hunk_ends: Vec<usize> = Vec::with_capacity(change.hunks.len());
    for h in &change.hunks {
        hunk_rows(h, budget, &mut all);
        hunk_ends.push(all.len());
    }
    let total = all.len();
    if total <= limit {
        return DiffRows { rows: all, hidden_rows: 0, hidden_hunks: 0 };
    }
    let shown_hunks = hunk_ends.iter().filter(|&&end| end <= limit).count()
        + usize::from(hunk_ends.iter().any(|&end| end > limit) && limit > 0);
    all.truncate(limit);
    DiffRows {
        rows: all,
        hidden_rows: total - limit,
        hidden_hunks: change.hunks.len().saturating_sub(shown_hunks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::file_change::{FileChangeKind, HunkLine};

    fn line(tag: LineTag, s: &str) -> HunkLine { HunkLine { tag, text: s.into() } }

    fn change(hunks: Vec<Hunk>) -> FileChange {
        FileChange { path: "a.rs".into(), kind: FileChangeKind::Modified, hunks, added: 0, removed: 0, unavailable: None }
    }

    #[test]
    fn paired_del_add_get_word_emphasis_only_on_the_changed_tokens() {
        let (d, a) = word_spans("let x = foo(1);", "let x = bar(1);", LCS_CELL_BUDGET_EXPANDED);
        let em_d: Vec<&str> = d.iter().filter(|s| s.emphasis).map(|s| s.text.as_str()).collect();
        let em_a: Vec<&str> = a.iter().filter(|s| s.emphasis).map(|s| s.text.as_str()).collect();
        assert_eq!(em_d, vec!["foo"]);
        assert_eq!(em_a, vec!["bar"]);
    }

    #[test]
    fn line_numbers_advance_per_side_and_dels_precede_adds() {
        let c = change(vec![Hunk { old_start: 10, new_start: 10, lines: vec![
            line(LineTag::Ctx, "a"), line(LineTag::Del, "b"), line(LineTag::Del, "c"),
            line(LineTag::Add, "B"), line(LineTag::Ctx, "d"),
        ]}]);
        let r = diff_rows(&c, true);
        let nums: Vec<(Option<u32>, Option<u32>, LineTag)> = r.rows.iter().map(|x| (x.old_no, x.new_no, x.tag)).collect();
        assert_eq!(nums, vec![
            (Some(10), Some(10), LineTag::Ctx),
            (Some(11), None, LineTag::Del),
            (Some(12), None, LineTag::Del),
            (None, Some(11), LineTag::Add),
            (Some(13), Some(12), LineTag::Ctx),
        ]);
    }

    #[test]
    fn collapsed_shows_two_rows_and_counts_hidden_rows_and_hunks() {
        let h = |start: u32| Hunk { old_start: start, new_start: start, lines: vec![line(LineTag::Ctx, "x"), line(LineTag::Add, "y"), line(LineTag::Ctx, "z")] };
        let c = change(vec![h(1), h(50), h(90)]);
        let r = diff_rows(&c, false);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.hidden_rows, 7);
        assert_eq!(r.hidden_hunks, 2);
    }

    #[test]
    fn over_budget_or_overlong_lines_fall_back_to_plain_spans() {
        let long = "a ".repeat(400); // 800 chars > MAX_INLINE_LINE_CHARS
        let (d, a) = word_spans(&long, &format!("{long}b"), LCS_CELL_BUDGET_EXPANDED);
        assert!(d.iter().all(|s| !s.emphasis) && a.iter().all(|s| !s.emphasis));
        let (d2, _) = word_spans("x y z", "x q z", 2); // budget too small for 5x5 tokens
        assert!(d2.iter().all(|s| !s.emphasis));
    }

    #[test]
    fn stats_label_reports_counts_or_the_unavailable_reason() {
        let mut c = change(vec![]);
        c.added = 12; c.removed = 3;
        assert_eq!(stats_label(&c), "+12 -3");
        c.unavailable = Some(Unavailable::TooLarge);
        assert_eq!(stats_label(&c), "+12 -3", "TooLarge keeps exact stats");
        c.unavailable = Some(Unavailable::PreImageUnavailable);
        assert!(stats_label(&c).starts_with("diff unavailable"));
    }
}
```

Add to `transcript/mod.rs`:

```rust
mod diff_view;
pub use diff_view::{
    diff_rows, stats_label, word_spans, DiffRow, DiffRows, Span, COLLAPSED_DIFF_ROWS,
    EXPANDED_DIFF_ROWS, LCS_CELL_BUDGET_COLLAPSED, LCS_CELL_BUDGET_EXPANDED,
    MAX_INLINE_LINE_CHARS,
};
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p shared-ui-logic transcript::diff_view`
Expected: 5 passed. Also `cargo clippy -p shared-ui-logic --no-default-features -- -D warnings` clean (remove the `let _ = bytes;` scaffolding if clippy flags it — it exists only to keep `bytes` used; simply drop the `bytes` binding).

- [ ] **Step 3: Commit**

```bash
git add shared/ui_logic/src/transcript/
git commit -m "ui-logic: diff rows with per-side line numbers and budgeted word emphasis

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 7: `md_enhance` — admonitions, bare-URL autolink, `path:line` refs, mermaid fences (text → structure)

**Files:**
- Create: `shared/ui_logic/src/transcript/md_enhance.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Produces: `enhance(markdown: &str, streaming: bool) -> Enhanced { markdown: String, blocks: Vec<Block> }`; `Block::{Mermaid { start_line: usize, source: String }}`; `AdmonitionKind::{Note, Tip, Important, Warning, Caution}`; `AdmonitionKind::parse(&str) -> Option<Self>`; `AdmonitionKind::label(self) -> &'static str`; `ADMONITION_MARKER: &str = "admonition-"` (the rewritten blockquote's first line becomes `> **[!NOTE]**` style marker the renderers recognise); `linkify_bare_urls(line: &str) -> String`; `trim_url(&str) -> &str`; `find_path_refs(line: &str) -> Vec<PathRef>`; `PathRef { start: usize, end: usize, path: String, line: u32, col: Option<u32> }`; `KNOWN_EXTENSIONS`.
- Streaming gate: when `streaming` is true, only the multiline-link normalisation runs (pi E.5); everything else waits for completion.

- [ ] **Step 1: Write the implementation + tests**

```rust
//! Text→text markdown pre-pass shared by both renderers. No regex crate: the
//! patterns are fixed machine text (fences, `> [!TYPE]`, `https://`,
//! `name.ext:NN`), so hand scanners are smaller for WASM and easier to test.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmonitionKind { Note, Tip, Important, Warning, Caution }

impl AdmonitionKind {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "NOTE" => Some(Self::Note), "TIP" => Some(Self::Tip), "IMPORTANT" => Some(Self::Important),
            "WARNING" => Some(Self::Warning), "CAUTION" => Some(Self::Caution), _ => None,
        }
    }
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self { Self::Note => "NOTE", Self::Tip => "TIP", Self::Important => "IMPORTANT", Self::Warning => "WARNING", Self::Caution => "CAUTION" }
    }
    /// The marker the renderers match on the FIRST line of the blockquote.
    #[must_use]
    pub fn marker(self) -> String { format!("> **[!{}]**", self.label()) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A ```mermaid fence, replaced in `markdown` by a placeholder line the
    /// renderer swaps for its diagram (Panel: sandboxed iframe; TUI: source box).
    Mermaid { index: usize, source: String },
}

/// Placeholder a renderer looks for: `<!--aleph-mermaid:N-->`.
pub const MERMAID_PLACEHOLDER_PREFIX: &str = "<!--aleph-mermaid:";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Enhanced {
    pub markdown: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef { pub start: usize, pub end: usize, pub path: String, pub line: u32, pub col: Option<u32> }

pub const KNOWN_EXTENSIONS: &[&str] = &[
    "rs","ts","tsx","js","jsx","py","go","java","kt","swift","c","h","cc","cpp","hpp","cs","rb","php",
    "md","toml","yaml","yml","json","css","scss","html","sh","ps1","sql","proto","txt","lock",
];

const URL_STOP: &[char] = &[' ', '\t', '<', '>', '\'', '"', '|', '，', '。', '；', '：', '！', '？', '、', '」', '』', '】', '（', '）', '【', '《', '》', '『', '「'];

/// Strip trailing punctuation, then unbalanced `)` / `]`.
#[must_use]
pub fn trim_url(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let Some(last) = s.chars().last() else { break };
        if ".,;:!?\"'》）}".contains(last) || last == '】' || last == '」' || last == '』' {
            end -= last.len_utf8();
            continue;
        }
        if last == ')' && s.matches(')').count() > s.matches('(').count() { end -= 1; continue; }
        if last == ']' && s.matches(']').count() > s.matches('[').count() { end -= 1; continue; }
        break;
    }
    &url[..end]
}

fn is_inside_inline_code(line: &str, at: usize) -> bool {
    line[..at].matches('`').count() % 2 == 1
}

/// `https://…` not already in `<…>` or `](…)` → `[url](url)`.
#[must_use]
pub fn linkify_bare_urls(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let hit = ["https://", "http://"].iter().find(|p| rest.starts_with(**p));
        if let Some(_) = hit {
            let prev = line[..i].chars().last();
            let preceded_by_link = line[..i].ends_with("](") || prev == Some('<');
            if !preceded_by_link && !is_inside_inline_code(line, i) {
                let stop = rest.find(|c: char| URL_STOP.contains(&c)).unwrap_or(rest.len());
                let raw = &rest[..stop];
                let url = trim_url(raw);
                out.push_str(&format!("[{url}]({url})"));
                out.push_str(&raw[url.len()..]);
                i += stop;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `src/a.rs:12` / `src/a.rs:12:5` outside inline code, extension in `KNOWN_EXTENSIONS`.
#[must_use]
pub fn find_path_refs(line: &str) -> Vec<PathRef> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let is_path_char = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'\\' | b'.' | b'_' | b'-' | b'~');
    let mut i = 0;
    while i < b.len() {
        if !is_path_char(b[i]) { i += 1; continue; }
        let start = i;
        while i < b.len() && is_path_char(b[i]) { i += 1; }
        let token = &line[start..i];
        // token must be `name.ext` and be followed by `:digits`
        let Some(dot) = token.rfind('.') else { continue };
        let ext = &token[dot + 1..];
        if !KNOWN_EXTENSIONS.contains(&ext) || i >= b.len() || b[i] != b':' { continue; }
        let mut j = i + 1;
        let ls = j;
        while j < b.len() && b[j].is_ascii_digit() { j += 1; }
        if j == ls { continue; }
        let Ok(line_no) = line[ls..j].parse::<u32>() else { continue };
        let mut col = None;
        let mut end = j;
        if j < b.len() && b[j] == b':' {
            let cs = j + 1;
            let mut k = cs;
            while k < b.len() && b[k].is_ascii_digit() { k += 1; }
            if k > cs { col = line[cs..k].parse().ok(); end = k; }
        }
        if !is_inside_inline_code(line, start) {
            out.push(PathRef { start, end, path: token.to_string(), line: line_no, col });
        }
        i = end;
    }
    out
}

fn normalize_multiline_links(md: &str) -> String {
    // `[label\n  more](url)` → `[label more](url)`; fenced code exempt.
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    let mut pending: Option<String> = None;
    for line in md.split_inclusive('\n') {
        if line.trim_start().starts_with("```") { in_fence = !in_fence; }
        if let Some(mut p) = pending.take() {
            if !in_fence && !line.contains(']') && !line.trim().is_empty() {
                p.push(' ');
                p.push_str(line.trim());
                pending = Some(p);
                continue;
            }
            p.push_str(if !in_fence && line.contains("](") { line.trim_start() } else { line });
            out.push_str(&p);
            continue;
        }
        if !in_fence && line.contains('[') && !line.contains(']') {
            pending = Some(line.trim_end_matches('\n').to_string());
            continue;
        }
        out.push_str(line);
    }
    if let Some(p) = pending { out.push_str(&p); }
    out
}

/// The pre-pass. Streaming: only link normalisation. Complete: everything.
#[must_use]
pub fn enhance(markdown: &str, streaming: bool) -> Enhanced {
    let md = normalize_multiline_links(markdown);
    if streaming {
        return Enhanced { markdown: md, blocks: Vec::new() };
    }
    let mut out = String::with_capacity(md.len());
    let mut blocks = Vec::new();
    let mut lines = md.lines().peekable();
    let mut in_fence: Option<String> = None; // fence info string
    let mut mermaid_buf: Option<String> = None;
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if let Some(info) = in_fence.as_ref() {
            if t.starts_with("```") {
                if info == "mermaid" {
                    let idx = blocks.len();
                    blocks.push(Block::Mermaid { index: idx, source: mermaid_buf.take().unwrap_or_default() });
                    out.push_str(&format!("{MERMAID_PLACEHOLDER_PREFIX}{idx}-->\n"));
                } else {
                    out.push_str(line); out.push('\n');
                }
                in_fence = None;
                continue;
            }
            if info == "mermaid" { mermaid_buf.get_or_insert_with(String::new).push_str(line); mermaid_buf.as_mut().unwrap().push('\n'); }
            else { out.push_str(line); out.push('\n'); }
            continue;
        }
        if t.starts_with("```") {
            let info = t.trim_start_matches('`').trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            let info = match info.as_str() { "mermaid" | "mmd" => "mermaid".to_string(), other => other.to_string() };
            if info != "mermaid" { out.push_str(line); out.push('\n'); }
            in_fence = Some(info);
            continue;
        }
        // Admonition opener: `> [!TYPE] rest`
        if let Some(after) = t.strip_prefix('>') {
            let a = after.trim_start();
            if let Some(inner) = a.strip_prefix("[!") {
                if let Some(close) = inner.find(']') {
                    if let Some(kind) = AdmonitionKind::parse(inner[..close].trim()) {
                        let mut body: Vec<String> = Vec::new();
                        let first = inner[close + 1..].trim();
                        if !first.is_empty() { body.push(first.to_string()); }
                        while let Some(next) = lines.peek() {
                            let n = next.trim_start();
                            let Some(q) = n.strip_prefix('>') else { break };
                            if q.trim_start().starts_with("[!") { break; }
                            body.push(q.trim().to_string());
                            lines.next();
                        }
                        while body.last().is_some_and(|s| s.is_empty()) { body.pop(); }
                        out.push_str(&kind.marker());
                        if !body.is_empty() { out.push(' '); out.push_str(&body.join(" ")); }
                        out.push_str("\n\n");
                        continue;
                    }
                }
            }
        }
        out.push_str(&linkify_bare_urls(line));
        out.push('\n');
    }
    if let Some(buf) = mermaid_buf { // unclosed mermaid fence: emit as a plain fence
        out.push_str("```mermaid\n"); out.push_str(&buf); out.push_str("```\n");
    }
    if !markdown.ends_with('\n') { out.pop(); }
    Enhanced { markdown: out, blocks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_admonitions_become_a_marked_blockquote_and_swallow_continuation_lines() {
        let e = enhance("> [!WARNING] 磁盘不足\n> 续行\n\ntext", false);
        assert!(e.markdown.starts_with("> **[!WARNING]** 磁盘不足 续行\n\n"));
        assert!(e.markdown.ends_with("text"));
        // Two adjacent admonitions do not swallow each other.
        let e = enhance("> [!NOTE] a\n> [!tip] b", false);
        assert_eq!(e.markdown, "> **[!NOTE]** a\n\n> **[!TIP]** b\n\n");
        // A plain quote is untouched.
        assert_eq!(enhance("> quote", false).markdown, "> quote");
    }

    #[test]
    fn bare_urls_are_linkified_with_balanced_trimming_and_code_is_exempt() {
        assert_eq!(linkify_bare_urls("see https://a.b/c, ok"), "see [https://a.b/c](https://a.b/c), ok");
        assert_eq!(linkify_bare_urls("wiki https://en.wikipedia.org/wiki/A_(B) x"), "wiki [https://en.wikipedia.org/wiki/A_(B)](https://en.wikipedia.org/wiki/A_(B)) x");
        assert_eq!(linkify_bare_urls("(https://x.y/z)"), "([https://x.y/z](https://x.y/z))");
        assert_eq!(linkify_bare_urls("`https://x.y` and [t](https://x.y)"), "`https://x.y` and [t](https://x.y)");
        assert_eq!(linkify_bare_urls("v6 https://[::1]:8080/x！"), "v6 [https://[::1]:8080/x](https://[::1]:8080/x)！");
    }

    #[test]
    fn path_refs_need_a_known_extension_and_a_line_number() {
        let r = find_path_refs("see src/a.rs:12 and lib/b.ts:3:7 but not foo:3 or x.unknownext:9");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].path.as_str(), r[0].line, r[0].col), ("src/a.rs", 12, None));
        assert_eq!((r[1].path.as_str(), r[1].line, r[1].col), ("lib/b.ts", 3, Some(7)));
        assert!(find_path_refs("`src/a.rs:12`").is_empty(), "inline code is exempt");
    }

    #[test]
    fn mermaid_fences_become_blocks_with_a_placeholder_and_only_when_complete() {
        let md = "before\n```mermaid\ngraph TD; A-->B;\n```\nafter";
        let e = enhance(md, false);
        assert_eq!(e.blocks, vec![Block::Mermaid { index: 0, source: "graph TD; A-->B;\n".into() }]);
        assert!(e.markdown.contains("<!--aleph-mermaid:0-->"));
        assert!(!e.markdown.contains("graph TD"));
        // While streaming, nothing but link normalisation runs.
        let s = enhance(md, true);
        assert!(s.blocks.is_empty());
        assert!(s.markdown.contains("```mermaid"));
        // An unclosed fence at completion is emitted as a normal fence, not lost.
        let u = enhance("```mermaid\ngraph TD;", false);
        assert!(u.markdown.contains("```mermaid\ngraph TD;"));
        assert!(u.blocks.is_empty());
    }

    #[test]
    fn other_fences_and_urls_inside_them_are_left_alone() {
        let md = "```sh\ncurl https://x.y/z\n```";
        assert_eq!(enhance(md, false).markdown, md);
    }

    #[test]
    fn multiline_link_labels_are_joined_even_while_streaming() {
        let e = enhance("前缀 [\n](https://example.com)", true);
        assert_eq!(e.markdown, "前缀 [](https://example.com)");
    }
}
```

Add to `transcript/mod.rs`:

```rust
mod md_enhance;
pub use md_enhance::{
    enhance, find_path_refs, linkify_bare_urls, trim_url, AdmonitionKind, Block, Enhanced,
    PathRef, KNOWN_EXTENSIONS, MERMAID_PLACEHOLDER_PREFIX,
};
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p shared-ui-logic transcript::md_enhance`
Expected: 6 passed. Fix the implementation until the assertions hold — the assertions are the spec §5 `md_enhance` row made concrete; do not weaken them. Watch the `multiline_link_labels` case: the joined label is empty (`[]`), which is what pi asserts too.

- [ ] **Step 3: Commit**

```bash
git add shared/ui_logic/src/transcript/
git commit -m "ui-logic: markdown pre-pass for admonitions, bare URLs, path:line refs and mermaid fences

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 8: `affordance` + `turn_summary` + `context` — hints, spinner, verbs, durations, per-turn summary, context reconciliation

**Files:**
- Create: `shared/ui_logic/src/transcript/affordance.rs`, `shared/ui_logic/src/transcript/turn_summary.rs`, `shared/ui_logic/src/transcript/context.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`, `shared/ui_logic/src/transcript/view_model.rs` (swap the Task 5 stub for `affordance::fmt_duration_ms`; delete `affordance_stub`)

**Interfaces:**
- Produces: `Modality::{Mouse, Key(&'static str)}`; `expand_hint(modality, hidden_rows) -> String` (`… +41 lines (ctrl+o to expand)` / `… +41 lines · click to expand`); `spinner_frame(now_ms: u64) -> char` (braille, 80 ms); `SPINNER_FRAMES`; `Locale::{En, Zh}`; `verb(locale, seed: u64) -> &'static str`; `VERBS_EN`, `VERBS_ZH` (30–40 each); `VERB_REROLL_MS: u64 = 7_000`; `worked_for(duration_ms) -> String` (`✻ Worked for 12s`); `fmt_duration_ms(ms) -> String` (`0.8s`, `12s`, `2m 05s`); `summarize_turn(rows: &[ToolRow]) -> Option<TurnSummaryEntry>` (≥2 tools); `turn_summary_text(&TurnSummaryEntry) -> String`; `ContextRow { label: String, tokens: u64, bytes: Option<u64> }`; `reconcile(&ContextBreakdown) -> ContextRows { rows: Vec<ContextRow>, total: Option<u64>, window: Option<u32>, percent: Option<f32>, other: u64 }`; `MIN_TOOLS_FOR_SUMMARY: usize = 2`; `PROVIDER_TOLERANCE: f64 = 0.001`.

- [ ] **Step 1: `affordance.rs`**

```rust
//! Small presentation decisions both surfaces share verbatim.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality { Mouse, Key(&'static str) }

/// `… +N lines (ctrl+o to expand)` or `… +N lines · click to expand`.
#[must_use]
pub fn expand_hint(modality: Modality, hidden_rows: usize) -> String {
    let unit = if hidden_rows == 1 { "line" } else { "lines" };
    match modality {
        Modality::Mouse => format!("… +{hidden_rows} {unit} · click to expand"),
        Modality::Key(k) => format!("… +{hidden_rows} {unit} ({k} to expand)"),
    }
}

pub const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
pub const SPINNER_PERIOD_MS: u64 = 80;

/// Pure function of wall-clock time, so every row repaints in step without
/// a shared timer (pi-cc-extensions `tool-loading-icon.ts`).
#[must_use]
pub fn spinner_frame(now_ms: u64) -> char {
    SPINNER_FRAMES[((now_ms / SPINNER_PERIOD_MS) % SPINNER_FRAMES.len() as u64) as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale { En, Zh }

pub const VERB_REROLL_MS: u64 = 7_000;

pub const VERBS_EN: &[&str] = &[
    "Thinking", "Pondering", "Reading", "Searching", "Tracing", "Weaving", "Mapping", "Sifting",
    "Assembling", "Composing", "Checking", "Untangling", "Refining", "Sketching", "Digging",
    "Connecting", "Comparing", "Testing", "Measuring", "Planning", "Drafting", "Reviewing",
    "Aligning", "Stitching", "Polishing", "Verifying", "Scanning", "Gathering", "Shaping", "Working",
];
pub const VERBS_ZH: &[&str] = &[
    "思考中", "琢磨中", "阅读中", "搜索中", "追踪中", "梳理中", "拼装中", "整理中", "核对中", "推演中",
    "勾勒中", "挖掘中", "串联中", "比对中", "测试中", "度量中", "规划中", "起草中", "复核中", "对齐中",
    "缝合中", "打磨中", "验证中", "扫描中", "收集中", "成形中", "构思中", "校准中", "编排中", "工作中",
];

#[must_use]
pub fn verb(locale: Locale, seed: u64) -> &'static str {
    let list = match locale { Locale::En => VERBS_EN, Locale::Zh => VERBS_ZH };
    list[(seed % list.len() as u64) as usize]
}

/// `0.8s`, `12s`, `2m 05s`, `1h 03m`.
#[must_use]
pub fn fmt_duration_ms(ms: u64) -> String {
    if ms < 10_000 { return format!("{:.1}s", ms as f64 / 1000.0); }
    let s = ms / 1000;
    if s < 60 { return format!("{s}s"); }
    let (m, s) = (s / 60, s % 60);
    if m < 60 { return format!("{m}m {s:02}s"); }
    format!("{}h {:02}m", m / 60, m % 60)
}

/// The turn trailer.
#[must_use]
pub fn worked_for(duration_ms: u64) -> String { format!("✻ Worked for {}", fmt_duration_ms(duration_ms)) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hint_is_a_function_of_modality() {
        assert_eq!(expand_hint(Modality::Mouse, 41), "… +41 lines · click to expand");
        assert_eq!(expand_hint(Modality::Key("ctrl+o"), 1), "… +1 line (ctrl+o to expand)");
    }
    #[test]
    fn spinner_is_periodic_in_time_and_verbs_are_stable_for_a_seed() {
        assert_eq!(spinner_frame(0), spinner_frame(800));
        assert_ne!(spinner_frame(0), spinner_frame(80));
        assert_eq!(verb(Locale::En, 7), verb(Locale::En, 7));
        assert_eq!(VERBS_EN.len(), 30);
        assert_eq!(VERBS_ZH.len(), 30);
    }
    #[test]
    fn durations_switch_units_at_the_right_edges() {
        assert_eq!(fmt_duration_ms(800), "0.8s");
        assert_eq!(fmt_duration_ms(12_000), "12s");
        assert_eq!(fmt_duration_ms(125_000), "2m 05s");
        assert_eq!(worked_for(12_000), "✻ Worked for 12s");
    }
}
```

- [ ] **Step 2: `turn_summary.rs`**

```rust
//! `Ran 3 commands, read 2 files, edited 1 file · 42s` — stored as DATA on
//! the transcript, formatted at paint time (pi-cc-extensions agent-summary).

use super::affordance::fmt_duration_ms;
use super::view_model::{RowStatus, ToolRow, TurnSummaryEntry};

pub const MIN_TOOLS_FOR_SUMMARY: usize = 2;

#[must_use]
pub fn summarize_turn(rows: &[ToolRow]) -> Option<TurnSummaryEntry> {
    if rows.len() < MIN_TOOLS_FOR_SUMMARY { return None; }
    let mut e = TurnSummaryEntry { commands: 0, reads: 0, edits: 0, writes: 0, others: 0, failed: 0, duration_ms: 0 };
    let mut read_paths = std::collections::HashSet::new();
    let mut edit_paths = std::collections::HashSet::new();
    let mut write_paths = std::collections::HashSet::new();
    for r in rows {
        match r.summary.display_name.as_str() {
            "Bash" => e.commands += 1,
            "Read" => { read_paths.insert(r.summary.args_text.clone()); }
            "Edit" | "Patch" => { edit_paths.insert(r.summary.args_text.clone()); }
            "Write" => { write_paths.insert(r.summary.args_text.clone()); }
            _ => e.others += 1,
        }
        if let RowStatus::Err { .. } = r.status { e.failed += 1; }
        if let (Some(s), Some(t)) = (r.started_ms, r.ended_ms) { e.duration_ms += t.saturating_sub(s); }
    }
    e.reads = read_paths.len() as u32;
    e.edits = edit_paths.len() as u32;
    e.writes = write_paths.len() as u32;
    Some(e)
}

fn part(n: u32, verb: &str, noun: &str) -> Option<String> {
    (n > 0).then(|| format!("{verb} {n} {noun}{}", if n == 1 { "" } else { "s" }))
}

/// Empty string when nothing was counted (the renderer drops it).
#[must_use]
pub fn turn_summary_text(e: &TurnSummaryEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(part(e.commands, "ran", "command"));
    parts.extend(part(e.reads, "read", "file"));
    parts.extend(part(e.edits, "edited", "file"));
    parts.extend(part(e.writes, "wrote", "file"));
    if e.others > 0 { parts.push(format!("{} other tool{}", e.others, if e.others == 1 { "" } else { "s" })); }
    if e.failed > 0 { parts.push(format!("{} failed", e.failed)); }
    if parts.is_empty() { return String::new(); }
    let mut s = parts.join(", ");
    if let Some(f) = s.chars().next() { if f.is_ascii_lowercase() { s.replace_range(..1, &f.to_ascii_uppercase().to_string()); } }
    if e.duration_ms > 0 { s.push_str(&format!(" · {}", fmt_duration_ms(e.duration_ms))); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn row(tool: &str, args: serde_json::Value, ok: bool) -> ToolRow {
        let mut r = ToolRow::new("id", tool, &args);
        r.started_ms = Some(0); r.ended_ms = Some(14_000);
        r.status = if ok { RowStatus::Ok { duration_ms: 14_000 } } else { RowStatus::Err { duration_ms: 14_000, message: "x".into() } };
        r
    }
    #[test]
    fn formats_in_fixed_order_with_plurals_and_a_capital() {
        let rows = vec![
            row("shell_exec", json!({"command": "ls"}), true),
            row("shell_exec", json!({"command": "pwd"}), true),
            row("shell_exec", json!({"command": "id"}), false),
            row("file_read", json!({"path": "a.rs"}), true),
            row("file_read", json!({"path": "a.rs"}), true), // same file counts once
            row("file_read", json!({"path": "b.rs"}), true),
            row("file_edit", json!({"file_path": "c.rs"}), true),
            row("file_write", json!({"file_path": "d.rs"}), true),
        ];
        let e = summarize_turn(&rows).unwrap();
        assert_eq!(turn_summary_text(&e), "Ran 3 commands, read 2 files, edited 1 file, wrote 1 file, 1 failed · 1m 52s");
    }
    #[test]
    fn one_tool_is_below_the_gate() {
        assert!(summarize_turn(&[row("file_read", json!({"path": "a"}), true)]).is_none());
    }
}
```

- [ ] **Step 3: `context.rs`**

```rust
//! Turn a `ContextBreakdown` into the rows a `/context` view paints, with
//! provider reconciliation and an explicit `Other` remainder — never an
//! inflated estimate (pi-cc-extensions `context.ts` H.4, but Aleph's layer
//! bytes are MEASURED, not chars/4).

use aleph_protocol::context_breakdown::ContextBreakdown;

/// Provider count vs. our total: beyond this fraction the provider wins.
pub const PROVIDER_TOLERANCE: f64 = 0.001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow { pub label: String, pub tokens: u64, pub bytes: Option<u64> }

#[derive(Debug, Clone, PartialEq)]
pub struct ContextRows {
    pub rows: Vec<ContextRow>,
    /// Best-known occupancy: provider-reported when present, else our sum.
    pub total: Option<u64>,
    pub window: Option<u32>,
    pub percent: Option<f32>,
    /// `total - Σrows`, ≥ 0, shown as its own row.
    pub other: u64,
}

#[must_use]
pub fn reconcile(b: &ContextBreakdown) -> ContextRows {
    let mut rows: Vec<ContextRow> = b.layers.iter().map(|l| ContextRow { label: l.name.clone(), tokens: l.tokens, bytes: Some(l.bytes) }).collect();
    let tool_bytes = b.tool_bytes();
    if !b.tools.is_empty() {
        rows.push(ContextRow { label: format!("Tools ({} schemas)", b.tools.len()), tokens: tool_bytes / 4, bytes: Some(tool_bytes) });
    }
    if let Some(m) = b.messages_tokens { rows.push(ContextRow { label: "Messages".into(), tokens: m, bytes: None }); }
    let ours: u64 = rows.iter().map(|r| r.tokens).sum();
    let total = match b.provider_reported {
        Some(u) => {
            let reported = u.input + u.cache_read + u.cache_creation;
            let diff = (reported as f64 - ours as f64).abs();
            if diff > (ours as f64 * PROVIDER_TOLERANCE).max(32.0) { Some(reported) } else { Some(ours) }
        }
        None => None, // unknown after compaction: render `?`, not our sum
    };
    let other = total.map_or(0, |t| t.saturating_sub(ours));
    let percent = match (total, b.context_window) {
        (Some(t), Some(w)) if w > 0 => Some((t as f32 / w as f32) * 100.0),
        _ => None,
    };
    if other > 0 { rows.push(ContextRow { label: "Other".into(), tokens: other, bytes: None }); }
    ContextRows { rows, total, window: b.context_window, percent, other }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{LayerSizeView, ToolSchemaSize, UsageTokens};
    fn breakdown(provider: Option<UsageTokens>) -> ContextBreakdown {
        ContextBreakdown {
            session_key: "k".into(), turn: 1,
            layers: vec![LayerSizeView { name: "System".into(), bytes: 4000, tokens: 1000, zone: "stable".into() }],
            tools: vec![ToolSchemaSize { name: "grep".into(), schema_bytes: 400, description_bytes: 0 }],
            messages_tokens: Some(500), provider_reported: provider, context_window: Some(200_000),
        }
    }
    #[test]
    fn provider_wins_when_it_disagrees_and_the_remainder_is_an_explicit_other_row() {
        let r = reconcile(&breakdown(Some(UsageTokens { input: 2000, output: 0, cache_read: 0, cache_creation: 0 })));
        assert_eq!(r.total, Some(2000));
        assert_eq!(r.other, 400); // 2000 - (1000 + 100 + 500)
        assert_eq!(r.rows.last().unwrap().label, "Other");
        assert!((r.percent.unwrap() - 1.0).abs() < 0.01);
    }
    #[test]
    fn no_provider_usage_means_unknown_not_our_sum() {
        let r = reconcile(&breakdown(None));
        assert_eq!(r.total, None);
        assert_eq!(r.percent, None);
        assert_eq!(r.other, 0);
        assert!(r.rows.iter().all(|x| x.label != "Other"));
    }
    #[test]
    fn a_provider_count_within_tolerance_keeps_our_rows_unchanged() {
        let r = reconcile(&breakdown(Some(UsageTokens { input: 1600, output: 0, cache_read: 0, cache_creation: 0 })));
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.other, 0);
    }
}
```

- [ ] **Step 4: Wire the modules; remove the Task 5 stub**

In `transcript/mod.rs` add:

```rust
mod affordance;
mod context;
mod turn_summary;
pub use affordance::{
    expand_hint, fmt_duration_ms, spinner_frame, verb, worked_for, Locale, Modality,
    SPINNER_FRAMES, SPINNER_PERIOD_MS, VERBS_EN, VERBS_ZH, VERB_REROLL_MS,
};
pub use context::{reconcile, ContextRow, ContextRows, PROVIDER_TOLERANCE};
pub use turn_summary::{summarize_turn, turn_summary_text, MIN_TOOLS_FOR_SUMMARY};
```

In `view_model.rs` delete `mod affordance_stub { … }` and change the call to `super::affordance::fmt_duration_ms(total_ms)`.

- [ ] **Step 5: Run everything in the crate**

Run: `cargo test -p shared-ui-logic` and `cargo clippy -p shared-ui-logic --no-default-features --all-targets -- -D warnings`
Expected: all green, zero warnings, and the `group_headline_counts_calls_and_sums_duration` test still passes (`0.2s`).

- [ ] **Step 6: Commit**

```bash
git add shared/ui_logic/src/transcript/
git commit -m "ui-logic: hints, spinner, locale verbs, per-turn summary and context reconciliation

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 9: `shared-ui-logic` entropy — CUT the inert `leptos` feature and three dead items

**Files:**
- Modify: `shared/ui_logic/Cargo.toml`, `shared/ui_logic/src/connection/mod.rs:14`, `shared/ui_logic/src/connection/failure.rs:46,82,205`, `shared/ui_logic/src/safety/prompt_injection.rs:31`, `shared/ui_logic/src/safety/mod.rs:9-12`, `interfaces/webchat/Cargo.toml:124`

**Interfaces:** removes `DefaultConnector`, `PromptInjectionCheck.reasons` + `PromptInjectionReason`, `FailureStage::RpcTimeout`. Keeps everything else byte-identical.

- [ ] **Step 1: Prove each is dead (the grep IS the test)**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/cc-render-r1
grep -rn "DefaultConnector" --include=*.rs interfaces shared src | grep -v 'shared/ui_logic/src/connection/mod.rs'
grep -rn "PromptInjectionReason\|\.reasons" --include=*.rs interfaces shared/client src | grep -v shared/ui_logic
grep -rn "RpcTimeout" --include=*.rs interfaces shared src | grep -v shared/ui_logic
grep -rn 'cfg(feature = "leptos")\|use leptos' shared/ui_logic/src
```

Expected: every command prints nothing. If any prints a consumer, STOP for that item and leave it in place (report it in the task summary).

- [ ] **Step 2: Remove**

- `Cargo.toml`: delete the `leptos` dependency line and its comment, `default = ["wasm"]`, delete the `leptos = ["dep:leptos"]` feature line.
- `connection/mod.rs`: delete the two-line `#[cfg(feature = "wasm")] pub use wasm::WasmConnector as DefaultConnector;`.
- `failure.rs`: delete the `RpcTimeout` variant, its `classify` arm, and the test at `:205`. If `ConnectionFailure::Timeout` becomes unreachable (no other constructor), delete it too and its arm(s); if the Panel matches on it (`interfaces/webchat/src/context.rs:1196,1436,1550`), keep the variant and add a one-line doc comment saying which path still produces it — do not delete a variant a consumer matches on.
- `prompt_injection.rs`: delete the `reasons` field and its population; delete `PromptInjectionReason`; update `safety/mod.rs` re-exports.
- `interfaces/webchat/Cargo.toml:124`: `features = ["wasm"]`.

- [ ] **Step 3: Verify both consumers still build**

Run: `cargo test -p shared-ui-logic && cargo check -p aleph-tui && CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: green. (The Panel check needs the wasm target installed; `just wasm` is the full-fidelity alternative if the check errors on `web-sys` features.)

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic interfaces/webchat/Cargo.toml
git commit -m "ui-logic: cut the inert leptos feature and three zero-consumer items

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

## Part 2 — Server: diff computation, the presentation side-channel, replay, two RPCs

### Task 10: `similar` into alephcore + `file_ops/diff.rs::compute_file_change`

**Files:**
- Modify: root `Cargo.toml` `[dependencies]` (alephcore's own, lines 158-313), `src/builtin_tools/file_ops/mod.rs:6-35`
- Create: `src/builtin_tools/file_ops/diff.rs`

**Interfaces:**
- Produces: `pub(crate) fn compute_file_change(path: &str, before: Option<&str>, after: Option<&str>) -> aleph_protocol::FileChange`; `pub(crate) fn presentation_for(changes: Vec<FileChange>) -> aleph_protocol::Presentation`.
- Consumers: Task 11 (edit/write/apply_patch).

- [ ] **Step 1: Add the dependency (root `Cargo.toml`, alephcore `[dependencies]`, alphabetical near `serde`/`similar`)**

```toml
# Line-level diffs for the FileChange presentation side-channel. Same 2.x the
# Panel already pins — `cargo tree -p alephcore -i similar` must show one copy.
similar = { version = "2", default-features = false, features = ["text"] }
```

Run: `cargo tree -i similar 2>/dev/null | head -5` — one version line only.

- [ ] **Step 2: Write `diff.rs` with tests**

```rust
//! The ONLY `similar` call site in alephcore. Turns a pre-image + post-image
//! into the wire `FileChange` a file-mutating tool attaches under
//! `_presentation` (hoisted by `apply_layer_two`, never seen by the model).

use aleph_protocol::file_change::{
    FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Presentation, Unavailable, CONTEXT_LINES,
    MAX_HUNK_LINES,
};
use similar::{ChangeTag, TextDiff};

/// Pre-images larger than this are not diffed (stats still counted by lines).
pub(crate) const MAX_DIFF_INPUT_BYTES: usize = 2 * 1024 * 1024;

fn count_lines(s: &str) -> u32 {
    u32::try_from(s.lines().count()).unwrap_or(u32::MAX)
}

/// `before == None` → Created; `after == None` → Deleted; both → Modified.
/// Both `None` is a programming error and yields `ToolFailed`.
pub(crate) fn compute_file_change(path: &str, before: Option<&str>, after: Option<&str>) -> FileChange {
    let kind = match (before, after) {
        (None, Some(_)) => FileChangeKind::Created,
        (Some(_), None) => FileChangeKind::Deleted,
        (Some(_), Some(_)) => FileChangeKind::Modified,
        (None, None) => return FileChange::unavailable(path, FileChangeKind::Modified, Unavailable::ToolFailed),
    };
    let old = before.unwrap_or("");
    let new = after.unwrap_or("");
    if old.len() > MAX_DIFF_INPUT_BYTES || new.len() > MAX_DIFF_INPUT_BYTES {
        let mut c = FileChange::unavailable(path, kind, Unavailable::TooLarge);
        c.added = if before.is_none() { count_lines(new) } else { 0 };
        c.removed = if after.is_none() { count_lines(old) } else { 0 };
        return c;
    }

    let diff = TextDiff::from_lines(old, new);
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut total_lines = 0usize;
    for group in diff.grouped_ops(CONTEXT_LINES) {
        let Some(first) = group.first() else { continue };
        let (old_start, new_start) = (first.old_range().start + 1, first.new_range().start + 1);
        let mut lines: Vec<HunkLine> = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let tag = match change.tag() {
                    ChangeTag::Equal => LineTag::Ctx,
                    ChangeTag::Delete => { removed += 1; LineTag::Del }
                    ChangeTag::Insert => { added += 1; LineTag::Add }
                };
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                lines.push(HunkLine { tag, text });
            }
        }
        total_lines += lines.len();
        hunks.push(Hunk {
            old_start: u32::try_from(old_start).unwrap_or(u32::MAX),
            new_start: u32::try_from(new_start).unwrap_or(u32::MAX),
            lines,
        });
    }
    let mut change = FileChange { path: path.to_string(), kind, hunks, added, removed, unavailable: None };
    if total_lines > MAX_HUNK_LINES {
        change.hunks.clear();
        change.unavailable = Some(Unavailable::TooLarge);
    }
    change
}

pub(crate) fn presentation_for(changes: Vec<FileChange>) -> Presentation {
    Presentation::FileChanges { changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_line_edit_yields_one_hunk_with_context_and_exact_stats() {
        let old = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old.replace("line 10\n", "line ten\n");
        let c = compute_file_change("a.rs", Some(&old), Some(&new));
        assert_eq!(c.kind, FileChangeKind::Modified);
        assert_eq!((c.added, c.removed), (1, 1));
        assert_eq!(c.hunks.len(), 1);
        let h = &c.hunks[0];
        assert_eq!((h.old_start, h.new_start), (6, 6)); // 4 context lines before line 10
        assert_eq!(h.lines.iter().filter(|l| l.tag == LineTag::Del).count(), 1);
        assert_eq!(h.lines.iter().filter(|l| l.tag == LineTag::Add).count(), 1);
        assert!(h.lines.iter().all(|l| !l.text.ends_with('\n')));
    }

    #[test]
    fn two_distant_edits_yield_two_hunks() {
        let old = (1..=100).map(|i| format!("l{i}\n")).collect::<String>();
        let new = old.replace("l5\n", "L5\n").replace("l90\n", "L90\n");
        let c = compute_file_change("a", Some(&old), Some(&new));
        assert_eq!(c.hunks.len(), 2);
    }

    #[test]
    fn created_and_deleted_files_are_all_adds_or_all_dels() {
        let c = compute_file_change("n.rs", None, Some("a\nb\n"));
        assert_eq!(c.kind, FileChangeKind::Created);
        assert_eq!((c.added, c.removed), (2, 0));
        let d = compute_file_change("n.rs", Some("a\nb\nc\n"), None);
        assert_eq!(d.kind, FileChangeKind::Deleted);
        assert_eq!((d.added, d.removed), (0, 3));
    }

    #[test]
    fn a_huge_change_keeps_stats_but_drops_hunks_as_too_large() {
        let old = (1..=1000).map(|i| format!("{i}\n")).collect::<String>();
        let new = (1..=1000).map(|i| format!("{}\n", i * 2)).collect::<String>();
        let c = compute_file_change("a", Some(&old), Some(&new));
        assert_eq!(c.unavailable, Some(Unavailable::TooLarge));
        assert!(c.hunks.is_empty());
        assert!(c.added > 0 && c.removed > 0, "stats survive the cap");
    }

    #[test]
    fn crlf_text_diffs_without_carriage_returns_leaking_into_rows() {
        let c = compute_file_change("w.txt", Some("a\r\nb\r\n"), Some("a\r\nB\r\n"));
        assert!(c.hunks[0].lines.iter().all(|l| !l.text.contains('\r')));
    }

    #[test]
    fn identical_content_is_a_modified_change_with_no_hunks() {
        let c = compute_file_change("a", Some("x\n"), Some("x\n"));
        assert!(c.hunks.is_empty());
        assert_eq!((c.added, c.removed), (0, 0));
        assert!(c.unavailable.is_none());
    }
}
```

Register in `file_ops/mod.rs`: add `pub(crate) mod diff;` after `mod batch;`.

- [ ] **Step 3: Run**

Run: `CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib file_ops::diff`
Expected: 6 passed. (First alephcore lib-test build after a dep change is ~13 min — detach per memory `alephcore-build-memory`; subsequent filtered runs are incremental.) If `first.old_range()` does not exist on the `DiffOp` type in this `similar` version, use `similar::DiffOp::old_range(&op)` / `new_range` — both are on `DiffOp`.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/builtin_tools/file_ops/mod.rs src/builtin_tools/file_ops/diff.rs
git commit -m "file_ops: compute a bounded FileChange with similar for the presentation side-channel

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 11: Attach `_presentation` in `file_edit`, `file_write`, `apply_patch`; declare `mutates_file_content` on the trait

**Files:**
- Modify: `src/tools/traits.rs:62-201` (`AlephTool`), `src/builtin_tools/file_ops/edit.rs` (`FileEditOutput` :205, `call_impl` :262-419, `apply_multi_edit` :428-546, `impl AlephTool` :573-593), `src/builtin_tools/file_ops/write.rs` (`FileWriteOutput` :53-65, `call` :123-193), `src/builtin_tools/file_ops/ops.rs` (`execute_write` :189-244, `WriteOutcome` :167-175), `src/builtin_tools/file_ops/apply_patch.rs` (`FileOutcome` :71, `ApplyPatchOutput` :80, `Planned::commit` :724-806, `run` :160-220, `impl AlephTool` ~:668)

**Interfaces:**
- Produces: `AlephTool::mutates_file_content(&self) -> bool` (default `false`; `true` on `FileEditTool`, `FileWriteTool`, `ApplyPatchTool`); each of the three `Output` structs gains `#[serde(rename = "_presentation", skip_serializing_if = "Option::is_none")] pub presentation: Option<Presentation>`; `WriteOutcome.previous: Option<String>` (the pre-image, `None` when the file did not exist), `WriteOutcome.pre_image_unreadable: bool`.
- Consumers: Task 12 hoist; Task 14 census.

- [ ] **Step 1: Trait predicate — `src/tools/traits.rs`, after `requires_confirmation()`**

```rust
    /// Whether this tool rewrites the CONTENT of user files (edit / write /
    /// patch). Tools answering `true` attach a `_presentation` FileChange
    /// to their output — `presentation_census` asserts it. Moves, deletes
    /// and config/state writers answer `false`.
    fn mutates_file_content(&self) -> bool {
        false
    }
```

Also expose it on `AlephToolDyn` (`traits.rs:326-348`): add `fn mutates_file_content(&self) -> bool;` to the object-safe trait and forward it in the blanket impl at `:361`.

- [ ] **Step 2: `file_edit`**

`FileEditOutput` — add the field:

```rust
    /// UI side-channel; hoisted out before the model sees this output.
    #[serde(rename = "_presentation", skip_serializing_if = "Option::is_none")]
    pub presentation: Option<aleph_protocol::Presentation>,
```

In `call_impl` (single-edit branch) after `let new_content = apply_ranges(&content, applied, &replacement);` and before the atomic write, nothing; after the write succeeds and before `Ok(FileEditOutput { .. })`:

```rust
        let change = super::diff::compute_file_change(&path_str, Some(&content), Some(&new_content));
        let presentation = Some(super::diff::presentation_for(vec![change]));
```

and add `presentation` to the struct literal. Same in `apply_multi_edit` Step 4 (it has `content` and `new_content` in scope; ONE `FileChange` for the whole call). In `impl AlephTool for FileEditTool` add `fn mutates_file_content(&self) -> bool { true }`.

- [ ] **Step 3: `file_write` — read the pre-image under the path lock**

`ops.rs` `WriteOutcome`:

```rust
pub(super) struct WriteOutcome {
    pub canonical: PathBuf,
    pub bytes: u64,
    pub unchanged: bool,
    /// Old content when the file existed and was UTF-8 text; `None` = did not exist.
    pub previous: Option<String>,
    /// The file existed but could not be read as text (binary / too large / IO).
    pub pre_image_unreadable: bool,
}
```

In `execute_write`, immediately after `let _path_guard = …lock_path(&canonical).await;` and BEFORE `create_dir_all` / the byte-equal check:

```rust
    // Pre-image for the diff side-channel. Read under the same lock the
    // write holds, so the diff describes exactly the bytes we replace.
    let (previous, pre_image_unreadable) = match tokio::fs::metadata(&canonical).await {
        Err(_) => (None, false), // does not exist → Created
        Ok(meta) if meta.len() > super::diff::MAX_DIFF_INPUT_BYTES as u64 => (None, true),
        Ok(_) => match tokio::fs::read(&canonical).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => (Some(text), false),
                Err(_) => (None, true),
            },
            Err(_) => (None, true),
        },
    };
```

and populate both new fields in every `Ok(WriteOutcome { .. })`. In `write.rs` `call`, after `execute_write` returns `outcome`:

```rust
        let change = if outcome.pre_image_unreadable {
            aleph_protocol::FileChange::unavailable(
                outcome.canonical.to_string_lossy(), aleph_protocol::FileChangeKind::Modified,
                aleph_protocol::Unavailable::PreImageUnavailable)
        } else {
            super::diff::compute_file_change(
                &outcome.canonical.to_string_lossy(), outcome.previous.as_deref(), Some(&args.content))
        };
        let presentation = Some(super::diff::presentation_for(vec![change]));
```

Add the `_presentation` field to `FileWriteOutput` and the literal; `fn mutates_file_content(&self) -> bool { true }` on the impl.

- [ ] **Step 4: `apply_patch` — one `FileChange` per committed file**

`Planned::commit()` returns `FileOutcome`; extend `FileOutcome` with

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<aleph_protocol::FileChange>,
```

and fill it in each arm: `Effect::Add { path, body }` → `compute_file_change(path, None, Some(body))`; `Effect::Delete { path }` → the planner must have read the old text (it does, in `plan_delete`/`read_text` — carry it on the `Effect::Delete` variant as `old: Option<String>`; if unreadable → `unavailable(Deleted, Binary)`); `Effect::Update { write_to, content, .. }` → the planner read the old text in `plan_update` — carry it as `old: String` on the variant → `compute_file_change(write_to, Some(&old), Some(content))`. A failed `commit` arm → `FileChange::unavailable(path, kind, Unavailable::ToolFailed)`. Then in `run`, after `execute` returns `(all_ok, outcomes)`:

```rust
        let changes: Vec<aleph_protocol::FileChange> = outcomes.iter().filter_map(|o| o.change.clone()).collect();
        let presentation = (!changes.is_empty()).then(|| super::diff::presentation_for(changes));
```

`ApplyPatchOutput` gains the `_presentation` field. Keep `FileOutcome.change` OUT of the model text: `FileOutcome` is part of the model-facing output today, so ALSO mark it `#[serde(skip)]`? No — the hoist in Task 12 removes only the top-level `_presentation`. Therefore do NOT put `change` on `FileOutcome`'s serialized form: make it `#[serde(skip)] pub change: Option<FileChange>` (internal transport from `commit` to `run` only). `fn mutates_file_content(&self) -> bool { true }` on the impl.

- [ ] **Step 5: Tests (in each file's existing `#[cfg(test)]`)**

`edit.rs`:

```rust
    #[tokio::test]
    async fn single_and_multi_edit_attach_one_file_change_under_the_presentation_key() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        let tool = FileEditTool::default();
        let out = tool.call_impl(FileEditArgs { file_path: p.to_string_lossy().into(), old_string: "two".into(), new_string: "2".into(), replace_all: false, edits: vec![] }).await.unwrap();
        let v = serde_json::to_value(&out).unwrap();
        let changes = &v["_presentation"]["changes"];
        assert_eq!(changes.as_array().unwrap().len(), 1);
        assert_eq!(changes[0]["added"], 1);
        assert_eq!(changes[0]["removed"], 1);
        // multi-edit: still ONE FileChange, with both edits' stats
        let out = tool.call_impl(FileEditArgs { file_path: p.to_string_lossy().into(), old_string: String::new(), new_string: String::new(), replace_all: false,
            edits: vec![EditOp { old_string: "one".into(), new_string: "1".into() }, EditOp { old_string: "three".into(), new_string: "3".into() }] }).await.unwrap();
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["_presentation"]["changes"].as_array().unwrap().len(), 1);
        assert_eq!(v["_presentation"]["changes"][0]["added"], 2);
    }
```

`write.rs`: new file → `kind == "created"`, `added == line count`; overwrite → `kind == "modified"` with hunks; a binary pre-image (write bytes `[0xff, 0xfe, 0x00]` first) → `unavailable == "pre_image_unavailable"`. `apply_patch.rs`: a patch with one Update and one Add → two changes, kinds `modified` + `created`; a Delete → `deleted` with `removed == old line count`.

- [ ] **Step 6: Run**

Run: `CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib file_ops::`
Expected: all previous file_ops tests + the new ones green. If an existing test asserts the exact JSON key set of one of these outputs, it must now allow `_presentation` (Task 12's hoist removes it before the model sees it — the output struct is not the model text).

- [ ] **Step 7: Commit**

```bash
git add src/tools/traits.rs src/builtin_tools/file_ops/
git commit -m "file_ops: file_edit/file_write/apply_patch attach a FileChange presentation; write reads its pre-image

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 12: Hoist `_presentation` into `ToolOutputMetadata` before the model text is built

**Files:**
- Modify: `src/session/events.rs:105-121` (`ToolOutputMetadata`), `src/tools/result_processing.rs` (next to `hoist_inline_images` :381), `src/tools/scoped/dispatch.rs:1447-1458` (`apply_layer_two`)

**Interfaces:**
- Produces: `ToolOutputMetadata.presentation: Option<aleph_protocol::Presentation>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`); `pub fn hoist_presentation(value: &mut serde_json::Value) -> Option<aleph_protocol::Presentation>`.
- Guarantee: after `apply_layer_two`, `out.value` (the model text) contains no `_presentation`; `out.metadata.presentation` holds it; `session_events.payload_json` persists it for free (it serializes the whole `ToolOutput`).

- [ ] **Step 1: The metadata field**

In `src/session/events.rs` `ToolOutputMetadata`, after `images`:

```rust
    /// Structured UI presentation (file diffs) hoisted out of the tool's
    /// JSON by `apply_layer_two` BEFORE the value is flattened to model text.
    /// Rides `session_events` for replay and the callback for the live frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<aleph_protocol::Presentation>,
```

(`PartialEq, Eq` stay valid.) Grep `ToolOutputMetadata {` struct literals (`src/harness/agent/act.rs`, tests) and add `presentation: None` where the literal is not `..Default::default()`.

- [ ] **Step 2: The hoist — `src/tools/result_processing.rs`, after `hoist_inline_images`**

```rust
/// Lift the UI presentation a tool attached under
/// [`aleph_protocol::PRESENTATION_KEY`] out of its JSON and REMOVE the key,
/// so the model-facing text (built from `value` right after) never carries
/// it. Only a top-level object key is honoured — a nested one is a tool bug
/// the census (`presentation_census`) will name.
pub fn hoist_presentation(value: &mut serde_json::Value) -> Option<aleph_protocol::Presentation> {
    let obj = value.as_object_mut()?;
    let raw = obj.remove(aleph_protocol::PRESENTATION_KEY)?;
    match serde_json::from_value::<aleph_protocol::Presentation>(raw) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "tool attached a `_presentation` that is not a Presentation; dropped");
            None
        }
    }
}
```

- [ ] **Step 3: Call it in `apply_layer_two` (`dispatch.rs`, right after the image hoist block)**

```rust
        // Same discipline as the image hoist: structured UI data leaves the
        // value before flattening/truncation so the model never pays for it
        // and the UI never loses it.
        if let Some(p) = crate::tools::result_processing::hoist_presentation(&mut out.value) {
            out.metadata.presentation = Some(p);
        }
```

- [ ] **Step 4: Tests**

In `result_processing.rs` tests:

```rust
    #[test]
    fn hoist_presentation_removes_the_key_and_returns_the_typed_value() {
        let p = aleph_protocol::Presentation::FileChanges { changes: vec![] };
        let mut v = serde_json::json!({"success": true, "_presentation": serde_json::to_value(&p).unwrap()});
        assert_eq!(hoist_presentation(&mut v), Some(p));
        assert!(v.get("_presentation").is_none());
        assert_eq!(v["success"], true);
    }

    #[test]
    fn hoist_presentation_is_none_for_strings_and_for_a_malformed_payload() {
        let mut s = serde_json::json!("plain text");
        assert!(hoist_presentation(&mut s).is_none());
        let mut bad = serde_json::json!({"_presentation": {"kind": "nope"}});
        assert!(hoist_presentation(&mut bad).is_none());
        assert!(bad.get("_presentation").is_none(), "a malformed payload is still removed from the model text");
    }
```

In `src/tools/scoped/tests.rs` (find the existing `apply_layer_two` / dispatcher test helper and add):

```rust
    #[tokio::test]
    async fn the_model_text_never_contains_the_presentation_the_metadata_carries() {
        // Sentinel inside a hunk: if it is searchable in `out.value`, the
        // side-channel leaked into the prompt (R9 / spec §4.3 guard #2).
        let sentinel = "PRESENTATION_SENTINEL_9f3a";
        let change = aleph_protocol::FileChange {
            path: "a.rs".into(), kind: aleph_protocol::FileChangeKind::Modified,
            hunks: vec![aleph_protocol::Hunk { old_start: 1, new_start: 1, lines: vec![
                aleph_protocol::HunkLine { tag: aleph_protocol::LineTag::Add, text: sentinel.into() }] }],
            added: 1, removed: 0, unavailable: None,
        };
        let value = serde_json::json!({"success": true, "message": "ok",
            "_presentation": serde_json::to_value(aleph_protocol::Presentation::FileChanges { changes: vec![change] }).unwrap()});
        let dispatcher = test_dispatcher().await; // the file's existing constructor helper
        let out = dispatcher.apply_layer_two("file_edit", crate::session::events::ToolOutput { value, metadata: Default::default() }, std::time::Instant::now() + std::time::Duration::from_secs(5)).await;
        let text = out.value.as_str().expect("layer two flattens to text");
        assert!(!text.contains(sentinel), "presentation leaked into model text: {text}");
        assert!(!text.contains("_presentation"));
        assert!(matches!(out.metadata.presentation, Some(aleph_protocol::Presentation::FileChanges { ref changes }) if changes.len() == 1));
    }
```

(If `apply_layer_two` is private to `dispatch.rs`, put the test in `dispatch.rs`'s own `#[cfg(test)]` module instead — same body.)

- [ ] **Step 5: Run**

Run: `CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib result_processing:: && CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib tools::scoped::`
Expected: green, including the sentinel test.

- [ ] **Step 6: Commit**

```bash
git add src/session/events.rs src/tools/result_processing.rs src/tools/scoped/
git commit -m "tools: hoist _presentation into ToolOutputMetadata before the model text is built

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 13: `plugins/diff-viewer` — record the CUT, do not delete here

`plugins/` is the **Aleph-plugins git submodule** (CLAUDE.md 📚 note), embedded into `aleph-server` by `include_dir!`. Its contents cannot be deleted from this repo; the CUT belongs to the sibling repo.

**Files:**
- Create: `docs/superpowers/notes/2026-09-06-diff-viewer-cut.md` (a 10-line note, committed here so the follow-up is not lost)

- [ ] **Step 1: Prove it has no consumer in this repo**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/cc-render-r1
grep -rn "diff-viewer\|diff_viewer\|DiffSummaryInput" --include=*.rs --include=*.toml --include=*.json --include=*.yml --include=*.yaml --include=justfile . | grep -v '^./plugins/' | grep -v target/
```

Expected: nothing. (If a bundled-plugin manifest or `src/bundled/` census names it, that line is the ONE place to remove it from once the submodule drops the directory — note it in the file below.)

- [ ] **Step 2: Write the note**

```markdown
# diff-viewer plugin — CUT pending in Aleph-plugins (2026-09-06)

`plugins/diff-viewer` (Extism, `DiffSummaryInput`) has zero references in
Aleph outside the submodule; the Panel never called it and the server-side
`FileChange` presentation (this round) supersedes it. Remove it in the
Aleph-plugins repo, bump the submodule here, then re-run the grep above.
Consumers found in this repo by the grep: <paste output or "none">.
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/notes/2026-09-06-diff-viewer-cut.md
git commit -m "docs: record the diff-viewer plugin CUT pending in the plugins submodule

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 14: `presentation_census` — derived from the live registry, not a name list

**Files:**
- Create: `src/tools/presentation_census.rs` (`#[cfg(test)]`-only module)
- Modify: `src/tools/mod.rs` (add `#[cfg(test)] mod presentation_census;`), `src/executor/builtin_registry/registry/inherent.rs` (a by-name dyn accessor if none exists)

**Interfaces:**
- Consumes: `BuiltinToolRegistry::with_config(BuiltinToolConfig { .. }).await` (`builder/tests.rs:22-29`), `registry.unified_tools()` (`inherent.rs:330`), `registry.execute_tool(name, json)` (`builder/tests.rs:246`), `AlephToolDyn::mutates_file_content` (Task 11), `IsolatedAlephHome::new()` (`src/utils/paths.rs:155`), `shared_ui_logic` is NOT a dependency of alephcore — the summarizer census (spec §5 guard #1) therefore lives here too, reading `DISPLAY_NAMES` via `include_str!` of `shared/ui_logic/src/transcript/summarize.rs` and the same source-scan discipline `frame_census.rs` uses.

- [ ] **Step 1: Find or add the dyn accessor**

`grep -n "pub fn\|pub(crate) fn" src/executor/builtin_registry/registry/inherent.rs`. If there is no `fn <name>(&self, name: &str) -> Option<&dyn AlephToolDyn>` (or an `Arc<dyn …>`), add:

```rust
    /// The registered tool object by name, for censuses that need a trait
    /// predicate (`mutates_file_content`) rather than a schema.
    pub(crate) fn tool_dyn(&self, name: &str) -> Option<&dyn crate::tools::traits::AlephToolDyn> {
        self.unified_tools().find(|t| t.name() == name).and_then(|t| t.as_aleph_tool_dyn())
    }
```

adapting to how `UnifiedTool` wraps the tool (read `struct_def.rs:28` and the `UnifiedTool` definition; if the wrapper is an enum with a builtin arm, match on it). The census below only needs `mutates_file_content()`.

- [ ] **Step 2: Write the census**

```rust
//! Two censuses that keep the presentation contract honest.
//!
//! 1. Every registered builtin whose `mutates_file_content()` is true must
//!    attach `_presentation` when run against a fixture — and every fixture
//!    must name a registered tool that answers true. A new mutating tool
//!    without a fixture is RED, a fixture for a tool that stopped mutating is
//!    RED. Derived from the registry, not from a list of names (判据 §3/§5).
//! 2. Every registered builtin either has a `DISPLAY_NAMES` entry in
//!    `shared-ui-logic::transcript::summarize` or is on the explicit
//!    `DELIBERATE_FALLBACK` list here — so a new tool shows up red until
//!    someone decides how its row reads.

use std::path::Path;

use serde_json::{json, Value};

/// (tool name, args builder run inside a tempdir) — one per content-mutating tool.
const PRESENTATION_FIXTURES: &[(&str, fn(&Path) -> Value)] = &[
    ("file_edit", |dir| {
        let p = dir.join("e.txt");
        std::fs::write(&p, "alpha\nbeta\n").unwrap();
        json!({"file_path": p.to_string_lossy(), "old_string": "beta", "new_string": "BETA"})
    }),
    ("file_write", |dir| json!({"file_path": dir.join("w.txt").to_string_lossy(), "content": "new\n"})),
    ("apply_patch", |dir| {
        let p = dir.join("p.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        json!({"patch": format!("*** Begin Patch\n*** Update File: {}\n@@\n-two\n+TWO\n*** End Patch", p.to_string_lossy())})
    }),
];

/// Registered builtins that DELIBERATELY read as humanised names (no
/// `DISPLAY_NAMES` entry). Adding a tool here is a decision, not a default.
const DELIBERATE_FALLBACK: &[&str] = &[
    // e.g. "agent_create", "team_create", … — fill from the first red run,
    // one line per tool, each with the reason it needs no Claude Code label.
];

async fn registry() -> crate::executor::builtin_registry::BuiltinToolRegistry {
    crate::executor::builtin_registry::BuiltinToolRegistry::with_config(
        crate::executor::builtin_registry::BuiltinToolConfig {
            injection_mode: crate::memory::MemoryInjectionMode::Hybrid,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn every_content_mutating_tool_attaches_a_presentation_and_every_fixture_names_one() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let reg = registry().await;
    let mutating: Vec<String> = reg
        .unified_tools()
        .filter(|t| reg.tool_dyn(t.name()).is_some_and(|d| d.mutates_file_content()))
        .map(|t| t.name().to_string())
        .collect();
    assert!(mutating.len() >= 3, "vacuity: expected file_edit/file_write/apply_patch to answer true, got {mutating:?}");

    let fixture_names: Vec<&str> = PRESENTATION_FIXTURES.iter().map(|(n, _)| *n).collect();
    let missing: Vec<&String> = mutating.iter().filter(|m| !fixture_names.contains(&m.as_str())).collect();
    assert!(missing.is_empty(), "content-mutating tools with no presentation fixture (add one here): {missing:?}");
    let stale: Vec<&&str> = fixture_names.iter().filter(|f| !mutating.contains(&(**f).to_string())).collect();
    assert!(stale.is_empty(), "fixtures naming tools that no longer mutate content: {stale:?}");

    for (name, build) in PRESENTATION_FIXTURES {
        let dir = tempfile::tempdir().unwrap();
        let args = build(dir.path());
        let out = reg.execute_tool(name, args).await.unwrap_or_else(|e| panic!("{name} fixture failed: {e}"));
        let p = out.get(aleph_protocol::PRESENTATION_KEY)
            .unwrap_or_else(|| panic!("{name} output carries no `_presentation`: {out}"));
        let parsed: aleph_protocol::Presentation = serde_json::from_value(p.clone())
            .unwrap_or_else(|e| panic!("{name} `_presentation` is not a Presentation: {e}"));
        let aleph_protocol::Presentation::FileChanges { changes } = parsed;
        assert!(!changes.is_empty(), "{name} attached an empty change list");
    }
}

#[tokio::test]
async fn every_registered_tool_has_a_row_label_or_a_deliberate_fallback() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let reg = registry().await;
    let src = include_str!("../../shared/ui_logic/src/transcript/summarize.rs");
    // Scrape `("name", "Label")` pairs out of DISPLAY_NAMES — same source-scan
    // discipline as frame_census.rs; the table is `&[(&str, &str)]`.
    let table = src.split("pub const DISPLAY_NAMES").nth(1).and_then(|s| s.split("];").next()).expect("DISPLAY_NAMES table");
    let labelled: Vec<&str> = table.split('(').skip(1).filter_map(|e| e.split('"').nth(1)).collect();
    assert!(labelled.len() >= 10, "vacuity: DISPLAY_NAMES scrape found {labelled:?}");

    let registered: Vec<String> = reg.unified_tools().map(|t| t.name().to_string()).collect();
    let unlabelled: Vec<&String> = registered.iter()
        .filter(|n| !labelled.contains(&n.as_str()) && !DELIBERATE_FALLBACK.contains(&n.as_str()))
        .collect();
    assert!(unlabelled.is_empty(), "registered tools with neither a DISPLAY_NAMES label nor a DELIBERATE_FALLBACK ruling: {unlabelled:?}");
    let ghosts: Vec<&&str> = labelled.iter().filter(|l| !registered.contains(&(**l).to_string())).collect();
    assert!(ghosts.is_empty(), "DISPLAY_NAMES labels tools that are not registered (a lie on every row): {ghosts:?}");
}
```

- [ ] **Step 3: Run, then fill `DELIBERATE_FALLBACK` from the first red list**

Run: `CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib presentation_census`
The second test WILL be red the first time with the list of unlabelled tools. For each: either add a `DISPLAY_NAMES` entry in `summarize.rs` (if the tool deserves a Claude-Code-style label, e.g. the shell tool's real name) or add it to `DELIBERATE_FALLBACK` with a one-line reason. Re-run until green. Also fix any `ghosts` (labels for unregistered names — e.g. if the shell tool is `shell` not `shell_exec`).

- [ ] **Step 4: Commit**

```bash
git add src/tools/presentation_census.rs src/tools/mod.rs src/executor/builtin_registry/registry/inherent.rs shared/ui_logic/src/transcript/summarize.rs
git commit -m "tools: presentation and row-label censuses derived from the live registry

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 15: Carry the presentation to the live `tool_end` frame (harness callback → orchestrator → drain)

**Files:**
- Modify: `src/harness/callback.rs:36-44` (trait method signature), `src/harness/agent/act.rs:552,635,963` (three success call sites), `src/harness/tests/guardrails.rs:279`, `src/harness/tests/act.rs:1455`, `src/harness/callback.rs:98` (test callbacks — param type only), `src/orchestrator/dispatch.rs:58-66` (`FlowStreamEvent::ToolCallDone`), `src/orchestrator/harness_bridge/callback.rs:19,163-176` (`BroadcastCallback`), `src/gateway/execution_engine/event_drain.rs:136-185`, `src/gateway/event_emitter/types.rs:410-437` (gateway `ToolResult` twin)

**Interfaces:**
- Produces: `fn on_tool_call_done(&mut self, id: &str, result: Option<&crate::session::events::ToolOutput>, error: Option<&str>, duration_ms: u64)`; `FlowStreamEvent::ToolCallDone { id, result: Option<Value>, error, duration_ms, presentation: Option<aleph_protocol::Presentation> }`; wire `stream.tool_end` `result.presentation` populated.
- R10: the harness delta is intended to be **zero budgeted lines** (a type change on one signature line and `&output.value` → `&output` on three lines). Measure with `cargo test -p alephcore --lib harness::tests::budget` — if it reports growth, the change was not made as specified.

- [ ] **Step 1: The trait (`src/harness/callback.rs:36-44`)**

Change only the parameter type and its doc:

```rust
    /// Invoked when a tool call finishes. `result` and `error` are mutually
    /// exclusive. `duration_ms` is the tool's measured wall-clock execution
    /// time (0 for a within-batch memo hit, which re-executes nothing).
    /// `result` is the whole `ToolOutput` — value AND metadata — so the
    /// broadcast side can forward out-of-band presentation without the loop
    /// knowing what a UI is.
    fn on_tool_call_done(
        &mut self,
        _id: &str,
        _result: Option<&crate::session::events::ToolOutput>,
        _error: Option<&str>,
        _duration_ms: u64,
    ) {
    }
```

- [ ] **Step 2: The three success call sites in `act.rs`**

`:552` (memo hit): `callback.on_tool_call_done(&call.id, Some(&output_value), None, 0);` → `callback.on_tool_call_done(&call.id, Some(&output), None, 0);` — `output` is the cloned `ToolOutput` two lines above; keep `output_value` for the trace event below it.
`:635` (serial): `Some(&output.value)` → `Some(&output)`.
`:963` (parallel PASS 1): `Some(&output.value)` → `Some(&output)`.
Error sites (`:969`, `:1204`, `guardrails.rs:138`) pass `None` — unchanged.
Test callbacks that override the method (`callback.rs:98`, `tests/guardrails.rs:279`, `tests/act.rs:1455`): change the parameter type; if a test asserted on `result.unwrap()["…"]`, index through `.value` instead.

- [ ] **Step 3: Orchestrator**

`src/orchestrator/dispatch.rs:58-66` — add the field:

```rust
    ToolCallDone {
        id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
        duration_ms: u64,
        /// UI side-channel hoisted out of the tool's JSON (`ToolOutputMetadata::presentation`).
        presentation: Option<aleph_protocol::Presentation>,
    },
```

`src/orchestrator/harness_bridge/callback.rs:163-176`:

```rust
    fn on_tool_call_done(
        &mut self,
        id: &str,
        result: Option<&crate::session::events::ToolOutput>,
        error: Option<&str>,
        duration_ms: u64,
    ) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallDone {
            id: id.to_string(),
            result: result.map(|o| o.value.clone()),
            error: error.map(|s| s.to_string()),
            duration_ms,
            presentation: result.and_then(|o| o.metadata.presentation.clone()),
        });
    }
```

Update the contract doc line at `:19` to name `presentation`. Grep `ToolCallDone {` across `src/` for other pattern matches (tests, `orchestrator/**`) and add `presentation` / `..` as needed.

- [ ] **Step 4: Gateway — the twin and the drain**

`src/gateway/event_emitter/types.rs:410-437`: delete the gateway `ToolResult` struct + impl and replace with `pub use aleph_protocol::ToolResult;` (byte-identical twins, and the protocol one now carries `presentation`). `frame.rs:9` imports `ToolResult` from `event_emitter` — the re-export keeps it compiling. Run `cargo check -p alephcore`; fix any struct-literal construction (`ToolResult { success, output, error, metadata }`) by adding `presentation: None`.

`event_drain.rs:136-185` — destructure the new field and set it:

```rust
        FlowStreamEvent::ToolCallDone { id, result, error, duration_ms, presentation } => {
            // … existing plan-latch block unchanged …
            let tool_result = if let Some(err) = error {
                crate::gateway::event_emitter::ToolResult::error(err)
            } else {
                let output = result.map(|v| match v { serde_json::Value::String(text) => text, other => other.to_string() }).unwrap_or_default();
                crate::gateway::event_emitter::ToolResult::success(output).with_presentation(presentation)
            };
```

Also `src/gateway/event_emitter/redacting.rs:156-175` rebuilds a `ToolEnd` — carry `result.presentation` through (redaction must not drop it; file paths inside hunks are not secrets, but the redactor's own text pass may run over `output` only — leave `presentation` untouched and say so in a comment).

- [ ] **Step 5: Tests**

`src/gateway/execution_engine/event_drain.rs` tests (or the drain's existing test module): feed `FlowStreamEvent::ToolCallDone { presentation: Some(Presentation::FileChanges { changes: vec![…] }), .. }` through the drain with a capturing emitter; assert the emitted `StreamEvent::ToolEnd.result.presentation` is `Some` and `result.output` is the plain text (not re-encoded). Add a protocol-side wire test in `shared/protocol/src/events.rs`: serialize a `ToolResult` with a presentation, assert `["presentation"]["kind"] == "file_changes"`.

`src/harness/tests/budget.rs` — run `cargo test -p alephcore --lib harness::tests::budget::the_harness_line_budget_does_not_grow`. Expected: green with CEILING unchanged (5239). If red, the delta exceeded zero — re-check Steps 1–2 (no new lines; rustfmt may wrap a call — shorten a variable name rather than raise CEILING).

- [ ] **Step 6: Run**

`CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness:: && … --lib gateway::execution_engine::event_drain && … --lib orchestrator::harness_bridge`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/harness/callback.rs src/harness/agent/act.rs src/harness/tests/ src/orchestrator/ src/gateway/execution_engine/event_drain.rs src/gateway/event_emitter/ shared/protocol/src/events.rs
git commit -m "gateway: carry the tool presentation side-channel to the live tool_end frame

HarnessCallback::on_tool_call_done now receives the whole ToolOutput (same
line count in src/harness/, ratchet unchanged); BroadcastCallback forwards
metadata.presentation on FlowStreamEvent::ToolCallDone; the drain sets
ToolResult.presentation. The gateway ToolResult twin becomes a re-export.

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 16: Replay — `trace.by_runs` enriches `tool_call_completed` with the persisted presentation

**Files:**
- Modify: `src/gateway/handlers/trace_replay.rs:123-193` (`handle_by_runs`), its tests `:551-701`

**Interfaces:**
- Consumes: `crate::session::store::global_session_event_store() -> Option<Arc<dyn SessionEventStore>>`, `SessionEventStore::load_all_events(&SessionKey)`, `SessionEvent::ToolResult { call_id, output, .. }` (`src/session/events.rs:385`), `AgentTraceEvent::ToolCallCompleted { call: AgentTraceToolCallEnd, .. }`.
- Produces: `trace.by_runs` responses whose `tool_call_completed` events carry `call.presentation` when the event log has one for that `tool_id`.
- Why here and not in the harness trace event: the trace plane (`LoopTraceEvent` → `TracePersistence`) is R10-budgeted and would need `aleph_protocol` in `src/harness/trace.rs`; the event log already persists `ToolOutputMetadata.presentation` (Task 12) under the same id. One read per replay, no new plumbing.

- [ ] **Step 1: The enrichment**

In `handle_by_runs`, after the visibility gate and before the `runs` loop:

```rust
    // Presentations (file diffs) live in the session EVENT log, not in the
    // trace rows — hoisted there by `apply_layer_two`. Index them once per
    // request by call id so a replayed `tool_call_completed` carries the same
    // side-channel the live `tool_end` did. `None` on either absence (no store
    // published, read failed) = "we did not find out": replay without diffs,
    // never a fabricated one.
    let presentations: std::collections::HashMap<String, aleph_protocol::Presentation> =
        match crate::session::store::global_session_event_store() {
            Some(store) => match store.load_all_events(&session_key).await {
                Ok(events) => events
                    .into_iter()
                    .filter_map(|rec| match rec.event {
                        crate::session::events::SessionEvent::ToolResult { call_id, output, .. } => {
                            output.metadata.presentation.map(|p| (call_id, p))
                        }
                        _ => None,
                    })
                    .collect(),
                Err(e) => {
                    tracing::warn!(session_key = %key_str, error = %e, "trace.by_runs: event log read failed; replaying without presentations");
                    Default::default()
                }
            },
            None => Default::default(),
        };
```

(Confirm the record field names against `SessionEventRecord` in `src/session/store.rs` — the dossier shows `load_all_events -> Vec<SessionEventRecord>`; use whatever field holds the `SessionEvent`.)

In the loop, replace `traces.into_iter().map(|t| serde_json::to_value(&t.event)…)` with:

```rust
                    .map(|t| {
                        let mut event = t.event;
                        if let aleph_protocol::AgentTraceEvent::ToolCallCompleted { call, .. } = &mut event {
                            if call.presentation.is_none() {
                                call.presentation = presentations.get(&call.tool_id).cloned();
                            }
                        }
                        serde_json::to_value(&event).unwrap_or(Value::Null)
                    })
```

- [ ] **Step 2: Test (in `trace_replay.rs` tests, reusing `seed_run` / `seed_session` / `session_store`)**

Seed a run whose second trace is `AgentTraceEvent::ToolCallCompleted { iteration: 0, call: AgentTraceToolCallEnd { tool_id: "c1".into(), tool_name: "file_edit".into(), input: json!({}), duration_ms: 3, presentation: None }, result: AgentTraceToolResult::Success { output: json!("ok") } }`. Install a session event store for the test: build `SqliteEventStore::new(conn)` after `migrate_add_session_events(&conn)` (pattern at `src/session/tool_trace.rs:43-48`), append a `SessionEvent::ToolResult { call_id: "c1", output: ToolOutput { value: json!("ok"), metadata: ToolOutputMetadata { presentation: Some(Presentation::FileChanges { changes: vec![FileChange::unavailable("a.rs", FileChangeKind::Modified, Unavailable::TooLarge)] }), ..Default::default() } }, .. }` under the same `SessionKey`, and install it with `set_global_session_event_store`. Because the slot is process-global and install-once, this test MUST hold `crate::utils::paths::ALEPH_HOME_TEST_GUARD`-style exclusivity: use a dedicated `static BY_RUNS_STORE_GUARD: std::sync::Mutex<()>` in the test module and accept that a second test cannot install a different store (assert on `global_session_event_store().is_some()` and skip installing if already present, reusing it).

Assert: the replayed `tool_call_completed` event has `call.presentation.kind == "file_changes"`; a `tool_call_completed` for an unknown id stays `presentation: null`/absent.

- [ ] **Step 3: Run**

`CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib trace_replay`
Expected: green including the existing visibility tests.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/trace_replay.rs
git commit -m "gateway: trace.by_runs replays the persisted tool presentation from the event log

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 17: `trace.tool_output` — the untruncated tool output, paged, fail-closed

**Files:**
- Create: `src/gateway/handlers/tool_output.rs`
- Modify: `src/gateway/handlers/mod.rs` (`pub mod tool_output;` + a phase-1 placeholder like `session.usage`'s at `:479-485`), `src/tools/result_store.rs` (add `read_blob`), `src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs:68-81` (register next to `trace.by_runs`), `src/gateway/method_census.rs:468` (`("trace.tool_output", Class::Open)`), `src/gateway/method_visibility.rs:615` (`("trace.tool_output", Treatment::KeyChecked)`), `src/gateway/method_admin.rs:362` (`"trace.tool_output",` in `MEMBER_CARVE_OUTS` — the `trace.` prefix is admin-gated)

**Interfaces:**
- Produces: RPC `trace.tool_output { session_key, tool_call_id, offset?: u64, limit?: u64 }` → `aleph_protocol::ToolOutputPage` (Task 3); `ToolResultStore::read_blob(&self, path: &Path) -> std::io::Result<String>` (refuses paths outside the store root); `pub const MAX_TOOL_OUTPUT_PAGE_BYTES: u64 = 2 * 1024 * 1024`.
- Consumes: `global_session_event_store()`, `SessionEvent::ToolResult`, `crate::tools::result_store::{extract_persisted_ref, global_tool_result_store}`, `visibility::{session_visible, not_found_response}`.

- [ ] **Step 1: `ToolResultStore::read_blob` (`src/tools/result_store.rs`, next to `persist_if_large`)**

```rust
    /// Read a blob this store wrote. The path comes from a marker inside
    /// persisted tool text — server-written, but still DATA: refuse anything
    /// that does not resolve under this store's root.
    pub fn read_blob(&self, path: &std::path::Path) -> std::io::Result<String> {
        let root = self.root_dir().canonicalize()?;
        let target = path.canonicalize()?;
        if !target.starts_with(&root) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "blob path outside the tool_results root"));
        }
        std::fs::read_to_string(target)
    }
```

(`root_dir()` — the store's base dir accessor; if only `blob_dir()` exists, add `pub(crate) fn root_dir(&self) -> PathBuf` returning the `tool_results/<root>` directory the constructor computed.)

- [ ] **Step 2: The handler**

```rust
//! `trace.tool_output` — the full text of one tool result, paged.
//!
//! The wire `tool_end` carries the MODEL-facing copy: `apply_layer_two`
//! replaces the value with budgeted text (8 000 tokens) and offloads the
//! original to a blob only when it overflowed. So: within budget → the event
//! log's text IS the whole output (`Inline`); over budget → follow the
//! `[Full output persisted: <path> …]` marker to the blob (`Persisted`); blob
//! swept (7-day TTL) → the budgeted text is all that survives, and the page
//! says so (`Expired`, `truncated: true`) instead of pretending.

use std::sync::Arc;

use aleph_protocol::{ToolOutputPage, ToolOutputSource};
use serde::Deserialize;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, RESOURCE_NOT_FOUND, SERVICE_UNAVAILABLE};
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::routing::session_key::SessionKey;

pub const MAX_TOOL_OUTPUT_PAGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
struct Params {
    #[serde(default)] session_key: Option<String>,
    #[serde(default)] tool_call_id: Option<String>,
    #[serde(default)] offset: u64,
    #[serde(default)] limit: Option<u64>,
}

/// Cut `[offset, offset+limit)` on char boundaries.
fn page(text: &str, offset: u64, limit: u64) -> (String, bool) {
    let total = text.len() as u64;
    let start = (offset.min(total)) as usize;
    let mut start = start; while !text.is_char_boundary(start) { start -= 1; }
    let mut end = ((offset.saturating_add(limit)).min(total)) as usize;
    while !text.is_char_boundary(end) { end -= 1; }
    (text[start..end].to_string(), (end as u64) < total)
}

pub async fn handle_tool_output(request: JsonRpcRequest, sessions: Arc<dyn SessionStore>) -> JsonRpcResponse {
    let params: Params = match request.params.as_ref().map(|v| serde_json::from_value(v.clone())) {
        Some(Ok(p)) => p,
        Some(Err(_)) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params"),
        None => Params::default(),
    };
    let Some(key_str) = params.session_key.as_deref().filter(|s| !s.is_empty()) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
    };
    let Some(session_key) = SessionKey::from_key_string(key_str) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key");
    };
    let Some(call_id) = params.tool_call_id.as_deref().filter(|s| !s.is_empty()) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing tool_call_id");
    };
    match sessions.get_metadata(&session_key).await {
        Ok(Some(meta)) if visibility::session_visible(&meta) => {}
        _ => return visibility::not_found_response(request.id),
    }
    let Some(store) = crate::session::store::global_session_event_store() else {
        return JsonRpcResponse::error(request.id, SERVICE_UNAVAILABLE, "session event log not available");
    };
    let events = match store.load_all_events(&session_key).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(session_key = %key_str, error = %e, "trace.tool_output: event log read failed");
            return JsonRpcResponse::error(request.id, SERVICE_UNAVAILABLE, "session event log unreadable");
        }
    };
    let Some(text) = events.into_iter().find_map(|rec| match rec.event {
        crate::session::events::SessionEvent::ToolResult { call_id: id, output, .. } if id == call_id => Some(match output.value {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        }),
        _ => None,
    }) else {
        return JsonRpcResponse::error(request.id, RESOURCE_NOT_FOUND, "no tool result with that id in this session");
    };

    let (full, source) = match crate::tools::result_store::extract_persisted_ref(&text) {
        None => (text, ToolOutputSource::Inline),
        Some(path) => match crate::tools::result_store::global_tool_result_store()
            .and_then(|s| s.read_blob(std::path::Path::new(path)).ok())
        {
            Some(blob) => (blob, ToolOutputSource::Persisted),
            None => (text, ToolOutputSource::Expired),
        },
    };
    let limit = params.limit.unwrap_or(MAX_TOOL_OUTPUT_PAGE_BYTES).min(MAX_TOOL_OUTPUT_PAGE_BYTES);
    let (slice, more) = page(&full, params.offset, limit);
    let out = ToolOutputPage {
        tool_call_id: call_id.to_string(),
        offset: params.offset,
        total_bytes: full.len() as u64,
        truncated: more || matches!(source, ToolOutputSource::Expired),
        source,
        text: slice,
    };
    JsonRpcResponse::success(request.id, serde_json::to_value(out).unwrap_or(json!(null)))
}
```

- [ ] **Step 3: Register + census tables**

`common_handlers.rs` (next to `trace.by_runs`, same store):

```rust
        let tool_output_sessions = session_store.clone();
        server.handlers_mut().register("trace.tool_output", move |req| {
            let sessions = tool_output_sessions.clone();
            async move { alephcore::gateway::handlers::tool_output::handle_tool_output(req, sessions).await }
        });
```

`method_census.rs`: `("trace.tool_output", Class::Open),` beside `trace.by_runs`. `method_visibility.rs`: `("trace.tool_output", Treatment::KeyChecked),`. `method_admin.rs` `MEMBER_CARVE_OUTS`: `"trace.tool_output",`. Run `cargo test -p alephcore --lib method_census method_visibility method_admin` — the census tests are the checklist; each red names the table you missed.

- [ ] **Step 4: Tests (in `tool_output.rs`)**

Reuse `trace_replay.rs`'s helpers (copy `req`, `session_store`, `seed_session`; or move them to a `#[cfg(test)] pub(crate) mod test_support` in `trace_replay.rs` and import). Cases:
1. `missing id → RESOURCE_NOT_FOUND` (an `Err`, never an empty page).
2. Inline: seed a `ToolResult` event with `value: json!("hello world")`; `offset: 6, limit: 5` → `text == "world"`, `truncated == false`, `source == Inline`, `total_bytes == 11`.
3. Persisted: write a blob via `ToolResultStore::new("test-root")` + `persist_if_large("c1","grep", <9000 'x'>, 1)` (threshold 1 token forces persistence), install with `set_global_tool_result_store`, seed the event's value as the returned marker; assert `source == Persisted` and `total_bytes == 9000`.
4. Expired: seed a marker pointing at a path under the store root that does not exist → `source == Expired`, `truncated == true`, `text` is the marker text.
5. A marker pointing OUTSIDE the root (e.g. `C:\Windows\win.ini` / `/etc/hosts`) → `Expired` (read refused), never the file's contents.
6. Foreign session → `not_found_response` (same as by_runs).
The global slots are install-once: guard with the same module-level mutex discipline Task 16 uses.

- [ ] **Step 5: Run + commit**

`CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib tool_output && … --lib method_`

```bash
git add src/gateway/handlers/tool_output.rs src/gateway/handlers/mod.rs src/tools/result_store.rs src/gateway/method_census.rs src/gateway/method_visibility.rs src/gateway/method_admin.rs src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs
git commit -m "gateway: add trace.tool_output — paged untruncated tool output, fail-closed on expiry

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

### Task 18: `context.breakdown` — measure the real prompt once per turn, serve it per session

**Files:**
- Create: `src/thinker/prompt_size_registry.rs`, `src/gateway/handlers/context_breakdown.rs`
- Modify: `src/thinker/mod.rs` (register the module), `src/thinker/prompt_pipeline.rs:93-127` (measured variants), `src/thinker/prompt_builder/cache.rs:32-60,110-130` (use them; replace the double render in `maybe_trace_prompt_size`), `src/thinker/prompt_builder/mod.rs:387-436` (`maybe_trace_prompt_size` takes the measured `Vec<LayerSize>`), `src/orchestrator/harness_bridge/prompt_build.rs:764-770` (record), the place in `src/orchestrator/harness_bridge/**` where the turn's `Vec<ToolDefinition>` is assembled for `with_tools` (record schema bytes), `src/bin/aleph-server/commands/start/mod.rs:456-478` (install the slot at boot), `src/capability/mod.rs:364-366` (roster), `src/gateway/handlers/mod.rs`, `src/bin/aleph-server/commands/start/builder/handlers/session.rs:52-58` (register beside `session.usage`), `src/gateway/method_census.rs:406`, `src/gateway/method_visibility.rs:536`

**Interfaces:**
- Produces: `PromptPipeline::execute_stable_with_mode_measured(path, input, mode) -> (String, Vec<LayerSize>)` and `execute_dynamic_with_mode_measured(...)`; `PromptBuilder::build_system_prompt_cached_with_mode_measured(&[ToolInfo], PromptMode) -> (Vec<SystemPromptPart>, Vec<LayerSize>)` (the existing fn delegates and drops the sizes); `pub struct PromptSizeRecord { turn: u64, layers: Vec<LayerSize>, tools: Vec<(String, u64, u64)>, recorded_at_ms: i64 }`; `pub struct PromptSizeRegistry` (bounded, latest record per session key, `MAX_TRACKED_SESSIONS = 256`) with `record_layers(session_key: &str, layers)`, `record_tools(session_key: &str, tools)`, `latest(session_key) -> Option<PromptSizeRecord>`; process-global `global_prompt_size_registry() -> Option<Arc<PromptSizeRegistry>>` / `set_global_prompt_size_registry` / `decline_…` / `global_prompt_size_registry_slot()`; RPC `context.breakdown { session_key }` → `aleph_protocol::ContextBreakdown`.
- Facts that shape this (from the dossiers): production passes `&[]` tools to the pipeline (schemas travel as native `tool_use`, never as prompt text), so tool bytes are measured where the request's tool list is built, NOT in a layer; `SessionMetadata` has `model` / `model_provider` but no `context_window` — the handler resolves it via `crate::providers::model_catalog::resolve_context_window_with_override(config_override, model)`; `provider_reported` is left `None` — clients reconcile with the live `ContextGauge` they already receive (spec §5 `context` row).

- [ ] **Step 1: Measured pipeline variants (`prompt_pipeline.rs`, next to `:93-127`)**

```rust
    /// [`Self::execute_stable_with_mode`] that also returns each layer's size.
    /// One pass: the section is rendered once into a scratch buffer, measured,
    /// then appended — so the bytes are exactly the bytes sent.
    pub fn execute_stable_with_mode_measured(&self, path: AssemblyPath, input: &LayerInput, mode: PromptMode) -> (String, Vec<LayerSize>) {
        self.execute_filtered_measured(path, input, mode, Some(LayerStability::Stable), 16384)
    }
    pub fn execute_dynamic_with_mode_measured(&self, path: AssemblyPath, input: &LayerInput, mode: PromptMode) -> (String, Vec<LayerSize>) {
        self.execute_filtered_measured(path, input, mode, Some(LayerStability::Dynamic), 4096)
    }
    fn execute_filtered_measured(&self, path: AssemblyPath, input: &LayerInput, mode: PromptMode, stability: Option<LayerStability>, cap: usize) -> (String, Vec<LayerSize>) {
        let mut output = String::with_capacity(cap);
        let mut sizes = Vec::new();
        let mut section = String::new();
        for layer in &self.layers {
            if !layer.paths().contains(&path) || !layer.supports_mode(mode) { continue; }
            if stability.is_some_and(|s| layer.stability() != s) { continue; }
            section.clear();
            layer.inject(&mut section, input);
            if section.is_empty() { continue; }
            sizes.push(LayerSize { priority: layer.priority(), name: layer.name(), stability: layer.stability(),
                chars: section.chars().count(), bytes: section.len(), tokens: estimate_tokens_aware(&section, DEFAULT_PROSE_RATIO) });
            output.push_str(&section);
        }
        (output, sizes)
    }
```

Test: for a fixed `LayerInput`, `execute_stable_with_mode_measured(...).0 == execute_stable_with_mode(...)` byte-for-byte, and `sizes.iter().map(|l| l.bytes).sum::<usize>() == output.len()`.

- [ ] **Step 2: `cache.rs` — measured build; `maybe_trace_prompt_size` stops re-rendering**

Add `build_system_prompt_cached_with_mode_measured` producing the same `Vec<SystemPromptPart>` as today plus `stable_sizes ++ dynamic_sizes`; make `build_system_prompt_cached_with_mode` call it and drop the sizes. Change `maybe_trace_prompt_size(pipeline, path, input, mode)` to `maybe_trace_prompt_size(path, sizes: &[LayerSize])` (log only; env gate unchanged) and call it with the measured vector from both entry points (`cache.rs:40`, `:113` — the Basic path can keep `layer_breakdown` or gain a measured twin; keep the Basic one minimal: call the new `execute_*_measured` there too so both paths share the fn).

- [ ] **Step 3: The registry (`src/thinker/prompt_size_registry.rs`)**

```rust
//! Latest measured prompt layout per session, for `context.breakdown`.
//! Written at the moment the bytes are produced (never re-derived later —
//! see the LRU that was removed at prompt_build.rs:753-763 for why).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::thinker::prompt_pipeline::LayerSize;

pub const MAX_TRACKED_SESSIONS: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct PromptSizeRecord {
    pub turn: u64,
    pub layers: Vec<LayerSize>,
    /// (tool name, schema bytes, description bytes)
    pub tools: Vec<(String, u64, u64)>,
    pub recorded_at_ms: i64,
}

#[derive(Default)]
pub struct PromptSizeRegistry {
    inner: Mutex<HashMap<String, PromptSizeRecord>>,
}

impl PromptSizeRegistry {
    fn with<R>(&self, key: &str, f: impl FnOnce(&mut PromptSizeRecord) -> R) -> R {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !map.contains_key(key) && map.len() >= MAX_TRACKED_SESSIONS {
            // Evict the stalest session; bounded memory for a long-lived daemon.
            if let Some(oldest) = map.iter().min_by_key(|(_, r)| r.recorded_at_ms).map(|(k, _)| k.clone()) { map.remove(&oldest); }
        }
        let rec = map.entry(key.to_string()).or_default();
        rec.recorded_at_ms = chrono::Utc::now().timestamp_millis();
        f(rec)
    }
    pub fn record_layers(&self, session_key: &str, layers: Vec<LayerSize>) {
        self.with(session_key, |r| { r.turn += 1; r.layers = layers; });
    }
    pub fn record_tools(&self, session_key: &str, tools: Vec<(String, u64, u64)>) {
        self.with(session_key, |r| r.tools = tools);
    }
    #[must_use]
    pub fn latest(&self, session_key: &str) -> Option<PromptSizeRecord> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).get(session_key).cloned()
    }
}

static GLOBAL: CapabilitySlot<Arc<PromptSizeRegistry>> = CapabilitySlot::new("thinker/prompt-size-registry", MissingSemantics::ConsumerDecides);
pub fn set_global_prompt_size_registry(r: Arc<PromptSizeRegistry>) { let _ = GLOBAL.install(r); }
pub fn decline_global_prompt_size_registry(because: &'static str) { GLOBAL.decline(because); }
#[inline] pub fn global_prompt_size_registry() -> Option<Arc<PromptSizeRegistry>> { GLOBAL.get().cloned() }
pub(crate) const fn global_prompt_size_registry_slot() -> &'static dyn SlotStatus { &GLOBAL }
```

Add it to the roster in `src/capability/mod.rs:364-366` (the `every_installed_global_is_a_capability_slot` census is already red at baseline — do not make it worse; the roster line is what that census wants). Install at boot in `start/mod.rs` beside the event-store install: `set_global_prompt_size_registry(Arc::new(PromptSizeRegistry::default()))` — this one cannot fail, so no `decline` arm is reachable; still call `decline` in any early-return path that skips installs, mirroring the siblings.

Tests: `record_layers` bumps `turn`; eviction keeps the newest 256; `latest` on an unknown key is `None`.

- [ ] **Step 4: Record at the two production sites**

`prompt_build.rs:764` — replace `let parts = builder.build_system_prompt_cached_with_mode(&[], self.default_prompt_mode);` with the measured call and:

```rust
        let (parts, layer_sizes) = builder.build_system_prompt_cached_with_mode_measured(&[], self.default_prompt_mode);
        if let Some(reg) = crate::thinker::prompt_size_registry::global_prompt_size_registry() {
            reg.record_layers(&session_key_str, layer_sizes);
        }
```

Tool bytes: find where this runner builds the request's tool definitions (`grep -n "with_tools\|tool_definitions\|Vec<ToolDefinition>" src/orchestrator/harness_bridge/*.rs`), and right after that Vec is final:

```rust
        if let Some(reg) = crate::thinker::prompt_size_registry::global_prompt_size_registry() {
            let sizes = tools.iter().map(|t| (t.name.clone(),
                serde_json::to_string(&t.parameters).map_or(0, |s| s.len() as u64),
                t.description.len() as u64)).collect();
            reg.record_tools(&session_key_str, sizes);
        }
```

(If the session key is not in scope there, thread the string in — it is a `String`, not a new dependency.)

- [ ] **Step 5: The handler (`src/gateway/handlers/context_breakdown.rs`)**

```rust
//! `context.breakdown` — the measured layout of the LAST prompt this session
//! sent. Constructed from `aleph_protocol::ContextBreakdown`; never `json!`.

use std::sync::Arc;
use aleph_protocol::{ContextBreakdown, LayerSizeView, ToolSchemaSize};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, RESOURCE_NOT_FOUND};
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::routing::session_key::SessionKey;

pub async fn handle_context_breakdown(
    request: JsonRpcRequest,
    sessions: Arc<dyn SessionStore>,
    app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
) -> JsonRpcResponse {
    let key_str = match request.params.as_ref().and_then(|p| p.get("session_key")).and_then(|v| v.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };
    let Some(session_key) = SessionKey::from_key_string(&key_str) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format");
    };
    let meta = match sessions.get_metadata(&session_key).await {
        Ok(Some(m)) if visibility::session_visible(&m) => m,
        _ => return visibility::not_found_response(request.id),
    };
    let Some(record) = crate::thinker::prompt_size_registry::global_prompt_size_registry().and_then(|r| r.latest(&key_str)) else {
        // Unknown ≠ empty: no turn has been measured for this session (or the
        // daemon restarted). The client renders "not measured yet", not zeros.
        return JsonRpcResponse::error(request.id, RESOURCE_NOT_FOUND, "no measured prompt for this session yet");
    };
    let context_window = match meta.model.as_deref() {
        Some(model) => {
            let override_w = match (&app_config, meta.model_provider.as_deref()) {
                (Some(cfg), Some(provider)) => cfg.read().await.providers.get(provider).and_then(|p| p.context_window),
                _ => None,
            };
            Some(crate::providers::model_catalog::resolve_context_window_with_override(override_w, model))
        }
        None => None,
    };
    let out = ContextBreakdown {
        session_key: key_str,
        turn: record.turn,
        layers: record.layers.iter().map(|l| LayerSizeView {
            name: l.name.to_string(), bytes: l.bytes as u64, tokens: l.tokens as u64,
            zone: if l.stability == crate::thinker::prompt_layer::LayerStability::Stable { "stable" } else { "dynamic" }.to_string(),
        }).collect(),
        tools: record.tools.iter().map(|(n, s, d)| ToolSchemaSize { name: n.clone(), schema_bytes: *s, description_bytes: *d }).collect(),
        messages_tokens: None,
        provider_reported: None,
        context_window,
    };
    JsonRpcResponse::success(request.id, serde_json::to_value(out).unwrap_or(serde_json::Value::Null))
}
```

(Adjust the `LayerStability` path and the `providers` config accessor to the real names — `src/config/types/provider.rs:149` holds `context_window`; `engine.rs:78` shows `app_config: Option<Arc<RwLock<crate::Config>>>` as the live handle to pass in.)

- [ ] **Step 6: Register + tables**

`builder/handlers/session.rs` beside `session.usage`: `register_handler!(server, "context.breakdown", context_breakdown::handle_context_breakdown, session_store, app_config)` (use the 2-arg macro arm; if `app_config` is not in that builder's scope, pass `None` and note it — the catalogue fallback still answers). Phase-1 placeholder in `handlers/mod.rs` like `session.usage`'s. `method_census.rs`: `("context.breakdown", Class::Open)`. `method_visibility.rs`: `("context.breakdown", Treatment::KeyChecked)`. Run the three census tests.

- [ ] **Step 7: Tests**

Handler: (a) no record → `RESOURCE_NOT_FOUND`; (b) after `record_layers` + `record_tools` for the key, the response deserializes into `aleph_protocol::ContextBreakdown` with the same layer names/bytes, `tools.len()` matching, `provider_reported == None`, and `context_window == Some(resolve_context_window("<the seeded model>"))`; (c) foreign session → not-found. Contract-key guard in the style of `the_trace_list_response_has_exactly_the_contract_keys`: the response's top-level keys == the struct's serialized keys.

Prompt bytes unchanged: `cargo test -p alephcore --lib thinker::prompt_contract` must stay green (the stable-prefix byte-identity tests are the R9 guard).

- [ ] **Step 8: Run + commit**

`CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib thinker:: && … --lib context_breakdown && … --lib method_ && … --lib capability::census` (the last one: same red set as baseline, no new name).

```bash
git add src/thinker/ src/orchestrator/harness_bridge/ src/gateway/handlers/context_breakdown.rs src/gateway/handlers/mod.rs src/gateway/method_census.rs src/gateway/method_visibility.rs src/capability/mod.rs src/bin/aleph-server/commands/start/
git commit -m "context: measure the real prompt per turn and serve context.breakdown per session

Claude-Session: https://claude.ai/code/session_0114WyGPDNTB4W1D8BpC9NbP"
```

---

## Part 3 — Phase A verification and hand-off

### Task 19: Full verification, baseline diff, prompt-byte equality

**Files:** none modified (report only, plus a `scratchpad` baseline comparison).

- [ ] **Step 1: Crate-local suites**

```bash
cargo test -p aleph-protocol
cargo test -p shared-ui-logic
cargo clippy -p shared-ui-logic --no-default-features --all-targets -- -D warnings
cargo test -p aleph-tui -p aleph-cli          # untouched crates must stay green (protocol fields are additive)
```

- [ ] **Step 2: alephcore `--lib` — detached, names not counts**

Detach exactly as the baseline was (PowerShell `Start-Process cargo … --lib` with `CARGO_TARGET_DIR=D:/Workspace/Aleph/target`, `CARGO_BUILD_JOBS=2`, Git `usr/bin` prepended to PATH), output to `scratchpad/baseline/alephcore-lib-phase-a.out.txt`, watch for `test result:`. Then:

```bash
grep -E '^test .* \.\.\. FAILED' scratchpad/baseline/alephcore-lib-phase-a.out.txt | sed -E 's/^test (.*) \.\.\. FAILED.*/\1/' | sort > /tmp/phase-a.failed
comm -13 scratchpad/baseline/alephcore-lib-branch-start.failed-names.txt /tmp/phase-a.failed   # NEW reds — must be empty
comm -23 scratchpad/baseline/alephcore-lib-branch-start.failed-names.txt /tmp/phase-a.failed   # reds that went green — informational
```

The 20 inherited reds at branch start are listed in `scratchpad/baseline/alephcore-lib-branch-start.failed-names.txt` (6 `skill_manage`, `capability::census`, `catalog_description_bytes_ratchet`, `gateway::pty`, `sandbox::worktree`, `search::providers::base`, 2 `secrets`, `security::audit`, `skill::usage`, `markdown_skill::loader`, `tools::usage::store`, 2 `atomic_io`, `utils::host`). Any name outside that list is this phase's regression.

- [ ] **Step 3: The rest of the minimum set**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --bins
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo check -p alephcore --features test-helpers --all-targets   # the cheap stand-in for `--test '*' --no-run` on the shared target dir
cargo test -p aleph-panel --lib                          # Panel must still compile+pass with the additive protocol fields
just _stage-shell-placeholders && cargo clippy --workspace --all-targets   # zero warnings is the ratchet
```

- [ ] **Step 4: Prompt-byte equality (R9 guard, spec §4.3 #5)**

```bash
git stash list >/dev/null   # do NOT stash; compare against main's build instead:
cd /d/Workspace/Aleph && cargo run --bin aleph-server -- prompt-size --path basic --mode full --paradigm webrich --json > /tmp/ps-main.json
cd /d/Workspace/Aleph/.claude/worktrees/cc-render-r1 && CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo run --bin aleph-server -- prompt-size --path basic --mode full --paradigm webrich --json > /tmp/ps-branch.json
diff /tmp/ps-main.json /tmp/ps-branch.json && echo "prompt bytes unchanged"
```

Expected: identical. (Building `aleph-server` in main's checkout writes into the same shared target dir — run main's first, then the branch's, and do not interleave.)

- [ ] **Step 5: Report**

Summarise: tasks done, commits (hash + subject), the `comm` outputs, clippy warning count (must be 0), prompt-size diff (must be empty), and the explicit list of what Phase A did NOT do (client consumers of the two RPCs — Phase B/C; `ReasoningBlock` producer; `RunSummary` twin unification beyond the two fields).

---
