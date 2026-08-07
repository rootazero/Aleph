use crate::exec::secret_patterns::secret_masker_patterns;
use regex::Regex;
use std::sync::LazyLock;

static SECRET_PATTERNS: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
    secret_masker_patterns()
        .into_iter()
        .map(|p| (p.regex, p.replacement))
        .collect()
});

/// `SecretMasker` for redacting sensitive information.
#[derive(Debug, Clone, Default)]
pub struct SecretMasker {
    /// Additional custom patterns
    custom_patterns: Vec<(Regex, String)>,
}

impl SecretMasker {
    /// Create a new secret masker with default patterns.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom pattern with replacement.
    pub fn add_pattern(&mut self, pattern: &str, replacement: &str) -> Result<(), regex::Error> {
        self.custom_patterns.push((
            crate::security::safe_regex::bounded_builder(pattern).build()?,
            replacement.to_string(),
        ));
        Ok(())
    }

    pub fn mask(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (regex, replacement) in SECRET_PATTERNS.iter() {
            result = regex.replace_all(&result, *replacement).to_string();
        }
        for (regex, replacement) in &self.custom_patterns {
            result = regex.replace_all(&result, replacement.as_str()).to_string();
        }
        result
    }

    pub fn contains_secrets(&self, text: &str) -> bool {
        for (regex, _) in SECRET_PATTERNS.iter() {
            if regex.is_match(text) {
                return true;
            }
        }
        for (regex, _) in &self.custom_patterns {
            if regex.is_match(text) {
                return true;
            }
        }
        false
    }
}

/// Mask every string leaf of a JSON value in place; `true` when anything
/// changed. Depth-first over arrays and objects.
///
/// Single source for both redaction legs of an unattended run — the trace sink
/// (`gateway::execution_engine::UnattendedRedactingSink`) and the event emitter
/// (`gateway::event_emitter::RedactingEmitter`). They must agree byte for byte:
/// the same tool result reaches a human down both, and one masked copy plus one
/// clear copy is not redaction.
pub fn mask_json_strings(masker: &SecretMasker, value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            let masked = masker.mask(s);
            if masked == *s {
                false
            } else {
                *s = masked;
                true
            }
        }
        // Plain loops on purpose: `.any(..)` (clippy's suggestion for the
        // former fold) short-circuits on the first masked item and would
        // leave every later secret unmasked. Masking must visit ALL items.
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                changed |= mask_json_strings(masker, item);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for item in map.values_mut() {
                changed |= mask_json_strings(masker, item);
            }
            changed
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_openai_key() {
        let masker = SecretMasker::new();
        let input = "API key is sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        let output = masker.mask(input);
        assert!(output.contains("sk-***REDACTED***"));
        assert!(!output.contains("abcdefgh"));
    }

    #[test]
    fn test_mask_anthropic_key() {
        let masker = SecretMasker::new();
        let input = "Key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let output = masker.mask(input);
        assert!(output.contains("sk-ant-***REDACTED***"));
    }

    #[test]
    fn test_mask_aws_key() {
        let masker = SecretMasker::new();
        let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let output = masker.mask(input);
        assert!(output.contains("AKIA***REDACTED***"));
    }

    #[test]
    fn test_mask_github_token() {
        let masker = SecretMasker::new();
        let input = "GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let output = masker.mask(input);
        assert!(output.contains("gh*_***REDACTED***"));
    }

    #[test]
    fn test_mask_private_key() {
        let masker = SecretMasker::new();
        let input = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8DHGP...
-----END RSA PRIVATE KEY-----"#;
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("MIIEpAIBAAKCAQEA"));
    }

    #[test]
    fn test_mask_pkcs8_private_key_without_algorithm_word() {
        // Regression: the standard PKCS#8 header has no algorithm word
        // (`-----BEGIN PRIVATE KEY-----`); it must still be fully redacted.
        let masker = SecretMasker::new();
        let input = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQ...
-----END PRIVATE KEY-----"#;
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(
            !output.contains("MIIEvQIBADANBgkqhkiG"),
            "PKCS#8 key body must be redacted"
        );
    }

    #[test]
    fn test_mask_github_fine_grained_pat() {
        let masker = SecretMasker::new();
        let input = "GH_PAT=github_pat_11ABCDE0Y0abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        let output = masker.mask(input);
        assert!(output.contains("github_pat_***REDACTED***"));
        assert!(!output.contains("11ABCDE0Y0abcdefghij"));
    }

    #[test]
    fn test_mask_password_in_url() {
        let masker = SecretMasker::new();
        let input = "postgres://user:secretpassword123@localhost:5432/db";
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("secretpassword123"));
    }

    #[test]
    fn test_mask_generic_password() {
        let masker = SecretMasker::new();
        let input = "DATABASE_PASSWORD=mysupersecretpassword";
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("mysupersecret"));
    }

    #[test]
    fn test_contains_secrets() {
        let masker = SecretMasker::new();
        assert!(masker.contains_secrets("sk-abcdefghijklmnopqrstuvwxyz12345678"));
        assert!(!masker.contains_secrets("This is just normal text"));
    }

    #[test]
    fn test_custom_pattern() {
        let mut masker = SecretMasker::new();
        masker
            .add_pattern(r"CUSTOM_SECRET_\d+", "CUSTOM_***")
            .unwrap();
        let input = "Value: CUSTOM_SECRET_12345";
        let output = masker.mask(input);
        assert!(output.contains("CUSTOM_***"));
    }

    #[test]
    fn test_no_false_positives() {
        let masker = SecretMasker::new();
        // Normal text should not be masked
        let input = "Hello world, this is a normal message";
        let output = masker.mask(input);
        assert_eq!(input, output);
    }
}
