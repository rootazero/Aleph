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

mod config;
mod policy;
mod types;

pub use config::{matches_glob, ConfigApprovalPolicy, PolicyConfig, PolicyRule};
pub use policy::ApprovalPolicy;
pub use types::{ActionRequest, ActionType, ApprovalDecision, DefaultDecision};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Helper to build a request for testing.
    fn make_request(action_type: ActionType, target: &str) -> ActionRequest {
        ActionRequest {
            action_type,
            target: target.to_string(),
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

        let defaults_map: HashMap<ActionType, DefaultDecision> =
            defaults.into_iter().collect();

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
            version: 1,
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
                (ActionType::ShellExec, DefaultDecision::Deny),
                (ActionType::DesktopClick, DefaultDecision::Ask),
            ],
            vec![],
            vec![],
        );

        // Allow
        let req = make_request(ActionType::BrowserNavigate, "https://google.com");
        assert_eq!(policy.check(&req).await, ApprovalDecision::Allow);

        // Deny
        let req = make_request(ActionType::ShellExec, "rm -rf /");
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
                (ActionType::ShellExec, "rm -rf **"),
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

        // Shell blocklist wildcard
        let req = make_request(ActionType::ShellExec, "rm -rf /important");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));

        // Malicious URL blocklist
        let req = make_request(
            ActionType::BrowserNavigate,
            "https://malicious.com/payload",
        );
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // File loading tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_load_missing_file_returns_default() {
        // ConfigApprovalPolicy::load() should not panic when the file is missing.
        // It falls back to default which allows browser navigate/click/type.
        let policy = ConfigApprovalPolicy::load();

        let req = make_request(ActionType::BrowserNavigate, "https://example.com");
        assert_eq!(policy.check(&req).await, ApprovalDecision::Allow);

        let req = make_request(ActionType::ShellExec, "ls");
        assert!(matches!(
            policy.check(&req).await,
            ApprovalDecision::Deny { .. }
        ));
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
            ActionType::ShellExec,
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
            "version": 1,
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
                "shell_exec": "deny"
            },
            "allowlist": [
                { "type": "browser_navigate", "pattern": "https://*.github.com/*" },
                { "type": "desktop_launch_app", "pattern": "com.apple.*" }
            ],
            "blocklist": [
                { "type": "shell_exec", "pattern": "rm -rf *" },
                { "type": "browser_navigate", "pattern": "*://malicious.com/*" }
            ]
        }"#;

        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.defaults.len(), 10);
        assert_eq!(config.allowlist.len(), 2);
        assert_eq!(config.blocklist.len(), 2);
        assert_eq!(
            config.defaults.get(&ActionType::ShellExec).unwrap(),
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
        let json = r#"{"version":1,"defaults":{"shell_exec":"Deny"},"allowlist":[],"blocklist":[]}"#;
        let result: Result<PolicyConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Serde should reject capitalized 'Deny'");
    }
}
