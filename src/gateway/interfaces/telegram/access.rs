//! Policy-based access control for the Telegram channel.
//!
//! Centralizes all authorization logic (DM policy, group policy, pairing flow)
//! that was previously scattered across the message handler closure.

use super::config::PairingEntry;
use super::config_resolver::ResolvedConfig;
use super::config_v2::{DmPolicy, GroupPolicy};
use crate::resilience::StateDatabase;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// Result of an access check on an incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessDecision {
    /// User is authorized — process the message.
    Allowed,
    /// User is not yet authorized but may pair — prompt for a code.
    NeedsPairing,
    /// User is not authorized and cannot pair — silently drop.
    Denied,
}

/// Result of a pairing attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum PairingResult {
    /// Pairing succeeded — user is now authorized.
    Success,
    /// The code was valid but has expired.
    Expired,
    /// No matching code found.
    InvalidCode,
}

/// Centralized access controller for the Telegram channel.
///
/// Owns the runtime-paired user list, active pairing codes, and rate-limit
/// state for pairing prompts. Shared (via `Arc`) between the channel struct
/// and handler closures.
pub struct AccessController {
    resolved_config: ResolvedConfig,
    /// Users authorized at runtime via the pairing flow.
    runtime_users: Arc<RwLock<Vec<i64>>>,
    /// Active pairing codes: code → entry.
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    /// Rate-limit map: user_id → last prompt time.
    prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
    /// Optional state database for persisting paired users across restarts.
    db: Option<Arc<StateDatabase>>,
}

impl AccessController {
    /// Create a new controller from the resolved channel config.
    pub fn new(resolved_config: ResolvedConfig) -> Self {
        Self {
            resolved_config,
            runtime_users: Arc::new(RwLock::new(Vec::new())),
            pairing_codes: Arc::new(RwLock::new(HashMap::new())),
            prompt_times: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }

    /// Inject the state database for pairing persistence.
    ///
    /// Must be called **before** the controller is wrapped in `Arc`.
    pub fn set_state_database(&mut self, db: Arc<StateDatabase>) {
        self.db = Some(db);
    }

    /// Load previously-paired users from the database into the runtime set.
    ///
    /// Silently no-ops when no database is configured.
    pub async fn load_from_database(&self, channel_id: &str) {
        if let Some(ref db) = self.db {
            match db.load_paired_users(channel_id) {
                Ok(users) => {
                    let users: Vec<i64> = users;
                    if !users.is_empty() {
                        let mut runtime = self.runtime_users.write().await;
                        for uid in &users {
                            if !runtime.contains(uid) {
                                runtime.push(*uid);
                            }
                        }
                        tracing::info!(count = users.len(), "loaded paired users from database");
                    }
                }
                Err(e) => {
                    tracing::warn!("failed to load paired users from database: {}", e);
                }
            }
        }
    }

    /// Check whether a message should be processed, needs pairing, or is denied.
    pub async fn check_message(
        &self,
        user_id: i64,
        chat_id: i64,
        is_group: bool,
    ) -> AccessDecision {
        if is_group {
            self.check_group(chat_id).await
        } else {
            self.check_dm(user_id).await
        }
    }

    /// Attempt to pair a user with the given code.
    ///
    /// `channel_id` is used to persist the pairing to the database (when configured).
    pub async fn try_pair(&self, user_id: i64, code: &str, channel_id: &str) -> PairingResult {
        let normalized = code.trim().to_uppercase();
        let mut codes = self.pairing_codes.write().await;

        if let Some(entry) = codes.get(&normalized) {
            if entry.is_expired() {
                codes.remove(&normalized);
                return PairingResult::Expired;
            }
            codes.remove(&normalized);
            drop(codes);
            self.runtime_users.write().await.push(user_id);
            tracing::info!("User {} paired via code {}", user_id, normalized);

            // Persist to database so the pairing survives restarts.
            if let Some(ref db) = self.db {
                if let Err(e) = db.add_paired_user(channel_id, user_id) {
                    tracing::warn!("failed to persist paired user to database: {}", e);
                }
            }

            PairingResult::Success
        } else {
            PairingResult::InvalidCode
        }
    }

    /// Generate a new 6-character pairing code, cleaning up expired entries.
    pub async fn generate_code(&self) -> String {
        use rand::Rng;

        let code: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(6)
            .map(char::from)
            .collect::<String>()
            .to_uppercase();

        let entry = PairingEntry::new(code.clone());
        let mut codes = self.pairing_codes.write().await;
        codes.insert(code.clone(), entry);
        codes.retain(|_, e| !e.is_expired());

        code
    }

    /// List active (non-expired) pairing codes with remaining TTL in seconds.
    pub async fn list_codes(&self) -> Vec<(String, u64)> {
        let mut codes = self.pairing_codes.write().await;
        codes.retain(|_, e| !e.is_expired());
        codes
            .values()
            .map(|e| {
                let elapsed = e.created_at.elapsed().as_secs();
                let remaining = e.ttl_secs.saturating_sub(elapsed);
                (e.code.clone(), remaining)
            })
            .collect()
    }

    /// Whether we should show a pairing prompt to this user (rate-limited to
    /// once per 5 minutes).
    pub async fn should_prompt(&self, user_id: i64) -> bool {
        let mut times = self.prompt_times.write().await;
        let should = times
            .get(&user_id)
            .map(|t| t.elapsed().as_secs() > 300)
            .unwrap_or(true);
        if should {
            times.insert(user_id, Instant::now());
        }
        should
    }

    /// Reference to the runtime users list (for callback handler checks).
    pub fn runtime_users(&self) -> &Arc<RwLock<Vec<i64>>> {
        &self.runtime_users
    }

    /// Reference to the underlying resolved config.
    pub fn config(&self) -> &ResolvedConfig {
        &self.resolved_config
    }

    // --- Private helpers ---

    async fn check_dm(&self, user_id: i64) -> AccessDecision {
        match self.resolved_config.dm_policy.clone() {
            DmPolicy::Disabled => AccessDecision::Denied,
            DmPolicy::Open => AccessDecision::Allowed,
            DmPolicy::Allowlist => {
                if self.is_user_authorized(user_id).await {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
            DmPolicy::Pairing => {
                if self.is_user_authorized(user_id).await {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::NeedsPairing
                }
            }
        }
    }

    async fn check_group(&self, chat_id: i64) -> AccessDecision {
        match self.resolved_config.group_policy.clone() {
            GroupPolicy::Disabled => AccessDecision::Denied,
            GroupPolicy::Open => AccessDecision::Allowed,
            GroupPolicy::Allowlist => {
                if self.resolved_config.allowed_groups.is_empty()
                    || self.resolved_config.allowed_groups.contains(&chat_id)
                {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
        }
    }

    /// Check static allowlist + runtime-paired users.
    async fn is_user_authorized(&self, user_id: i64) -> bool {
        if self.resolved_config.allowed_users.contains(&user_id) {
            return true;
        }
        self.runtime_users.read().await.contains(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_v2::{ErrorPolicy, StreamingOptions};

    fn make_config(dm: DmPolicy, group: GroupPolicy, users: Vec<i64>) -> ResolvedConfig {
        ResolvedConfig {
            account_id: "main".to_string(),
            bot_token: "123:ABC".to_string(),
            bot_username: None,
            default_agent: None,
            dm_policy: dm,
            group_policy: group,
            send_typing: true,
            max_retries: 3,
            allowed_users: users,
            allowed_groups: vec![],
            streaming: StreamingOptions::default(),
            error_policy: ErrorPolicy::default(),
        }
    }

    #[tokio::test]
    async fn test_dm_disabled() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Disabled,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, false).await,
            AccessDecision::Denied,
        );
    }

    #[tokio::test]
    async fn test_dm_open() {
        let ctrl =
            AccessController::new(make_config(DmPolicy::Open, GroupPolicy::default(), vec![]));
        assert_eq!(
            ctrl.check_message(111, 111, false).await,
            AccessDecision::Allowed,
        );
    }

    #[tokio::test]
    async fn test_dm_pairing_unknown_user() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, false).await,
            AccessDecision::NeedsPairing,
        );
    }

    #[tokio::test]
    async fn test_dm_pairing_after_pair() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));

        // Generate a code and pair
        let code = ctrl.generate_code().await;
        let result = ctrl.try_pair(111, &code, "test-channel").await;
        assert_eq!(result, PairingResult::Success);

        // Now the user should be allowed
        assert_eq!(
            ctrl.check_message(111, 111, false).await,
            AccessDecision::Allowed,
        );
    }

    #[tokio::test]
    async fn test_dm_allowlist_allowed() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, false).await,
            AccessDecision::Allowed,
        );
    }

    #[tokio::test]
    async fn test_dm_allowlist_denied() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(
            ctrl.check_message(999, 999, false).await,
            AccessDecision::Denied,
        );
    }

    #[tokio::test]
    async fn test_group_disabled() {
        let config = ResolvedConfig {
            account_id: "main".to_string(),
            bot_token: "123:ABC".to_string(),
            bot_username: None,
            default_agent: None,
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::Disabled,
            send_typing: true,
            max_retries: 3,
            allowed_users: vec![],
            allowed_groups: vec![],
            streaming: StreamingOptions::default(),
            error_policy: ErrorPolicy::default(),
        };
        let ctrl = AccessController::new(config);
        assert_eq!(
            ctrl.check_message(111, -100123, true).await,
            AccessDecision::Denied,
        );
    }

    #[tokio::test]
    async fn test_group_open() {
        let ctrl =
            AccessController::new(make_config(DmPolicy::default(), GroupPolicy::Open, vec![]));
        assert_eq!(
            ctrl.check_message(111, -100123, true).await,
            AccessDecision::Allowed,
        );
    }

    #[tokio::test]
    async fn test_group_allowlist_empty_allows_all() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::default(),
            GroupPolicy::Allowlist,
            vec![],
        ));
        // Empty allowed_groups with Allowlist policy → allow all
        assert_eq!(
            ctrl.check_message(111, -100123, true).await,
            AccessDecision::Allowed,
        );
    }

    #[tokio::test]
    async fn test_group_allowlist_denied() {
        let config = ResolvedConfig {
            account_id: "main".to_string(),
            bot_token: "123:ABC".to_string(),
            bot_username: None,
            default_agent: None,
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::Allowlist,
            send_typing: true,
            max_retries: 3,
            allowed_users: vec![],
            allowed_groups: vec![-100111],
            streaming: StreamingOptions::default(),
            error_policy: ErrorPolicy::default(),
        };
        let ctrl = AccessController::new(config);
        assert_eq!(
            ctrl.check_message(111, -100999, true).await,
            AccessDecision::Denied,
        );
    }

    #[tokio::test]
    async fn test_pairing_expired_code() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));

        // Insert a code and backdate it
        let code = ctrl.generate_code().await;
        {
            let mut codes = ctrl.pairing_codes.write().await;
            if let Some(entry) = codes.get_mut(&code) {
                entry.created_at = Instant::now() - std::time::Duration::from_secs(3601);
            }
        }

        assert_eq!(
            ctrl.try_pair(111, &code, "test-channel").await,
            PairingResult::Expired
        );
    }

    #[tokio::test]
    async fn test_pairing_invalid_code() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.try_pair(111, "NOPE99", "test-channel").await,
            PairingResult::InvalidCode,
        );
    }

    #[tokio::test]
    async fn test_should_prompt_rate_limit() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));

        // First call should prompt
        assert!(ctrl.should_prompt(111).await);
        // Immediate second call should be rate-limited
        assert!(!ctrl.should_prompt(111).await);
    }

    #[tokio::test]
    async fn test_list_codes() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));

        let code = ctrl.generate_code().await;
        let list = ctrl.list_codes().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, code);
        assert!(list[0].1 > 0); // remaining TTL > 0
    }

    #[tokio::test]
    async fn test_backward_compat_allowlist_promotion() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![123],
        ));
        assert_eq!(
            ctrl.check_message(123, 123, false).await,
            AccessDecision::Allowed,
        );
        assert_eq!(
            ctrl.check_message(999, 999, false).await,
            AccessDecision::Denied,
        );
    }
}
