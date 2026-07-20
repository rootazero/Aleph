//! Secret-bearing environment variable detection for child-process hardening.
//!
//! Single source of truth shared by every Aleph subprocess boundary:
//! `PlaywrightCliDriver` (browser child) and `StdioTransport` (external MCP
//! server, e.g. `chrome-devtools-mcp`). Both must not silently forward the
//! parent's credentials to a child — a malicious or compromised child process
//! could otherwise read them straight out of its own environment.
//!
//! Rules are tuned for credential-bearing names (API keys, tokens, secrets,
//! passwords, private keys) and deliberately conservative: false positives
//! only strip a non-secret var, which is harmless. False negatives would leak
//! a credential.

const SECRET_ENV_EXACT: &[&str] = &[
    "ALEPH_VAULT_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "TOGETHER_API_KEY",
    "COHERE_API_KEY",
    "PERPLEXITY_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "AZURE_CLIENT_SECRET",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "NPM_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "DOCKER_PASSWORD",
    "SLACK_BOT_TOKEN",
    "SLACK_APP_TOKEN",
    "TELEGRAM_BOT_TOKEN",
    "STRIPE_SECRET_KEY",
    "TWILIO_AUTH_TOKEN",
    "SENDGRID_API_KEY",
    "TAVILY_API_KEY",
    "DATABASE_URL",
];

const SECRET_SUFFIXES: &[&str] = &[
    "_API_KEY",
    "_SECRET",
    "_SECRET_KEY",
    "_TOKEN",
    "_PASSWORD",
    "_CREDENTIALS",
    "_ACCESS_KEY",
    "_PRIVATE_KEY",
];

#[must_use]
pub fn is_secret_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if SECRET_ENV_EXACT.contains(&upper.as_str()) {
        return true;
    }
    SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_matches_are_secret() {
        assert!(is_secret_env("ANTHROPIC_API_KEY"));
        assert!(is_secret_env("ALEPH_VAULT_KEY"));
        assert!(is_secret_env("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_env("DATABASE_URL"));
        assert!(is_secret_env("anthropic_api_key"));
    }

    #[test]
    fn suffix_heuristic_catches_long_tail() {
        assert!(is_secret_env("ACME_API_KEY"));
        assert!(is_secret_env("SOME_SERVICE_TOKEN"));
        assert!(is_secret_env("MY_DB_PASSWORD"));
        assert!(is_secret_env("APP_PRIVATE_KEY"));
    }

    #[test]
    fn ordinary_vars_are_allowed() {
        assert!(!is_secret_env("PATH"));
        assert!(!is_secret_env("HOME"));
        assert!(!is_secret_env("LANG"));
        assert!(!is_secret_env("ALEPH_CHROME_PATH"));
    }
}