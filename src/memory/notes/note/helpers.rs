//! Serialization and utility helpers for knowledge notes.

use sha2::{Digest, Sha256};

/// Compute SHA-256 hex digest of content.
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Return `true` if `s`, emitted unquoted in YAML flow context, would parse
/// as a non-string scalar (null, bool, number, etc.) and break a round-trip
/// to `Vec<String>`.
fn is_yaml_implicit_scalar(s: &str) -> bool {
    matches!(
        s,
        "" | "~"
            | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || s.parse::<i64>().is_ok()
        || s.parse::<f64>().is_ok()
        || s.starts_with("0x")
        || s.starts_with("-0x")
        || s.starts_with("+0x")
        || s.starts_with("0o")
        || s.starts_with("-0o")
        || s.starts_with("+0o")
}

/// Emit a YAML flow-style array, quoting any element that contains a YAML
/// reserved character so the round-trip survives `serde_yaml::from_str`.
pub(crate) fn yaml_inline_array(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let parts: Vec<String> = items
        .iter()
        .map(|s| {
            let needs_quote = s.chars().any(|c| {
                matches!(
                    c,
                    '\'' | '"'
                        | ','
                        | ':'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '#'
                        | '&'
                        | '*'
                        | '!'
                        | '|'
                        | '>'
                        | '%'
                        | '@'
                        | '`'
                )
            }) || s.starts_with(' ')
                || s.ends_with(' ')
                || s.is_empty()
                || is_yaml_implicit_scalar(s);
            if needs_quote {
                let escaped = s.replace('\'', "''");
                format!("'{escaped}'")
            } else {
                s.clone()
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// Sanitize a note title for safe use as a filename.
///
/// Strips path separators, null bytes, and filesystem-unsafe characters
/// to prevent path traversal attacks from LLM-generated titles.
///
/// Returns `Err(AlephError::Validation)` if the result is empty / all-dots /
/// all-whitespace — callers should reject the operation rather than write a
/// note with an unstable filename.
pub fn sanitize_title(title: &str) -> Result<String, crate::error::AlephError> {
    let cleaned: String = title
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "")
        .replace("..", "")
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.' || c.is_whitespace()) {
        return Err(crate::error::AlephError::Validation(format!(
            "note title sanitizes to empty: {title:?}"
        )));
    }
    Ok(cleaned)
}
