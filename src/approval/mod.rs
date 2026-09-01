//! Approval module for desktop and browser action authorization.
//!
//! This module provides a policy-driven approval system that decides whether
//! agent-initiated actions (browser navigation, desktop clicks, shell commands,
//! etc.) should be allowed, denied, or escalated for user confirmation.
//!
//! # Architecture
//!
//! ```text
//! ActionRequest ──▶ ApprovalPolicy::check() ──▶ ApprovalDecision
//!                        │
//!                   ┌────┴────┐
//!                   │ Config  │  (blocklist → allowlist → defaults → ask)
//!                   └─────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use alephcore::approval::{
//!     ActionRequest, ActionType, ApprovalDecision,
//!     ApprovalPolicy, ConfigApprovalPolicy,
//! };
//! use chrono::Utc;
//!
//! # async fn example() {
//! let policy = ConfigApprovalPolicy::load();
//!
//! let request = ActionRequest {
//!     action_type: ActionType::BrowserNavigate,
//!     target: "https://github.com".to_string(),
//!     display_target: "https://github.com".to_string(),
//!     agent_id: "agent-1".to_string(),
//!     context: "Opening GitHub".to_string(),
//!     timestamp: Utc::now(),
//! };
//!
//! match policy.check(&request).await {
//!     ApprovalDecision::Allow => { /* proceed */ }
//!     ApprovalDecision::Deny { reason } => { /* abort */ }
//!     ApprovalDecision::Ask { prompt } => { /* ask user */ }
//! }
//! # }
//! ```

pub mod adapters;
mod audit;
pub mod callback_sink;
mod config;
pub mod guardian_requester;
pub mod node_requester;
pub mod operator_requester;
mod policy;
mod session_route;
pub mod tool_call;
mod types;

pub use audit::audit_identity;
pub use node_requester::run_node_approval;
pub use operator_requester::OperatorApprovalRequester;
pub use tool_call::{
    current_call_identity, current_tool_call_id, with_call_identity, CallIdentity,
};

pub use config::{matches_glob, ConfigApprovalPolicy, PolicyConfig, PolicyRule};
pub use policy::ApprovalPolicy;
pub use types::{ActionRequest, ActionType, ApprovalDecision, DefaultDecision};

/// Re-resolve a policy-level `Ask` against the ambient turn's trust facts.
///
/// The five tool-internal policy gates (`system`, `desktop`, `automation`,
/// `media`, `pim`) consume a [`ConfigApprovalPolicy`] decision directly, and
/// none of them has an interactive consumer for `Ask` — on a
/// policy-file-absent install their `Ask` arm is a refusal string, which made
/// the curated `DesktopLaunchApp = Ask` default a silent, permanent denial of
/// `system.open_path` (FEATURE_LOCATOR §7.3 item ⓒ). Two operator rulings:
///
/// * 2026-08-27 (ⓒ): the Full exec tier IS the operator's answer to the ask —
///   `Ask` lifts to `Allow` under an ambient Full tier. The tier is resolved
///   upstream of every clamp (channel ceiling, non-operator ceiling,
///   side-question floor), so a caller who could not legitimately hold Full
///   never reaches this arm.
/// * 2026-08-27 (second ruling, verbatim: "打开浏览器是非常重要的功能，必须授权
///   Aleph 使用。包括启动任何软件，都不能限制"): `DesktopLaunchApp` — opening
///   a file/URL in the default app, launching any application — is not gated
///   AT ALL for an operator call, at any tier — but only for an ATTENDED turn
///   (`TurnContext::unattended` excludes cron/goal/heartbeat runs: a ruling
///   about the operator's browser grants nothing to a run with no human on
///   any surface). The tier gate (World B) still
///   applies first: an operator at the Ask tier gets the interactive card, at
///   Plan the call never reaches the tool. This lift only retires the
///   fail-dead World A refusal that fired *after* World B had already let the
///   call through. Guests and members (`caller_role` other than operator)
///   keep the pre-existing posture — launching apps on the host stays out of
///   channel reach.
///
/// Every other combination passes through unchanged: `Ask` under `Ask`/`Auto`
/// for a non-launch action or a non-operator caller keeps the fail-closed
/// posture, and `Allow`/`Deny` are never touched. Each lift is logged so the
/// audit trail can tell "the policy file allowed this" from "a ruling did".
///
/// Call it on the decision right after `ApprovalPolicy::check`, before the
/// match — all five gates do — and let the existing `Allow` arm do the
/// `policy.record` audit write.
#[must_use]
pub fn lift_ask(decision: ApprovalDecision, action_type: ActionType) -> ApprovalDecision {
    use crate::config::types::policies::ExecTier;
    let ApprovalDecision::Ask { ref prompt } = decision else {
        return decision;
    };

    // Ruling 1: Full tier answers every ask.
    if crate::tools::turn_context::current_exec_tier() == Some(ExecTier::Full) {
        tracing::info!(
            prompt = %prompt,
            "Approval Ask lifted to Allow: conversation runs under the Full exec tier"
        );
        return ApprovalDecision::Allow;
    }

    // Ruling 2: launching/opening is unrestricted for the operator. An
    // UNATTENDED run (cron, goal/loop continuation, heartbeat) does NOT lift
    // even when its context reads as operator — a ruling about the operator's
    // browser grants no silent app-launch capability to a run with no human
    // on any surface. Absent turn context (internal runs) does not lift
    // either.
    if action_type == ActionType::DesktopLaunchApp
        && crate::tools::turn_context::current_turn_context()
            .is_some_and(|t| t.caller_is_operator() && !t.unattended)
    {
        tracing::info!(
            prompt = %prompt,
            "Approval Ask lifted to Allow: DesktopLaunchApp is unrestricted for the operator"
        );
        return ApprovalDecision::Allow;
    }

    decision
}

/// The approval timeout for the turn currently executing a tool.
///
/// Ruled 2026-08-28 (verbatim: "不要使用超时，应该使用通知+永久等待"):
/// an ATTENDED turn — a human is on some surface (Panel, Telegram, …) — gets
/// [`crate::exec::manager::NO_APPROVAL_TIMEOUT`]: the card is raised, the
/// notification is delivered, and the call parks until somebody answers. The
/// 120 s deadline's failure mode was worse than the wait: the card expired
/// while the human was reading, the tool call failed with "nobody answered",
/// and the model worked around the gate entirely (observed in s143, where two
/// `file_ops` approvals expired unanswered mid-conversation).
///
/// An UNATTENDED turn (cron, goal/loop continuation, heartbeat, A2A) keeps the
/// bounded [`crate::exec::manager::DEFAULT_APPROVAL_TIMEOUT_MS`] fail-closed
/// posture: nobody will ever answer, so parking forever would only wedge the
/// run. Absent turn context (internal paths) reads as unattended here.
#[must_use]
pub fn approval_timeout_for_current_turn() -> u64 {
    match crate::tools::turn_context::current_turn_context() {
        Some(t) if !t.unattended => crate::exec::manager::NO_APPROVAL_TIMEOUT,
        _ => crate::exec::manager::DEFAULT_APPROVAL_TIMEOUT_MS,
    }
}

/// How long to leave a parked, unanswered approval alone before re-raising the
/// operator's interrupt, and the same answer for every reminder after it.
///
/// The no-timeout ruling removed the deadline; it did not remove the way a card
/// gets missed. What replaced "the card expires" is "the card waits" — which is
/// only an improvement if the human eventually looks, and the one thing that
/// fetches a human who has walked away fires exactly once, at raise time
/// (`r5_router` → `surface.approval` → the shell's OS banner). A persistent card
/// nobody is looking at is not a notification.
///
/// The schedule backs off — 2 min, 5 min, then every 15 min — because the two
/// failure modes are not symmetric. Under-reminding costs a wait the user did
/// not intend; over-reminding trains them to dismiss the banner, which costs
/// every future approval. A fixed short interval would ring dozens of times
/// against a card left overnight; the cap keeps a very long park at four
/// interrupts an hour.
///
/// The last entry repeats forever: the list is a schedule, not a budget. Giving
/// up after the last entry would reintroduce the silent timeout in the one place
/// it is least visible — the run would still be parked, but nothing would ever
/// say so again.
///
/// Reminders are raised ONLY for a card with no deadline
/// ([`crate::exec::manager::NO_APPROVAL_TIMEOUT`]); see
/// [`reminder_schedule`].
pub const APPROVAL_REMINDER_BACKOFF_SECS: &[u64] = &[120, 300, 900];

/// The reminder schedule for a wait of `timeout`, as an endless sequence of
/// sleeps between successive re-announcements.
///
/// `None` — no reminders — for a BOUNDED wait, and that is the whole predicate:
/// a bounded card ends on its own, so a reminder can only fire against a card
/// that is about to stop mattering. Deriving it from the deadline the wait is
/// actually running under (rather than re-asking "is this turn attended?") keeps
/// one answer to "does anything besides a human end this wait" — a second
/// derivation could disagree with the first, and the disagreement would be
/// invisible in both directions: reminders for a card that expires anyway, or
/// silence on one that never will.
pub fn reminder_schedule(
    timeout: Option<std::time::Duration>,
) -> Option<impl Iterator<Item = std::time::Duration>> {
    if timeout.is_some() {
        return None;
    }
    let last = *APPROVAL_REMINDER_BACKOFF_SECS
        .last()
        .expect("APPROVAL_REMINDER_BACKOFF_SECS is a non-empty literal");
    Some(
        APPROVAL_REMINDER_BACKOFF_SECS
            .iter()
            .copied()
            .chain(std::iter::repeat(last))
            .map(std::time::Duration::from_secs),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Helper to build a request for testing.
    fn make_request(action_type: ActionType, target: &str) -> ActionRequest {
        ActionRequest {
            action_type,
            target: target.to_string(),
            display_target: String::new(),
            agent_id: "test-agent".to_string(),
            context: "test context".to_string(),
            timestamp: Utc::now(),
        }
    }

    /// Helper to build a policy with custom config.
    fn make_policy(
        defaults: Vec<(ActionType, DefaultDecision)>,
        allowlist: Vec<(ActionType, &str)>,
        blocklist: Vec<(ActionType, &str)>,
    ) -> ConfigApprovalPolicy {
        use std::collections::HashMap;

        let defaults_map: HashMap<ActionType, DefaultDecision> = defaults.into_iter().collect();

        let allowlist_rules: Vec<PolicyRule> = allowlist
            .into_iter()
            .map(|(action_type, pattern)| PolicyRule {
                action_type,
                pattern: pattern.to_string(),
            })
            .collect();

        let blocklist_rules: Vec<PolicyRule> = blocklist
            .into_iter()
            .map(|(action_type, pattern)| PolicyRule {
                action_type,
                pattern: pattern.to_string(),
            })
            .collect();

        ConfigApprovalPolicy::new(PolicyConfig {
            defaults: defaults_map,
            allowlist: allowlist_rules,
            blocklist: blocklist_rules,
        })
    }

    // -----------------------------------------------------------------------
    // Decision priority tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_blocklist_takes_priority() {
        // Even though the default is "allow", a blocklist match should deny.
        let policy = make_policy(
            vec![(ActionType::BrowserNavigate, DefaultDecision::Allow)],
            vec![(ActionType::BrowserNavigate, "https://*.example.com/*")],
            vec![(ActionType::BrowserNavigate, "https://evil.example.com/*")],
        );

        let req = make_request(
            ActionType::BrowserNavigate,
            "https://evil.example.com/phish",
        );
        let decision = policy.check(&req).await;

        assert!(
            matches!(decision, ApprovalDecision::Deny { .. }),
            "Blocklist should override both allowlist and defaults"
        );
    }

    #[tokio::test]
    async fn test_allowlist_overrides_default() {
        // Default is "ask", but allowlist should let it through.
        let policy = make_policy(
            vec![(ActionType::DesktopLaunchApp, DefaultDecision::Ask)],
            vec![(ActionType::DesktopLaunchApp, "com.apple.*")],
            vec![],
        );

        let req = make_request(ActionType::DesktopLaunchApp, "com.apple.Safari");
        let decision = policy.check(&req).await;

        assert_eq!(decision, ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn test_default_decision() {
        let policy = make_policy(
            vec![
                (ActionType::BrowserNavigate, DefaultDecision::Allow),
                (ActionType::DesktopAutomation, DefaultDecision::Deny),
                (ActionType::DesktopClick, DefaultDecision::Ask),
            ],
            vec![],
            vec![],
        );

        // Allow
        let req = make_request(ActionType::BrowserNavigate, "https://google.com");
        assert_eq!(policy.check(&req).await, ApprovalDecision::Allow);

        // Deny
        let req = make_request(ActionType::DesktopAutomation, "rm -rf /");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));

        // Ask
        let req = make_request(ActionType::DesktopClick, "some-target");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Ask { .. }
        ));
    }

    #[tokio::test]
    async fn test_missing_default_returns_ask() {
        // No defaults at all → should ask.
        let policy = make_policy(vec![], vec![], vec![]);

        let req = make_request(ActionType::BrowserEvaluate, "document.title");
        let decision = policy.check(&req).await;

        assert!(
            matches!(decision, ApprovalDecision::Ask { .. }),
            "Missing default should return Ask"
        );
    }

    // -----------------------------------------------------------------------
    // Glob pattern tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_glob_patterns() {
        let policy = make_policy(
            vec![(ActionType::BrowserNavigate, DefaultDecision::Deny)],
            vec![
                (ActionType::BrowserNavigate, "https://*.github.com/**"),
                (ActionType::DesktopLaunchApp, "com.apple.*"),
            ],
            vec![
                (ActionType::DesktopAutomation, "rm -rf **"),
                (ActionType::BrowserNavigate, "*://malicious.com/*"),
            ],
        );

        // URL pattern allowlist
        let req = make_request(
            ActionType::BrowserNavigate,
            "https://docs.github.com/en/actions",
        );
        assert_eq!(policy.check(&req).await, ApprovalDecision::Allow);

        // Bundle ID pattern allowlist
        let req = make_request(ActionType::DesktopLaunchApp, "com.apple.TextEdit");
        assert_eq!(policy.check(&req).await, ApprovalDecision::Allow);

        // Automation-script blocklist wildcard
        let req = make_request(ActionType::DesktopAutomation, "rm -rf /important");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));

        // Malicious URL blocklist
        let req = make_request(ActionType::BrowserNavigate, "https://malicious.com/payload");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // File loading tests
    // -----------------------------------------------------------------------

    /// A missing policy file yields the CURATED defaults, not all-Ask.
    ///
    /// The first assertion used to demand `Ask` for `BrowserNavigate` on the
    /// ground that a missing file "must escalate to Ask (safe default), not
    /// silently Allow". That reading only holds if `Ask` is a question someone
    /// can answer — and at this layer it is not: `check_browser_approval`
    /// turns `Ask` into a refusal *string* handed back to the model, and
    /// nothing in this repo ever writes `~/.aleph/approval-policy.json`, so
    /// the user has nothing to edit and no card to click. What the old
    /// assertion pinned was therefore not a gate but a wall: every browser
    /// entry point refused on every install that had not hand-written a policy
    /// file. See `ConfigApprovalPolicy::load_from` for the split by cause —
    /// file ABSENT takes the curated map, file BROKEN still takes all-Ask.
    ///
    /// The four desktop/pim assertions below are unchanged and are the point
    /// of keeping this test: they prove the loosening is scoped to browser
    /// motion and did not leak into the action families that genuinely need a
    /// human.
    #[tokio::test]
    async fn test_load_missing_file_yields_curated_defaults() {
        let temp_path = std::env::temp_dir().join("aleph-test-approval-nonexistent");
        let policy = ConfigApprovalPolicy::load_from(temp_path.join("policy.json"));

        let req = make_request(ActionType::BrowserNavigate, "https://example.com");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Allow),
            "Missing policy file must fall back to the curated map, where browser \
             navigation is Allow — an unconfigured install has to be able to browse"
        );

        let req = make_request(ActionType::BrowserEvaluate, "fetch('/admin')");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "…while the powerful browser verbs stay Ask in that same map"
        );

        let req = make_request(ActionType::DesktopClick, "click(10,20)");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "Missing policy file must escalate DesktopClick to Ask"
        );

        let req = make_request(ActionType::DesktopLaunchApp, "com.apple.Safari");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "Missing policy file must escalate DesktopLaunchApp to Ask"
        );

        let req = make_request(ActionType::DesktopAutomation, "rm -rf /");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "Missing policy file must escalate DesktopAutomation to Ask"
        );

        let req = make_request(ActionType::PimWrite, "delete all notes");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "Missing policy file must escalate PimWrite to Ask"
        );
    }

    // -----------------------------------------------------------------------
    // Serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_action_type_serialization() {
        // Ensure snake_case round-trip works.
        let action = ActionType::BrowserNavigate;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"browser_navigate\"");

        let parsed: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ActionType::BrowserNavigate);

        // All variants round-trip
        let all = vec![
            ActionType::BrowserNavigate,
            ActionType::BrowserClick,
            ActionType::BrowserType,
            ActionType::BrowserFill,
            ActionType::BrowserEvaluate,
            ActionType::DesktopClick,
            ActionType::DesktopType,
            ActionType::DesktopKeyCombo,
            ActionType::DesktopLaunchApp,
            ActionType::DesktopAutomation,
            ActionType::PimWrite,
            ActionType::MediaCapture,
        ];

        for action in all {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: ActionType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn test_policy_config_deserialization() {
        let json = r#"{
            "defaults": {
                "browser_navigate": "allow",
                "browser_click": "allow",
                "browser_type": "allow",
                "browser_fill": "allow",
                "browser_evaluate": "ask",
                "desktop_click": "ask",
                "desktop_type": "ask",
                "desktop_key_combo": "ask",
                "desktop_launch_app": "ask",
                "desktop_automation": "deny"
            },
            "allowlist": [
                { "type": "browser_navigate", "pattern": "https://*.github.com/*" },
                { "type": "desktop_launch_app", "pattern": "com.apple.*" }
            ],
            "blocklist": [
                { "type": "desktop_automation", "pattern": "do shell script*" },
                { "type": "browser_navigate", "pattern": "*://malicious.com/*" }
            ]
        }"#;

        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.defaults.len(), 10);
        assert_eq!(config.allowlist.len(), 2);
        assert_eq!(config.blocklist.len(), 2);
        assert_eq!(
            config.defaults.get(&ActionType::DesktopAutomation).unwrap(),
            &DefaultDecision::Deny
        );
    }

    // -----------------------------------------------------------------------
    // Record (audit) test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_record_does_not_panic() {
        let policy = ConfigApprovalPolicy::default();
        let req = make_request(ActionType::BrowserNavigate, "https://example.com");
        let decision = ApprovalDecision::Allow;

        // Should not panic — currently just logs via tracing::debug.
        policy.record(&req, &decision).await;
    }

    // -----------------------------------------------------------------------
    // Serde validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_invalid_default_value_rejected_by_serde() {
        let json = r#"{"defaults":{"desktop_click":"Deny"},"allowlist":[],"blocklist":[]}"#;
        let result: Result<PolicyConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Serde should reject capitalized 'Deny'");
    }

    #[test]
    fn test_action_type_display() {
        assert_eq!(ActionType::BrowserNavigate.to_string(), "browser navigate");
        assert_eq!(
            ActionType::DesktopLaunchApp.to_string(),
            "desktop launch app"
        );
    }
}

#[cfg(test)]
mod lift_tests {
    use super::*;
    use crate::config::types::policies::ExecTier;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT, TURN_EXEC_TIER};

    fn ask() -> ApprovalDecision {
        ApprovalDecision::Ask {
            prompt: "Action desktop launch app on target '/tmp/x' requires approval".to_string(),
        }
    }

    fn turn_ctx(caller_role: Option<&str>) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("main"),
            run_id: "run-1".to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: caller_role.map(str::to_string),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    /// Ruling 1: a conversation the operator set to Full has already answered
    /// the ask — the fail-dead refusal must lift.
    #[tokio::test]
    async fn ask_under_full_tier_lifts_to_allow() {
        TURN_EXEC_TIER
            .scope(ExecTier::Full, async {
                assert_eq!(
                    lift_ask(ask(), ActionType::DesktopType),
                    ApprovalDecision::Allow
                );
            })
            .await;
    }

    /// Ask and Auto keep the pre-existing posture for gated actions — ruling 1
    /// is the Full tier's contract, not a general weakening of the gate.
    #[tokio::test]
    async fn ask_under_other_tiers_is_untouched() {
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Plan] {
            TURN_EXEC_TIER
                .scope(tier, async {
                    assert!(matches!(
                        lift_ask(ask(), ActionType::DesktopType),
                        ApprovalDecision::Ask { .. }
                    ));
                })
                .await;
        }
    }

    /// No ambient tier and no turn context (cron, internal runs) —
    /// fail-closed, nothing lifts. An unattended run must not gain silent
    /// app-launch capability from ruling 2.
    #[tokio::test]
    async fn ask_without_ambient_context_is_untouched() {
        assert!(matches!(
            lift_ask(ask(), ActionType::DesktopLaunchApp),
            ApprovalDecision::Ask { .. }
        ));
    }

    /// Ruling 2 (verbatim: "打开浏览器是非常重要的功能…包括启动任何软件，都不能
    /// 限制"): DesktopLaunchApp lifts for an operator at ANY tier — including
    /// no tier at all, the default Auto-ish session the ruling came from.
    /// Loopback carries `caller_role: None`, which `caller_is_operator`
    /// reads as trusted.
    #[tokio::test]
    async fn desktop_launch_lifts_for_operator_at_any_tier() {
        for role in [None, Some("operator")] {
            TURN_CONTEXT
                .scope(turn_ctx(role), async {
                    assert_eq!(
                        lift_ask(ask(), ActionType::DesktopLaunchApp),
                        ApprovalDecision::Allow,
                        "operator role {role:?} must lift DesktopLaunchApp"
                    );
                })
                .await;
        }
    }

    /// Ruling 2 is operator-only: a guest (channel member) keeps the
    /// fail-closed posture — launching apps on the host stays out of channel
    /// reach.
    #[tokio::test]
    async fn desktop_launch_stays_gated_for_guest() {
        TURN_CONTEXT
            .scope(turn_ctx(Some("guest")), async {
                assert!(matches!(
                    lift_ask(ask(), ActionType::DesktopLaunchApp),
                    ApprovalDecision::Ask { .. }
                ));
            })
            .await;
    }

    /// Ruling 2 excludes unattended runs: a cron/goal/heartbeat turn whose
    /// context reads as operator (loopback `None` role) must NOT gain silent
    /// app-launch capability — no human is on any surface to mean it.
    #[tokio::test]
    async fn desktop_launch_stays_gated_when_unattended() {
        let mut ctx = turn_ctx(None);
        ctx.unattended = true;
        TURN_CONTEXT
            .scope(ctx, async {
                assert!(matches!(
                    lift_ask(ask(), ActionType::DesktopLaunchApp),
                    ApprovalDecision::Ask { .. }
                ));
            })
            .await;
    }

    /// Ruling 2 is launch-only: every other gated action (typing, clipboard,
    /// capture) keeps its posture for the operator — "open any app" is not
    /// "do anything on the desktop".
    #[tokio::test]
    async fn other_actions_stay_gated_for_operator() {
        TURN_CONTEXT
            .scope(turn_ctx(None), async {
                assert!(matches!(
                    lift_ask(ask(), ActionType::DesktopType),
                    ApprovalDecision::Ask { .. }
                ));
            })
            .await;
    }

    /// Allow and Deny are never rewritten — the lift answers asks, it does
    /// not second-guess verdicts.
    #[tokio::test]
    async fn allow_and_deny_pass_through_under_full() {
        TURN_EXEC_TIER
            .scope(ExecTier::Full, async {
                assert_eq!(
                    lift_ask(ApprovalDecision::Allow, ActionType::DesktopLaunchApp),
                    ApprovalDecision::Allow
                );
                let deny = ApprovalDecision::Deny {
                    reason: "blocked".to_string(),
                };
                assert!(matches!(
                    lift_ask(deny, ActionType::DesktopLaunchApp),
                    ApprovalDecision::Deny { .. }
                ));
            })
            .await;
    }
}

#[cfg(test)]
mod reminder_tests {
    use super::*;
    use std::time::Duration;

    /// A bounded wait ends on its own, so nothing needs re-announcing. The
    /// predicate is the DEADLINE, not the turn's attendedness: re-asking
    /// "is this attended?" here would be a second derivation, free to disagree
    /// with the one that actually chose the timeout.
    #[test]
    fn a_bounded_wait_raises_no_reminders() {
        assert!(reminder_schedule(Some(Duration::from_secs(120))).is_none());
        assert!(reminder_schedule(Some(Duration::ZERO)).is_none());
    }

    /// The unbounded wait backs off and then repeats its last step FOREVER.
    /// A schedule that ran out would be the silent timeout again in its least
    /// visible form: still parked, and nothing left to say so.
    #[test]
    fn an_unbounded_wait_backs_off_then_repeats_the_last_step() {
        let got: Vec<u64> = reminder_schedule(None)
            .expect("no deadline must schedule reminders")
            .take(6)
            .map(|d| d.as_secs())
            .collect();
        assert_eq!(got, vec![120, 300, 900, 900, 900, 900]);
    }

    /// The first reminder must land AFTER the bounded default would have
    /// expired. Otherwise a wait that is about to end on its own could still
    /// ring — the one case `reminder_schedule` returns `None` for, arriving by
    /// a second route if the constants ever drift toward each other.
    #[test]
    fn the_first_reminder_is_later_than_the_bounded_default() {
        let first = *APPROVAL_REMINDER_BACKOFF_SECS
            .first()
            .expect("non-empty literal");
        assert!(
            first * 1000 >= crate::exec::manager::DEFAULT_APPROVAL_TIMEOUT_MS,
            "first reminder at {first}s would fire inside the {}ms bounded wait",
            crate::exec::manager::DEFAULT_APPROVAL_TIMEOUT_MS
        );
    }

    /// The schedule is monotonically non-decreasing. A dip would mean the
    /// interrupts get MORE frequent the longer a card is ignored, which is the
    /// shape that trains a user to dismiss the banner.
    #[test]
    fn the_backoff_never_decreases() {
        for w in APPROVAL_REMINDER_BACKOFF_SECS.windows(2) {
            assert!(
                w[0] <= w[1],
                "backoff dips: {APPROVAL_REMINDER_BACKOFF_SECS:?}"
            );
        }
    }
}
