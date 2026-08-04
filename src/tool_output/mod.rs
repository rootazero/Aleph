//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Compression of verbose tool outputs (e.g. Chrome `DevTools` MCP)
//! - Semantic distillation of command / log output (errors + paths)
//! - Sanitization of raw command output (ANSI escapes + binary control bytes)
//! - Content-type-routed [`structured`] reduction (log / search / diff / json)
//! - The [`hygiene`] ingress cleaner that applies the above to a tool's own
//!   structured result *before* it is flattened into the model's context
//!
//! Ordering matters: [`hygiene`] runs on the tool's `serde_json::Value` while
//! its text fields still carry real newlines. Once the value is flattened with
//! `Value::to_string()` every `\n` becomes a two-character escape and the whole
//! result collapses onto one line — at which point `structured::classify` and
//! [`distill`] can no longer see the line structure they route on.

use crate::context::budget::pressure::chars_for_result_token_budget;
use crate::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS;

pub(crate) mod compressor;
pub mod distill;
pub mod hygiene;
pub mod sanitize;
pub mod structured;

/// Scale a default size cap linearly with a caller's token budget, clamped to
/// `[floor, default]`.
///
/// The single source for every "how big may this be" knob in this module tree —
/// [`structured::Profile`] and the tier-2 digest's salient-line cap both read
/// it, so a tool that declares a small budget gets a proportionately smaller
/// artifact from whichever tier claims its output.
///
/// The reference point is [`DEFAULT_RESULT_BUDGET_TOKENS`], the budget the
/// overwhelming majority of tools actually declare, and the conversion is the
/// project's own [`chars_for_result_token_budget`]. Two consequences worth
/// stating: at the default budget every knob equals its default, so the common
/// call is byte-for-byte unaffected; and a *larger* budget never raises a cap,
/// because these defaults also encode "a digest orients, it does not reproduce
/// the output".
pub(crate) fn scale_to_budget(default: usize, floor: usize, budget_tokens: usize) -> usize {
    let reference = chars_for_result_token_budget(DEFAULT_RESULT_BUDGET_TOKENS).max(1);
    default
        .saturating_mul(chars_for_result_token_budget(budget_tokens))
        .saturating_div(reference)
        .clamp(floor.min(default), default)
}
