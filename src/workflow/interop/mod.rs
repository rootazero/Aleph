//! `.workflow.js` interoperability — bidirectional bridge between Aleph's
//! declarative `WorkflowDef` and Claude Code's workflow engineering format.
//!
//! Pure data layer (R7/R10): a declarative `WorkflowManifest` superset is the
//! single source of truth; only the executable core maps into `WorkflowDef`.

pub mod export;
pub mod import;
pub mod manifest;

pub use export::render_workflow_js;
pub use import::{parse_workflow_js, ImportOutcome};
pub use manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};
