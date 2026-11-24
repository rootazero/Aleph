//! One owner-local recovery state machine. Time is supplied by the host timer.
use crate::{
    host::TerminalConnection,
    session::{self, ApplyOutcome, AttachOutcome, ClientScreen},
    tabs::TabModel,
};
use aleph_protocol::pty::{PtyAttachResponse, PtyScreenFrame, PtySpawnResponse};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestFence {
    pub epoch: u64,
    pub selection_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Subscribe,
    List,
    Attach,
    Spawn,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: RequestKind,
    pub fence: RequestFence,
    pub session_id: Option<String>,
    serial: u64,
    mutation_revision: u64,
}

impl Request {
    pub fn rpc(&self) -> (&'static str, Value) {
        match self.kind {
            RequestKind::List => ("pty.list", json!({})),
            RequestKind::Attach => ("pty.attach", json!({"session_id": self.session_id})),
            RequestKind::Spawn => ("pty.spawn", json!({"rows":24,"cols":80})),
            RequestKind::Close => ("pty.close", json!({"session_id": self.session_id})),
            RequestKind::Subscribe => unreachable!("subscription is host-owned"),
        }
    }
}

pub struct Recovery {
    pub tabs: TabModel,
    pub screen: Option<ClientScreen>,
    pub error: Option<String>,
    pub ready: bool,
    pub stopped: bool,
    pub covered: bool,
    pub paint_revision: u64,
    connection: TerminalConnection,
    selection_revision: u64,
    mutation_revision: u64,
    serial: u64,
    pending: Option<Request>,
    effect: Option<Request>,
    list_dirty: bool,
    attach_dirty: bool,
    retry_at: u64,
    failures: usize,
}

impl Recovery {
    pub fn new(tabs: TabModel) -> Self {
        Self {
            tabs,
            screen: None,
            error: None,
            ready: false,
            stopped: false,
            covered: false,
            paint_revision: 0,
            connection: TerminalConnection {
                connected: false,
                epoch: 0,
            },
            selection_revision: 0,
            mutation_revision: 0,
            serial: 0,
            pending: None,
            effect: None,
            list_dirty: true,
            attach_dirty: true,
            retry_at: 0,
            failures: 0,
        }
    }
    pub fn current_connection(&self) -> TerminalConnection {
        self.connection
    }
    pub fn selected_id(&self) -> Option<&str> {
        self.tabs.selected_id()
    }
    pub(crate) fn fence(&self) -> RequestFence {
        RequestFence {
            epoch: self.connection.epoch,
            selection_revision: self.selection_revision,
        }
    }
    fn request(&mut self, kind: RequestKind, session_id: Option<String>) -> Request {
        self.serial = self.serial.wrapping_add(1);
        Request {
            kind,
            fence: self.fence(),
            session_id,
            serial: self.serial,
            mutation_revision: self.mutation_revision,
        }
    }
    fn reset_budget(&mut self) {
        self.failures = 0;
        self.retry_at = 0;
        self.stopped = false;
    }
    pub fn connect(&mut self, connection: TerminalConnection) {
        if self.connection != connection {
            self.connection = connection;
            self.pending = None;
            self.covered = false;
            self.list_dirty = true;
            self.attach_dirty = true;
            self.ready = false;
            self.error = None;
            if let Some(screen) = &mut self.screen {
                screen.abort_attach();
            }
            self.reset_budget();
        }
    }
    pub fn select(&mut self, sid: &str) {
        if self.selected_id() == Some(sid) || !self.tabs.select(sid) {
            return;
        }
        self.selection_changed();
    }
    fn selection_changed(&mut self) {
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.pending = None;
        self.screen = self
            .selected_id()
            .map(|sid| ClientScreen::new(24, 80, 0, sid));
        self.paint_revision = self.paint_revision.wrapping_add(1);
        self.ready = false;
        self.attach_dirty = true;
        self.error = None;
        self.reset_budget();
    }
    pub fn retry(&mut self) {
        self.reset_budget();
        self.list_dirty = true;
        self.attach_dirty = true;
        self.ready = false;
        self.error = None;
    }
    pub fn loss(&mut self, now: u64, error: String) {
        self.pending = None;
        self.covered = false;
        self.list_dirty = true;
        self.attach_dirty = true;
        self.ready = false;
        if let Some(screen) = &mut self.screen {
            screen.abort_attach();
        }
        self.failed(now, error);
    }
    fn failed(&mut self, now: u64, error: String) {
        self.ready = false;
        self.attach_dirty |= self.selected_id().is_some();
        self.error = Some(error);
        const DELAYS: [u64; 4] = [100, 250, 500, 1000];
        if let Some(delay) = DELAYS.get(self.failures) {
            self.retry_at = now.saturating_add(*delay);
            self.failures += 1;
        } else {
            self.stopped = true;
        }
    }
    pub fn next(&mut self, now: u64) -> Option<Request> {
        if !self.connection.connected
            || self.stopped
            || self.pending.is_some()
            || now < self.retry_at
        {
            return None;
        }
        let kind = if !self.covered {
            RequestKind::Subscribe
        } else if self.list_dirty {
            RequestKind::List
        } else if self.attach_dirty && self.selected_id().is_some() {
            RequestKind::Attach
        } else {
            return None;
        };
        let sid = if kind == RequestKind::Attach {
            self.selected_id().map(str::to_string)
        } else {
            None
        };
        let request = self.request(kind, sid);
        if request.kind == RequestKind::Attach {
            if self.screen.is_none() {
                self.screen = self
                    .selected_id()
                    .map(|sid| ClientScreen::new(24, 80, 0, sid));
            }
            if let Some(screen) = &mut self.screen {
                screen.begin_attach();
            }
        }
        self.pending = Some(request.clone());
        Some(request)
    }
    pub fn accepts(&self, request: &Request) -> bool {
        self.connection.connected
            && request.fence == self.fence()
            && self.pending.as_ref() == Some(request)
            && (request.kind != RequestKind::Attach
                || request.session_id.as_deref() == self.selected_id())
    }
    pub fn complete(&mut self, request: Request, result: Result<Value, String>, now: u64) {
        if !self.accepts(&request) {
            return;
        }
        self.pending = None;
        if request.kind == RequestKind::List && request.mutation_revision != self.mutation_revision
        {
            self.list_dirty = true;
            return;
        }
        let result = match request.kind {
            RequestKind::Subscribe => result.map(|_| {
                self.covered = true;
            }),
            RequestKind::List => match session::resolve_session_list(result) {
                session::ListOutcome::Read(sessions) => {
                    let old = self.selected_id().map(str::to_string);
                    self.tabs.reconcile(&sessions);
                    self.list_dirty = false;
                    if old.as_deref() != self.selected_id() {
                        self.selection_changed();
                    }
                    if self.selected_id().is_none() {
                        self.screen = None;
                        self.attach_dirty = false;
                    }
                    Ok(())
                }
                session::ListOutcome::Fail(error) => Err(error),
            },
            RequestKind::Attach => result
                .and_then(|v| {
                    serde_json::from_value::<PtyAttachResponse>(v).map_err(|e| e.to_string())
                })
                .and_then(|response| {
                    if self.screen.as_mut().map(|s| s.finish_attach(response))
                        == Some(AttachOutcome::Resynced)
                    {
                        self.attach_dirty = false;
                        self.paint_revision = self.paint_revision.wrapping_add(1);
                        Ok(())
                    } else {
                        Err("Terminal replay is incomplete".into())
                    }
                }),
            _ => unreachable!(),
        };
        if let Err(error) = result {
            if let Some(screen) = &mut self.screen {
                screen.abort_attach();
            }
            self.failed(now, error);
        } else if self.covered && !self.list_dirty && !self.attach_dirty {
            self.ready = true;
            self.error = None;
            self.reset_budget();
        }
    }
    pub fn frame(&mut self, frame: PtyScreenFrame, now: u64) {
        if !self.covered
            || (!self.ready
                && !self
                    .pending
                    .as_ref()
                    .is_some_and(|r| r.kind == RequestKind::Attach))
        {
            return;
        }
        let Some(screen) = &mut self.screen else {
            return;
        };
        match screen.apply(frame) {
            ApplyOutcome::Gap { .. } => {
                self.ready = false;
                self.attach_dirty = true;
                // An in-flight capture validates the complete stage on arrival.
                if self.pending.is_none() {
                    self.retry_at = now;
                }
            }
            ApplyOutcome::Applied => {
                self.paint_revision = self.paint_revision.wrapping_add(1);
                if let (Some(sid), Some(title)) =
                    (self.tabs.selected_id().map(str::to_string), screen.title())
                {
                    if self.tabs.osc_title_differs(&sid, title) {
                        self.tabs.on_title(&sid, title);
                    }
                }
            }
            _ => {}
        }
    }
    pub fn exited(&mut self, sid: &str) {
        let old = self.selected_id().map(str::to_string);
        self.tabs.on_exit(sid);
        self.mutation_revision = self.mutation_revision.wrapping_add(1);
        self.list_dirty = true;
        if old.as_deref() != self.selected_id() {
            self.selection_changed();
        }
    }
    pub fn effect_pending(&self) -> bool {
        self.effect.is_some()
    }
    pub fn gesture(&mut self, close: Option<String>) -> Option<Request> {
        if !self.connection.connected || !self.covered || self.effect.is_some() {
            return None;
        }
        let request = self.request(
            if close.is_some() {
                RequestKind::Close
            } else {
                RequestKind::Spawn
            },
            close,
        );
        self.effect = Some(request.clone());
        Some(request)
    }
    pub fn mutation_settled(&mut self) {
        self.mutation_revision = self.mutation_revision.wrapping_add(1);
        self.list_dirty = true;
    }
    pub fn complete_effect(&mut self, request: Request, result: Result<Value, String>) {
        if self.effect.as_ref() != Some(&request) {
            return;
        }
        self.effect = None;
        // An old gesture may settle after the replacement epoch's list. Its
        // payload is stale, but its possible server-side mutation is not.
        self.mutation_settled();
        if !self.connection.connected || request.fence.epoch != self.connection.epoch {
            return;
        }
        let result = if request.kind == RequestKind::Spawn {
            result
                .and_then(|v| {
                    serde_json::from_value::<PtySpawnResponse>(v).map_err(|e| e.to_string())
                })
                .map(|response| {
                    let select = request.fence.selection_revision == self.selection_revision;
                    self.tabs
                        .adopt_spawned(&response.session_id, &response.shell, select);
                    if select {
                        self.selection_changed();
                    }
                })
        } else {
            result.map(|_| {
                if let Some(sid) = &request.session_id {
                    self.exited(sid);
                }
            })
        };
        if let Err(error) = result {
            if request.fence == self.fence() {
                self.error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller() -> Recovery {
        let mut c = Recovery::new(TabModel::default());
        c.connect(TerminalConnection {
            connected: true,
            epoch: 1,
        });
        c
    }
    fn complete(c: &mut Recovery, value: Value, now: u64) -> Request {
        let r = c.next(now).unwrap();
        c.complete(r.clone(), Ok(value), now);
        r
    }
    fn listed(c: &mut Recovery) {
        complete(c, json!(null), 0);
        complete(
            c,
            json!({"sessions":[{"session_id":"s","shell":"sh","rows":24,"cols":80,"created_at":0,"closed":false}]}),
            0,
        );
    }
    #[test]
    fn prior_owner_settlement_invalidates_only_lists_started_before_it() {
        for timing in ["before-list", "during-list", "after-list"] {
            let mut c = controller();
            complete(&mut c, json!(null), 0);
            if timing == "before-list" {
                c.mutation_settled();
            }
            let list = c.next(0).unwrap();
            assert_eq!(list.kind, RequestKind::List);
            if timing == "during-list" {
                c.mutation_settled();
            }
            c.complete(list, Ok(json!({"sessions":[]})), 0);
            if timing == "after-list" {
                c.mutation_settled();
            }
            if timing == "before-list" {
                assert!(
                    c.next(0).is_none(),
                    "a later list already covers the settlement"
                );
            } else {
                let refresh = c
                    .next(0)
                    .expect("settlement must invalidate the earlier list");
                assert_eq!(refresh.kind, RequestKind::List);
                c.complete(refresh, Ok(json!({"sessions":[]})), 0);
                assert!(c.next(0).is_none(), "invalidation must be consumed once");
            }
            assert!(c.ready);
            assert!(c.error.is_none());
        }
    }

    #[test]
    fn stale_rpc_result_cannot_mutate_current_capture() {
        for error in [false, true] {
            for attach in [false, true] {
                let mut c = controller();
                if attach {
                    listed(&mut c);
                } else {
                    complete(&mut c, json!(null), 0);
                }
                let old = c.next(0).unwrap();
                c.connect(TerminalConnection {
                    connected: true,
                    epoch: 2,
                });
                let tabs = c.tabs.clone();
                c.complete(
                    old,
                    if error {
                        Err("old error".into())
                    } else {
                        Ok(json!({"sessions":[]}))
                    },
                    0,
                );
                assert_eq!(c.tabs, tabs);
                assert!(c.error.is_none());
                assert!(!c.ready);
            }
        }
    }
    #[test]
    fn failed_subscription_and_drain_retry_without_another_event() {
        let mut c = controller();
        let request = c.next(0).unwrap();
        c.complete(request, Err("subscription failed".into()), 0);
        assert!(c.next(99).is_none());
        assert_eq!(c.next(100).unwrap().kind, RequestKind::Subscribe);
        c.loss(200, "drain lost".into());
        assert!(c.next(449).is_none());
        assert_eq!(c.next(450).unwrap().kind, RequestKind::Subscribe);
    }
    #[test]
    fn terminal_shell_effects_require_explicit_gestures() {
        let mut c = controller();
        complete(&mut c, json!(null), 0);
        complete(&mut c, json!({"sessions":[]}), 0);
        assert!(c.next(0).is_none());
        let effect = c.gesture(None).unwrap();
        assert_eq!(effect.kind, RequestKind::Spawn);
        assert!(c.gesture(None).is_none());
        c.complete_effect(effect, Err("timeout".into()));
        assert_eq!(c.next(0).unwrap().kind, RequestKind::List);
        assert!(c.next(10000).is_none());
    }
    #[test]
    fn retries_are_bounded_and_manual_retry_resets_budget() {
        let mut c = controller();
        let mut now = 0;
        for delay in [100, 250, 500, 1000] {
            let r = c.next(now).unwrap();
            c.complete(r, Err("failure".into()), now);
            assert!(c.next(now + delay - 1).is_none());
            now += delay;
        }
        let r = c.next(now).unwrap();
        c.complete(r, Err("failure".into()), now);
        assert!(c.stopped);
        assert!(c.next(u64::MAX).is_none());
        c.retry();
        assert!(c.next(now).is_some());
    }
    fn snapshot(seq: u64, text: &str) -> Value {
        json!({"seq":seq,"rows":24,"cols":80,"patch":{"rows":[{"row":0,"runs":[{"text":text}]}]},"scrollback_len":0})
    }
    fn frame(seq: u64, text: &str) -> PtyScreenFrame {
        serde_json::from_value(json!({"session_id":"s","seq":seq,"rows":24,"cols":80,"patch":{"rows":[{"row":0,"runs":[{"text":text}]}]}})).unwrap()
    }
    fn ready() -> Recovery {
        let mut c = controller();
        listed(&mut c);
        complete(&mut c, snapshot(11, "old"), 0);
        assert!(c.ready);
        c
    }
    #[test]
    fn quiet_replay_gap_recovers_without_another_frame() {
        let mut c = ready();
        c.retry();
        complete(
            &mut c,
            json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]}),
            0,
        );
        let request = c.next(0).unwrap();
        c.frame(frame(12, "partial"), 0);
        c.frame(frame(14, "hole"), 0);
        c.complete(request, Ok(snapshot(11, "new")), 0);
        assert_eq!(c.screen.as_ref().unwrap().row_text(0), "old");
        assert!(!c.ready);
        assert!(c.next(99).is_none());
        assert_eq!(c.next(100).unwrap().kind, RequestKind::Attach);
        assert!(c.next(100).is_none());
    }
    #[test]
    fn same_session_refresh_keeps_old_paint_and_input_inactive() {
        let mut c = ready();
        c.retry();
        assert_eq!(c.screen.as_ref().unwrap().row_text(0), "old");
        assert!(!c.ready);
        c.frame(frame(12, "unsafe"), 0);
        assert_eq!(c.screen.as_ref().unwrap().row_text(0), "old");
    }
    #[test]
    fn stale_list_after_confirmed_mutation_cannot_erase_a_tab_or_error() {
        for failure in [false, true] {
            let mut c = ready();
            c.retry();
            let list = c.next(0).unwrap();
            let close = c.gesture(Some("other".into())).unwrap();
            c.complete_effect(close, Ok(json!({})));
            c.complete(
                list,
                if failure {
                    Err("old error".into())
                } else {
                    Ok(json!({"sessions":[]}))
                },
                0,
            );
            assert_eq!(c.selected_id(), Some("s"));
            assert!(c.error.is_none());
            assert_eq!(c.next(0).unwrap().kind, RequestKind::List);
        }
    }
    #[test]
    fn list_error_is_not_authorized_empty_and_loss_reconciles_exits() {
        let mut c = ready();
        c.loss(0, "lost exit".into());
        complete(&mut c, json!(null), 100);
        let list = c.next(100).unwrap();
        c.complete(list, Err("denied".into()), 100);
        assert_eq!(c.selected_id(), Some("s"));
        assert_eq!(c.screen.as_ref().unwrap().row_text(0), "old");
        complete(&mut c, json!({"sessions":[]}), 350);
        assert!(c.selected_id().is_none());
        assert!(c.screen.is_none());
        assert!(c.ready);
    }
    #[test]
    fn confirmed_spawn_does_not_override_newer_selection_and_is_applied_once() {
        let mut c = ready();
        let spawn = c.gesture(None).unwrap();
        // A later authoritative exit changes selection while the effect is in flight.
        c.exited("s");
        let response = json!({"session_id":"new","shell":"sh","seq":0,"rows":24,"cols":80});
        c.complete_effect(spawn.clone(), Ok(response.clone()));
        assert!(
            c.selected_id().is_none(),
            "a newer empty selection must not be overridden"
        );
        c.complete_effect(spawn, Ok(response));
        assert_eq!(
            c.tabs
                .tabs()
                .iter()
                .filter(|t| t.session_id == "new")
                .count(),
            1
        );
    }
    #[test]
    fn selected_session_fences_both_success_and_error_without_mutating_new_paint() {
        for failed in [false, true] {
            let mut c = ready();
            c.retry();
            complete(
                &mut c,
                json!({"sessions":[
                    {"session_id":"s","shell":"sh","created_at":0,"closed":false},
                    {"session_id":"other","shell":"sh","created_at":0,"closed":false}
                ]}),
                0,
            );
            let old_attach = c.next(0).unwrap();
            c.select("other");
            complete(&mut c, snapshot(20, "current"), 0);
            c.complete(
                old_attach,
                if failed {
                    Err("old failed".into())
                } else {
                    Ok(snapshot(100, "wrong"))
                },
                0,
            );
            assert_eq!(c.selected_id(), Some("other"));
            assert!(c.ready);
            assert!(c.error.is_none());
            assert_eq!(c.screen.as_ref().unwrap().row_text(0), "current");
        }
    }
    #[test]
    fn closing_last_session_and_remount_never_spawn_and_steady_state_does_not_poll() {
        let mut c = ready();
        for now in 0..100 {
            assert!(c.next(now).is_none());
        }
        let close = c.gesture(Some("s".into())).unwrap();
        c.complete_effect(close, Ok(json!({})));
        assert!(c.selected_id().is_none());
        complete(&mut c, json!({"sessions":[]}), 100);
        assert!(c.screen.is_none());
        assert!(c.next(1000).is_none());
        let mut remount = Recovery::new(c.tabs.clone());
        remount.connect(TerminalConnection {
            connected: true,
            epoch: 2,
        });
        complete(&mut remount, json!(null), 0);
        complete(&mut remount, json!({"sessions":[]}), 0);
        assert!(remount.next(10000).is_none());
    }
    #[test]
    fn ambiguous_old_selection_effect_cannot_replace_current_error_status() {
        let mut c = ready();
        c.tabs.reconcile(
            &serde_json::from_value::<aleph_protocol::pty::PtyListResponse>(json!({"sessions":[
                {"session_id":"s","shell":"sh","created_at":0,"closed":false},
                {"session_id":"other","shell":"sh","created_at":0,"closed":false}
            ]}))
            .unwrap()
            .sessions,
        );
        let effect = c.gesture(None).unwrap();
        c.select("other");
        complete(&mut c, snapshot(30, "other paint"), 0);
        c.complete_effect(effect, Err("old selection effect error".into()));
        assert!(c.error.is_none());
        assert!(c.ready);
        assert_eq!(c.screen.as_ref().unwrap().row_text(0), "other paint");
        assert_eq!(c.next(0).unwrap().kind, RequestKind::List);
    }
    #[test]
    fn old_epoch_effect_settlement_refreshes_once_without_adopting_its_result() {
        for close in [false, true] {
            for failure in [false, true] {
                let mut c = ready();
                let effect = c.gesture(close.then(|| "s".to_string())).unwrap();
                c.connect(TerminalConnection {
                    connected: true,
                    epoch: 2,
                });
                listed(&mut c);
                complete(&mut c, snapshot(20, "current epoch"), 0);
                let tabs = c.tabs.clone();
                let paint_revision = c.paint_revision;
                let result = if failure {
                    Err("old effect outcome unknown".to_string())
                } else if close {
                    Ok(json!({}))
                } else {
                    Ok(json!({"session_id":"old-spawn","shell":"sh","seq":0,"rows":24,"cols":80}))
                };
                c.complete_effect(effect.clone(), result.clone());
                assert!(!c.effect_pending());
                assert_eq!(c.tabs, tabs);
                assert_eq!(c.paint_revision, paint_revision);
                assert_eq!(c.screen.as_ref().unwrap().row_text(0), "current epoch");
                assert!(c.ready);
                assert!(c.error.is_none());
                let refresh = c
                    .next(0)
                    .expect("old effect may have settled after replacement list");
                assert_eq!(refresh.kind, RequestKind::List);
                c.complete_effect(effect, result);
                c.complete(refresh, Ok(json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]})), 0);
                assert!(
                    c.next(10_000).is_none(),
                    "refresh once, never replay an effect"
                );
            }
        }
    }

    #[test]
    fn old_epoch_effect_invalidates_a_replacement_list_still_in_flight() {
        for failure in [false, true] {
            let mut c = ready();
            let effect = c.gesture(None).unwrap();
            c.connect(TerminalConnection {
                connected: true,
                epoch: 2,
            });
            complete(&mut c, json!(null), 0);
            let list = c.next(0).unwrap();
            c.complete_effect(
                effect,
                if failure {
                    Err("ambiguous".into())
                } else {
                    Ok(json!({"session_id":"old-spawn","shell":"sh","seq":0,"rows":24,"cols":80}))
                },
            );
            c.complete(list, Ok(json!({"sessions":[]})), 0);
            assert_eq!(c.selected_id(), Some("s"));
            assert!(c.error.is_none());
            assert_eq!(c.next(0).unwrap().kind, RequestKind::List);
        }
    }

    #[test]
    fn failed_live_list_requires_attach_before_reenabling_input() {
        let mut c = ready();
        c.exited("unrelated");
        let list = c.next(0).unwrap();
        c.complete(list, Err("list failed".into()), 0);
        c.frame(frame(12, "missed while uncertain"), 1);
        complete(
            &mut c,
            json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]}),
            100,
        );
        assert!(!c.ready);
        assert_eq!(c.next(100).unwrap().kind, RequestKind::Attach);
    }
}
