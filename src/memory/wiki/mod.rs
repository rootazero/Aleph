//! LLM-facing orientation layer: SCHEMA.md + index.md + log.md.
//!
//! The three markdown files under `~/.aleph/memory/note/{agent_id}/` give the
//! LLM a global map each session. SQLite remains a rebuildable index; this
//! module owns the human-and-LLM-readable projection.

pub mod index_md;
pub mod log_md;
pub mod orientation;
pub mod prompts;
pub mod schema;
pub mod types;

pub use index_md::{IndexMdGenerator, INDEX_FILENAME};
pub use log_md::{LogMdWriter, LOG_FILENAME, LOG_ROTATE_LINES};
pub use orientation::{FsWikiOrientation, WikiOrientation};
pub use prompts::{schema_via_llm, PROMPT_ORIENTATION_BOOTSTRAP};
pub use schema::{SchemaDoc, SchemaStore, DEFAULT_SCHEMA, SCHEMA_FILENAME, SCHEMA_SECTIONS};
pub use types::{IndexStats, LogAction, LogEntry, OrientationSnapshot, TokenBudget};
