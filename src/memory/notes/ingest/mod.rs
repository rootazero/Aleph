//! Compound-ingest pipeline: retrieve → plan → apply → record.
//!
//! One LLM call plans cross-page edits for a whole batch of raw memories:
//! `prompts::build_compound_system_prompt` builds the system prompt (the
//! `IngestPlan` contract plus a source-specific guidance block), `plan`
//! parses the returned plan, and `apply` commits its ops to the note store.

pub mod apply;
pub mod plan;
pub mod prompts;
pub mod ref_table;
pub mod retrieve;

pub use apply::{ApplyError, CompoundApplyTx};
pub use plan::{ApplyReport, IngestPlan, PageOp, SchemaProposal};
pub use prompts::{build_compound_system_prompt, PROMPT_COMPOUND_PLAN};
pub use ref_table::{RefTable, ResolveStats};
pub use retrieve::{gather_related, RelatedBudget, RelatedPage};

pub mod ingestor;
pub use ingestor::{CompoundIngestor, DefaultCompoundIngestor};
