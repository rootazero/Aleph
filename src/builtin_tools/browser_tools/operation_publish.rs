//! Best-effort browser-live publication after a SUCCESSFUL CDP mutation.
//!
//! The receipt comes from `CdpBackend::*_with_record`, which captures the exact
//! live owner / browser session / target inside the mutation's own permit
//! scope. This module never looks a target up afterwards (no manager, no
//! "active tab"), never receives a URL or model-supplied id, and never lets a
//! publication problem change what the tool reports: the original operation's
//! result is returned unchanged whether or not anything was published.
//!
//! Policy for what is published (explicit, not inferred from "navigation
//! succeeded"):
//! * `kind: Page`, `display: ExplicitOnly` - never auto-revealed to the viewer;
//!   the user must ask for it.
//! * `redaction: Pending` - a successful navigation says nothing about the page
//!   being safe to show, so it is NEVER marked `Safe`. Resources stay
//!   unrevealed-by-default until a later redaction decision.
//! * `source: BrowserTool` / `NotManaged` - the tab belongs to the browser
//!   driver, not to a managed dev-server process launched by Aleph.
//!
//! Only the CDP backend produces receipts. Any other backend (Chrome MCP,
//! Playwright CLI, fakes) runs the plain operation and publishes nothing.

use std::time::Duration;

use super::present::{
    BrowserLiveProducer, BrowserPresentOperation, BrowserPresentation, PresentError,
};
use crate::browser::backend::BrowserBackend;
use crate::browser::cdp_backend::{BrowserOperationRecord, CdpBackend};
use crate::browser::error::BrowserError;
use crate::browser::types::{HistoryNav, TabId};
use aleph_protocol::browser_live::{DisplayPolicy, PreviewKind, RedactionStatus};

/// Lifetime of a tool-published page resource before the owner sweep closes it.
pub(crate) const BROWSER_OPERATION_PREVIEW_TTL: Duration = Duration::from_secs(10 * 60);

fn presentation() -> BrowserPresentation {
    BrowserPresentation {
        kind: PreviewKind::Page,
        display: DisplayPolicy::ExplicitOnly,
        redaction: RedactionStatus::Pending,
        ttl: BROWSER_OPERATION_PREVIEW_TTL,
        companion_candidates: Vec::new(),
    }
}

/// Stable, content-free label: error `Display` text may embed registry details.
fn error_class(error: &PresentError) -> &'static str {
    match error {
        PresentError::NotConfigured => "not_configured",
        PresentError::MissingTurn => "missing_turn",
        PresentError::MissingIdentity => "missing_identity",
        PresentError::MissingTarget => "missing_target",
        PresentError::BindingMismatch => "binding_mismatch",
        PresentError::AlreadyClosed => "already_closed",
        PresentError::OwnerGone => "owner_gone",
        PresentError::Registry(_) => "registry",
        PresentError::Event(_) => "event",
    }
}

/// Publish `record`; every failure is logged by class only and swallowed.
/// Returns whether a resource was published (for tests; callers ignore it).
pub(crate) fn publish_record(
    producer: &BrowserLiveProducer,
    record: &BrowserOperationRecord,
) -> bool {
    let operation = BrowserPresentOperation::from_operation_record(record);
    match producer.browser_present(Some(operation), presentation()) {
        Ok(_) => true,
        Err(error) => {
            tracing::debug!(
                class = error_class(&error),
                "browser-live publication skipped after successful browser operation"
            );
            false
        }
    }
}

/// `open_tab`, publishing the opened tab when the backend is CDP and a producer
/// is installed. Result and error are exactly the plain `open_tab`'s.
pub(crate) async fn open_tab_and_present(
    backend: &dyn BrowserBackend,
    producer: Option<&BrowserLiveProducer>,
    url: &str,
) -> Result<TabId, BrowserError> {
    if let (Some(producer), Some(cdp)) = (producer, backend.as_any().downcast_ref::<CdpBackend>()) {
        let (tab_id, record) = cdp.open_tab_with_record(url).await?;
        publish_record(producer, &record);
        return Ok(tab_id);
    }
    backend.open_tab(url).await
}

/// `navigate`, publishing the navigated tab under the same rules.
pub(crate) async fn navigate_and_present(
    backend: &dyn BrowserBackend,
    producer: Option<&BrowserLiveProducer>,
    tab_id: &str,
    url: &str,
) -> Result<(), BrowserError> {
    if let (Some(producer), Some(cdp)) = (producer, backend.as_any().downcast_ref::<CdpBackend>()) {
        let record = cdp.navigate_with_record(tab_id, url).await?;
        publish_record(producer, &record);
        return Ok(());
    }
    backend.navigate(tab_id, url).await
}

/// `history` (back / forward / refresh), publishing under the same rules.
pub(crate) async fn history_and_present(
    backend: &dyn BrowserBackend,
    producer: Option<&BrowserLiveProducer>,
    tab_id: &str,
    nav: HistoryNav,
) -> Result<(), BrowserError> {
    if let (Some(producer), Some(cdp)) = (producer, backend.as_any().downcast_ref::<CdpBackend>()) {
        let record = cdp.history_with_record(tab_id, nav).await?;
        publish_record(producer, &record);
        return Ok(());
    }
    backend.history(tab_id, nav).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_protocol::browser_live::{
        DisplayPolicy, IssuerIdentity, PreviewKind, RedactionStatus,
    };
    use serde_json::json;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;
    use crate::browser::live::grant::ViewerGrantStore;
    use crate::browser::live::resource::ResourceRegistry;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::events::GatewayEventFrame;
    use crate::routing::session_key::SessionKey;
    use crate::scope::ScopeAttribution;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn turn() -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("operation-publish-test"),
            run_id: "operation-publish-run".into(),
            channel_id: "".into(),
            conversation_id: "".into(),
            caller_role: None,
            telecom_authority: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    /// Production shape: the run's TURN_CONTEXT plus the run's persisted scope
    /// attribution. CALLER_USER/CALLER_CONN_ID are RPC-boundary task-locals
    /// and are deliberately NOT scoped here.
    ///
    /// The scoped future is boxed: the CDP navigation futures are large in a
    /// debug build, and nesting them inline in the test task's frame overflows
    /// the default 2 MiB test-thread stack.
    async fn scoped<F: std::future::Future>(future: F) -> F::Output {
        Box::pin(crate::scope::with_scope(
            Some(ScopeAttribution::personal("user-42")),
            Box::pin(TURN_CONTEXT.scope(turn(), Box::pin(future))),
        ))
        .await
    }

    fn producer_with_bus() -> (BrowserLiveProducer, Arc<GatewayEventBus>) {
        let bus = Arc::new(GatewayEventBus::default());
        let producer =
            BrowserLiveProducer::new(ResourceRegistry::default(), ViewerGrantStore::default())
                .with_event_bus(bus.clone());
        (producer, bus)
    }

    /// Fake CDP wire with one attached tab `T1` whose `Page.navigate` replies
    /// with `reply`; returns the shared server and the backend.
    async fn navigable_backend(reply: serde_json::Value) -> (Arc<FakeCdpServer>, CdpBackend) {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on("Page.navigate", Responder::Reply(reply));
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        (Arc::new(server), backend)
    }

    fn spawn_load_events(server: &Arc<FakeCdpServer>) -> tokio::task::JoinHandle<()> {
        let server = Arc::clone(server);
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
    }

    fn ok_reply() -> serde_json::Value {
        json!({ "frameId": "F1", "loaderId": "L1" })
    }

    /// A successful CDP navigation publishes exactly one explicit-only, pending
    /// redaction page resource bound to the real owner session and target.
    #[tokio::test]
    async fn cdp_navigation_publishes_the_exact_session_and_target() {
        let (server, backend) = navigable_backend(ok_reply()).await;
        let (producer, bus) = producer_with_bus();
        let mut rx = bus.subscribe_typed();
        let emitter = spawn_load_events(&server);

        scoped(navigate_and_present(
            &backend,
            Some(&producer),
            "T1",
            "https://ok.example/",
        ))
        .await
        .expect("navigation result is unchanged");
        emitter.await.expect("emitter must not panic");

        let owner = backend.handle().await.unwrap().live_owner();
        let resource = match rx.try_recv().expect("one resource event") {
            GatewayEventFrame::BrowserLiveResource { resource } => resource,
            other => panic!("unexpected frame: {other:?}"),
        };
        let target = &resource.viewer_binding().expect("browser binding").target;
        assert_eq!(target.profile, "default");
        assert_eq!(target.browser_session_id, owner.browser_session_id());
        assert_eq!(target.target_id, "T1");
        assert_eq!(resource.kind, PreviewKind::Page);
        assert_eq!(resource.display, DisplayPolicy::ExplicitOnly);
        assert_eq!(resource.redaction, RedactionStatus::Pending);
        assert_eq!(
            resource.viewer_binding().unwrap().principal,
            IssuerIdentity::User {
                user_id: "user-42".into()
            },
            "the publication names the run scope's person"
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// No producer installed: the plain operation runs and nothing is published.
    #[tokio::test]
    async fn without_a_producer_the_plain_operation_runs_and_nothing_publishes() {
        let (server, backend) = navigable_backend(ok_reply()).await;
        let (_unused_producer, bus) = producer_with_bus();
        let mut rx = bus.subscribe_typed();
        let emitter = spawn_load_events(&server);

        scoped(navigate_and_present(
            &backend,
            None,
            "T1",
            "https://ok.example/",
        ))
        .await
        .expect("plain navigation succeeds");
        emitter.await.expect("emitter must not panic");

        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// A publication problem (no turn / identity in scope) never changes the
    /// operation's result and publishes nothing.
    #[tokio::test]
    async fn publication_failure_does_not_change_the_operation_result() {
        let (server, backend) = navigable_backend(ok_reply()).await;
        let (producer, bus) = producer_with_bus();
        let mut rx = bus.subscribe_typed();
        let emitter = spawn_load_events(&server);

        // Deliberately NOT inside `scoped`: current_binding() => MissingTurn.
        navigate_and_present(&backend, Some(&producer), "T1", "https://ok.example/")
            .await
            .expect("result is unchanged by a skipped publication");
        emitter.await.expect("emitter must not panic");

        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// A failed navigation returns its own error and publishes nothing.
    #[tokio::test]
    async fn failed_navigation_publishes_nothing_and_keeps_its_error() {
        let (_server, backend) =
            navigable_backend(json!({ "frameId": "F1", "errorText": "net::ERR_FAILED" })).await;
        let (producer, bus) = producer_with_bus();
        let mut rx = bus.subscribe_typed();

        let err = scoped(navigate_and_present(
            &backend,
            Some(&producer),
            "T1",
            "https://ok.example/",
        ))
        .await
        .expect_err("errorText fails the navigation");

        assert!(matches!(err, BrowserError::NavigationFailed(_)), "{err:?}");
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// `publish_record` reports `false` (and does not panic) when the producer
    /// has no event bus: publication is denied, nothing leaks.
    #[tokio::test]
    async fn publish_record_without_a_bus_is_denied() {
        let (server, backend) = navigable_backend(ok_reply()).await;
        let producer =
            BrowserLiveProducer::new(ResourceRegistry::default(), ViewerGrantStore::default());
        let emitter = spawn_load_events(&server);
        let record = backend
            .navigate_with_record("T1", "https://ok.example/")
            .await
            .expect("navigation with record");
        emitter.await.expect("emitter must not panic");

        let published = scoped(async { publish_record(&producer, &record) }).await;
        assert!(!published, "no bus => publication denied");
    }
}
