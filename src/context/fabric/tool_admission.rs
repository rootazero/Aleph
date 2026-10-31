//! Host-scoped admission context for the existing tool-dispatch chokepoint.
//!
//! A delegated tool service may still carry its parent's TurnContext. The
//! harness scopes the actual SessionId here, so a child cannot borrow a
//! parent's context grant. Scope None explicitly to clear an inherited frame.

use super::{AdmissionOutcome, AuthenticatedCaller, ContextFabric, ContextFabricError};
use crate::tools::result_processing::RecoveryTools;
use crate::tools::result_store::ToolResultStore;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct AdmissionFrame {
    fabric: Arc<ContextFabric>,
    caller: AuthenticatedCaller,
    cancel: CancellationToken,
}

tokio::task_local! {
    static TOOL_ADMISSION: Option<Result<AdmissionFrame, ContextFabricError>>;
}

pub(crate) async fn with_tool_admission<F: Future>(
    fabric: Option<Arc<ContextFabric>>,
    session_id: String,
    cancel: CancellationToken,
    future: F,
) -> F::Output {
    let frame = fabric.map(|fabric| {
        AuthenticatedCaller::from_host_boundary(session_id)
            .map(|caller| AdmissionFrame {
                fabric,
                caller,
                cancel,
            })
            .map_err(ContextFabricError::Contract)
    });
    let finishing_frame = frame
        .as_ref()
        .and_then(|frame| frame.as_ref().ok().cloned());
    TOOL_ADMISSION
        .scope(frame, async move {
            let result = future.await;
            if let Some(frame) = finishing_frame {
                frame
                    .fabric
                    .notify_lifecycle(
                        super::ContextLifecycleEvent::TurnEnd,
                        &frame.caller,
                        Some(&frame.cancel),
                    )
                    .await;
            }
            result
        })
        .await
}

/// The actual harness session, including delegated child runs.
pub(crate) fn current_context_caller() -> Option<AuthenticatedCaller> {
    if let Some(frame) = TOOL_ADMISSION
        .try_with(|frame| {
            frame
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|frame| frame.caller.clone())
        })
        .ok()
        .flatten()
    {
        return Some(frame);
    }
    None
}

/// Notify observers for the real scoped harness caller, carrying no raw data.
pub(crate) async fn notify_current_lifecycle(event: super::ContextLifecycleEvent) {
    if let Some(frame) = TOOL_ADMISSION
        .try_with(|frame| {
            frame
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned()
        })
        .ok()
        .flatten()
    {
        frame
            .fabric
            .notify_lifecycle(event, &frame.caller, Some(&frame.cancel))
            .await;
    }
}

/// None preserves the existing native ingress path and its blocking offload.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn admit_scoped_tool_output(
    name: &str,
    call_id: &str,
    value: &mut Value,
    budget: Option<usize>,
    store: Option<&ToolResultStore>,
    recovery: RecoveryTools,
) -> Result<Option<(AdmissionOutcome, String)>, ContextFabricError> {
    if budget.is_none() {
        return Ok(None);
    }
    let invalid = TOOL_ADMISSION
        .try_with(|frame| {
            frame
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(ToString::to_string)
        })
        .ok()
        .flatten();
    if let Some(error) = invalid {
        return Err(ContextFabricError::Handler(error));
    }
    let Some(frame) = TOOL_ADMISSION
        .try_with(|frame| {
            frame
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned()
        })
        .ok()
        .flatten()
    else {
        return Ok(None);
    };
    let Some(key) = frame.fabric.admission_key(&frame.caller).await? else {
        return Err(ContextFabricError::BuiltinUnavailable("scoped admission"));
    };
    let original = crate::providers::message::value_as_model_text(value).into_owned();
    frame
        .fabric
        .admit_tool_output_with_context(
            &key,
            name,
            call_id,
            value,
            budget,
            store,
            recovery,
            &frame.caller,
            Some(&frame.cancel),
        )
        .await
        .map(|outcome| Some((outcome, original)))
}
