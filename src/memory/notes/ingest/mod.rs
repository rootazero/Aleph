//! Compound-ingest pipeline: retrieve → plan → apply → record.
//!
//! Replaces the single-step `FactExtractor::extract_note_updates_for_source`
//! with a two-phase flow that updates multiple note pages per batch.

pub mod plan;
pub mod prompts;

pub use plan::{ApplyReport, IngestPlan, PageOp, SchemaProposal};
pub use prompts::{build_compound_system_prompt, PROMPT_COMPOUND_PLAN};
