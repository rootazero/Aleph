//! Credential/PII redaction for MCP error surfaces.
//!
//! MCP tool failures and transport errors can echo back arguments, URLs, or
//! headers that contain secrets the agent supplied. Before such a string is
//! handed to the LLM — which may persist it in conversation history or a
//! provider's request logs — it is run through the global [`PiiEngine`], the
//! same secret/PII rule set the rest of Aleph uses. MCP therefore needs no
//! bespoke regex of its own (R3 core minimalism).

use crate::pii::PiiEngine;

/// Redact secrets and PII from an MCP-originated error string.
///
/// Falls back to the input unchanged when the global `PiiEngine` has not been
/// initialised (e.g. unit tests that skip `PiiEngine::init`). The lock is
/// taken poison-safe, matching the idiom used elsewhere in `pii`.
pub fn redact_mcp_error(text: &str) -> String {
    match PiiEngine::global() {
        Some(engine) => {
            let guard = engine.read().unwrap_or_else(|e| e.into_inner());
            guard.filter(text).text
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_engine_uninitialised() {
        // In an isolated unit test the global engine is typically absent;
        // redaction must then be an identity function rather than panic.
        let input = "plain error text with no secrets";
        assert_eq!(redact_mcp_error(input), input);
    }
}
