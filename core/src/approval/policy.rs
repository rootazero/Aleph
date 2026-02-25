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
    async fn check(&self, request: &ActionRequest) -> ApprovalDecision;

    /// Record that a decision was made for an action request.
    ///
    /// This is called after a decision has been finalized (including after
    /// user confirmation for `Ask` decisions). Implementations can use this
    /// for audit logging or learning.
    async fn record(&self, request: &ActionRequest, decision: &ApprovalDecision);
}
