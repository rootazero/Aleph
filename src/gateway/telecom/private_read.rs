//! Private-read driver seam for the public telecom run path (Task #14, C-step).
//!
//! The A-stage approval lets a weak SMS/PSTN conversation read a single piece
//! of owner data into the *current turn* only. The reading itself is
//! implemented behind a trait so the public run path can be unit-tested
//! without dragging the owner memory/workspace/hooks stores in, and so a
//! future live driver can be swapped in without touching the orchestrator.
//!
//! # Default safety: no driver in production
//!
//! Production installs no driver at all (the runner's driver is `None` and
//! `PublicRunRequest.scope` is `None`), so the seam is never called. The
//! [`NullPrivateReadDriver`] returns [`PrivateReadResult::Unavailable`] for
//! every query and exists for tests and as a safe explicit stand-in. There is
//! no fall-back to owner memory, owner workspace, owner secrets or owner
//! history — those stores are simply not reachable through this seam.
//!
//! # Why this is an `async_trait` (not a bare `async fn` in trait)
//!
//! The codebase's existing trait dispatch through `Arc<dyn ...>` is uniformly
//! `async_trait`; introducing a non-`async_trait` driver here would force a
//! second dispatch style into the public run path and break the `Arc<dyn
//! PrivateReadDriver>` shape the rest of the work uses. Sticking with
//! `async_trait` keeps the seam a one-line change at the call site.
//!
//! # Cancellation
//!
//! The driver MUST honour the [`CancellationToken`] at its own poll points. A
//! caller that races `cancel()` against an in-flight read expects an
//! [`PrivateReadError::Cancelled`]; never a silently-truncated
//! [`PrivateReadResult::Unavailable`] (which would degrade A to "no private
//! data" without telling the orchestrator the read was cancelled, and a
//! later re-run would observe a different answer).

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Arc;

use super::public_approval::PublicApprovalScope;

/// What the private read produced, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateReadResult {
    /// A transient, owner-side body. MUST NOT be appended to the encrypted
    /// public history and MUST NOT be written to any owner store. While the
    /// operator disclosure (B) path is not wired, the public run fails closed
    /// on this arm: the body is dropped and the LLM is not called.
    Transient(Arc<str>),
    /// The read is permitted but the source has no relevant data for the
    /// query, or no driver is installed. The orchestrator proceeds as a
    /// normal public run.
    Unavailable,
}

/// Why the private read failed. None of the three arms reveal owner content
/// in their payload (no bodies, no error messages from the owner store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateReadError {
    /// A-approval refused this scope/query combination, or the driver itself
    /// does not allow the read. Degrade to a normal public run; do not retry.
    Denied,
    /// Cancellation fired while the read was in flight. Aborts the public
    /// run with `FlowError::Cancelled` (no LLM call, no reply); it is NOT
    /// degraded to a normal public run.
    Cancelled,
    /// The driver itself failed (transport, decoding, plumbing). Degrade to
    /// a normal public run; a later turn may succeed.
    Failed,
}

/// The single seam a public run calls once per turn, after A is approved and
/// before the LLM sees the prompt. Implementations MUST NOT reach owner
/// memory, owner workspace, owner secrets or owner history directly: they go
/// through whatever bounded owner-side reader the install site picks, and
/// every such reader is expected to be auditable through the A approval
/// ledger.
///
/// # Preconditions before any real source is installed
///
/// No real private source is enabled today (product decision). Before any
/// driver that can return [`PrivateReadResult::Transient`] is wired in:
///
/// 1. The orchestrator must consume an operator-approved A record for the
///    live turn scope, and use ONLY the approved body as the query (never
///    the raw caller text).
/// 2. The operator disclosure (B) path must be wired, so any reply derived
///    from a private body is approved before it reaches SMS or history.
///    Until then the public run fails closed on `Transient`.
#[async_trait]
pub trait PrivateReadDriver: Send + Sync {
    async fn read(
        &self,
        scope: &PublicApprovalScope,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<PrivateReadResult, PrivateReadError>;
}

/// The default, safe impl. It answers [`PrivateReadResult::Unavailable`]
/// regardless of `scope`, `query` and `cancel`. It never reads owner state.
///
/// Use it:
/// - in tests that need a `Arc<dyn PrivateReadDriver>` but do not exercise
///   private reads;
/// - in startup paths that have not yet installed a real driver (e.g. a
///   minimal build that does not ship the operator control plane).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPrivateReadDriver;

impl NullPrivateReadDriver {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PrivateReadDriver for NullPrivateReadDriver {
    async fn read(
        &self,
        _scope: &PublicApprovalScope,
        _query: &str,
        _cancel: &CancellationToken,
    ) -> Result<PrivateReadResult, PrivateReadError> {
        Ok(PrivateReadResult::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::public_approval::PublicApprovalScope;
    use crate::gateway::telecom::public_context::test_support::sample_public_context;

    fn scope() -> PublicApprovalScope {
        let ctx = sample_public_context("priv-bind");
        PublicApprovalScope::new(&ctx, "core-1", "agent-1", "turn-1", 1)
    }

    /// 1. Null driver always returns Unavailable, regardless of scope, query
    ///    or cancel state. It is the safe default.
    #[tokio::test]
    async fn null_driver_always_returns_unavailable() {
        let driver: Arc<dyn PrivateReadDriver> = Arc::new(NullPrivateReadDriver::new());
        let cancel = CancellationToken::new();

        let r1 = driver.read(&scope(), "owner files", &cancel).await.unwrap();
        let r2 = driver.read(&scope(), "", &cancel).await.unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let r3 = driver.read(&scope(), "anything", &cancelled).await.unwrap();

        assert_eq!(r1, PrivateReadResult::Unavailable);
        assert_eq!(r2, PrivateReadResult::Unavailable);
        assert_eq!(r3, PrivateReadResult::Unavailable);
    }

    /// 2. A pre-cancelled token makes the driver return Cancelled, NOT
    ///    Unavailable. The distinction matters: a cancelled read is not the
    ///    same as "no data" and must not be retried on the same cancel.
    #[tokio::test]
    async fn pre_cancelled_token_yields_cancelled_error() {
        struct CancelOnPoll;
        #[async_trait]
        impl PrivateReadDriver for CancelOnPoll {
            async fn read(
                &self,
                _scope: &PublicApprovalScope,
                _query: &str,
                cancel: &CancellationToken,
            ) -> Result<PrivateReadResult, PrivateReadError> {
                if cancel.is_cancelled() {
                    Err(PrivateReadError::Cancelled)
                } else {
                    Ok(PrivateReadResult::Unavailable)
                }
            }
        }
        let driver: Arc<dyn PrivateReadDriver> = Arc::new(CancelOnPoll);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let r = driver.read(&scope(), "q", &cancel).await;
        assert_eq!(r, Err(PrivateReadError::Cancelled));
    }

    /// 3. A failure path (transport, decode, plumbing) surfaces as
    ///    [`PrivateReadError::Failed`], not as `Unavailable` (which would
    ///    pretend the read succeeded with no data). The orchestrator
    ///    degrades either way, but the diagnostic surface is different.
    #[tokio::test]
    async fn driver_failure_yields_failed_error() {
        struct AlwaysFails;
        #[async_trait]
        impl PrivateReadDriver for AlwaysFails {
            async fn read(
                &self,
                _scope: &PublicApprovalScope,
                _query: &str,
                _cancel: &CancellationToken,
            ) -> Result<PrivateReadResult, PrivateReadError> {
                Err(PrivateReadError::Failed)
            }
        }
        let driver: Arc<dyn PrivateReadDriver> = Arc::new(AlwaysFails);
        let r = driver.read(&scope(), "q", &CancellationToken::new()).await;
        assert_eq!(r, Err(PrivateReadError::Failed));
    }

    /// 4. A driver that returns `Transient` is observed as such. The
    ///    orchestrator's responsibility to NOT append the body to public
    ///    history is a separate test (in `public_run.rs`); this test only
    ///    pins the wire shape.
    #[tokio::test]
    async fn transient_body_is_observed_as_transient() {
        struct StaticBody;
        #[async_trait]
        impl PrivateReadDriver for StaticBody {
            async fn read(
                &self,
                _scope: &PublicApprovalScope,
                _query: &str,
                _cancel: &CancellationToken,
            ) -> Result<PrivateReadResult, PrivateReadError> {
                Ok(PrivateReadResult::Transient(Arc::from("private body")))
            }
        }
        let driver: Arc<dyn PrivateReadDriver> = Arc::new(StaticBody);
        let r = driver
            .read(&scope(), "q", &CancellationToken::new())
            .await
            .unwrap();
        match r {
            PrivateReadResult::Transient(body) => assert_eq!(&*body, "private body"),
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    /// 5. The `NullPrivateReadDriver` does NOT contain any reference to
    ///    owner memory, owner workspace, owner secrets or owner history
    ///    paths. This is a structural test, not a runtime one: the driver's
    ///    source is scanned for the four forbidden identifiers, which
    ///    guarantees the default is what it says it is without having to
    ///    reason about every call path.
    ///
    ///    A driver that is, in the future, *built* on top of an owner reader
    ///    is a different file; this guard fails the moment such a driver
    ///    lands in the same module as the null default.
    #[test]
    fn null_driver_source_does_not_reference_owner_state() {
        // Scan only the production half: this test names the forbidden
        // identifiers itself, so the whole file would always match.
        let full = include_str!("private_read.rs");
        let source = full
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields a first part");
        for forbidden in [
            "owner_memory",
            "owner_workspace",
            "owner_secrets",
            "owner_history",
        ] {
            assert!(
                !source.contains(forbidden),
                "NullPrivateReadDriver must not reference `{forbidden}`; \
                 owner-state reads belong in a non-null driver, not in the \
                 default that startup installs",
            );
        }
    }
}
