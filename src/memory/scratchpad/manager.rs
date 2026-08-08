// core/src/memory/scratchpad/manager.rs

//! Scratchpad Manager
//!
//! Manages the lifecycle of agent scratchpad files stored under
//! `~/.aleph/workspaces/<agent_id>/` — see
//! [`default_workspace_root`] for the resolution rule. The first arg to
//! [`Self::new`] is the on-disk subdirectory name (historically called
//! `project_id`); in the unified agent model this is the agent id.
//!
//! Per-run project overrides (the desktop App's "进入项目工作" flow,
//! see [`crate::projects`]) do NOT relocate the scratchpad — runtime
//! working memory stays bound to the agent, so a single agent's
//! scratchpad survives a user toggling between project folders.
//!
//! [`default_workspace_root`]: crate::config::agent_resolver::default_workspace_root

use crate::error::AlephError;
use std::path::PathBuf;
use tokio::fs;

use super::template::{generate_scratchpad, DEFAULT_TEMPLATE};

/// Configuration for scratchpad behavior
#[derive(Debug, Clone)]
pub struct ScratchpadConfig {
    /// Scratchpad filename (default: "scratchpad.md")
    pub filename: String,
    /// Create backup before overwrite
    pub backup_on_write: bool,
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            filename: "scratchpad.md".to_string(),
            backup_on_write: true,
        }
    }
}

/// Lifecycle state of a plan item — mirrors Claude Code's `TodoWrite`
/// 3-state model (`pending` → `in_progress` → `completed`). Modeled as an
/// enum rather than parallel bools so the illegal `done && in_progress`
/// state is unrepresentable (leverages Rust's type system; see the task
/// directive on exceeding the reference via stronger type safety).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanItemStatus {
    /// `- [ ]` — not started.
    Pending,
    /// `- [~]` — the single step currently being worked.
    InProgress,
    /// `- [x]` — finished.
    Done,
}

impl PlanItemStatus {
    /// Markdown checkbox glyph (without the leading `- `) for this state.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Done => "[x]",
        }
    }
}

/// A single plan item parsed from the scratchpad's `## Plan` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Item text (the part after the `- [ ]` / `- [~]` / `- [x]` marker).
    pub text: String,
    /// Lifecycle state of this item.
    pub status: PlanItemStatus,
}

impl PlanItem {
    /// A not-yet-started item.
    #[must_use]
    pub fn pending(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: PlanItemStatus::Pending,
        }
    }

    /// `true` when the item is finished (`[x]`).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.status == PlanItemStatus::Done
    }

    /// `true` when this is the active step (`[~]`).
    #[must_use]
    pub fn is_in_progress(&self) -> bool {
        self.status == PlanItemStatus::InProgress
    }
}

/// Structural snapshot of a scratchpad's objective + plan, used by the
/// goal-loop hook. Carries no judgment — just the parsed checkbox state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScratchpadSnapshot {
    /// The objective text, or `None` when unset / still `[No active task]`.
    pub objective: Option<String>,
    /// Plan items in document order (excludes the `- [ ] ...` placeholder).
    pub items: Vec<PlanItem>,
}

/// Stable sentinel prefix marking a goal-loop **completion summary** (收尾).
/// Both the scratchpad tool echo (model-facing) and the progress sink
/// (user-facing) key off this so a finished objective surfaces as a "✅ 目标
/// 达成" 收尾 rather than a plain in-progress checklist. Keep in sync with the
/// `scratchpad_progress_sink` detection.
pub const COMPLETION_BANNER: &str = "✅ 目标达成";

impl ScratchpadSnapshot {
    /// Plan items not yet finished (pending **or** in-progress).
    #[must_use]
    pub fn incomplete(&self) -> Vec<&PlanItem> {
        self.items.iter().filter(|i| !i.is_done()).collect()
    }

    /// `true` when an objective is set and **every** real plan item is
    /// finished (`[x]`) — the structural success/收尾 condition, the exact
    /// complement of [`Self::has_pending_work`] once a plan exists. Empty
    /// plans never count as complete (nothing was decomposed to finish).
    #[must_use]
    pub fn is_objective_complete(&self) -> bool {
        self.objective.is_some() && !self.items.is_empty() && self.items.iter().all(|i| i.is_done())
    }

    /// Render a judgment-free **completion summary** for a finished goal-loop:
    /// the objective plus the full list of completed steps. Structural only —
    /// the model's own checked boxes, never a semantic verdict (R7). The
    /// "what was achieved" prose is the model's final reply; this is just the
    /// scaffold both it and the user see at the 收尾 moment. Mirrors hermes-
    /// agent's `mark_done` → "✓ Goal achieved" without an LLM judge call.
    #[must_use]
    pub fn render_completion(&self) -> String {
        let mut out = String::from(COMPLETION_BANNER);
        if let Some(obj) = &self.objective {
            out.push('：');
            out.push_str(obj);
        }
        out.push_str(&format!("\n全部 {} 个步骤已完成：", self.items.len()));
        for item in &self.items {
            out.push_str("\n- [x] ");
            out.push_str(&item.text);
        }
        out
    }

    /// The single in-progress item, if any — the "current step".
    #[must_use]
    pub fn current(&self) -> Option<&PlanItem> {
        self.items.iter().find(|i| i.is_in_progress())
    }

    /// `true` when an objective is set AND at least one real plan item is
    /// not yet finished. This is the structural condition the goal-loop
    /// hook fires on — not a semantic completion judgment.
    #[must_use]
    pub fn has_pending_work(&self) -> bool {
        self.objective.is_some() && self.items.iter().any(|i| !i.is_done())
    }

    /// Compact, judgment-free progress block echoed back to the model after
    /// each mutating scratchpad action (Claude Code `TodoWrite` parity: the
    /// tool always returns the updated list, giving the loop continuous
    /// visibility without touching the harness prompt builder). Pure render.
    #[must_use]
    pub fn render_progress(&self) -> String {
        let mut out = String::new();
        if let Some(obj) = &self.objective {
            out.push_str("Objective: ");
            out.push_str(obj);
            out.push('\n');
        }
        if self.items.is_empty() {
            out.push_str("Plan: (none)");
            return out;
        }
        out.push_str("Plan:\n");
        for item in &self.items {
            out.push_str("- ");
            out.push_str(item.status.glyph());
            out.push(' ');
            out.push_str(&item.text);
            out.push('\n');
        }
        let done = self.items.iter().filter(|i| i.is_done()).count();
        out.push_str(&format!("Progress: {}/{} done", done, self.items.len()));
        if let Some(cur) = self.current() {
            out.push_str(" · current: ");
            out.push_str(&cur.text);
        }
        out
    }
}

/// Manages agent scratchpad files under `~/.aleph/workspaces/<agent_id>/`.
pub struct ScratchpadManager {
    /// Base directory for this project's scratchpad files
    project_dir: PathBuf,
    session_id: String,
    config: ScratchpadConfig,
}

impl ScratchpadManager {
    /// Create a new `ScratchpadManager` for an agent workspace
    ///
    /// Files are stored under `~/.aleph/workspaces/<agent_id>/`.
    /// Falls back to a temp-style path for testing.
    #[must_use]
    pub fn new(project_id: &str, session_id: &str) -> Self {
        let project_dir = crate::config::agent_resolver::default_workspace_root().join(project_id);

        Self {
            project_dir,
            session_id: session_id.to_string(),
            config: ScratchpadConfig::default(),
        }
    }

    /// Create with an explicit base directory (for testing)
    #[must_use]
    pub fn with_dir(project_dir: PathBuf, session_id: &str) -> Self {
        Self {
            project_dir,
            session_id: session_id.to_string(),
            config: ScratchpadConfig::default(),
        }
    }

    /// Create with custom configuration
    #[must_use]
    pub fn with_config(project_dir: PathBuf, session_id: &str, config: ScratchpadConfig) -> Self {
        Self {
            project_dir,
            session_id: session_id.to_string(),
            config,
        }
    }

    /// Get the project directory path
    #[must_use]
    pub const fn project_dir(&self) -> &PathBuf {
        &self.project_dir
    }

    /// Get the scratchpad file path
    #[must_use]
    pub fn scratchpad_path(&self) -> PathBuf {
        self.project_dir.join(&self.config.filename)
    }

    /// Ensure the project directory exists
    pub async fn ensure_dir(&self) -> Result<(), AlephError> {
        fs::create_dir_all(&self.project_dir)
            .await
            .map_err(|e| AlephError::other(format!("Failed to create project dir: {e}")))
    }

    /// Check if scratchpad file exists
    #[must_use]
    pub fn exists(&self) -> bool {
        self.scratchpad_path().exists()
    }

    /// Check if scratchpad has meaningful content (not just template)
    pub async fn has_content(&self) -> Result<bool, AlephError> {
        if !self.exists() {
            return Ok(false);
        }

        let content = self.read().await?;

        // Check if it's more than just the default template
        let has_objective = !content.contains("[No active task]");
        let has_plan_items = content.contains("- [x]")
            || (content.contains("- [ ]") && !content.contains("- [ ] ..."));
        let has_working_state = {
            const HEADER: &str = "## Working State";
            if let Some(pos) = content.find(HEADER) {
                let after = &content[pos..];
                if let Some(next_section) = after[HEADER.len()..].find("##") {
                    let working_content = &after[HEADER.len()..HEADER.len() + next_section];
                    !working_content.trim().is_empty()
                } else {
                    false
                }
            } else {
                false
            }
        };

        Ok(has_objective || has_plan_items || has_working_state)
    }

    /// Parse the objective + plan checkboxes into a [`ScratchpadSnapshot`].
    ///
    /// Pure structural read — uses the same markers as [`Self::has_content`]
    /// (`## Objective` / `## Plan` sections, `- [ ]` / `- [x]` checkboxes,
    /// skipping the `- [ ] ...` placeholder). Returns an empty snapshot when
    /// no scratchpad file exists.
    pub async fn snapshot(&self) -> Result<ScratchpadSnapshot, AlephError> {
        if !self.exists() {
            return Ok(ScratchpadSnapshot::default());
        }
        Ok(parse_snapshot(&self.read().await?))
    }

    /// Read scratchpad content
    pub async fn read(&self) -> Result<String, AlephError> {
        fs::read_to_string(self.scratchpad_path())
            .await
            .map_err(|e| AlephError::other(format!("Failed to read scratchpad: {e}")))
    }

    /// Write content to scratchpad (creates backup if configured)
    pub async fn write(&self, content: &str) -> Result<(), AlephError> {
        self.ensure_dir().await?;

        // Backup existing file if configured
        if self.config.backup_on_write && self.exists() {
            let backup_path = self.scratchpad_path().with_extension("md.bak");
            if let Ok(existing) = fs::read_to_string(self.scratchpad_path()).await {
                if let Err(e) = fs::write(&backup_path, &existing).await {
                    tracing::warn!(error = %e, path = %backup_path.display(), "scratchpad backup write failed");
                }
            }
        }

        crate::utils::atomic_write::atomic_write_file(&self.scratchpad_path(), content).await
    }

    /// Initialize scratchpad with default template
    pub async fn initialize(&self, objective: Option<&str>) -> Result<(), AlephError> {
        let content = generate_scratchpad(objective, &self.session_id);
        self.write(&content).await
    }

    /// Append a note to the Notes section
    pub async fn append_note(&self, note: &str) -> Result<(), AlephError> {
        let mut content = if self.exists() {
            self.read().await?
        } else {
            generate_scratchpad(None, &self.session_id)
        };

        // Find Notes section and append
        if let Some(notes_pos) = content.find("## Notes") {
            let insert_pos = notes_pos + "## Notes".len();
            let timestamp = chrono::Utc::now().format("%H:%M");
            let note_line = format!("\n- [{timestamp}] {note}");
            content.insert_str(insert_pos, &note_line);
        }

        // Update timestamp
        content = self.update_timestamp(content);

        self.write(&content).await
    }

    /// Update the objective.
    pub async fn set_objective(&self, objective: &str) -> Result<(), AlephError> {
        let content = if self.exists() {
            self.read().await?
        } else {
            generate_scratchpad(Some(objective), &self.session_id)
        };

        let content = self.update_timestamp(upsert_section(&content, OBJECTIVE_HEADER, objective));
        self.write(&content).await
    }

    /// Replace the plan with `items`, **preserving the status each item
    /// carries**, and optionally set the objective in the same write.
    ///
    /// Whole-list replace (codex `update_plan` / kimi `SetTodoList` / hermes
    /// `todo(merge=false)` semantics) — but unlike all three references the
    /// single-in-progress invariant is enforced here in code, not left to
    /// prose: the first `[~]` wins and later ones are demoted to `[ ]`, so the
    /// illegal "two current steps" document state cannot be written at all.
    ///
    /// Setting the objective in the same call exists because a plan without an
    /// objective is inert for every downstream consumer (`has_pending_work`,
    /// `<execution_plan>`, the stop verifier); folding it in lets the model
    /// arm the whole execution list in one tool call.
    ///
    /// Returns the number of items written.
    pub async fn set_plan(
        &self,
        objective: Option<&str>,
        items: &[PlanItem],
    ) -> Result<usize, AlephError> {
        let content = if self.exists() {
            self.read().await?
        } else {
            generate_scratchpad(objective, &self.session_id)
        };

        let normalized = normalize_single_in_progress(items);
        let body = if normalized.is_empty() {
            PLAN_PLACEHOLDER.to_string()
        } else {
            normalized
                .iter()
                .map(|item| format!("- {} {}", item.status.glyph(), item.text))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let content = upsert_section(&content, PLAN_HEADER, &body);
        let content = match objective.map(str::trim).filter(|o| !o.is_empty()) {
            Some(obj) => upsert_section(&content, OBJECTIVE_HEADER, obj),
            None => content,
        };

        let content = self.update_timestamp(content);
        self.write(&content).await?;
        Ok(normalized.len())
    }

    /// Mark a plan item as complete (`[x]`).
    pub async fn complete_item(&self, item_index: usize) -> Result<(), AlephError> {
        self.set_item_status(item_index, PlanItemStatus::Done).await
    }

    /// Mark a plan item as the in-progress current step (`[~]`).
    ///
    /// Mirrors Claude Code's `TodoWrite` "exactly one `in_progress`" discipline
    /// at the action level: the model marks which step it is actively working.
    pub async fn start_item(&self, item_index: usize) -> Result<(), AlephError> {
        self.set_item_status(item_index, PlanItemStatus::InProgress)
            .await
    }

    /// Rewrite the nth plan checkbox to `status`, counting **every** item
    /// marker (`- [ ]` / `- [~]` / `- [x]`) in document order and skipping
    /// the `- [ ] ...` placeholder.
    ///
    /// Indexing over all states — not just `- [ ]` as the original
    /// `complete_item` did — keeps `item_index` stable once an item moves to
    /// `[~]`/`[x]`; otherwise completing an already-started step would skip
    /// to the wrong pending item. Byte-preserving (only the matched marker is
    /// rewritten).
    ///
    /// An out-of-range `item_index` is an **error**, not a silent success: the
    /// scan simply never matched, so returning `Ok` would report "item 7
    /// marked as complete" for a 3-item plan and let the model believe work it
    /// never recorded was recorded (and then get vetoed at stop time by
    /// `ScratchpadGoalVerifier` with no idea why).
    async fn set_item_status(
        &self,
        item_index: usize,
        status: PlanItemStatus,
    ) -> Result<(), AlephError> {
        let content = self.read().await?;
        let mut out = String::with_capacity(content.len());
        let mut count = 0usize;
        // Maintain the single-in-progress invariant: starting a new item reverts
        // any other active `[~]` item to pending.
        let demote_others = status == PlanItemStatus::InProgress;

        // Count and rewrite ONLY inside `## Plan`.
        //
        // The read side (`parse_snapshot`) has always scoped itself with
        // `extract_section(content, PLAN_HEADER)`, but this write side scanned
        // the whole document, so "item N" had two different answers on any
        // scratchpad carrying a checkbox outside the plan — and one is reachable
        // from model output alone: `set_objective` / `append_note` pass model
        // text straight into `upsert_section`, so a multi-line objective or note
        // containing a `- [ ]` line shifts every plan index on the write side
        // while the read side, the tool echo and the Panel all keep the old
        // numbering. `complete_item(0)` then rewrote the decoy and reported
        // success while the real step stayed pending.
        //
        // `section_span` already declares itself the single source for both
        // sides; this writer was the one that never joined. A document with no
        // `## Plan` yields count == 0 and the existing "plan is empty" error.
        let plan_span = section_span(&content, PLAN_HEADER);
        let mut offset = 0usize;

        for line in content.split_inclusive('\n') {
            let line_start = offset;
            offset += line.len();
            let in_plan = plan_span.is_some_and(|(s, e)| line_start >= s && line_start < e);
            let trimmed = line.trim_start();
            let body = trimmed.trim_end_matches(['\n', '\r']);
            let is_item = in_plan
                && (body.starts_with("- [ ]")
                    || body.starts_with("- [~]")
                    || body.starts_with("- [x]"));
            if is_item && body != "- [ ] ..." {
                let this = count;
                count += 1;
                let indent = &line[..line.len() - trimmed.len()];
                // All three markers are exactly 5 ASCII bytes; the slice keeps
                // the item text and any trailing newline intact.
                let after_marker = &trimmed[5..];
                if this == item_index {
                    out.push_str(indent);
                    out.push_str("- ");
                    out.push_str(status.glyph());
                    out.push_str(after_marker);
                    continue;
                } else if demote_others && body.starts_with("- [~]") {
                    out.push_str(indent);
                    out.push_str("- ");
                    out.push_str(PlanItemStatus::Pending.glyph());
                    out.push_str(after_marker);
                    continue;
                }
            }
            out.push_str(line);
        }

        if item_index >= count {
            return Err(AlephError::tool(if count == 0 {
                format!(
                    "Plan item {item_index} does not exist: the plan is empty. \
                     Call action='set_plan' with the steps first."
                )
            } else {
                format!(
                    "Plan item {item_index} does not exist: the plan has {count} item(s), \
                     so valid indices are 0..={}. Call action='read' to see the current list.",
                    count - 1
                )
            }));
        }

        let out = self.update_timestamp(out);
        self.write(&out).await
    }

    /// Clear scratchpad (reset to empty template)
    pub async fn clear(&self) -> Result<(), AlephError> {
        self.write(DEFAULT_TEMPLATE).await
    }

    /// Update the "Last updated" timestamp
    fn update_timestamp(&self, mut content: String) -> String {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

        if let Some(pos) = content.find("_Last updated:") {
            if let Some(end) = content[pos..].find("_\n") {
                let before = &content[..pos];
                let after = &content[pos + end + 2..];
                content = format!("{before}_Last updated: {now}_\n{after}");
            }
        }

        content
    }
}

/// Canonical section headers — single source for both the parse side
/// ([`parse_snapshot`]) and the write side ([`upsert_section`]).
pub(crate) const OBJECTIVE_HEADER: &str = "## Objective";
pub(crate) const PLAN_HEADER: &str = "## Plan";
/// Placeholder written when the plan is emptied; [`parse_snapshot`] drops it.
const PLAN_PLACEHOLDER: &str = "- [ ] ...";
/// Start of the trailing metadata block (`---` / `_Last updated_` / `_Session_`).
const FOOTER_MARK: &str = "\n---\n";

/// Byte offset just past the `header` **line**.
///
/// Line-anchored and exact: a bare `content.find(header)` would also match
/// `## Planning` when looking for `## Plan`, and would match a literal
/// `## Objective` typed into the Notes section — either one silently
/// relocating every subsequent read and write.
fn find_section_start(content: &str, header: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim_end() == header {
            return Some(offset + line.len());
        }
        offset += line.len();
    }
    None
}

/// Byte range of a section's body: from just past its header line to the next
/// `## ` heading, the metadata footer, or EOF — whichever comes first.
fn section_span(content: &str, header: &str) -> Option<(usize, usize)> {
    let start = find_section_start(content, header)?;
    let rest = &content[start..];
    let end_rel = ["\n## ", FOOTER_MARK]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .unwrap_or(rest.len());
    Some((start, start + end_rel))
}

/// Replace a section's body, **creating the section when it is absent**.
///
/// The previous implementation required both the target header *and* the
/// following one to be present and silently returned the document unchanged
/// otherwise — so a hand-edited scratchpad, or one whose sections had been
/// reordered, made every `set_plan` a no-op that still reported success.
/// Self-healing instead: a missing section is appended ahead of the metadata
/// footer, so the write always lands.
fn upsert_section(content: &str, header: &str, body: &str) -> String {
    let block = if body.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", body.trim_end())
    };

    match section_span(content, header) {
        Some((start, end)) => {
            let mut out = String::with_capacity(content.len() + block.len());
            out.push_str(&content[..start]);
            out.push_str(&block);
            out.push_str(&content[end..]);
            out
        }
        None => {
            let insert_at = content.find(FOOTER_MARK).unwrap_or(content.len());
            let mut out = String::with_capacity(content.len() + header.len() + block.len() + 4);
            out.push_str(content[..insert_at].trim_end());
            out.push_str("\n\n");
            out.push_str(header);
            out.push('\n');
            out.push_str(&block);
            out.push_str(&content[insert_at..]);
            out
        }
    }
}

/// Enforce "at most one in-progress step" over an incoming list: the first
/// `[~]` keeps its status, any later one is demoted to `[ ]`. Pure — returns a
/// new vector rather than mutating the caller's slice.
fn normalize_single_in_progress(items: &[PlanItem]) -> Vec<PlanItem> {
    let mut claimed = false;
    items
        .iter()
        .map(|item| {
            if item.status == PlanItemStatus::InProgress {
                if claimed {
                    return PlanItem::pending(item.text.clone());
                }
                claimed = true;
            }
            item.clone()
        })
        .collect()
}

/// Return the trimmed text of the markdown section body for `header`.
fn extract_section<'a>(content: &'a str, header: &str) -> Option<&'a str> {
    let (start, end) = section_span(content, header)?;
    Some(content[start..end].trim())
}

/// Parse objective + plan checkboxes out of raw scratchpad markdown.
///
/// Free function (no I/O) so it is trivially unit-testable. Mirrors the
/// marker conventions of [`ScratchpadManager::has_content`].
pub(crate) fn parse_snapshot(content: &str) -> ScratchpadSnapshot {
    let objective = extract_section(content, OBJECTIVE_HEADER)
        .map(str::trim)
        .filter(|o| !o.is_empty() && *o != "[No active task]")
        .map(str::to_string);

    let items = extract_section(content, PLAN_HEADER)
        .map(|plan| {
            plan.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if let Some(text) = line.strip_prefix("- [ ] ") {
                        Some((text.trim(), PlanItemStatus::Pending))
                    } else if let Some(text) = line.strip_prefix("- [~] ") {
                        Some((text.trim(), PlanItemStatus::InProgress))
                    } else if let Some(text) = line.strip_prefix("- [x] ") {
                        Some((text.trim(), PlanItemStatus::Done))
                    } else {
                        None
                    }
                })
                // Drop the default `- [ ] ...` placeholder.
                .filter(|(text, status)| !(*status == PlanItemStatus::Pending && *text == "..."))
                .map(|(text, status)| PlanItem {
                    text: text.to_string(),
                    status,
                })
                .collect::<Vec<PlanItem>>()
        })
        .unwrap_or_default();

    ScratchpadSnapshot { objective, items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// All-pending items, the shape the old `set_plan(&[&str])` produced.
    fn pending(texts: &[&str]) -> Vec<PlanItem> {
        texts.iter().map(|t| PlanItem::pending(*t)).collect()
    }

    fn item(text: &str, status: PlanItemStatus) -> PlanItem {
        PlanItem {
            text: text.to_string(),
            status,
        }
    }

    #[test]
    fn parse_snapshot_empty_template_has_no_pending_work() {
        let snap = parse_snapshot(DEFAULT_TEMPLATE);
        assert_eq!(snap.objective, None);
        assert!(snap.items.is_empty(), "placeholder must be skipped");
        assert!(!snap.has_pending_work());
    }

    #[test]
    fn parse_snapshot_objective_plus_mixed_checkboxes() {
        let md = "# Current Task\n\n## Objective\nShip auth\n\n## Plan\n- [x] Design API\n- [ ] Implement\n- [ ] Test\n\n## Working State\n\n## Notes\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective.as_deref(), Some("Ship auth"));
        assert_eq!(snap.items.len(), 3);
        let pending = snap.incomplete();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].text, "Implement");
        assert!(snap.has_pending_work());
    }

    #[test]
    fn parse_snapshot_all_done_has_no_pending_work() {
        let md = "## Objective\nDone goal\n\n## Plan\n- [x] A\n- [x] B\n\n## Working State\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective.as_deref(), Some("Done goal"));
        assert!(!snap.has_pending_work(), "all boxes checked → no pending");
    }

    #[test]
    fn parse_snapshot_plan_without_objective_does_not_fire() {
        // Items present but objective never set → hook stays dormant.
        let md =
            "## Objective\n[No active task]\n\n## Plan\n- [ ] orphan step\n\n## Working State\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective, None);
        assert!(!snap.has_pending_work());
    }

    #[test]
    fn parse_snapshot_in_progress_counts_as_pending_and_is_current() {
        let md = "## Objective\nShip\n\n## Plan\n- [x] A\n- [~] B\n- [ ] C\n\n## Working State\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.items.len(), 3);
        // in_progress is not done → still pending work (keeps the loop alive).
        assert!(snap.has_pending_work());
        assert_eq!(
            snap.incomplete().len(),
            2,
            "[~] and [ ] are both incomplete"
        );
        let cur = snap.current().expect("an in-progress step");
        assert_eq!(cur.text, "B");
        assert_eq!(cur.status, PlanItemStatus::InProgress);
    }

    #[test]
    fn render_progress_shows_glyphs_count_and_current() {
        let md = "## Objective\nShip\n\n## Plan\n- [x] A\n- [~] B\n- [ ] C\n\n## Working State\n";
        let rendered = parse_snapshot(md).render_progress();
        assert!(rendered.contains("Objective: Ship"));
        assert!(rendered.contains("- [x] A"));
        assert!(rendered.contains("- [~] B"));
        assert!(rendered.contains("- [ ] C"));
        assert!(rendered.contains("Progress: 1/3 done"));
        assert!(rendered.contains("current: B"));
    }

    #[test]
    fn is_objective_complete_only_when_objective_set_and_all_done() {
        // All done + objective → complete.
        let done =
            parse_snapshot("## Objective\nShip\n\n## Plan\n- [x] A\n- [x] B\n\n## Working State\n");
        assert!(done.is_objective_complete());
        // A box still open → not complete.
        let mixed =
            parse_snapshot("## Objective\nShip\n\n## Plan\n- [x] A\n- [ ] B\n\n## Working State\n");
        assert!(!mixed.is_objective_complete());
        // In-progress is not done → not complete.
        let wip = parse_snapshot("## Objective\nShip\n\n## Plan\n- [~] A\n\n## Working State\n");
        assert!(!wip.is_objective_complete());
        // No objective → never complete (dormant gate, matches the verifier).
        let no_obj = parse_snapshot(
            "## Objective\n[No active task]\n\n## Plan\n- [x] A\n\n## Working State\n",
        );
        assert!(!no_obj.is_objective_complete());
        // Empty plan → nothing was decomposed to finish.
        let empty = parse_snapshot("## Objective\nShip\n\n## Plan\n\n## Working State\n");
        assert!(!empty.is_objective_complete());
    }

    #[test]
    fn render_completion_banner_objective_and_done_steps() {
        let snap = parse_snapshot(
            "## Objective\nShip auth\n\n## Plan\n- [x] Design API\n- [x] Implement\n\n## Working State\n",
        );
        let out = snap.render_completion();
        assert!(
            out.starts_with(COMPLETION_BANNER),
            "must lead with the 收尾 sentinel"
        );
        assert!(out.contains("Ship auth"));
        assert!(out.contains("全部 2 个步骤已完成"));
        assert!(out.contains("- [x] Design API"));
        assert!(out.contains("- [x] Implement"));
    }

    #[tokio::test]
    async fn start_then_complete_targets_the_same_item() {
        // Regression for the original complete_item bug: once an item is
        // marked [~], completing it by the same index must still hit it (the
        // old `- [ ]`-only scan would have completed the next pending item).
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .set_plan(None, &pending(&["alpha", "beta", "gamma"]))
            .await
            .unwrap();

        manager.start_item(1).await.unwrap();
        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.current().unwrap().text, "beta");

        manager.complete_item(1).await.unwrap();
        let snap = manager.snapshot().await.unwrap();
        assert!(
            snap.current().is_none(),
            "no item should remain in progress"
        );
        assert_eq!(snap.items[1].text, "beta");
        assert_eq!(snap.items[1].status, PlanItemStatus::Done);
        // alpha + gamma still pending — beta did not bleed into a sibling.
        assert_eq!(snap.items[0].status, PlanItemStatus::Pending);
        assert_eq!(snap.items[2].status, PlanItemStatus::Pending);
    }

    #[tokio::test]
    async fn start_item_demotes_previous_in_progress() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager.initialize(Some("obj")).await.unwrap();
        manager
            .set_plan(None, &pending(&["a", "b", "c"]))
            .await
            .unwrap();
        manager.start_item(0).await.unwrap();
        manager.start_item(1).await.unwrap(); // must demote item 0
        let snap = manager.snapshot().await.unwrap();
        let in_prog: Vec<usize> = snap
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.is_in_progress())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            in_prog,
            vec![1],
            "only the newest started item stays in progress"
        );
        assert!(
            !snap.items[0].is_in_progress(),
            "previous in-progress demoted to pending"
        );
    }

    #[tokio::test]
    async fn set_plan_preserves_statuses_it_is_given() {
        // The regression this whole change exists for: the old signature took
        // `&[&str]` and wrote every item back as `- [ ]`, so refining a plan
        // mid-run silently reset all progress.
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager.initialize(Some("Ship auth")).await.unwrap();
        manager
            .set_plan(
                None,
                &[
                    item("Design", PlanItemStatus::Done),
                    item("Build", PlanItemStatus::InProgress),
                    item("Test", PlanItemStatus::Pending),
                    item("Ship", PlanItemStatus::Pending),
                ],
            )
            .await
            .unwrap();

        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.items.len(), 4, "the added step landed");
        assert_eq!(snap.items[0].status, PlanItemStatus::Done);
        assert_eq!(snap.current().unwrap().text, "Build");
        assert!(snap.has_pending_work());
    }

    #[tokio::test]
    async fn set_plan_enforces_single_in_progress() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .set_plan(
                Some("Obj"),
                &[
                    item("a", PlanItemStatus::InProgress),
                    item("b", PlanItemStatus::InProgress),
                ],
            )
            .await
            .unwrap();
        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.current().unwrap().text, "a", "first [~] wins");
        assert_eq!(
            snap.items[1].status,
            PlanItemStatus::Pending,
            "the second [~] is demoted, so the document can never hold two current steps"
        );
    }

    #[tokio::test]
    async fn set_plan_can_arm_the_objective_in_the_same_write() {
        // A plan with no objective is inert for every consumer (has_pending_work,
        // <execution_plan>, the stop verifier), so one call must be able to set both.
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .set_plan(Some("Ship auth"), &pending(&["Design", "Build"]))
            .await
            .unwrap();
        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.objective.as_deref(), Some("Ship auth"));
        assert!(snap.has_pending_work());
    }

    #[tokio::test]
    async fn set_plan_recreates_a_missing_section_instead_of_no_op() {
        // A hand-edited / reordered scratchpad used to make set_plan a silent
        // no-op that still reported success.
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .write("# Current Task\n\n## Objective\nShip\n\n## Notes\n\n---\n_Last updated: _\n")
            .await
            .unwrap();
        manager
            .set_plan(None, &pending(&["only step"]))
            .await
            .unwrap();
        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.items.len(), 1);
        assert_eq!(snap.items[0].text, "only step");
        assert_eq!(snap.objective.as_deref(), Some("Ship"));
        // The footer survives the insertion.
        assert!(manager.read().await.unwrap().contains("_Last updated:"));
    }

    #[tokio::test]
    async fn set_plan_with_no_items_clears_the_list() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .set_plan(Some("Obj"), &pending(&["a", "b"]))
            .await
            .unwrap();
        assert_eq!(manager.set_plan(None, &[]).await.unwrap(), 0);
        let snap = manager.snapshot().await.unwrap();
        assert!(snap.items.is_empty(), "placeholder is not a real item");
        assert!(!snap.has_pending_work());
    }

    #[tokio::test]
    async fn item_index_out_of_range_is_an_error_not_a_silent_success() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .set_plan(Some("Obj"), &pending(&["a", "b"]))
            .await
            .unwrap();

        let err = manager.complete_item(7).await.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "got: {err}");
        assert!(
            err.contains("0..=1"),
            "the error names the valid range: {err}"
        );

        // The document is untouched by the rejected write.
        let snap = manager.snapshot().await.unwrap();
        assert!(snap
            .items
            .iter()
            .all(|i| i.status == PlanItemStatus::Pending));
    }

    #[tokio::test]
    async fn item_index_on_an_empty_plan_names_set_plan() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager.initialize(Some("Obj")).await.unwrap();
        let err = manager.start_item(0).await.unwrap_err().to_string();
        assert!(err.contains("set_plan"), "got: {err}");
    }

    #[test]
    fn section_lookup_is_line_anchored() {
        // `## Planning` must not satisfy a `## Plan` lookup, and a `## Objective`
        // line typed into Notes must not shadow the real one.
        let md = "## Objective\nreal\n\n## Planning\n- [ ] decoy\n\n## Plan\n- [x] real step\n\n## Notes\n## Objective\nfake\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective.as_deref(), Some("real"));
        assert_eq!(snap.items.len(), 1);
        assert_eq!(snap.items[0].text, "real step");
    }

    #[test]
    fn upsert_section_replaces_body_without_disturbing_neighbours() {
        let out = upsert_section(DEFAULT_TEMPLATE, OBJECTIVE_HEADER, "Ship auth");
        assert!(out.contains("## Objective\nShip auth\n\n## Plan"));
        assert!(out.contains("## Working State"));
        assert!(out.contains("_Last updated:"));
    }

    #[tokio::test]
    async fn test_manager_creates_directory() {
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("test-project");

        let manager = ScratchpadManager::with_dir(project_dir.clone(), "test-session");
        manager.ensure_dir().await.unwrap();

        assert!(manager.project_dir().exists());
    }

    #[tokio::test]
    async fn test_initialize_creates_file() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess-123");

        manager.initialize(Some("Test objective")).await.unwrap();

        assert!(manager.exists());
        let content = manager.read().await.unwrap();
        assert!(content.contains("Test objective"));
        assert!(content.contains("sess-123"));
    }

    #[tokio::test]
    async fn test_has_content_empty() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();

        assert!(!manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_content_with_objective() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(Some("Build feature X")).await.unwrap();

        assert!(manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_append_note() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager.append_note("This is a test note").await.unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("This is a test note"));
    }

    #[tokio::test]
    async fn test_set_plan() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager
            .set_plan(None, &pending(&["Step 1", "Step 2", "Step 3"]))
            .await
            .unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("- [ ] Step 1"));
        assert!(content.contains("- [ ] Step 2"));
        assert!(content.contains("- [ ] Step 3"));
    }

    #[tokio::test]
    async fn test_complete_item() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager
            .set_plan(None, &pending(&["Step 1", "Step 2"]))
            .await
            .unwrap();
        manager.complete_item(0).await.unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("- [x] Step 1"));
        assert!(content.contains("- [ ] Step 2"));
    }

    #[tokio::test]
    async fn write_roundtrips_and_leaves_no_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess-atomic");
        manager.write("# Objective\nhello\n").await.unwrap();
        assert_eq!(manager.read().await.unwrap(), "# Objective\nhello\n");
        // No `.aleph_atomic_*` staging files survive a successful write.
        let mut read_dir = tokio::fs::read_dir(manager.scratchpad_path().parent().unwrap())
            .await
            .unwrap();
        let mut leftovers = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if entry
                .file_name()
                .to_string_lossy()
                .contains(".aleph_atomic_")
            {
                leftovers.push(entry);
            }
        }
        assert!(leftovers.is_empty(), "no atomic temp files should remain");
    }

    #[tokio::test]
    async fn test_backup_on_write() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.write("First version").await.unwrap();
        manager.write("Second version").await.unwrap();

        let backup_path = manager.scratchpad_path().with_extension("md.bak");
        assert!(backup_path.exists());

        let backup = tokio::fs::read_to_string(&backup_path).await.unwrap();
        assert_eq!(backup, "First version");
    }

    /// "Item N" must mean the same thing to the writer and to the reader.
    ///
    /// `parse_snapshot` has always scoped itself to `## Plan`; `set_item_status`
    /// used to count every checkbox in the document. A checkbox above the plan
    /// therefore shifted the write side's numbering while the read side, the
    /// tool echo and the Panel kept theirs. Reachable from model output alone:
    /// `set_objective` / `append_note` pass model text straight through
    /// `upsert_section`, so one multi-line objective containing a `- [ ]` line
    /// is enough.
    #[tokio::test]
    async fn item_status_writes_are_scoped_to_the_plan_section() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .write(
                "## Objective\nShip auth\n- [ ] a checkbox the model typed into its objective\n\n\
                 ## Plan\n- [ ] real first step\n- [ ] real second step\n\n\
                 ## Notes\n- [ ] a checkbox in a note\n",
            )
            .await
            .unwrap();

        manager.complete_item(0).await.unwrap();

        let snap = manager.snapshot().await.unwrap();
        assert_eq!(snap.items.len(), 2, "the reader still sees only the plan");
        assert_eq!(snap.items[0].text, "real first step");
        assert_eq!(
            snap.items[0].status,
            PlanItemStatus::Done,
            "complete_item(0) must move the item the reader calls 0, not the \
             first checkbox in the file"
        );
        assert_eq!(snap.items[1].status, PlanItemStatus::Pending);

        let raw = manager.read().await.unwrap();
        assert!(
            raw.contains("- [ ] a checkbox the model typed into its objective"),
            "text outside the plan must be byte-preserved"
        );
        assert!(raw.contains("- [ ] a checkbox in a note"));
    }

    /// The out-of-range error must also be computed on the plan alone,
    /// otherwise stray checkboxes make an invalid index look valid.
    #[tokio::test]
    async fn out_of_range_is_measured_against_the_plan_only() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");
        manager
            .write("## Objective\nO\n- [ ] decoy\n\n## Plan\n- [ ] only step\n")
            .await
            .unwrap();

        let err = manager.complete_item(1).await.unwrap_err().to_string();
        assert!(
            err.contains("1 item(s)"),
            "the plan has one item, not two: {err}"
        );
    }
}
