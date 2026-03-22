//! Utility functions for the Codex protocol adapter
//!
//! Standalone helpers that don't depend on the ProtocolAdapter trait:
//! - JWT token parsing for account ID extraction
//! - JSON schema normalization for Codex API compatibility

/// Extract codex account_id from a Codex OAuth JWT token.
///
/// The token payload contains `{"https://api.openai.com/auth": {"chatgpt_account_id": "..."}}`
pub(crate) fn extract_codex_account_id(token: &str) -> Option<String> {
    use base64::Engine;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None; // Not a valid JWT (too many parts)
    }

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let payload_json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    payload_json
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Recursively ensure all object schemas have a "properties" field.
/// Codex Responses API rejects object schemas without it.
pub(crate) fn ensure_properties_recursive(schema: &mut serde_json::Value) {
    let Some(obj) = schema.as_object_mut() else { return };

    if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
        if !obj.contains_key("properties") {
            obj.insert("properties".into(), serde_json::Value::Object(serde_json::Map::new()));
        }
        if !obj.contains_key("required") {
            obj.insert("required".into(), serde_json::Value::Array(vec![]));
        }
    }

    // Recurse into nested schemas
    let keys: Vec<String> = obj.keys().cloned().collect();
    for key in keys {
        if let Some(v) = obj.get_mut(&key) {
            match v {
                serde_json::Value::Object(_) => ensure_properties_recursive(v),
                serde_json::Value::Array(arr) => {
                    for item in arr {
                        ensure_properties_recursive(item);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_account_id_valid_jwt() {
        // Build a minimal JWT with the expected claim
        use base64::Engine;
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_abc123"
            }
        });
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);
        assert_eq!(
            extract_codex_account_id(&token),
            Some("acct_abc123".to_string())
        );
    }

    #[test]
    fn test_extract_account_id_missing_claim() {
        use base64::Engine;
        let payload = serde_json::json!({"sub": "user_123"});
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);
        assert_eq!(extract_codex_account_id(&token), None);
    }

    #[test]
    fn test_extract_account_id_invalid_jwt() {
        assert_eq!(extract_codex_account_id("not-a-jwt"), None);
        assert_eq!(extract_codex_account_id("a.b.c.d"), None);
    }

    #[test]
    fn test_ensure_properties_adds_missing_fields() {
        let mut schema = serde_json::json!({
            "type": "object"
        });
        ensure_properties_recursive(&mut schema);
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }

    #[test]
    fn test_ensure_properties_preserves_existing() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"]
        });
        let original = schema.clone();
        ensure_properties_recursive(&mut schema);
        assert_eq!(schema, original);
    }

    #[test]
    fn test_ensure_properties_recursive_nested() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object"
                }
            }
        });
        ensure_properties_recursive(&mut schema);
        let inner = &schema["properties"]["inner"];
        assert!(inner.get("properties").is_some());
        assert!(inner.get("required").is_some());
    }
}
