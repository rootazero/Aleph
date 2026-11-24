//! Approval policy trait.
//!
//! Defines the interface that all approval policy implementations must satisfy.
//! Policies decide whether a given action request should be allowed, denied,
//! or escalated for user confirmation.

use async_trait::async_trait;

use super::types::{ActionRequest, ApprovalDecision};

/// Trait for approval policy implementations.
///
/// A policy inspects an [`ActionRequest`] and returns an [`ApprovalDecision`].
/// Implementations may consult configuration files, allowlists, blocklists,
/// or external services to make their determination.
#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    /// Evaluate whether the given action request should be allowed.
    ///
    /// The returned decision is the one the caller acts on. A decision is NOT
    /// audited by this call: the caller decides what to do with it and then
    /// records the FINAL decision through [`Self::record`].
    async fn check(&self, request: &ActionRequest) -> ApprovalDecision;

    /// Record that a decision was made for an action request.
    ///
    /// This is called with the FINAL decision the caller acted on — `Allow`
    /// or `Deny` — after an ask (if any) was resolved. Implementations can
    /// use this for audit logging or learning.
    ///
    /// An `Ask` decision is recorded by the policy implementation that
    /// PRODUCED it, inside [`Self::check`]: the caller of an ask may resolve
    /// it in several ways (lift it, refuse it, put it to a human), and the
    /// refusal path records no final decision at all — so an ask that never
    /// produced an `Allow`/`Deny` would leave no trace unless the ask itself
    /// is audited.
    async fn record(&self, request: &ActionRequest, decision: &ApprovalDecision);
}
