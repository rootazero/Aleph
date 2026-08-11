// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as Markdown files
//! under `~/.aleph/workspaces/<agent_id>/` that are immune to compression.
//!
//! Runtime working memory is bound to the agent, not to a project folder —
//! per-run project overrides do not relocate the scratchpad. All agent
//! working memory lives in the unified `~/.aleph/` workspace.
//!
//! ## Architecture
//!
//! - **scratchpad.md**: Current active task state
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Production: uses ~/.aleph/workspaces/<agent_id>/
//! let manager = ScratchpadManager::new("my-agent", "session-id");
//!
//! // Testing: uses an explicit directory
//! let manager = ScratchpadManager::with_dir(temp_dir, "session-id");
//!
//! manager.initialize(Some("Build auth module")).await?;
//! manager.set_plan(Some("Ship auth"), &[PlanItem::pending("Design API"), PlanItem::pending("Implement")]).await?;
//! manager.complete_item(0).await?;
//! ```

mod manager;
pub mod template;

pub use manager::{
    PlanItem, PlanItemStatus, PlanRenderLimits, ScratchpadManager, ScratchpadSnapshot,
    COMPLETION_BANNER, PROMPT_PLAN_LIMITS,
};
pub use template::{generate_scratchpad, DEFAULT_TEMPLATE};
