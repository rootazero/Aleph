//! Invocation authority for a single, admitted desktop tool call.

use std::cell::RefCell;
use std::future::Future;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::approval::OperatorApprovalRequester;
use crate::capability::facade::{BackendLease, Zahir};
use crate::capability::zahir_facade::ZahirFacade;
use crate::sandbox::exec_approval::action::ApprovalAction;
use crate::sandbox::exec_approval::gate::ApprovalResponse;
use crate::sync_primitives::Arc;
use crate::tools::registry::{CanonicalEntryLease, ToolHandlerRegistry};
use crate::tools::service::ToolError;

pub(crate) struct DesktopApprovalCapture {
    registry: Arc<ToolHandlerRegistry>,
    facade: Arc<ZahirFacade>,
    entry: CanonicalEntryLease,
    backend: BackendLease,
}

impl DesktopApprovalCapture {
    pub(crate) fn new(
        registry: Arc<ToolHandlerRegistry>,
        facade: Arc<ZahirFacade>,
        entry: CanonicalEntryLease,
        backend: BackendLease,
    ) -> Self {
        Self {
            registry,
            facade,
            entry,
            backend,
        }
    }
}

pub(crate) struct DesktopApprovalInvocation {
    capture: DesktopApprovalCapture,
    capability_policy: Option<Arc<crate::capability::policy::PolicyEngine>>,
    requester: Arc<OperatorApprovalRequester>,
    cancel: CancellationToken,
    deadline: Instant,
}

impl DesktopApprovalInvocation {
    pub(crate) fn new(
        capture: DesktopApprovalCapture,
        capability_policy: Option<Arc<crate::capability::policy::PolicyEngine>>,
        requester: Arc<OperatorApprovalRequester>,
        cancel: CancellationToken,
        deadline: Instant,
    ) -> Self {
        Self {
            capture,
            capability_policy,
            requester,
            cancel,
            deadline,
        }
    }

    pub(super) async fn request(&mut self, action: &ApprovalAction) -> ApprovalResponse {
        // Share the same expiry with the approval UI and the eventual effect.
        self.deadline = self
            .deadline
            .min(Instant::now() + std::time::Duration::from_secs(60));
        self.requester
            .request_operator_once(action, &self.cancel, self.deadline)
            .await
    }

    pub(crate) fn validate_before_dispatch(&self) -> Result<(), ToolError> {
        let name = "desktop";
        if self.cancel.is_cancelled() {
            return Err(ToolError::Cancelled { name: name.into() });
        }
        if Instant::now() >= self.deadline {
            return Err(ToolError::PermissionDenied {
                name: name.into(),
                reason: "desktop approval invocation expired".into(),
            });
        }
        let capture = &self.capture;
        if self.capability_policy.as_ref().is_some_and(|policy| {
            !policy.admissions_open() || policy.is_configured(&capture.backend.descriptor.id)
        }) {
            return Err(ToolError::PermissionDenied {
                name: name.into(),
                reason: "desktop capability policy does not allow held approval".into(),
            });
        }
        let publication = capture.registry.canonical_publication();
        if !capture.entry.validate(&publication)
            || !publication
                .entry(name)
                .is_some_and(|current| capture.entry.entry().same_published_handler(current))
            || capture.facade.validate_lease(&capture.backend).is_err()
        {
            return Err(ToolError::PermissionDenied {
                name: name.into(),
                reason:
                    "desktop approval no longer holds the admitted handler and capability lease"
                        .into(),
            });
        }
        // Recheck the caller's interest after the synchronous registry reads.
        if self.cancel.is_cancelled() || Instant::now() >= self.deadline {
            return Err(ToolError::PermissionDenied {
                name: name.into(),
                reason: "desktop approval invocation cancelled or expired".into(),
            });
        }
        Ok(())
    }
}

pub(super) struct ApprovedDesktopAction {
    pub(super) invocation: Box<DesktopApprovalInvocation>,
    pub(super) prepared: super::native::PreparedDesktopAction,
    pub(super) policy_request: crate::approval::ActionRequest,
}

impl ApprovedDesktopAction {
    pub(super) async fn recheck(
        &self,
        tool: &super::DesktopTool,
        platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
        screen: &dyn aleph_desktop::ScreenCapability,
    ) -> Option<super::DesktopOutput> {
        use super::DesktopOutput;
        use crate::approval::ApprovalDecision;

        if let Err(error) = self.invocation.validate_before_dispatch() {
            return Some(DesktopOutput::failed(error.to_string()));
        }
        let Some(policy) = &tool.approval_policy else {
            return Some(DesktopOutput::failed(
                "Desktop approval policy is no longer available.",
            ));
        };
        // Recheck the original policy subject, not the redacted card. Never
        // lift a refusal or record the operator's response as a policy Allow.
        if matches!(
            policy.check(&self.policy_request).await,
            ApprovalDecision::Deny { .. }
        ) {
            return Some(DesktopOutput::failed(
                "Current desktop policy denies the approved action.",
            ));
        }
        if let Some(refusal) = self.prepared.revalidate(tool, platform, screen).await {
            return Some(refusal);
        }
        if !self.prepared.escape_unchanged(platform.as_ref()) {
            // Unwind inputs held by earlier calls only for a real Escape, not
            // for an unavailable or restarted monitor.
            if platform
                .escape_listener()
                .is_some_and(|listener| listener.is_aborted())
            {
                super::held_inputs::release_all_sessions(screen).await;
            }
            return Some(DesktopOutput::failed(
                "Desktop input was interrupted during operator approval.",
            ));
        }
        self.invocation
            .validate_before_dispatch()
            .err()
            .map(|error| DesktopOutput::failed(error.to_string()))
    }
}

tokio::task_local! {
    static INVOCATION: RefCell<Option<Box<DesktopApprovalInvocation>>>;
}

pub(crate) async fn scope_desktop_approval<F: Future>(
    invocation: Option<Box<DesktopApprovalInvocation>>,
    handler: F,
) -> F::Output {
    INVOCATION.scope(RefCell::new(invocation), handler).await
}

/// Only the outer desktop invocation can claim the context. Its child actions
/// (batch or script) cannot rediscover it while that invocation is held open.
pub(crate) fn take_desktop_approval() -> Option<Box<DesktopApprovalInvocation>> {
    INVOCATION
        .try_with(|invocation| invocation.borrow_mut().take())
        .ok()
        .flatten()
}
