//! SecretMasker - Redact sensitive information from output.
//!
//! Detects and masks:
//! - API keys (OpenAI, Anthropic, Google, AWS, etc.)
//! - Private keys (SSH, PEM)
//! - Passwords and tokens
//! - Connection strings

use once_cell::sync::Lazy;
use regex::Regex;

/// Secret pattern with replacement
struct SecretPattern {
    regex: Regex,
    replacement: &'static str,
}

/// All secret patterns
static SECRET_PATTERNS: Lazy<Vec<SecretPattern>> = Lazy::new(|| {
    vec![
        // OpenAI API Key (sk-...)
        SecretPattern {
            regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
            replacement: "sk-***REDACTED***",
        },
        // Anthropic API Key (sk-ant-...)
        SecretPattern {
            regex: Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
            replacement: "sk-ant-***REDACTED***",
        },
        // Google API Key
        SecretPattern {
            regex: Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap(),
            replacement: "AIza***REDACTED***",
        },
        // AWS Access Key ID
        SecretPattern {
            regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
            replacement: "AKIA***REDACTED***",
        },
        // AWS Secret Access Key
        SecretPattern {
            regex: Regex::new(r#"(?i)(aws_secret_access_key|secret_access_key)\s*[=:]\s*['"]?([a-zA-Z0-9/+=]{40})['"]?"#).unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // GitHub Token
        SecretPattern {
            regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
            replacement: "gh*_***REDACTED***",
        },
        // Generic Bearer Token
        SecretPattern {
            regex: Regex::new(r#"(?i)(bearer|token|authorization)\s*[=:]\s*['"]?([a-zA-Z0-9\-_.]{20,})['"]?"#).unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // Private Key Block
        SecretPattern {
            regex: Regex::new(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+ PRIVATE KEY-----").unwrap(),
            replacement: "-----BEGIN PRIVATE KEY-----\n***REDACTED***\n-----END PRIVATE KEY-----",
        },
        // Password in URL
        SecretPattern {
            regex: Regex::new(r"://([^:]+):([^@]+)@").unwrap(),
            replacement: "://$1:***REDACTED***@",
        },
        // Generic password assignment
        SecretPattern {
            regex: Regex::new(r#"(?i)(password|passwd|pwd|secret)\s*[=:]\s*['"]?([^\s'"]{8,})['"]?"#).unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // Slack Token
        SecretPattern {
            regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
            replacement: "xox*-***REDACTED***",
        },
        // Discord Token
        SecretPattern {
            regex: Regex::new(r"[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27}").unwrap(),
            replacement: "***DISCORD_TOKEN_REDACTED***",
        },
    ]
});

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

    /// Mask secrets in the given text.
    pub fn mask(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Apply default patterns
        for pattern in SECRET_PATTERNS.iter() {
            result = pattern
                .regex
                .replace_all(&result, pattern.replacement)
                .to_string();
        }

        // Apply custom patterns
        for (regex, replacement) in &self.custom_patterns {
            result = regex.replace_all(&result, replacement.as_str()).to_string();
        }

        result
    }

    /// Check if the text contains any secrets.
    pub fn contains_secrets(&self, text: &str) -> bool {
        // Check default patterns
        for pattern in SECRET_PATTERNS.iter() {
            if pattern.regex.is_match(text) {
                return true;
            }
        }

        // Check custom patterns
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
