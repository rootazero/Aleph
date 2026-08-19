//! `user_profile` — read the current user profile or view revision history.

use crate::error::AlephError;
use crate::memory::notes::profile::synthesizer::ProfileSynthesizer;
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum UserProfileArgs {
    Read,
    History,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserProfileOutput {
    pub content: Option<String>,
    pub revision: Option<u32>,
    pub confidence: Option<String>,
    pub history: Option<String>,
}

pub struct UserProfileTool {
    pub(crate) synthesizer: Arc<dyn ProfileSynthesizer>,
}

impl Clone for UserProfileTool {
    fn clone(&self) -> Self {
        Self {
            synthesizer: Arc::clone(&self.synthesizer),
        }
    }
}

impl UserProfileTool {
    /// Model-facing description — the single source for both the static
    /// catalog (`BUILTIN_TOOL_DEFINITIONS`) and the registry constructor.
    /// A catalog entry shadows whatever the constructor registers under the
    /// same name, so a second copy of this text anywhere is a copy the model
    /// never sees.
    pub const DESCRIPTION: &'static str =
        "Read the current user profile (interests, preferences, context) or view \
         its revision history. Use 'read' to get the latest profile, 'history' to \
         inspect the revision log.";

    pub fn new(synthesizer: Arc<dyn ProfileSynthesizer>) -> Self {
        Self { synthesizer }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: UserProfileArgs,
    ) -> Result<UserProfileOutput, AlephError> {
        match args {
            UserProfileArgs::Read => {
                let profile = self.synthesizer.current(agent_id).await?;
                match profile {
                    Some(p) => Ok(UserProfileOutput {
                        // BT-C-R4-04: the profile `raw` body is rendered
                        // verbatim into the model's context. A profile that
                        // happens to capture an email, phone number, or
                        // street address (the synthesizer pulls from
                        // session signals, which can include user-shared
                        // contact info) would otherwise land in the prompt
                        // unredacted. Apply a lightweight PII scan before
                        // returning; common shapes get a `[REDACTED:<kind>]`
                        // placeholder so the model still sees structure.
                        content: Some(redact_profile_pii(&p.raw)),
                        revision: Some(p.revision),
                        confidence: Some(p.confidence),
                        history: None,
                    }),
                    None => Ok(UserProfileOutput {
                        content: None,
                        revision: None,
                        confidence: None,
                        history: None,
                    }),
                }
            }
            UserProfileArgs::History => {
                let profile = self.synthesizer.current(agent_id).await?;
                Ok(UserProfileOutput {
                    content: None,
                    revision: profile.as_ref().map(|p| p.revision),
                    confidence: profile.as_ref().map(|p| p.confidence.clone()),
                    history: Some("Event-sourced history replay not yet implemented. Use revision number to track changes.".into()),
                })
            }
        }
    }
}

/// BT-C-R4-04: lightweight PII scan over the profile body before it lands
/// in the model's context. Matches common shapes without pulling in a regex
/// engine (the call path is a once-per-tool-call cost; we want zero
/// runtime allocations beyond the resulting string). Each match is replaced
/// with `[REDACTED:<kind>]`. False positives are acceptable for the
/// profile-text surface — a model that sees `[REDACTED:email]` where it
/// expected a name will still write a coherent continuation; the cost of
/// a leaked email or phone number is much higher.
fn redact_profile_pii(input: &str) -> String {
    // Iterate char-by-char to keep byte boundaries safe on multi-byte UTF-8.
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Email: match `[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}`.
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-') {
            if let Some(end) = scan_email(&chars, i) {
                out.push_str("[REDACTED:email]");
                i = end;
                continue;
            }
        }
        // Phone-like: 7+ consecutive digits, optionally separated by spaces
        // or dashes (US/NANP shape). Conservative — won't catch every
        // international format but catches the common ones.
        if c.is_ascii_digit() {
            if let Some(end) = scan_phone(&chars, i) {
                out.push_str("[REDACTED:phone]");
                i = end;
                continue;
            }
        }
        // SSN-like: three digits, dash or space, two digits, dash or space,
        // four digits.
        if c.is_ascii_digit() {
            if let Some(end) = scan_ssn(&chars, i) {
                out.push_str("[REDACTED:ssn]");
                i = end;
                continue;
            }
        }
        // IPv4 (any private range): four 1-3-digit dotted groups. SSRF
        // surface; avoid letting the profile echo an internal address.
        if c.is_ascii_digit() {
            if let Some(end) = scan_ipv4(&chars, i) {
                out.push_str("[REDACTED:ip]");
                i = end;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn scan_email(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    let mut local_len = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '@' {
            break;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')) {
            return None;
        }
        local_len += 1;
        i += 1;
    }
    if local_len == 0 || i >= chars.len() || chars[i] != '@' {
        return None;
    }
    i += 1; // skip '@'
    let domain_start = i;
    let mut last_dot: Option<usize> = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '.' {
            last_dot = Some(i);
        } else if !(c.is_ascii_alphanumeric() || c == '-') {
            break;
        }
        i += 1;
    }
    let tld_len = last_dot.map(|d| chars.len() - 1 - d).unwrap_or(0);
    if tld_len < 2 || tld_len > 24 {
        return None;
    }
    if i == domain_start {
        return None;
    }
    Some(i)
}

fn scan_phone(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    let mut digits = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            digits += 1;
            i += 1;
        } else if matches!(c, ' ' | '-' | '(' | ')' | '+') && digits > 0 {
            i += 1;
        } else {
            break;
        }
    }
    // Walked off the start without at least 7 digits → not a phone.
    if digits < 7 {
        return None;
    }
    // Must be terminated by a non-digit, non-separator (end of input /
    // punctuation / letter). Otherwise we just consumed the first 7 digits
    // of a longer digit string we should not match.
    if i < chars.len() {
        let next = chars[i];
        if next.is_ascii_digit() {
            return None;
        }
    }
    Some(i)
}

fn scan_ssn(chars: &[char], start: usize) -> Option<usize> {
    // First group: 3 digits.
    if start + 2 >= chars.len() {
        return None;
    }
    if !chars[start].is_ascii_digit()
        || !chars[start + 1].is_ascii_digit()
        || !chars[start + 2].is_ascii_digit()
    {
        return None;
    }
    // Separator.
    if !matches!(chars[start + 3], '-' | ' ') {
        return None;
    }
    // Second group: 2 digits.
    if start + 5 >= chars.len()
        || !chars[start + 4].is_ascii_digit()
        || !chars[start + 5].is_ascii_digit()
    {
        return None;
    }
    // Separator.
    if !matches!(chars[start + 6], '-' | ' ') {
        return None;
    }
    // Third group: 4 digits.
    if start + 9 >= chars.len()
        || !chars[start + 7].is_ascii_digit()
        || !chars[start + 8].is_ascii_digit()
        || !chars[start + 9].is_ascii_digit()
        || !chars[start + 10].is_ascii_digit()
    {
        return None;
    }
    // Must be terminated.
    let end = start + 11;
    if end < chars.len() && chars[end].is_ascii_digit() {
        return None;
    }
    Some(end)
}

fn scan_ipv4(chars: &[char], start: usize) -> Option<usize> {
    // Match d{1,3}.d{1,3}.d{1,3}.d{1,3}.
    let mut i = start;
    let mut octets = 0usize;
    let mut group_len = 0usize;
    while i < chars.len() && octets < 4 {
        let c = chars[i];
        if c.is_ascii_digit() {
            group_len += 1;
            if group_len > 3 {
                return None;
            }
            i += 1;
        } else if c == '.' && group_len > 0 && octets < 3 {
            i += 1;
            octets += 1;
            group_len = 0;
        } else {
            break;
        }
    }
    if octets == 3 && group_len > 0 {
        Some(i) // after the fourth group
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::profile::types::{SessionSignal, UpdateOutcome, UserProfile};
    use crate::sync_primitives::Arc;
    use async_trait::async_trait;

    struct MockSynth;

    #[async_trait]
    impl ProfileSynthesizer for MockSynth {
        async fn bootstrap(&self, _: &str) -> Result<UserProfile, AlephError> {
            unimplemented!()
        }
        async fn current(&self, _: &str) -> Result<Option<UserProfile>, AlephError> {
            Ok(Some(UserProfile {
                schema_version: 1,
                updated: "2026-04-17".into(),
                revision: 5,
                last_session: "s1".into(),
                confidence: "high".into(),
                sections: Default::default(),
                sources: Default::default(),
                raw: "## Identity\n- test user".into(),
                content_hash: "abc".into(),
            }))
        }
        async fn update(&self, _: &str, _: SessionSignal) -> Result<UpdateOutcome, AlephError> {
            Ok(UpdateOutcome::Unchanged)
        }
    }

    #[tokio::test]
    async fn read_returns_profile() {
        let tool = UserProfileTool::new(Arc::new(MockSynth));
        let out = tool.call("default", UserProfileArgs::Read).await.unwrap();
        assert_eq!(out.revision, Some(5));
        assert!(out.content.unwrap().contains("test user"));
    }

    #[tokio::test]
    async fn history_returns_placeholder() {
        let tool = UserProfileTool::new(Arc::new(MockSynth));
        let out = tool
            .call("default", UserProfileArgs::History)
            .await
            .unwrap();
        assert!(out.history.unwrap().contains("not yet implemented"));
    }
}
