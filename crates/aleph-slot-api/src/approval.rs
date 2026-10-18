//! Approval contract — the decision vocabulary, the wire request shape, the
//! outcome/response pair, the grant-scope token, and the per-channel approval
//! delivery capability.
//!
//! Moved from `alephcore` in Task 4 of the P0 modularization:
//!
//! * `alephcore::exec::socket` — `ApprovalDecisionType` with `clamped_for` /
//!   `to_outcome_within` (the single decision → outcome mapping).
//! * `alephcore::exec::allowed_decisions` — `full_set` ONLY (the serde
//!   backfill set). The live decision-set derivations (`once_only`,
//!   `session_max`, `with_persistent`, `for_confirm_gate`, the floor rules)
//!   stay in Core — they encode gate policy, not wire contract.
//! * `alephcore::exec::approval::types` — `ApprovalRequest` /
//!   `CommandApprovalRequest` (the tagged wire shape).
//! * `alephcore::sandbox::exec_approval::gate` — `ApprovalOutcome` /
//!   `ApprovalResponse` (pure value types; the `ApprovalGate` runtime and the
//!   `ApprovalRequester` transport trait stay in Core).
//! * `alephcore::sandbox::exec_approval::grants` — `GrantScope` ONLY (the
//!   wire token). `Grant`, `GrantStore`, persistence and policy stay in Core.
//! * `alephcore::gateway::channel_approval` — the channel-side approval
//!   delivery surface (`ChannelApprovalCapability` and friends), distinct
//!   from the Core runtime `ApprovalAction` (sandbox exec approval) — the
//!   gateway `ApprovalAction` here is the Approve/Deny click vocabulary.
//!
//! Every old Core path is now a `pub use` re-export pointing here; both paths
//! share one `TypeId`, pinned by `alephcore`'s
//! `tests/modularization_contracts.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::channel::{ChannelResult, ConversationId, OutboundMessage, UserId};

// ---------------------------------------------------------------------------
// Decision vocabulary (was `alephcore::exec::socket`)
// ---------------------------------------------------------------------------

/// Type of approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecisionType {
    /// Allow this execution only
    AllowOnce,
    /// Allow for the remainder of this session — remembered in the grant
    /// store's session tier so the same command/tool is not re-prompted.
    /// Mirrors codex `ApprovedForSession` and hermes' `session` consent tier.
    AllowSession,
    /// Allow until revoked, across restarts — the persistent tier of the Core
    /// grant store.
    ///
    /// Honored **only** when the approval record's `allowed_decisions` offered
    /// it (the Core decision-set derivation); on every other card it narrows
    /// to [`Self::AllowSession`]. That narrowing is enforced server-side, at
    /// [`Self::clamped_for`], not by a renderer declining to draw the button:
    /// the wire value has always been accepted from external clients, so "no
    /// surface offers it" was never a control.
    AllowAlways,
    /// Deny execution
    Deny,
}

impl ApprovalDecisionType {
    /// Narrow the decision to a grant scope this particular request may honor.
    ///
    /// `allowed` is the decision set the card was *raised* with — one
    /// derivation, computed at the gate (the Core `allowed_decisions` module)
    /// and carried on the record — so the question "may this answer create a
    /// standing grant?" is answered where the gate knows the rule and the
    /// caller's tier, not where the button is drawn.
    ///
    /// Only the grant *scope* is narrowed: an explicit human approval is never
    /// escalated, and never turned into a denial. Narrowing twice is a no-op,
    /// so a defensive second call at a downstream surface is always safe.
    #[must_use]
    pub fn clamped_for(self, allowed: &[Self]) -> Self {
        // Walk DOWN the ladder one rung at a time rather than mapping straight
        // to the bottom: a card that offers `once` and `always` but not
        // `session` (the legacy backfill set) must narrow an unoffered tier to
        // the widest tier it DID offer, not to something it also never showed.
        let mut decision = self;
        loop {
            if allowed.contains(&decision) {
                return decision;
            }
            decision = match decision {
                Self::AllowAlways => Self::AllowSession,
                Self::AllowSession => Self::AllowOnce,
                // `AllowOnce` and `Deny` are the floor: a human's answer is
                // never escalated, and never turned into a refusal here.
                other => return other,
            };
        }
    }

    /// The [`ApprovalOutcome`] this decision resolves to, within the decision
    /// set the request offered — the SINGLE decision → outcome mapping for
    /// every approval surface (channel bridge, operator requester, cluster node
    /// approval).
    ///
    /// It takes `allowed` and there is no argument-free variant **on purpose**.
    /// The unsafe direction here is a surface that turns a widest-tier wire
    /// value into a persistent grant it was never authorized to create; making
    /// every call site name the set it offered turns that into a compile error
    /// rather than a rule someone has to remember (判据 §5.17: 编译错误强于
    /// 注册表 pin). Sites that never offer a standing grant pass the Core
    /// `session_max` or `once_only` sets.
    ///
    /// A missing decision (timeout / closed channel) is not representable
    /// here — callers map `None` to [`ApprovalOutcome::Timeout`] themselves.
    #[must_use]
    pub fn to_outcome_within(self, allowed: &[Self]) -> ApprovalOutcome {
        match self.clamped_for(allowed) {
            Self::AllowOnce => ApprovalOutcome::Approved,
            Self::AllowSession => ApprovalOutcome::ApprovedForSession,
            Self::AllowAlways => ApprovalOutcome::ApprovedAlways,
            Self::Deny => ApprovalOutcome::Denied,
        }
    }
}

// ---------------------------------------------------------------------------
// Serde backfill decision set (was part of `alephcore::exec::allowed_decisions`)
// ---------------------------------------------------------------------------

/// The legacy three-decision set (`Once` / `Always` / `Deny`).
///
/// Sole purpose: the serde default for
/// [`CommandApprovalRequest::allowed_decisions`], so payloads serialized
/// before that field existed deserialize to the historical (unconstrained)
/// behavior. Byte-stable on purpose — it backfills *old* payloads, so it must
/// NOT gain the newer `AllowSession` tier. Live requests build their own set
/// at the render site.
///
/// It is **not** the serde default for a pending record's set (the Core
/// `session_max` set is): a missing field must never widen what a decision
/// may become, and this one contains `AllowAlways`.
#[must_use]
pub fn full_set() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

// ---------------------------------------------------------------------------
// Wire request (was `alephcore::exec::approval::types`)
// ---------------------------------------------------------------------------

/// Approval request delivered to a channel's approval capability.
///
/// Kept as a tagged enum rather than a bare [`CommandApprovalRequest`] so the
/// serde shape (`{"type": "command", ...}`) stays stable for any consumer
/// holding serialized payloads. The `Capability` variant was removed: no
/// production code ever constructed it — only tests did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
}

/// Command approval request (placeholder for existing type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
    /// Why this approval is being requested (escalation/confirmation context).
    /// Rendered to the user so they can make an informed decision; absent on
    /// payloads serialized before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The decision tiers this request permits (derived once at the gate in
    /// Core). Renderers consult this instead of hardcoding a button set; the
    /// serde default backfills pre-existing payloads with the historical
    /// unconstrained set.
    #[serde(default = "crate::approval::full_set")]
    pub allowed_decisions: Vec<ApprovalDecisionType>,
}

// ---------------------------------------------------------------------------
// Outcome / response (was `alephcore::sandbox::exec_approval::gate`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// Approved for this single invocation only.
    Approved,
    /// Approved for the remainder of the session ("allow session").
    /// Treated as approved everywhere; additionally recorded in the grant store
    /// so the SAME ACTION — same tool, same arguments — is not re-prompted this
    /// session. A different action of the same tool still asks.
    ApprovedForSession,
    /// Approved until revoked, across restarts ("always allow"). Recorded in
    /// the persistent tier of the grant store.
    ///
    /// Only reachable when the approval record's `allowed_decisions` offered
    /// the tier — [`ApprovalDecisionType::to_outcome_within`] cannot produce it
    /// otherwise, which is why every decision→outcome call site has to name the
    /// set it offered.
    ApprovedAlways,
    Denied,
    Timeout,
    /// Fail-closed because **nobody could be asked** — no approval transport is
    /// wired, the channel refused or failed the delivery, no route resolves, or
    /// the run is unattended.
    ///
    /// # Why this is not `Denied`
    ///
    /// A refusal and the absence of a decision are different facts, and every
    /// consumer downstream of this enum was reading the first when it was handed
    /// the second. The Core denial ledger makes a `Denied` **sticky for the
    /// action for the rest of the session** and advances the brute-force
    /// breaker; the model is told "the user already declined this exact
    /// action". None of that is true when a Telegram delivery timed out. Three
    /// such hiccups paused the session, and the sentence the model relayed to
    /// the user was a lie about something the user never did.
    ///
    /// The ledger already draws exactly this line for its
    /// `DenialReason::Timeout` ("a timeout is not a decision") — this variant
    /// is what lets the same rule reach the refusals that arrive labelled as a
    /// person's answer. Security posture is unchanged: [`Self::is_approved`]
    /// is false, so every one of these still fails closed.
    Unavailable,
}

impl ApprovalOutcome {
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(
            self,
            Self::Approved | Self::ApprovedForSession | Self::ApprovedAlways
        )
    }

    /// The scope of the standing grant this outcome creates, if any — the one
    /// derivation of "how long does this yes last", so a surface cannot record
    /// a grant at a scope the outcome never carried.
    ///
    /// `None` for a one-shot approval and for every refusal.
    #[must_use]
    pub const fn grant_scope(&self) -> Option<GrantScope> {
        match self {
            Self::ApprovedForSession => Some(GrantScope::Session),
            Self::ApprovedAlways => Some(GrantScope::Always),
            _ => None,
        }
    }
}

/// A human's answer to an approval request: the outcome, plus any free-text
/// reason attached to a denial (`/deny <reason>` from a channel, the `reason`
/// field on `exec.approval.resolve`).
///
/// The reason is what turns a bare "denied" into an instruction the model can
/// act on — hermes ships the same affordance. Requesters whose transport
/// cannot carry one (buttons, auto-deny policies, test stubs) build the
/// response via `From<ApprovalOutcome>`, which leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResponse {
    pub outcome: ApprovalOutcome,
    /// Set only when `outcome` is [`ApprovalOutcome::Denied`] and the human
    /// supplied a reason.
    pub deny_reason: Option<String>,
}

impl From<ApprovalOutcome> for ApprovalResponse {
    fn from(outcome: ApprovalOutcome) -> Self {
        Self {
            outcome,
            deny_reason: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Grant scope token (was part of `alephcore::sandbox::exec_approval::grants`)
// ---------------------------------------------------------------------------

/// How long a grant lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// The remainder of one conversation, in this process.
    Session,
    /// Until revoked, across restarts.
    Always,
}

impl GrantScope {
    /// Stable token for wire payloads, RPC filters and tests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Always => "always",
        }
    }

    /// Parse a wire token. `None` for anything else — an unrecognized scope is
    /// never treated as the wider one.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "session" => Some(Self::Session),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Channel approval delivery surface (was `alephcore::gateway::channel_approval`)
// ---------------------------------------------------------------------------

/// Action that can be taken on an approval request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAction {
    /// Approve the request
    Approve,
    /// Deny the request
    Deny,
}

/// Result of an authorization check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationResult {
    /// Actor is authorized to take this action
    Authorized,
    /// Actor is explicitly denied
    Denied,
    /// Actor is not authenticated
    NotAuthenticated,
}

/// Rendered approval UI ready for delivery
#[derive(Debug, Clone)]
pub struct RenderedApproval {
    /// The outbound message with approval UI
    pub message: OutboundMessage,
    /// Callback data prefix for button clicks
    pub callback_prefix: String,
}

/// Pending approval tracked for callback resolution
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Unique approval ID for tracking
    pub approval_id: String,
    /// The original approval request
    pub request: ApprovalRequest,
    /// Channel ID where approval was delivered
    pub channel_id: String,
    /// Conversation ID where approval was sent
    pub conversation_id: ConversationId,
    /// Message ID of the approval message (for edits/cancellation)
    pub message_id: Option<String>,
    /// When the approval expires
    pub expires_at: DateTime<Utc>,
}

impl PendingApproval {
    /// Create a new pending approval
    pub fn new(
        approval_id: impl Into<String>,
        request: ApprovalRequest,
        channel_id: impl Into<String>,
        conversation_id: ConversationId,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            approval_id: approval_id.into(),
            request,
            channel_id: channel_id.into(),
            conversation_id,
            message_id: None,
            expires_at,
        }
    }

    /// Set the message ID after delivery
    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// Check if the approval has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at < Utc::now()
    }
}

/// Capability trait for channel-specific approval delivery and authorization.
///
/// Implement this trait to enable a channel to deliver approval requests to users
/// via its native UI (e.g., inline keyboards, interactive components) and authorize
/// actors based on channel-specific rules.
///
/// # Example
///
/// ```ignore
/// struct TelegramApprovalCapability {
///     channel: Arc<TelegramChannel>,
/// }
///
/// #[async_trait]
/// impl ChannelApprovalCapability for TelegramApprovalCapability {
///     async fn deliver_approval(&self, conv: &ConversationId, req: &ApprovalRequest, approval_id: &str) -> ChannelResult<PendingApproval> {
///         // Send message with inline keyboard
///         let keyboard = InlineKeyboard::new()
///             .button("✅ Approve", format!("approve:{}", req.id))
///             .button("❌ Deny", format!("deny:{}", req.id));
///
///         let message = OutboundMessage::text(chat_id, render_approval_text(req))
///             .with_inline_keyboard(keyboard);
///
///         let result = self.channel.send(message).await?;
///         // ...
///     }
///
///     async fn authorize_actor(&self, actor: &Actor, action: ApprovalAction) -> AuthorizationResult {
///         // Check if actor matches paired user
///     }
/// }
/// ```
#[async_trait]
pub trait ChannelApprovalCapability: Send + Sync {
    /// Deliver an approval request to the user.
    ///
    /// `approval_id` is the caller-owned id (the `ExecApprovalManager` record
    /// id). The capability MUST embed it verbatim into the button callback
    /// data so the click resolves the correct pending approval.
    ///
    /// Returns a `PendingApproval` that can be used to resolve or cancel later.
    async fn deliver_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        approval_id: &str,
    ) -> ChannelResult<PendingApproval>;

    /// Authorize an actor for an approval action.
    ///
    /// This is called before showing approval controls to verify the actor
    /// has permission to approve/deny. Return `AuthorizationResult::Authorized`
    /// to proceed, or `AuthorizationResult::Denied` to hide controls.
    async fn authorize_actor(
        &self,
        actor_user_id: &UserId,
        action: ApprovalAction,
    ) -> AuthorizationResult;

    /// Render the approval UI for a request.
    ///
    /// Returns the text and inline keyboard markup for displaying the approval.
    /// This is used both for initial delivery and for re-rendering after state changes.
    async fn render_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
    ) -> ChannelResult<RenderedApproval>;

    /// Render the approval UI for a specific actor, checking authorization first.
    ///
    /// If the actor is not authorized, returns `RenderedApproval` with no interactive buttons.
    /// If authorized, returns the full approval UI with approve/deny buttons.
    ///
    /// The `approval_id` is used in callback data to identify which approval is being resolved.
    async fn render_approval_for_actor(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        actor_user_id: &UserId,
        _approval_id: &str,
    ) -> ChannelResult<RenderedApproval> {
        // Default implementation: render normally and check authorization
        let rendered = self.render_approval(conversation_id, request).await?;
        let auth_result = self
            .authorize_actor(actor_user_id, ApprovalAction::Approve)
            .await;

        match auth_result {
            AuthorizationResult::Authorized => Ok(rendered),
            AuthorizationResult::Denied | AuthorizationResult::NotAuthenticated => {
                // Return rendered message without inline keyboard buttons
                let mut message = rendered.message.clone();
                message.inline_keyboard = None;
                Ok(RenderedApproval {
                    message,
                    callback_prefix: rendered.callback_prefix,
                })
            }
        }
    }

    /// Resolve a pending approval with the given action.
    ///
    /// Called when the user clicks an approve/deny button or when the
    /// approval times out. The channel should clean up any message state.
    async fn resolve_approval(
        &self,
        pending: &PendingApproval,
        action: ApprovalAction,
    ) -> ChannelResult<()>;
}

/// Extension trait for `Channel` to optionally expose approval capability.
pub trait ChannelApprovalExt {
    /// Get the approval capability for this channel, if supported.
    ///
    /// Returns `None` if the channel doesn't support approval delivery.
    /// Default implementation returns `None`.
    fn approval_capability(&self) -> Option<Arc<dyn ChannelApprovalCapability>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_request() -> ApprovalRequest {
        ApprovalRequest::Command(CommandApprovalRequest {
            command: "test_tool".to_string(),
            cwd: None,
            reason: None,
            allowed_decisions: vec![ApprovalDecisionType::AllowOnce],
        })
    }

    // -- moved from `alephcore::exec::socket` --

    #[test]
    fn test_approval_decision_types() {
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowOnce).unwrap(),
            r#""allow-once""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowSession).unwrap(),
            r#""allow-session""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowAlways).unwrap(),
            r#""allow-always""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::Deny).unwrap(),
            r#""deny""#
        );
    }

    // -- moved from `alephcore::exec::allowed_decisions` --

    #[test]
    fn full_set_stays_three_for_serde_backfill() {
        // `full_set` is the on-the-wire backfill default for pre-`allowed_decisions`
        // payloads and must remain the historical three tiers — adding the newer
        // `AllowSession` here would silently rewrite old payloads.
        let set = full_set();
        assert_eq!(set.len(), 3);
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::AllowAlways));
        assert!(set.contains(&ApprovalDecisionType::Deny));
        assert!(!set.contains(&ApprovalDecisionType::AllowSession));
    }

    // -- moved from `alephcore::sandbox::exec_approval::gate` --

    #[test]
    fn approval_outcome_is_approved() {
        assert!(ApprovalOutcome::Approved.is_approved());
        assert!(!ApprovalOutcome::Denied.is_approved());
        assert!(!ApprovalOutcome::Timeout.is_approved());
    }

    // -- moved from `alephcore::sandbox::exec_approval::grants` --

    #[test]
    fn scope_tokens_round_trip_and_reject_unknown() {
        assert_eq!(GrantScope::parse("session"), Some(GrantScope::Session));
        assert_eq!(GrantScope::parse("always"), Some(GrantScope::Always));
        assert_eq!(GrantScope::parse("forever"), None);
        assert_eq!(GrantScope::Always.as_str(), "always");
    }

    // -- moved from `alephcore::gateway::channel_approval` --

    #[test]
    fn test_pending_approval_creation() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() + chrono::Duration::minutes(5),
        );

        assert_eq!(pending.approval_id, "approval-123");
        assert_eq!(pending.channel_id, "telegram:bot");
        assert!(!pending.is_expired());
    }

    #[test]
    fn test_pending_approval_expired() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() - chrono::Duration::minutes(1),
        );

        assert!(pending.is_expired());
    }

    #[test]
    fn test_pending_approval_with_message_id() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .with_message_id("msg-789");

        assert_eq!(pending.message_id, Some("msg-789".to_string()));
    }

    #[test]
    fn test_authorization_result_variants() {
        assert!(matches!(
            AuthorizationResult::Authorized,
            AuthorizationResult::Authorized
        ));
        assert!(matches!(
            AuthorizationResult::Denied,
            AuthorizationResult::Denied
        ));
        assert!(matches!(
            AuthorizationResult::NotAuthenticated,
            AuthorizationResult::NotAuthenticated
        ));
    }

    #[test]
    fn test_approval_action_variants() {
        assert!(matches!(ApprovalAction::Approve, ApprovalAction::Approve));
        assert!(matches!(ApprovalAction::Deny, ApprovalAction::Deny));
    }
}
