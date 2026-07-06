//! Single live-stream reducer (FEATURE_LOCATOR §4.7).
//!
//! One owner of "given the stream so far, what is the assembled visible
//! answer" — the drain feeds deltas through it so the live bubble and the
//! `full_text` snapshot can never drift from what was actually streamed.

mod assembler;

#[cfg(test)]
mod tests;

pub use assembler::MessageAssembler;
