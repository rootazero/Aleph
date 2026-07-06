//! Single assembled-message reducer (FEATURE_LOCATOR §4.7).
//!
//! One owner of "given the stream so far, what is the assembled visible answer
//! + reasoning" — reused by the drain, the final-answer extraction atoms, the
//! OpenAI-compat surface, and the `ReplyEmitter`, so the live bubble and the
//! persisted transcript can never drift.

mod assembler;

#[cfg(test)]
mod tests;

pub use assembler::{AssembledMessage, MessageAssembler};
