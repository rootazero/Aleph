use super::{ChatState, ConvId, SessionMap};
use crate::context::{GatewayEvent, TerminalEventQueue};
use leptos::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VoiceReceipt {
    serial: u64,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceRunEnd {
    Complete,
    Failed,
    Joined,
}

impl VoiceRunEnd {
    pub(crate) fn from_event(event_type: &str) -> Option<Self> {
        match event_type {
            "run_complete" => Some(Self::Complete),
            "run_error" => Some(Self::Failed),
            "run_joined" => Some(Self::Joined),
            _ => None,
        }
    }
}

type RunProjector = Arc<dyn Fn(GatewayEvent) + Send + Sync>;

#[derive(Default)]
pub(super) struct VoiceReceipts {
    serial: u64,
    pending: Option<PendingReceipt>,
    project: Option<RunProjector>,
}

struct PendingReceipt {
    ticket: VoiceReceipt,
    conv: ConvId,
    session_key: Option<String>,
    runs: HashMap<String, ObservedRun>,
    deferred: TerminalEventQueue,
    lost: bool,
}

struct ObservedRun {
    key: String,
    deferred: bool,
    terminal: Option<VoiceRunEnd>,
}

impl SessionMap {
    pub(crate) fn install_voice_run_projector(&self, project: RunProjector) {
        self.voice_receipts
            .update_value(|state| state.project = Some(project));
    }

    pub(crate) fn begin_voice_receipt(
        &self,
        epoch: u64,
        conv: ConvId,
        session_key: Option<String>,
    ) -> Option<VoiceReceipt> {
        self.voice_receipts.try_update_value(|state| {
            state.serial += 1;
            let ticket = VoiceReceipt {
                serial: state.serial,
                epoch,
            };
            state.pending = Some(PendingReceipt {
                ticket,
                conv,
                session_key,
                runs: HashMap::new(),
                deferred: TerminalEventQueue::default(),
                lost: false,
            });
            ticket
        })
    }

    pub(crate) fn discard_voice_receipt(&self, ticket: VoiceReceipt) {
        let _ = self.voice_receipts.try_update_value(|state| {
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.ticket == ticket)
            {
                state.pending = None;
            }
        });
    }

    /// Observe terminals before Chat settles their routes. An eligible first
    /// frame without a route waits for the exact receipt instead of claiming
    /// whichever keyless conversation happens to be active at delivery time.
    pub(crate) fn defer_voice_run_event(&self, epoch: Option<u64>, event: &GatewayEvent) -> bool {
        if !self
            .voice_receipts
            .try_with_value(|state| {
                state
                    .pending
                    .as_ref()
                    .is_some_and(|p| Some(p.ticket.epoch) == epoch)
            })
            .unwrap_or(false)
        {
            return false;
        }
        let Some(suffix) = event.topic.strip_prefix("run.") else {
            return false;
        };
        let Some(run_id) = run_id(event) else {
            return false;
        };
        let event_type = event
            .data
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or(suffix);
        let event_key = event.data.get("session_key").and_then(|key| key.as_str());
        let route = self.route_lookup(run_id);
        let route_key = route.and_then(|conv| self.meta(conv)?.session_key);
        let key_conv = event_key.and_then(|key| self.conv_for_session_key(key));
        let terminal = VoiceRunEnd::from_event(event_type);
        self.voice_receipts
            .try_update_value(|state| {
                let Some(pending) = state
                    .pending
                    .as_mut()
                    .filter(|p| Some(p.ticket.epoch) == epoch)
                else {
                    return false;
                };
                if let Some(run) = pending.runs.get_mut(run_id) {
                    if event_key.is_some_and(|key| key != run.key) {
                        return run.deferred;
                    }
                    if run.terminal.is_none() {
                        run.terminal = terminal;
                        if run.deferred {
                            pending.deferred.enqueue(event.clone());
                        }
                    }
                    return run.deferred;
                }
                let key = event_key.or(route_key.as_deref());
                let Some(key) = key.filter(|key| !key.is_empty()) else {
                    return false;
                };
                if pending
                    .session_key
                    .as_deref()
                    .is_some_and(|expected| expected != key)
                    || route.is_some_and(|conv| conv != pending.conv)
                    || key_conv.is_some_and(|conv| conv != pending.conv)
                    || route_key.as_deref().is_some_and(|routed| routed != key)
                {
                    return false;
                }
                let defer = route.is_none()
                    && (terminal.is_some()
                        || (key_conv.is_none()
                            && matches!(event_type, "run_accepted" | "run_queued")));
                let observe = defer || (route.is_some() && terminal.is_some());
                if observe {
                    if pending.runs.len() >= 256 {
                        pending.lost = true;
                        return defer;
                    }
                    pending.runs.insert(
                        run_id.to_owned(),
                        ObservedRun {
                            key: key.to_owned(),
                            deferred: defer,
                            terminal,
                        },
                    );
                    if defer {
                        pending.deferred.enqueue(event.clone());
                    }
                }
                defer
            })
            .unwrap_or(false)
    }

    /// Project only a confirmed, still-owned receipt. Previously projected
    /// terminals backfill the conversation identity without resurrecting a run.
    pub(crate) fn finish_voice_receipt(
        &self,
        epoch: Option<u64>,
        ticket: VoiceReceipt,
        singleton: ChatState,
        accepted_run: &str,
        session_key: &str,
    ) -> Result<Option<VoiceRunEnd>, ()> {
        let (mut pending, project) = self
            .voice_receipts
            .try_update_value(|state| {
                let pending = state.pending.as_ref().filter(|p| p.ticket == ticket)?;
                if Some(pending.ticket.epoch) != epoch {
                    state.pending = None;
                    return None;
                }
                Some((state.pending.take()?, state.project.clone()))
            })
            .flatten()
            .ok_or(())?;
        let replay = pending.deferred.drain().map_err(|_| ())?;
        let conv = pending.conv;
        let chat = self.chat_for(conv, singleton).ok_or(())?;
        if pending.lost
            || pending
                .session_key
                .as_deref()
                .is_some_and(|key| key != session_key)
            || self
                .meta(conv)
                .and_then(|meta| meta.session_key)
                .is_some_and(|key| key != session_key)
            || self
                .route_lookup(accepted_run)
                .is_some_and(|routed| routed != conv)
            || self
                .conv_for_session_key(session_key)
                .is_some_and(|bound| bound != conv)
        {
            return Err(());
        }
        let observation = pending.runs.remove(accepted_run);
        if observation
            .as_ref()
            .is_some_and(|run| run.key != session_key)
        {
            return Err(());
        }
        let terminal = observation.as_ref().and_then(|run| run.terminal);
        let deferred = observation.is_some_and(|run| run.deferred);
        self.set_session_key(conv, session_key);
        chat.session_key.set(Some(session_key.to_owned()));
        if deferred {
            let project = project.ok_or(())?;
            self.bind_run(accepted_run, conv, Some(session_key));
            for event in replay
                .into_iter()
                .filter(|event| run_id(event) == Some(accepted_run))
            {
                project(event);
            }
        } else if terminal.is_none() {
            self.bind_run(accepted_run, conv, Some(session_key));
        }
        Ok(terminal)
    }
}

fn run_id(event: &GatewayEvent) -> Option<&str> {
    event
        .data
        .get("run_id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
}
