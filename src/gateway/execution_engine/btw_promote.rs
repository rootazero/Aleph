//! Serving `/btw promote` — the engine half of the one crossing back.
//!
//! The reading and the append live in [`crate::gateway::btw::promote`], which
//! owns every derivation `/btw` shares. What is here is the part that is
//! specific to being reached from [`ExecutionEngine::execute`]: opening the
//! conversation's row, and putting a receipt on the wire for a request that
//! deliberately never becomes a run.
//!
//! # Why a receipt is owed on **every** exit
//!
//! `execute()` returning before `RunAccepted` is invisible on every surface —
//! no message on a channel, an in-flight bubble on the Panel that nothing will
//! ever close. That is why `emit_pre_admission_run_error` exists, and promote
//! returns from exactly that position **by design** rather than by failure. So
//! each of its three outcomes says so out loud:
//!
//! * it promoted something — name the question it carried;
//! * there was nothing completed to promote — a true answer, not an error;
//! * the side log could not be read — an error, and it must not be allowed to
//!   read as the empty case, or a broken side thread looks to the user like an
//!   empty one and they stop asking.

use super::{ExecutionEngine, ExecutionError, RunRequest};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::gateway::i18n::{t, Locale, Msg};
use crate::routing::session_key::SessionKey;

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Carry the latest completed side answer into `main`, and report what
    /// happened.
    ///
    /// `main` is the conversation the user typed in; `request.session_key` has
    /// already been redirected onto the side thread by the caller, so this
    /// reads the log side questions really ran on rather than deriving a second
    /// opinion about which one that is.
    ///
    /// Every frame here is charged to `main`, for the reason
    /// [`Self::emit_pre_admission_run_error`] states for its own `scope`
    /// parameter: the side session is a key the user's client has never heard
    /// of, and a receipt has to arrive where the person is looking.
    pub(super) async fn serve_btw_promote<E: EventEmitter + Send + Sync + 'static>(
        &self,
        main: &SessionKey,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &E,
        run_id: &str,
    ) -> Result<(), ExecutionError> {
        let locale = Locale::from_run_metadata(&request.metadata);

        // The row before the frames, and this order is the same load-bearing
        // one `execute()` documents for its own `RunAccepted`: that frame
        // classifies `SessionIdentity::BySessionKey`, so the delivery filter
        // resolves it by reading this row. It is also what the carrier's
        // `messages` projection appends into. A `/btw promote` can be the very
        // first thing said in a conversation, which is precisely when the row
        // does not exist yet.
        //
        // Under the run's own attribution rather than the ambient scope, for
        // the reason `run_loop::ensure_session_under_request_scope` gives: the
        // task-local does not survive the `tokio::spawn` every producer of a
        // run performs, so the attribution lives only in the metadata map.
        crate::scope::with_scope(
            crate::scope::scope_from_metadata(&request.metadata),
            agent.ensure_session(main),
        )
        .await;

        // Announced before the work, not after: this is the ONE frame carrying
        // `{run_id, session_key}`, so it is what seeds `EventVisibilityIndex`'s
        // run→session index and what the Panel binds run→conversation on. The
        // terminal frame below classifies `ByRunId` against that seed; without
        // this the receipt is dropped by the delivery filter for every
        // connection, the operator's included.
        if let Err(e) = emitter
            .emit(StreamEvent::RunAccepted {
                run_id: run_id.to_string(),
                session_key: main.to_key_string(),
                accepted_at: chrono::Utc::now().to_rfc3339(),
            })
            .await
        {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "failed to emit RunAccepted for a /btw promote"
            );
        }

        let receipt = match self.promoted_exchange(main, request).await {
            Ok(Some(question)) => t(
                Msg::BtwPromoted {
                    question: &question,
                },
                locale,
            ),
            Ok(None) => t(Msg::BtwNothingToPromote, locale),
            Err(e) => {
                // NOT `BtwNothingToPromote`. "I could not read the side thread"
                // and "the side thread has nothing finished in it" are two
                // different answers, and only one of them means the user should
                // stop asking.
                self.emit_pre_admission_run_error(emitter, run_id, main, request, &e)
                    .await;
                return Err(e);
            }
        };

        let seq = emitter.next_seq();
        if let Err(e) = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq,
                summary: RunSummary {
                    // No model was asked anything, so there is nothing to bill
                    // and no tool call to count — the same honesty the L0 slash
                    // fast path applies to its own zeroes.
                    total_tokens: 0,
                    tool_calls: 0,
                    loops: 0,
                    final_response: Some(receipt),
                    duration_ms: Some(0),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
        {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "failed to emit the /btw promote receipt"
            );
        }

        // The main transcript gained a row without a run having written it, so
        // every attached client needs to re-read it — the same announcement the
        // fast path makes for the pair of events it writes by hand.
        self.publish_session_updated(
            main,
            request.metadata.get("channel_id").map(String::as_str),
            run_id,
        );

        Ok(())
    }

    /// Do the crossing; hand back the question that was carried, if any.
    ///
    /// Split from the frames above so the failure modes stay legible: this
    /// function's `Err` is the only thing that must not be mistaken for
    /// `Ok(None)`.
    ///
    /// An engine with no orchestrator is an `Err` here and deliberately not the
    /// shrug that [`Self::seed_side_session`] gives the same condition. Seeding
    /// is an enrichment of a question that will be answered anyway; promoting
    /// **is** the whole request, and reporting "nothing to promote" for
    /// "I have no session service" would be this module's one forbidden
    /// confusion, dressed up as a boot-shape fact.
    async fn promoted_exchange(
        &self,
        main: &SessionKey,
        request: &RunRequest,
    ) -> Result<Option<String>, ExecutionError> {
        let Some(orchestrator) = self.orchestrator.get() else {
            return Err(ExecutionError::Failed(
                "cannot promote a side answer: this engine has no orchestrator, so there \
                 is no session service to read the side thread from"
                    .to_string(),
            ));
        };
        crate::gateway::btw::promote::promote_latest_exchange(
            orchestrator.session_service.as_ref(),
            &request.session_key,
            main,
        )
        .await
        .map(|carried| carried.map(|exchange| exchange.question))
        .map_err(ExecutionError::Failed)
    }
}

#[cfg(test)]
mod tests;
