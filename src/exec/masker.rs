use crate::exec::secret_patterns::secret_masker_patterns;
use once_cell::sync::Lazy;
use regex::Regex;

static SECRET_PATTERNS: Lazy<Vec<(regex::Regex, &'static str)>> =
    Lazy::new(|| secret_masker_patterns().into_iter().map(|p| (p.regex, p.replacement)).collect());

/// SecretMasker for redacting sensitive information.
#[derive(Debug, Clone, Default)]
pub struct SecretMasker {
    /// Additional custom patterns
    custom_patterns: Vec<(Regex, String)>,
}

impl SecretMasker {
    /// Create a new secret masker with default patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom pattern with replacement.
    pub fn add_pattern(&mut self, pattern: &str, replacement: &str) -> Result<(), regex::Error> {
        self.custom_patterns
            .push((Regex::new(pattern)?, replacement.to_string()));
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
