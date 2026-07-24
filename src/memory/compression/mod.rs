//! Memory Compression Module
//!
//! Orchestrates compression of raw conversation memories into Knowledge Notes
//! via [`CompoundIngestor`](crate::memory::notes::ingest::CompoundIngestor).
//!
//! ## Components
//!
//! - [`CompressionService`]: Main service that orchestrates compression
//! - [`CompressionScheduler`]: Determines when to trigger compression
//!
//! ## Why there is no signal detector
//!
//! A `SignalDetector` used to scan every user message against a bilingual keyword
//! table ("that's wrong" / "correction:" / "actually" / "wrong" / …) to classify it as a
//! Correction / Learning / Milestone and, on a "correction" hit, fire an immediate
//! compression. That is deterministic content classification of natural language —
//! exactly what **R7** (LLM sovereignty) and **P8** (no brittle pattern matching)
//! forbid — and it was brittle in the classic way: `contains("actually")` meant
//! "Actually, can you also check the logs?" triggered a full LLM ingest cycle.
//!
//! The LLM-driven path for that same judgement already exists and is R8-clean:
//! `flag_user_correction`. The model decides "this is a correction", and the tool
//! now kicks the drain itself (`memory::flush::flush_agent_memory`). Compression
//! triggers here are content-blind cadence only (turn threshold / background /
//! session end).

mod scheduler;
mod service;
pub mod source_prompts;

pub use scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
pub use service::{CompressionConfig, CompressionService, PostCompressionHook};
