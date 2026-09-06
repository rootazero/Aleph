use crate::exec::secret_patterns::secret_masker_patterns;
use regex::Regex;
use std::borrow::Cow;
use std::sync::{Arc, LazyLock, RwLock};

static SECRET_PATTERNS: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
    secret_masker_patterns()
        .into_iter()
        .map(|p| (p.regex, p.replacement))
        .collect()
});

/// Upper bound on operator-installed redaction patterns. See
/// [`install_operator_patterns`] for the rationale; the cap is the only thing
/// standing between a misconfigured `[[security.mask_patterns]]` and a regex
/// DoS on every outbound JSON payload.
pub const MAX_OPERATOR_PATTERNS: usize = 64;

/// Operator-configured patterns from `[[security.mask_patterns]]`, compiled
/// once at boot by [`install_operator_patterns`].
///
/// **Process-global on purpose.** `SecretMasker::new()` has *seven* production
/// construction sites (background persistence, the guardian requester, the
/// redacting emitter, the unattended trace sink, `execute.rs`, the sandbox
/// approval card, the cron executor). Threading config to one of them would
/// have redacted one leg and left six spelling the secret out — which is the
/// failure this whole type exists to prevent, and which no test would have
/// caught because each leg is tested alone. Config reaches the *type*, so
/// every site inherits it whether or not its author knew this existed.
static OPERATOR_PATTERNS: LazyLock<RwLock<Arc<Vec<(Regex, String)>>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Vec::new())));

fn operator_patterns() -> Arc<Vec<(Regex, String)>> {
    OPERATOR_PATTERNS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Compile and install the operator's `[[security.mask_patterns]]`, replacing
/// any previous set. Returns the number installed.
///
/// Invalid regexes are **reported, not swallowed**: a redaction pattern that
/// silently failed to compile is a secret printed in the clear with no symptom
/// anywhere. The valid ones still install, because dropping the whole list
/// over one typo is the worse failure.
///
/// **Capped at [`MAX_OPERATOR_PATTERNS`] entries.** A config typo or a future
/// "user-supplied redaction" tool that points at a multi-thousand-entry file
/// would otherwise make every `mask()` call run thousands of regex passes,
/// turning the redacting emitter's hot path into a regex DoS. Truncated
/// installs are logged at `warn!` so the operator can see "installed 64 of
/// 1000 patterns; remainder refused — see the docs".
pub fn install_operator_patterns<'a>(
    patterns: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> (usize, Vec<(String, regex::Error)>) {
    let mut compiled = Vec::new();
    let mut rejected = Vec::new();
    let mut truncated = 0usize;
    for (pattern, replacement) in patterns {
        if compiled.len() >= MAX_OPERATOR_PATTERNS {
            truncated += 1;
            continue;
        }
        match crate::security::safe_regex::bounded_builder(pattern).build() {
            Ok(re) => compiled.push((re, replacement.to_string())),
            Err(e) => rejected.push((pattern.to_string(), e)),
        }
    }
    let installed = compiled.len();
    if truncated > 0 {
        tracing::warn!(
            truncated,
            installed,
            cap = MAX_OPERATOR_PATTERNS,
            "install_operator_patterns: cap reached; remainder of [[security.mask_patterns]] refused"
        );
    }
    for (pattern, err) in &rejected {
        tracing::warn!(pattern = %pattern, error = %err, "install_operator_patterns: invalid regex");
    }
    *OPERATOR_PATTERNS.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(compiled);
    (installed, rejected)
}

/// `SecretMasker` for redacting sensitive information.
///
/// Carries no per-instance state: the vendor floor and the operator's patterns
/// are both process-wide, and they are read at `mask()` time rather than
/// snapshotted at construction so a masker built before boot finished seeding
/// still redacts. Kept as a struct rather than free functions because the two
/// redaction legs pass it around as a value.
#[derive(Debug, Clone, Default)]
pub struct SecretMasker;

impl SecretMasker {
    /// Create a new secret masker with default patterns.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn mask(&self, text: &str) -> String {
        // `replace_all` returns a borrow on no-match; only promote to an
        // owned accumulator when a pattern actually fires, so clean text
        // costs zero copies instead of one full allocation per pattern.
        let mut result = Cow::Borrowed(text);
        for (regex, replacement) in SECRET_PATTERNS.iter() {
            if let Cow::Owned(masked) = regex.replace_all(&result, *replacement) {
                result = Cow::Owned(masked);
            }
        }
        for (regex, replacement) in operator_patterns().iter() {
            if let Cow::Owned(masked) = regex.replace_all(&result, replacement.as_str()) {
                result = Cow::Owned(masked);
            }
        }
        result.into_owned()
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

/// Mask every text leaf of a [`aleph_protocol::Presentation`] — the structured
/// UI side-channel a file-mutating tool attaches to its result.
///
/// `HunkLine.text` is **file content verbatim** — every added and deleted line
/// of whatever the tool wrote. A `file_write` / `file_edit` touching a `.env`,
/// a config carrying a token, or a key file puts that credential here in
/// plaintext, and on the live `tool_end` frame it is the *same string*
/// `result.output` gets masked for, on the *same frame*. Skipping this walk
/// masks a secret in one field and ships it in another — so this is not
/// cosmetic and must not be optimised away as redundant with the text fields
/// beside it.
///
/// `FileChange.path` is masked too, deliberately, for the reason the trace leg
/// already settled (see `unattended_redacting_sink`'s module doc): an
/// identifier-shaped field costs one regex pass that will not match, and that
/// is cheaper than a rule which needs a person to re-classify each new field
/// correctly. It is also not ours to promise that a path is never
/// credential-shaped — `[[security.mask_patterns]]` lets an operator install
/// arbitrary regexes, and an operator masking their own internal format has no
/// reason to expect one field on one frame to be exempt.
///
/// Every level destructures **without `..`** and the variant match has no
/// wildcard, so a new text-bearing field on `Presentation` / `FileChange` /
/// `Hunk` / `HunkLine` — and a second `Presentation` variant — is a compile
/// error here. That is the axis a caller's variant-by-variant exhaustiveness
/// cannot see: `presentation` itself arrived as a new *field* on an *existing*
/// `StreamEvent` variant, which no amount of arm-level exhaustiveness caught.
///
/// Single source for the two surfaces that carry a presentation to a human:
/// the live `tool_end` frame (`gateway::event_emitter::RedactingEmitter`) and
/// the replayed `tool_call_completed`
/// (`gateway::handlers::trace_replay::handle_by_runs`, which reads it out of
/// the deliberately-unmasked `session_events` log). They must agree byte for
/// byte — the same diff reaches the same person down both, and one masked copy
/// plus one clear copy is not redaction.
pub fn mask_presentation(masker: &SecretMasker, presentation: &mut aleph_protocol::Presentation) {
    match presentation {
        aleph_protocol::Presentation::FileChanges { changes } => {
            for aleph_protocol::FileChange {
                path,
                kind: _,
                hunks,
                added: _,
                removed: _,
                unavailable: _,
            } in changes.iter_mut()
            {
                *path = masker.mask(path);
                for aleph_protocol::Hunk {
                    old_start: _,
                    new_start: _,
                    lines,
                } in hunks.iter_mut()
                {
                    for aleph_protocol::HunkLine { tag: _, text } in lines.iter_mut() {
                        *text = masker.mask(text);
                    }
                }
            }
        }
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

    /// The operator's patterns must reach a masker that was constructed
    /// *without* ever being told about them — that is the whole point of
    /// hanging them off the type instead of a constructor argument, and it is
    /// what makes the other six construction sites correct for free.
    #[test]
    fn operator_patterns_reach_a_masker_nobody_configured() {
        let (installed, rejected) = install_operator_patterns([
            (r"CUSTOM_SECRET_\d+", "CUSTOM_***"),
            ("([unclosed", "never"),
        ]);
        assert_eq!(installed, 1, "the valid pattern still installs");
        assert_eq!(rejected.len(), 1, "the broken one is reported, not dropped");

        let masker = SecretMasker::new();
        let output = masker.mask("Value: CUSTOM_SECRET_12345");
        assert!(output.contains("CUSTOM_***"));
        assert!(!output.contains("12345"));

        // Leave the process as we found it — this static outlives the test.
        let _ = install_operator_patterns(std::iter::empty());
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
