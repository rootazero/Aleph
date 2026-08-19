//! Context-window engineering: budget sensing, compaction, and retrieval of
//! offloaded tool output.

pub mod budget;
pub mod compact;
/// Deterministic classification of file-operation tool calls, shared by the
/// last-write-wins cheap pass and the compaction file ledger so the two can
/// never disagree about what counts as a read, a write, or a success.
pub(crate) mod file_ops;
pub mod retrieval;
