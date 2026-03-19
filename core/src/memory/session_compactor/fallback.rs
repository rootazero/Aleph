//! Deterministic fallback compaction.
//!
//! When LLM-based summarization is unavailable (e.g., no provider configured
//! or rate-limited), this module applies rule-based truncation to keep the
//! context within budget.
