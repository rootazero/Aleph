//! Operation receipts for CDP mutations.
//!
//! A receipt is built by `CdpBackend::mutate_recorded` **inside** the same
//! pinned `MUTATION_SCOPE` / live-owner permit scope that ran the mutation,
//! from that scope's own handle and owner. Nothing here re-resolves a profile
//! manager, an engine registry or the "current" tab after the operation, so the
//! receipt names exactly the owner/session/target the operation touched.

use std::sync::Arc;

use aleph_protocol::browser_live::BrowserTargetBinding;

use crate::browser::engine::{Engine, EngineHandle};
use crate::browser::live::LiveProfileOwner;
use crate::browser::types::HistoryNav;

/// Which successful verb produced a [`BrowserOperationRecord`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserOperationKind {
    Open,
    Navigate,
    History(HistoryNav),
}

/// Attestation that one CDP mutation succeeded on an exact live owner + target.
#[derive(Clone)]
pub(crate) struct BrowserOperationRecord {
    owner: Arc<LiveProfileOwner>,
    // The owner's resolver is Weak to this handle: keep the exact operation
    // handle alive without creating an owner -> handle reference cycle.
    _handle: Arc<EngineHandle>,
    target_id: String,
    kind: BrowserOperationKind,
    engine: Engine,
    engine_pid: u32,
}

impl BrowserOperationRecord {
    /// Only `CdpBackend::mutate_recorded` may mint one, from the handle/owner
    /// pair it pinned for the operation.
    pub(super) fn capture(
        owner: &Arc<LiveProfileOwner>,
        handle: &Arc<EngineHandle>,
        target_id: String,
        kind: BrowserOperationKind,
    ) -> Self {
        Self {
            owner: Arc::clone(owner),
            _handle: Arc::clone(handle),
            target_id,
            kind,
            engine: handle.engine,
            engine_pid: handle.launched.pid,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_for_test(
        owner: &Arc<LiveProfileOwner>,
        handle: &Arc<EngineHandle>,
        target_id: String,
        kind: BrowserOperationKind,
    ) -> Self {
        Self::capture(owner, handle, target_id, kind)
    }

    pub(crate) fn owner(&self) -> &Arc<LiveProfileOwner> {
        &self.owner
    }

    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) fn target_id(&self) -> &str {
        &self.target_id
    }

    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) fn kind(&self) -> BrowserOperationKind {
        self.kind
    }

    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) fn engine(&self) -> Engine {
        self.engine
    }

    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) fn engine_pid(&self) -> u32 {
        self.engine_pid
    }

    /// The exact (profile, browser session, target) the operation touched.
    pub(crate) fn target_binding(&self) -> BrowserTargetBinding {
        BrowserTargetBinding {
            profile: self.owner.profile().to_owned(),
            browser_session_id: self.owner.browser_session_id().to_owned(),
            target_id: self.target_id.clone(),
        }
    }
}

impl std::fmt::Debug for BrowserOperationRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserOperationRecord")
            .field("profile", &self.owner.profile())
            .field("browser_session_id", &self.owner.browser_session_id())
            .field("target_id", &self.target_id)
            .field("kind", &self.kind)
            .field("engine", &self.engine)
            .field("engine_pid", &self.engine_pid)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use super::{BrowserOperationKind, BrowserOperationRecord};
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;
    use crate::browser::error::BrowserError;
    use crate::browser::types::HistoryNav;

    /// The receipt carries exactly the pinned owner's identity: profile and a
    /// real UUID browser session, plus the target it was handed — nothing is
    /// invented or re-derived from a URL.
    #[tokio::test]
    async fn record_binds_the_pinned_owner_session_and_target() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("pre-seeded handle");
        let owner = handle.live_owner();

        let record = BrowserOperationRecord::capture(
            &owner,
            &handle,
            "T-7".to_owned(),
            BrowserOperationKind::Navigate,
        );
        let binding = record.target_binding();

        assert_eq!(binding.profile, "default");
        assert_eq!(binding.profile, owner.profile());
        assert_eq!(binding.browser_session_id, owner.browser_session_id());
        assert!(
            uuid::Uuid::parse_str(&binding.browser_session_id).is_ok(),
            "the session id is the owner's UUID: {}",
            binding.browser_session_id
        );
        assert_eq!(binding.target_id, "T-7");
        assert_eq!(record.target_id(), "T-7");
        assert_eq!(record.kind(), BrowserOperationKind::Navigate);
        assert_eq!(record.engine(), Engine::Chromium);
        assert_eq!(record.engine_pid(), handle.launched.pid);
        assert!(std::sync::Arc::ptr_eq(record.owner(), &owner));

        let history = BrowserOperationRecord::capture(
            &owner,
            &handle,
            "T-7".to_owned(),
            BrowserOperationKind::History(HistoryNav::Back),
        );
        assert_eq!(
            history.kind(),
            BrowserOperationKind::History(HistoryNav::Back)
        );
        assert_eq!(history.target_binding().target_id, "T-7");
    }

    /// Two owners are two browser sessions: a receipt from one can never be
    /// mistaken for a receipt from the other.
    #[tokio::test]
    async fn distinct_owners_yield_distinct_browser_sessions() {
        let server_a = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let server_b = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let (_ra, a) = backend_with(&server_a, Engine::Chromium, open_guard()).await;
        let (_rb, b) = backend_with(&server_b, Engine::Chromium, open_guard()).await;
        let (ha, hb) = (a.handle().await.unwrap(), b.handle().await.unwrap());
        let (oa, ob) = (ha.live_owner(), hb.live_owner());
        let ra = BrowserOperationRecord::capture(&oa, &ha, "T".into(), BrowserOperationKind::Open);
        let rb = BrowserOperationRecord::capture(&ob, &hb, "T".into(), BrowserOperationKind::Open);
        assert_ne!(
            ra.target_binding().browser_session_id,
            rb.target_binding().browser_session_id
        );
        assert_ne!(ra.target_binding(), rb.target_binding());
    }

    /// A successful `navigate_with_record` names the tab it navigated, under the
    /// session of the owner that backend resolves.
    #[tokio::test]
    async fn navigate_with_record_names_the_navigated_tab() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L1" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let server = std::sync::Arc::new(server);
        let emitter = {
            let server = std::sync::Arc::clone(&server);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.frameNavigated",
                    "params": { "frame": {
                        "id": "F1", "loaderId": "L1", "url": "https://ok.example/"
                    }}
                }));
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.loadEventFired",
                    "params": {}
                }));
            })
        };
        let record = backend
            .navigate_with_record("T1", "https://ok.example/")
            .await
            .expect("navigation completes");
        emitter.await.expect("emitter must not panic");

        assert_eq!(record.target_id(), "T1");
        assert_eq!(record.kind(), BrowserOperationKind::Navigate);
        let owner = handle.live_owner();
        assert_eq!(
            record.target_binding().browser_session_id,
            owner.browser_session_id()
        );
        assert_eq!(record.target_binding().profile, "default");
    }

    /// A failed mutation yields no receipt at all (the `Err` carries none).
    #[tokio::test]
    async fn failed_navigation_produces_no_record() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "errorText": "net::ERR_FAILED" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let err = backend
            .navigate_with_record("T1", "https://ok.example/")
            .await
            .expect_err("errorText fails the navigation");
        assert!(matches!(err, BrowserError::NavigationFailed(_)), "{err:?}");
    }

    /// `open_tab_with_record` names the tab it created, not any pre-existing one.
    #[tokio::test]
    async fn open_tab_with_record_names_the_created_tab() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Target.createTarget",
            Responder::Reply(json!({ "targetId": "T-NEW" })),
        );
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L1" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let server = std::sync::Arc::new(server);
        let emitter = {
            let server = std::sync::Arc::clone(&server);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.frameNavigated",
                    "params": { "frame": {
                        "id": "F1", "loaderId": "L1", "url": "https://ok.example/"
                    }}
                }));
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.loadEventFired",
                    "params": {}
                }));
            })
        };
        let (tab, record) = backend
            .open_tab_with_record("https://ok.example/")
            .await
            .expect("open completes");
        emitter.await.expect("emitter must not panic");

        assert_eq!(tab, "T-NEW");
        assert_eq!(record.target_id(), "T-NEW");
        assert_eq!(record.kind(), BrowserOperationKind::Open);
    }
}
