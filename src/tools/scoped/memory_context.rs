//! Explicit context transfer for a source-owned, already-admitted Memory call.

use std::future::Future;

use futures::future::BoxFuture;

/// Capture on the caller task, before MemoryOperation::spawn creates a task.
/// This carries existing facts; dispatch authority is scoped separately from
/// the original sealed GateAdmissionToken inside that task.
pub(super) fn carry<F>(future: F) -> BoxFuture<'static, F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    use crate::gateway::caller_identity;
    use crate::sandbox::context;
    use crate::tools::turn_context;

    let attribution = crate::scope::carried::CarriedAttribution::capture();
    let identity = crate::approval::current_call_identity();
    let actor = crate::identity::current_actor();
    let turn = turn_context::current_turn_context();
    let tier = turn_context::current_exec_tier();
    let inline_shell = turn_context::TURN_INLINE_SHELL
        .try_with(|value| *value)
        .ok();
    let originator = turn_context::current_originator();
    let fs = crate::tools::fs_scope::current();
    let role = caller_identity::current_caller_role();
    let user = caller_identity::current_caller_user();
    let loopback = caller_identity::current_caller_is_loopback();
    let connection = caller_identity::current_caller_conn_id();
    let verified = caller_identity::current_caller_verified_user();
    let session = context::current_session();
    let justification = context::current_justification();
    let sandbox = context::current_sandbox_override();
    let tail = context::current_live_tail();
    Box::pin(async move {
        let future = crate::approval::with_call_identity(identity, future);
        let future = async move {
            match actor {
                Some(actor) => crate::identity::as_actor(&actor, future).await,
                None => future.await,
            }
        };
        let future = optional_scope(&turn_context::TURN_CONTEXT, turn, future);
        let future = optional_scope(&turn_context::TURN_EXEC_TIER, tier, future);
        let future = optional_scope(&turn_context::TURN_INLINE_SHELL, inline_shell, future);
        let future = turn_context::with_originator(originator, future);
        let future = crate::tools::fs_scope::with_fs_scope(fs, future);
        let future = optional_scope(&context::SESSION_ID, session, future);
        let future = optional_scope(&context::EXEC_JUSTIFICATION, justification, future);
        let future = optional_scope(&context::SANDBOX_OVERRIDE, sandbox, future);
        let future = optional_scope(&context::LIVE_TAIL, tail, future);
        // Personal caller scoping happens first; captured project attribution
        // is restored inside it and therefore cannot be overwritten by it.
        caller_identity::with_verified_caller_identity(
            role,
            user,
            loopback,
            connection,
            verified,
            attribution.reestablish(Box::pin(future)),
        )
        .await
    })
}

// Erase each optional scope independently so the two branches do not multiply
// the nested future's stack layout at every captured task-local boundary.
fn optional_scope<T, F>(
    key: &'static tokio::task::LocalKey<T>,
    value: Option<T>,
    future: F,
) -> BoxFuture<'static, F::Output>
where
    T: Send + 'static,
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    Box::pin(async move {
        match value {
            Some(value) => key.scope(value, future).await,
            None => future.await,
        }
    })
}
