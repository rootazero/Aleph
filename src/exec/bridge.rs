//! Approval Bridge - connects `ExecApprovalManager` with chat channels.
//!
//! Provides utilities for:
//! - Building approval inline keyboards
//! - Parsing callback data from button clicks

use crate::gateway::channel::{InlineButton, InlineKeyboard};

use super::socket::ApprovalDecisionType;

/// The wire tokens of the `approve:<id>:<decision>` vocabulary — the only
/// place these four strings are spelled.
///
/// The callback data is a public surface: anyone with access to the bot can
/// craft one. So the tokens have exactly one writer — the keyboard builds
/// them, [`ApprovalBridge::parse_callback`] reads them back, and the Discord
/// command parser asks for the token of the tier its button means.
mod token {
    pub(super) const ONCE: &str = "once";
    pub(super) const SESSION: &str = "session";
    pub(super) const ALWAYS: &str = "always";
    pub(super) const DENY: &str = "deny";
}

/// Bridge utilities for approval message handling
pub struct ApprovalBridge;

impl ApprovalBridge {
    /// The wire token of a decision tier — the `<decision>` half of the
    /// `approve:<id>:<decision>` callback.
    ///
    /// One writer for the wire contract: the keyboard builds the token, the
    /// parser reads it back ([`Self::parse_callback`]), and the Discord
    /// command parser asks for the token of the tier its button means.
    #[must_use]
    pub fn decision_token(decision: &ApprovalDecisionType) -> &'static str {
        match decision {
            ApprovalDecisionType::AllowOnce => token::ONCE,
            ApprovalDecisionType::AllowSession => token::SESSION,
            ApprovalDecisionType::AllowAlways => token::ALWAYS,
            ApprovalDecisionType::Deny => token::DENY,
        }
    }

    /// The decision tier a wire token names — `None` when the token is not
    /// part of the vocabulary.
    #[must_use]
    pub fn decision_from_token(raw: &str) -> Option<ApprovalDecisionType> {
        match raw {
            token::ONCE => Some(ApprovalDecisionType::AllowOnce),
            token::SESSION => Some(ApprovalDecisionType::AllowSession),
            token::ALWAYS => Some(ApprovalDecisionType::AllowAlways),
            token::DENY => Some(ApprovalDecisionType::Deny),
            _ => None,
        }
    }

    /// Build inline keyboard for an approval request, rendering only the
    /// decisions the request permits.
    ///
    /// `allowed` is the request's permitted decision set (see
    /// [`crate::exec::allowed_decisions`]). A low-risk command yields
    /// `[Allow Once] [Allow Session]` / `[Deny]`; a blocked command offers only
    /// `Deny`.
    ///
    /// `Allow Always` is rendered **only** when `allowed` carries
    /// [`ApprovalDecisionType::AllowAlways`] — which
    /// [`crate::exec::allowed_decisions::for_confirm_gate`] grants to an
    /// operator-tier turn outside the declared-confirmation floor, and to
    /// nobody else. It used to be suppressed unconditionally, because there was
    /// no persistent allowlist to write to and the button would have promised a
    /// permanence the system could not deliver; there is one now
    /// (`sandbox::exec_approval::grants`), and the promise is kept by the same
    /// list this keyboard reads, enforced again at the resolver.
    #[must_use]
    pub fn build_approval_keyboard(
        approval_id: &str,
        allowed: &[ApprovalDecisionType],
    ) -> InlineKeyboard {
        let allow_row = [
            (ApprovalDecisionType::AllowOnce, "✅ Allow Once"),
            (ApprovalDecisionType::AllowSession, "✅ Allow Session"),
            (ApprovalDecisionType::AllowAlways, "♾️ Allow Always"),
        ]
        .into_iter()
        .filter(|(decision, _)| allowed.contains(decision))
        .map(|(decision, label)| InlineButton {
            text: label.into(),
            callback_data: format!("approve:{approval_id}:{}", Self::decision_token(&decision)),
        })
        .collect::<Vec<_>>();

        let mut keyboard = InlineKeyboard::new();
        if !allow_row.is_empty() {
            keyboard = keyboard.row(allow_row);
        }
        if allowed.contains(&ApprovalDecisionType::Deny) {
            keyboard = keyboard.button(
                "❌ Deny",
                format!(
                    "approve:{approval_id}:{}",
                    Self::decision_token(&ApprovalDecisionType::Deny)
                ),
            );
        }
        keyboard
    }

    /// Parse callback data into (`approval_id`, decision)
    ///
    /// Expected format: "approve:{id}:{decision}"
    /// where decision is "once", "session", "always", or "deny"
    #[must_use]
    pub fn parse_callback(data: &str) -> Option<(String, ApprovalDecisionType)> {
        // Strict three-field split (not `rsplit_once`). A callback id that
        // happens to contain a `:` — a future SessionKey-prefixed id, a
        // debugging format, an attacker payload — would be silently
        // truncated by `rsplit_once` and the manager would look up a
        // non-existent record. Three-field split rejects anything that is
        // not exactly `approve:<id>:<decision>`, so the wire format is the
        // contract instead of a convention. The Telegram callback is a
        // public surface (any user with access to the bot can craft a
        // callback), so the parser is the only line of defence against
        // spoofed callbacks.
        //
        // A fourth field cannot be detected here: `splitn(3, ':')` yields at
        // most three items, so the remainder arrives glued to the decision
        // token (`approve:id:once:junk` → `"once:junk"`) and is rejected by
        // the token lookup below, which knows no such token.
        let mut parts = data.splitn(3, ':');
        let prefix = parts.next()?;
        if prefix != "approve" {
            return None;
        }
        let id = parts.next()?;
        let decision_str = parts.next()?;
        if id.is_empty() {
            return None;
        }

        let approval_id = id.to_string();
        let decision = Self::decision_from_token(decision_str)?;

        Some((approval_id, decision))
    }

    /// Get the response text for a decision.
    ///
    /// `AllowAlways` reports a session grant — that is what the manager
    /// actually applies (see `ExecApprovalManager::clamp_decision`).
    #[must_use]
    pub const fn decision_response_text(decision: &ApprovalDecisionType) -> &'static str {
        match decision {
            ApprovalDecisionType::AllowOnce => "✅ Allowed (once)",
            ApprovalDecisionType::AllowSession => "✅ Allowed (session)",
            // The decision reaching here is post-clamp, so `AllowAlways` means
            // the card really did offer the persistent tier and the grant
            // really is durable. Echoing "(session)" would understate what the
            // user just did — and this echo IS the read-what-you-approved loop.
            ApprovalDecisionType::AllowAlways => "✅ Allowed (always — revocable in settings)",
            ApprovalDecisionType::Deny => "❌ Denied",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_allow_once() {
        let result = ApprovalBridge::parse_callback("approve:abc123:once");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "abc123");
        assert!(matches!(decision, ApprovalDecisionType::AllowOnce));
    }

    #[test]
    fn test_parse_callback_allow_session() {
        let result = ApprovalBridge::parse_callback("approve:sess42:session");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "sess42");
        assert!(matches!(decision, ApprovalDecisionType::AllowSession));
    }

    #[test]
    fn test_parse_callback_allow_always() {
        let result = ApprovalBridge::parse_callback("approve:xyz789:always");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "xyz789");
        assert!(matches!(decision, ApprovalDecisionType::AllowAlways));
    }

    #[test]
    fn test_build_keyboard_renders_session_button() {
        let allowed = vec![
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ];
        let kb = ApprovalBridge::build_approval_keyboard("req-1", &allowed);
        // The session callback must be present and round-trip through parse.
        let json = serde_json::to_string(&kb).unwrap();
        assert!(
            json.contains("approve:req-1:session"),
            "session button missing: {json}"
        );
        assert!(
            !json.contains("approve:req-1:always"),
            "a card raised at the session ceiling must not offer allow-always"
        );

        // And the other half of the same rule: when the gate DID offer the
        // persistent tier, the button is there — otherwise the tier would be
        // reachable from the Panel and dead on every channel.
        let kb = ApprovalBridge::build_approval_keyboard(
            "req-2",
            &crate::exec::allowed_decisions::with_persistent(),
        );
        let json = serde_json::to_string(&kb).unwrap();
        assert!(
            json.contains("approve:req-2:always"),
            "allow-always button missing when the card offered it: {json}"
        );
    }

    #[test]
    fn test_parse_callback_deny() {
        let result = ApprovalBridge::parse_callback("approve:test:deny");
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert!(matches!(decision, ApprovalDecisionType::Deny));
    }

    #[test]
    fn test_parse_callback_invalid() {
        assert!(ApprovalBridge::parse_callback("invalid").is_none());
        assert!(ApprovalBridge::parse_callback("approve:only_two").is_none());
        assert!(ApprovalBridge::parse_callback("other:id:once").is_none());
        assert!(ApprovalBridge::parse_callback("approve:id:unknown").is_none());
    }

    #[test]
    fn test_build_approval_keyboard_renders_exactly_the_offered_tiers() {
        // The legacy backfill set (`AllowOnce` / `AllowAlways` / `Deny`, no
        // session tier): every offered tier is rendered and nothing else is
        // invented. This test used to assert that `always` was suppressed
        // unconditionally — true while no persistent allowlist existed, and a
        // silent lie the moment one did.
        let keyboard = ApprovalBridge::build_approval_keyboard(
            "test123",
            &crate::exec::allowed_decisions::full_set(),
        );
        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(keyboard.rows[0].len(), 2); // Allow Once + Allow Always
        assert_eq!(keyboard.rows[1].len(), 1); // Deny
        assert!(keyboard.rows[0][0].callback_data.contains("test123"));
        assert!(keyboard.rows[0][0].callback_data.contains("once"));
        assert!(keyboard.rows[0][1].callback_data.ends_with(":always"));
        assert!(keyboard.rows[1][0].callback_data.contains("deny"));
        assert!(
            !keyboard
                .rows
                .iter()
                .flatten()
                .any(|b| b.callback_data.ends_with(":session")),
            "a tier the set did not carry must not appear"
        );
    }

    #[test]
    fn test_build_approval_keyboard_blocked_only_deny() {
        let keyboard =
            ApprovalBridge::build_approval_keyboard("blk1", &[ApprovalDecisionType::Deny]);
        assert_eq!(keyboard.rows.len(), 1);
        assert_eq!(keyboard.rows[0].len(), 1);
        assert!(keyboard.rows[0][0].callback_data.contains("deny"));
    }

    #[test]
    fn test_decision_response_text() {
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::AllowOnce),
            "✅ Allowed (once)"
        );
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::AllowSession),
            "✅ Allowed (session)"
        );
        // The decision reaching this renderer is post-clamp, so `AllowAlways`
        // means the card really offered the persistent tier and the grant
        // really is durable. Reporting "(session)" — which is what this
        // asserted while the tier was legacy — would understate what the user
        // just did, on the one echo that closes the read-what-you-approved loop.
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::AllowAlways),
            "✅ Allowed (always — revocable in settings)"
        );
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::Deny),
            "❌ Denied"
        );
    }
}
