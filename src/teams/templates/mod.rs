//! Team templates — declarative TOML blueprints that materialize a team in one
//! shot: leader + workers + initial task DAG.
//!
//! Inspired by `ClawTeam`'s `templates/*.toml` registry. A template captures
//! "what does *this kind of team* look like" — roles, prompt addenda, and an
//! initial decomposition of the goal — so users (and the LLM via the
//! `team_from_template` tool) can stamp out a working team without manually
//! enumerating every member and task.
//!
//! ## Resolution order
//!
//! 1. `~/.aleph/teams/templates/*.toml` (user-provided, override)
//! 2. Built-in templates baked into the binary via `include_str!`
//!
//! ## Substitution
//!
//! Templates may reference `{goal}`, `{team_name}`, and `{leader}` in any
//! string field (prompt addenda, task subjects/descriptions). Unknown braces
//! are left intact so authoring stays forgiving.
//!
//! ## Dependency model
//!
//! Initial tasks reference each other by **stable `key`** (not subject), so
//! substitution never breaks a DAG edge. The materializer translates keys to
//! freshly-minted `CoordTask` ids.

pub mod loader;
pub mod materialize;
pub mod substitute;
pub mod types;

pub use loader::{load_template, TemplateRegistry};
pub use materialize::{
    materialize_template, MaterializeDeps, MaterializeRequest, MaterializedTeam,
};
pub use substitute::substitute;
pub use types::{
    TeamTemplate, TeamTemplateError, TemplateLeader, TemplateMember, TemplatePriority, TemplateTask,
};
