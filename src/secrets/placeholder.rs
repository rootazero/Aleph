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

/// Extract valid secret references, leaving malformed markers as literal text.
/// Names are limited to 128 ASCII bytes before allocating or resolving them.
pub fn extract_secret_refs(input: &str) -> Result<Vec<SecretRef>, SecretError> {
    const PREFIX: &str = "{{secret:";
    const SUFFIX: &str = "}}";

    let mut refs = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = input[cursor..].find(PREFIX) {
        let start = cursor + offset;
        let name_start = start + PREFIX.len();

        let Some(close_offset) = input[name_start..].find(SUFFIX) else {
            break;
        };

        let end = name_start + close_offset;
        let name = &input[name_start..end];
        let valid = !name.is_empty()
            && name.len() <= 128
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'));
        if !valid {
            // Resume inside the malformed marker so a later valid marker is
            // still found, even when nested inside this invalid candidate.
            cursor = name_start;
            continue;
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
    fn test_extract_placeholders_skips_malformed() {
        assert!(extract_secret_refs("Bearer {{secret:oops")
            .unwrap()
            .is_empty());
        let text = "{{secret:}} {{secret:bad name}} {{secret:oops {{secret:valid}}";
        let refs = extract_secret_refs(text).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "valid");
    }

    #[test]
    fn test_secret_name_length_limit() {
        for len in [128, 129] {
            let text = format!("{{{{secret:{}}}}} {{{{secret:valid}}}}", "a".repeat(len));
            let refs = extract_secret_refs(&text).unwrap();
            assert_eq!(refs.len(), if len == 128 { 2 } else { 1 });
            assert_eq!(refs.last().unwrap().name, "valid");
        }
    }

    #[test]
    fn test_extract_placeholders_accepts_namespace_colon() {
        let text = "Bearer {{secret:anthropic:prod-key_1}}";
        let refs = extract_secret_refs(text).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "anthropic:prod-key_1");
    }
}
