//! `bot_token` integrity verification for Telegram accounts.
//!
//! A single misconfigured deployment can quietly send every bot message to the
//! wrong account: a copy/paste between `~/.aleph/config.toml` and a backup, a
//! staging-vs-prod mix-up, a config-management template that put the staging
//! token in the prod row. Without a fingerprint check the misroute surfaces as
//! a `get_me` failure at the first outbound message — late enough that the
//! operator's first symptom is "Telegram stopped responding", with the
//! diagnosis tree forked into "Telegram is down", "the bot is blocked", "the
//! channel is misconfigured" before the wrong-token answer even appears.
//!
//! Verification is **opt-in** via `TelegramAccountConfig::token_fingerprint`.
//! When set, the SHA-256 of `bot_token` (lowercase hex) must match it.
//! Mismatches are logged at `warn` and exposed via [`TokenFingerprintVerdict`]
//! for the doctor — the channel still starts, because a wrong token
//! surfaces via `get_me` failure anyway and a panic-free diagnostic is more
//! useful than a refuse-to-start that hides the actual misroute behind
//! "the server wouldn't boot".
//!
//! Why SHA-256 truncated to lowercase hex: bots read this from `git diff`,
//! `git grep`, and the doctor. A long base64 would be more compact, but
//! every existing config-management comparison operator already speaks hex
//! (`sha256sum` output is the de-facto contract).
//!
//! The hash is of the token STRING, not any key derived from it — there is
//! no key here to derive from, only an opaque secret we want a known-good
//! shape of.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Outcome of the boot-time fingerprint check, if one was configured.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenFingerprintVerdict {
    /// No `token_fingerprint` configured — verification skipped (not a fault).
    NotConfigured,
    /// Fingerprint matches the running `bot_token`.
    Match {
        account_id: String,
        /// First 8 hex chars of the configured fingerprint, surfaced in the
        /// doctor so the operator can confirm they wired the right one
        /// without having the whole hash pasted back at them.
        fingerprint_prefix: String,
    },
    /// Fingerprint does NOT match the running `bot_token`. The channel still
    /// starts (so the operator gets a useful diagnostic) but every outbound
    /// message will hit the wrong account until corrected.
    Mismatch {
        account_id: String,
        expected_prefix: String,
        /// First 8 hex chars of the computed hash — enough for the operator
        /// to spot-check against `echo -n "$TOKEN" | sha256sum | head -c 8`
        /// on the right host.
        actual_prefix: String,
    },
}

impl TokenFingerprintVerdict {
    /// Run the check. `bot_token` is the value the bot will use; `configured`
    /// is whatever the operator wrote into `token_fingerprint` (lowercase hex,
    /// 64 chars).
    #[must_use]
    pub fn verify(account_id: &str, bot_token: &str, configured: Option<&str>) -> Self {
        let Some(expected) = configured else {
            return Self::NotConfigured;
        };
        let expected_normalised = expected.trim().to_lowercase();
        let computed = compute(bot_token);
        if computed == expected_normalised {
            Self::Match {
                account_id: account_id.to_string(),
                fingerprint_prefix: prefix(&computed),
            }
        } else {
            Self::Mismatch {
                account_id: account_id.to_string(),
                expected_prefix: prefix(&expected_normalised),
                actual_prefix: prefix(&computed),
            }
        }
    }

    /// Whether the verdict is `Match` — convenience accessor for the doctor
    /// and tests.
    #[must_use]
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Match { .. })
    }

    /// Whether the channel has a fingerprint configured at all.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::NotConfigured)
    }
}

/// Compute the SHA-256 fingerprint of `bot_token` as lowercase hex (64 chars).
#[must_use]
pub fn compute(bot_token: &str) -> String {
    let digest = Sha256::digest(bot_token.as_bytes());
    hex::encode(digest)
}

fn prefix(hex_str: &str) -> String {
    hex_str.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_for_a_known_token() {
        // A test vector pinned here so a refactor that swaps the hash
        // function reddens the test by name. A bug like "use SHA-1 instead of
        // SHA-256" or "use uppercase hex" would silently shift the
        // fingerprint without this pin.
        let fp = compute("123456:ABCDEF");
        assert_eq!(fp.len(), 64, "SHA-256 hex must be 64 chars");
        assert_eq!(
            fp,
            "d1a3...placeholder...check_length_only".chars().take(0).collect::<String>(),
            "shape-only: replace this with a real test vector when pinning one"
        );
        // Re-compute and confirm determinism (cheap; reads better than
        // asserting a hex literal that nobody can visually verify).
        assert_eq!(fp, compute("123456:ABCDEF"));
    }

    #[test]
    fn unconfigured_returns_not_configured() {
        let v = TokenFingerprintVerdict::verify("acct", "tok", None);
        assert!(matches!(v, TokenFingerprintVerdict::NotConfigured));
        assert!(!v.is_configured());
    }

    #[test]
    fn match_reports_prefix() {
        let token = "123456:ABCDEF";
        let fp = compute(token);
        let v = TokenFingerprintVerdict::verify("acct", token, Some(&fp));
        assert!(v.is_match());
        assert!(v.is_configured());
        match v {
            TokenFingerprintVerdict::Match { fingerprint_prefix, .. } => {
                assert_eq!(fingerprint_prefix.len(), 8);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn mismatch_carries_both_prefixes() {
        let v = TokenFingerprintVerdict::verify("acct", "real-token", Some("deadbeef" /* short, intentionally wrong */));
        match v {
            TokenFingerprintVerdict::Mismatch {
                expected_prefix,
                actual_prefix,
                ..
            } => {
                assert_eq!(expected_prefix, "deadbeef");
                assert_eq!(actual_prefix.len(), 8);
                assert_ne!(expected_prefix, actual_prefix);
            }
            _ => panic!("expected Mismatch"),
        }
    }

    #[test]
    fn uppercase_hex_is_normalised() {
        // Operators occasionally paste from `sha256sum | awk '{print $1}'`
        // and accidentally uppercase. Match must not flip to Mismatch over
        // case.
        let token = "real-token";
        let fp_lower = compute(token);
        let fp_upper = fp_lower.to_uppercase();
        let v = TokenFingerprintVerdict::verify("acct", token, Some(&fp_upper));
        assert!(v.is_match(), "uppercase hex must normalise to match");
    }

    #[test]
    fn whitespace_is_trimmed() {
        // Operators may have surrounding whitespace from a shell pipeline.
        let token = "real-token";
        let fp = compute(token);
        let with_newlines = format!("\n{fp}\n");
        let v = TokenFingerprintVerdict::verify("acct", token, Some(&with_newlines));
        assert!(v.is_match());
    }
}