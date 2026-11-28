//! Dispatch snapshot-bound mutations to the existing accessibility capability.

use aleph_desktop::{system_types::DesktopProcessIdentity, DesktopPlatform};
use aleph_protocol::desktop_bridge::methods::ax::{
    AxMutation, AxReferenceAction, AxReferenceError,
};

use super::element_ref::{self, PendingAction, TokenError};
use super::{native, operator_approval, DesktopArgs, DesktopOutput, DesktopTool};
use crate::approval::ApprovalDecision;
use crate::sandbox::exec_approval::{gate::ApprovalOutcome, ApprovalAction};

impl DesktopTool {
    pub(super) async fn dispatch_reference(
        &self,
        args: &DesktopArgs,
        token: &str,
    ) -> DesktopOutput {
        let refusal = |code: AxReferenceError, message: &str| {
            element_ref::reference_error_output(code.code(), message)
        };
        let mutation = match args.action.as_str() {
            "set_value" => match &args.text {
                Some(text) => AxMutation::SetValue {
                    value: text.clone(),
                },
                None => return DesktopOutput::failed("set_value requires text."),
            },
            "ax_action" => match &args.ax_action_name {
                Some(action) if !action.trim().is_empty() => AxMutation::Perform {
                    action: action.clone(),
                },
                _ => return DesktopOutput::failed("ax_action requires ax_action_name."),
            },
            _ => {
                return refusal(
                    AxReferenceError::ActionUnsupported,
                    "A desktop element token supports ax_action and set_value.",
                );
            }
        };
        if args.pid.is_some()
            || args.app.is_some()
            || args.window_id.is_some()
            || args.x.is_some()
            || args.y.is_some()
            || args.role.is_some()
            || args.element_title.is_some()
        {
            return element_ref::token_error_output(&TokenError::ConflictingTarget);
        }
        let Some(platform) = self.platform.clone() else {
            return self.no_capability_output();
        };
        if platform.ax().is_none() {
            return refusal(
                AxReferenceError::ReferenceUnsupported,
                "This desktop has no accessibility capability.",
            );
        }
        let session = super::held_inputs::current_session_id();
        let record = match element_ref::resolve(&session, token) {
            Ok(record) => record,
            Err(error) => return element_ref::token_error_output(&error),
        };
        let target = match app_for_reference(platform.as_ref(), record.pid).await {
            Ok(app) => app,
            Err(output) => return output,
        };
        if let Err(output) = self.check_escape().await {
            return output;
        }
        let escape_generation = platform
            .escape_listener()
            .and_then(|listener| listener.abort_generation());
        let dispatch_deadline = operator_approval::current_dispatch_deadline();
        let mut invocation = operator_approval::take_desktop_approval();
        let mut approved_policy_request = None;
        if let (Some(policy), Some((kind, subject))) =
            (&self.approval_policy, super::classify_approval(args))
        {
            let mut request = Self::approval_request(kind.clone(), &args.action, "");
            request.target = subject;
            request.display_target = match &mutation {
                AxMutation::SetValue { .. } => "write a value to an accessibility element".into(),
                AxMutation::Perform { .. } => "perform a native accessibility action".into(),
            };
            let decision = crate::approval::lift_ask(policy.check(&request).await, kind);
            match decision {
                ApprovalDecision::Allow => policy.record(&request, &decision).await,
                ApprovalDecision::Deny { ref reason } => {
                    policy.record(&request, &decision).await;
                    return DesktopOutput::failed(match mutation {
                        AxMutation::SetValue { .. } => {
                            "The desktop policy denied the value write.".into()
                        }
                        AxMutation::Perform { .. } => {
                            format!("Action denied by approval policy: {reason}")
                        }
                    });
                }
                ApprovalDecision::Ask { .. } => {
                    let Some(invocation) =
                        invocation.as_mut().filter(|_| escape_generation.is_some())
                    else {
                        return Self::approval_required(request.display_target);
                    };
                    if let Err(error) = invocation.validate_before_dispatch() {
                        return DesktopOutput::failed(error.to_string());
                    }
                    let mut card = ApprovalAction::bare(
                        "desktop",
                        "Allow one native accessibility action on the application shown below.",
                    );
                    let mut scope = format!("{} (PID {})", target.bundle_id, record.pid);
                    if let Some(window_id) = record.window_id {
                        scope.push_str(&format!(", window {window_id}"));
                    }
                    card.summary = match &mutation {
                        AxMutation::SetValue { .. } => format!("set_value → {scope}"),
                        AxMutation::Perform { action } => format!("ax_action ({action}) → {scope}"),
                    };
                    if invocation.request(&card).await.outcome != ApprovalOutcome::Approved {
                        return DesktopOutput::failed(
                            "Operator approval was not granted for this desktop call.",
                        );
                    }
                    approved_policy_request = Some((policy.clone(), request));
                }
            }
        }
        let session_guard = match self.acquire_lock() {
            Ok(Some(guard)) => Some(guard),
            Ok(None) if approved_policy_request.is_some() => {
                return DesktopOutput::failed("Approved desktop call has no owning session.");
            }
            Ok(None) => None,
            Err(output) => return output,
        };
        if let Some((policy, request)) = approved_policy_request {
            let current = match app_for_reference(platform.as_ref(), record.pid).await {
                Ok(app) => app,
                Err(output) => return output,
            };
            if current != target {
                return element_ref::token_error_output(&TokenError::Stale);
            }
            if matches!(policy.check(&request).await, ApprovalDecision::Deny { .. }) {
                return DesktopOutput::failed("Current desktop policy denies the approved action.");
            }
        }
        // The worker owns both guards through actual driver settlement, even
        // when the tool caller stops awaiting the result.
        let pending = match PendingAction::admit(&session, token, &record) {
            Ok(pending) => pending,
            Err(error) => return element_ref::token_error_output(&error),
        };
        let request = AxReferenceAction {
            reference: record.reference,
            mutation,
        };
        let worker_platform = platform.clone();
        let (caller, caller_lifetime) = tokio::sync::oneshot::channel::<()>();
        let result = tokio::spawn(async move {
            let _session_guard = session_guard;
            if let Some(invocation) = invocation {
                if let Err(error) = invocation.validate_before_dispatch() {
                    return DesktopOutput::failed(error.to_string());
                }
            }
            if worker_platform.escape_listener().is_some_and(|listener| {
                listener.is_aborted() || listener.abort_generation() != escape_generation
            }) {
                return DesktopOutput::failed("Desktop input was interrupted before dispatch.");
            }
            let Some(ax) = worker_platform.ax() else {
                return refusal(
                    AxReferenceError::ReferenceUnsupported,
                    "The accessibility capability is unavailable.",
                );
            };
            if caller.is_closed() || dispatch_deadline.is_some_and(|(cancel, deadline)| {
                cancel.is_cancelled() || tokio::time::Instant::now() >= deadline
            }) {
                return DesktopOutput::failed("Desktop call ended before native dispatch.");
            }
            match ax.act_on_reference(request).await {
                Ok(result) => native::ax_action_output(result),
                Err(aleph_desktop::DesktopError::AxReference {
                    code: AxReferenceError::NativeActionUnsettled, ..
                }) => {
                    // The caller can receive an error without releasing the PID
                    // fence. The target may still be executing the AX message.
                    tokio::spawn(hold_until_process_exits(worker_platform, target, pending));
                    refusal(
                        AxReferenceError::ActionOutcomeUnknown,
                        "The AX mutation may still be running. A fresh snapshot of this PID is blocked until the originating process exits.",
                    )
                }
                Err(error) => element_ref::driver_error_output(error),
            }
        })
        .await;
        drop(caller_lifetime);
        let mut output = match result {
            Ok(output) => output,
            Err(_) => return refusal(
                AxReferenceError::ActionOutcomeUnknown,
                "The accessibility action did not return a result. Re-observe before taking another action.",
            ),
        };
        if output.success {
            if matches!(args.observe.as_deref(), Some("state" | "screenshot")) {
                let window_id = record.window_id.and_then(|id| u32::try_from(id).ok());
                self.attach_post_observation(
                    platform.as_ref(),
                    platform.screen(),
                    super::observe::PostTarget::process(record.pid, window_id),
                    args.observe.as_deref() == Some("screenshot"),
                    &mut output,
                )
                .await;
            }
            output.grade_effect(&args.action);
        }
        output
    }
}

async fn hold_until_process_exits(
    platform: std::sync::Arc<dyn DesktopPlatform>,
    target: DesktopProcessIdentity,
    _pending: PendingAction,
) {
    let system = platform
        .system()
        .expect("native reference dispatch requires a system capability");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(current) = system.desktop_process_identity(target.pid).await {
            if current.as_ref() != Some(&target) {
                return;
            }
        }
    }
}

async fn app_for_reference(
    platform: &dyn DesktopPlatform,
    pid: i32,
) -> Result<DesktopProcessIdentity, DesktopOutput> {
    let identity = if let Some(system) = platform.system() {
        system.desktop_process_identity(pid).await.ok().flatten()
    } else {
        None
    };
    let Some(identity) = identity.filter(|identity| identity.pid == pid) else {
        return Err(DesktopOutput::failed(
            "Could not establish the native reference's application identity.",
        ));
    };
    let name = std::path::Path::new(&identity.bundle_id)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&identity.bundle_id);
    if let Some(reason) = super::safety::blocked_app_reason(name, &identity.bundle_id) {
        return Err(DesktopOutput::failed(reason));
    }
    Ok(identity)
}
