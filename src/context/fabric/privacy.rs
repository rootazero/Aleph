//! Reuse the existing credential detector at the plugin egress boundary.
//!
//! Recognised credentials block this external invocation; native context
//! handling can still proceed. This is pattern detection, not proof that an
//! arbitrary user-authored string cannot be sensitive.

use crate::exec::LeakDetector;
use serde_json::Value;
use std::sync::OnceLock;

pub(crate) fn contains_recognised_credential(payload: &Value) -> bool {
    static DETECTOR: OnceLock<LeakDetector> = OnceLock::new();
    let detector = DETECTOR.get_or_init(LeakDetector::default_patterns);
    // Structured serialization also inspects string values nested in tool
    // arguments, without treating business fields named `reasoning` as secret.
    let serialized = payload.to_string();
    let result = detector.scan_outbound(&serialized);
    result.has_blocks() || result.has_redacts()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_in_nested_tool_arguments_is_blocked() {
        let payload = serde_json::json!({"arguments": {"headers": {
            "Authorization": "Bearer sk-abc123def456ghi789jklmnopqrstuvwx"
        }}});
        assert!(contains_recognised_credential(&payload));
    }

    #[test]
    fn ordinary_business_reasoning_is_allowed() {
        assert!(!contains_recognised_credential(&serde_json::json!({
            "reasoning": "The parcel was delivered to the warehouse.",
            "arguments": {"quantity": 3}
        })));
    }
}
