//! Declarative, reusable workflow templates.
//!
//! Anthropic's *Building Effective Agents* distinguishes **agents** (the LLM
//! dynamically directs its own process — Aleph's Think→Act loop and the
//! LLM-authored team task DAG) from **workflows** (LLMs orchestrated through
//! predefined code paths). Aleph already has the agent side and a dynamic DAG
//! scheduler; this module adds the missing workflow side: a **named, saved,
//! re-runnable** template.
//!
//! A template ([`def::WorkflowDef`]) is pure data. It is persisted to disk
//! ([`store`]) and, on run, *compiled* ([`compile::materialize`]) into the
//! existing `coord_tasks` DAG, then executed by the existing
//! [`TeamDispatcher`](crate::teams::dispatcher::TeamDispatcher). This module
//! contributes **no scheduler and no reasoning** — it is a schema + a file
//! store + a deterministic compiler (R10 / R7 safe). Each step is a full
//! agent run; dependency edges drive Tokio-concurrent execution that the
//! single-agent reference designs (e.g. `OpenHands`) cannot express.

pub mod clarify;
pub mod compile;
pub mod def;
pub mod interop;
pub mod proposal;
pub mod store;

pub use clarify::{
    is_clarify_task, ClarifyContext, ClarifyTaskMeta, CLARIFY_META_KEY, CLARIFY_OWNER,
};
pub use compile::{
    materialize, workflow_model_override, MaterializedWorkflow, WORKFLOW_MODEL_KEY,
    WORKFLOW_NAME_KEY, WORKFLOW_RUN_ID_KEY, WORKFLOW_STEP_KEY, WORKFLOW_STRATEGY_KEY,
};
pub use def::{render_prompt, WorkflowDef, WorkflowStepDef, WorkflowStepKind};
pub use interop::{parse_workflow_js, render_workflow_js, ImportOutcome, WorkflowManifest};
pub use proposal::{accept as accept_proposal, list_proposals, proposals_dir};
pub use store::{WorkflowMeta, WORKFLOW_EXT};
