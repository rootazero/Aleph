//! Secret placeholder parsing utilities.

use super::SecretError;

/// Parsed secret placeholder reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    pub name: String,
    pub raw: String,
    /// Start index of the placeholder in the original input.
    pub start: usize,
    /// End index (exclusive) of the placeholder in the original input.
    pub end: usize,
}

/// Extract secret references from a text input.
pub fn extract_secret_refs(input: &str) -> Result<Vec<SecretRef>, SecretError> {
    const PREFIX: &str = "{{secret:";
    const SUFFIX: &str = "}}";

    let mut refs = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = input[cursor..].find(PREFIX) {
        let start = cursor + offset;
        let name_start = start + PREFIX.len();

        let Some(close_offset) = input[name_start..].find(SUFFIX) else {
            return Err(SecretError::InvalidPlaceholder(
                "missing closing '}}'".to_string(),
            ));
        };

        let end = name_start + close_offset;
        let name = &input[name_start..end];
        if name.is_empty() {
            return Err(SecretError::InvalidPlaceholder(
                "empty secret name".to_string(),
            ));
        }

        let valid = name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'));
        if !valid {
            return Err(SecretError::InvalidPlaceholder(format!(
                "invalid secret name '{name}'"
            )));
        }

        let placeholder_end = end + SUFFIX.len();
        refs.push(SecretRef {
            name: name.to_string(),
            raw: input[start..placeholder_end].to_string(),
            start,
            end: placeholder_end,
        });

        cursor = placeholder_end;
    }

    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::extract_secret_refs;

    #[test]
    fn test_extract_placeholders() {
        let text = "Bearer {{secret:openai_main_api_key}}";
        let refs = extract_secret_refs(text).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "openai_main_api_key");
    }

    #[test]
    fn test_extract_placeholders_keeps_order() {
        let text = "{{secret:first}} then {{secret:second}}";
        let refs = extract_secret_refs(text).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "first");
        assert_eq!(refs[1].name, "second");
    }

    #[test]
    fn test_extract_placeholders_rejects_malformed() {
        let err = extract_secret_refs("Bearer {{secret:oops").unwrap_err();
        assert!(format!("{}", err).contains("Invalid secret placeholder"));
    }

    #[test]
    fn test_extract_placeholders_accepts_namespace_colon() {
        let text = "Bearer {{secret:anthropic:prod-key_1}}";
        let refs = extract_secret_refs(text).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "anthropic:prod-key_1");
    }
}
