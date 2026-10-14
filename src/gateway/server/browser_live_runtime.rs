//! Single browser-live runtime composition (Task 20): ONE lifecycle (stores +
//! event bus), ONE exact-owner registry, and the producer / issuer / viewer
//! backend that all hold them. Nothing else may build a second store.
//!
//! Default is deny-only: [`GatewayServer::install_browser_live_runtime`]
//! installs nothing, and returns no producer, unless the operator configured an
//! explicit, valid `[gateway] browser_viewer_origin`. The origin is never
//! inferred from the bind address, a request Origin, or `allow_any_origin`.

use std::time::Duration;

use aleph_protocol::browser_live::ViewportCssPx;

use super::GatewayServer;
use crate::browser::error::BrowserError;
use crate::browser::live::control::InterventionSink;
use crate::browser::live::grant::{GrantDenied, ViewerGrantStore};
use crate::browser::live::issuer::BrowserViewerIssuerState;
use crate::browser::live::lifecycle::BrowserLiveLifecycle;
use crate::browser::live::owner_registry::LiveOwnerRegistry;
use crate::browser::live::resource::ResourceRegistry;
use crate::browser::live::viewer_backend::{LiveViewerBackend, ViewerBackend};
use crate::browser::network_policy::BrowserSsrfGuard;
use crate::builtin_tools::browser_tools::present::BrowserLiveProducer;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;

/// Longest viewer grant the issuer will mint.
const MAX_VIEWER_GRANT_TTL: Duration = Duration::from_secs(600);
/// Cadence of the TTL sweep. Expiry is wall-clock based inside the lifecycle,
/// so this only bounds how late an expired resource's `Closed` event is sent.
const TTL_SWEEP_INTERVAL: Duration = Duration::from_secs(1);
/// Fixed CSS viewport handed to `bind_control`.
const VIEWER_VIEWPORT: ViewportCssPx = ViewportCssPx {
    width: 1280,
    height: 800,
};

/// Fail-closed intervention destination. No durable human-intervention
/// destination exists yet, and the issuer only mints `Frames` grants, so input
/// (the only caller of the sink) is unreachable; if it ever were reached, the
/// delivery fails and `TargetControl` closes the control plane.
fn fail_closed_sink() -> InterventionSink {
    Arc::new(|_| {
        Err(BrowserError::ActionFailed(
            "no intervention destination is configured".into(),
        ))
    })
}

/// One sweep pass. A failure (e.g. a close event that could not be announced)
/// is logged and never stops the scheduler; the next tick retries.
fn sweep_once(lifecycle: &BrowserLiveLifecycle) {
    if let Err(error) = lifecycle.sweep_expired() {
        tracing::warn!(%error, "browser-live TTL sweep failed");
    }
}

/// Start the single TTL maintenance task for `lifecycle`, if a Tokio runtime is
/// current. Without one (synchronous unit tests) nothing is started and the
/// runtime still composes. Missed ticks are skipped, not replayed.
fn spawn_ttl_sweeper(lifecycle: BrowserLiveLifecycle) -> Option<tokio::task::JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    Some(handle.spawn(async move {
        let mut ticker = tokio::time::interval(TTL_SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            sweep_once(&lifecycle);
        }
    }))
}

/// The composed runtime. The lifecycle (stores + event bus) and the owner
/// registry are created once in [`Self::compose`] and cloned into exactly these
/// three members plus the TTL sweeper; there is no other handle to either.
pub(crate) struct BrowserLiveRuntime {
    producer: Arc<BrowserLiveProducer>,
    issuer: Arc<BrowserViewerIssuerState>,
    backend: Arc<LiveViewerBackend>,
    /// TTL sweeper task; `None` when composed outside a Tokio runtime. Dropping
    /// a `JoinHandle` detaches (does not cancel) the task.
    ttl_sweeper: Option<tokio::task::JoinHandle<()>>,
}

impl BrowserLiveRuntime {
    /// Build the runtime, or deny. `origin` must be the explicit trusted viewer
    /// origin; an invalid origin fails the whole composition (no partial parts).
    pub(crate) fn compose(
        origin: &str,
        event_bus: Arc<GatewayEventBus>,
        guard: BrowserSsrfGuard,
    ) -> Result<Self, GrantDenied> {
        let lifecycle =
            BrowserLiveLifecycle::new(ResourceRegistry::default(), ViewerGrantStore::default())
                .with_event_bus(event_bus);
        let owners = LiveOwnerRegistry::default();
        // Every owner that registers here reports document boundaries
        // (navigation, reload, event gap) straight to this lifecycle, which
        // demotes the approvals of that exact browser session + target.
        owners.set_boundary_sink(lifecycle.boundary_sink());
        // Every owner also reports exact-target destruction and owner end, so
        // the lifecycle closes the matching live resources (Closed events).
        owners.set_terminal_sink(lifecycle.terminal_sink());
        let issuer = BrowserViewerIssuerState::new_with_lifecycle(
            lifecycle.clone(),
            origin,
            MAX_VIEWER_GRANT_TTL,
        )?;
        let backend = Arc::new(LiveViewerBackend::new(
            owners.clone(),
            fail_closed_sink(),
            VIEWER_VIEWPORT,
            guard,
            lifecycle.resources().clone(),
        ));
        let producer = Arc::new(BrowserLiveProducer::new_with_lifecycle(
            lifecycle.clone(),
            owners.clone(),
        ));
        // Spawn only after every fallible step succeeded, so a denied
        // composition never leaves a sweeper behind.
        let ttl_sweeper = spawn_ttl_sweeper(lifecycle);
        Ok(Self {
            producer,
            issuer,
            backend,
            ttl_sweeper,
        })
    }
}

impl GatewayServer {
    /// Compose and mount the browser-live runtime on this server's event bus.
    ///
    /// `None` / invalid origin installs nothing: the generic deny handlers and
    /// the unmounted viewer route stay as they are, and no producer is returned
    /// (so `BuiltinToolConfig::browser_live_producer` stays `None`). On success
    /// the backend + issuer are installed and the SAME runtime's producer is
    /// returned for the browser tools.
    pub fn install_browser_live_runtime(
        &mut self,
        origin: Option<&str>,
        guard: BrowserSsrfGuard,
    ) -> Option<Arc<BrowserLiveProducer>> {
        let origin = origin?;
        let runtime = match BrowserLiveRuntime::compose(origin, self.event_bus.clone(), guard) {
            Ok(runtime) => runtime,
            Err(_) => {
                tracing::warn!(
                    "browser live viewer disabled: [gateway] browser_viewer_origin is not a valid http(s) origin"
                );
                return None;
            }
        };
        // Install live user validation before any issuer is exposed. A missing
        // SecurityStore denies issuance rather than trusting a stale caller.
        self.browser_viewer_grants
            .bind(runtime.issuer.grants.clone(), self.security_store.clone());
        let backend: Arc<dyn ViewerBackend> = runtime.backend.clone();
        self.set_browser_viewer_backend(backend, runtime.issuer.clone());
        // The sweeper handle is intentionally dropped (detached): the task owns
        // its lifecycle clone and lives until the Tokio runtime shuts down.
        // GatewayServer has no browser-maintenance shutdown seam yet.
        drop(runtime.ttl_sweeper);
        Some(runtime.producer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::live::issuer::tests::{ORIGIN, USER};
    use crate::browser::live::resource::ResourceDraft;
    use aleph_protocol::browser_live::{
        BrowserTargetBinding, ConversationBinding, DisplayPolicy, IssueViewerGrantRequest,
        IssuerIdentity, PreviewKind, PreviewProvenance, PreviewSource, RedactionStatus,
        ViewerBinding, ViewerPermission,
    };

    fn server() -> GatewayServer {
        let mut server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        let users = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        users
            .create_user(
                USER,
                "Viewer",
                crate::gateway::security::store::UserRole::Member,
            )
            .unwrap();
        server.set_security_store(users);
        server
    }

    #[test]
    fn absent_or_invalid_origin_installs_nothing_and_yields_no_producer() {
        for origin in [
            None,
            Some(""),
            Some("viewer.example.test"),
            Some("https://viewer.example.test/path"),
            Some("ftp://viewer.example.test"),
        ] {
            let mut server = server();
            assert!(server
                .install_browser_live_runtime(origin, BrowserSsrfGuard::default())
                .is_none());
            assert!(server.browser_viewer.is_none(), "origin {origin:?}");
        }
    }

    #[test]
    fn compose_without_tokio_runtime_starts_no_sweeper() {
        let runtime = BrowserLiveRuntime::compose(
            ORIGIN,
            Arc::new(GatewayEventBus::new()),
            BrowserSsrfGuard::default(),
        )
        .expect("valid origin composes without a runtime");
        assert!(runtime.ttl_sweeper.is_none());
    }

    #[tokio::test]
    async fn compose_inside_tokio_runtime_starts_one_sweeper() {
        let runtime = BrowserLiveRuntime::compose(
            ORIGIN,
            Arc::new(GatewayEventBus::new()),
            BrowserSsrfGuard::default(),
        )
        .expect("valid origin composes");
        let sweeper = runtime
            .ttl_sweeper
            .expect("sweeper spawned under a runtime");
        assert!(!sweeper.is_finished());
        sweeper.abort();
    }

    #[test]
    fn valid_origin_installs_backend_and_issuer_over_the_producers_stores() {
        let mut server = server();
        let producer = server
            .install_browser_live_runtime(Some(ORIGIN), BrowserSsrfGuard::default())
            .expect("explicit valid origin composes the runtime");
        let (issuer, _backend) = server
            .browser_viewer
            .as_ref()
            .expect("backend + issuer installed together");

        // Publish through the PRODUCER's lifecycle, issue through the INSTALLED
        // issuer: success proves they share the resource and grant stores.
        let binding = ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: USER.into(),
            },
            task: ConversationBinding {
                session_key: "session-one".into(),
                run_id: "run-one".into(),
            },
            target: BrowserTargetBinding {
                profile: "default".into(),
                browser_session_id: "browser-one".into(),
                target_id: "target-one".into(),
            },
        };
        let resource = producer
            .lifecycle()
            .publish(ResourceDraft {
                binding,
                kind: PreviewKind::Page,
                display: DisplayPolicy::AutoRevealCurrentRun,
                source: PreviewSource::BrowserTool,
                provenance: PreviewProvenance::NotManaged,
                redaction: RedactionStatus::Safe,
                ttl: Duration::from_secs(600),
                companion_candidates: Vec::new(),
            })
            .expect("the composed lifecycle has the server event bus");
        let issued = issuer
            .issue(
                USER,
                IssueViewerGrantRequest {
                    resource_id: resource.resource_id.clone(),
                    permissions: vec![ViewerPermission::Frames],
                },
            )
            .expect("issuer sees the producer's resource");
        assert!(producer
            .lifecycle()
            .grants()
            .grant_info(&issued.grant_id)
            .is_some());

        // The deactivation handle the boot kick holds (taken from the server,
        // late-bound by this install) revokes through the SAME grant store.
        let revocation = server.browser_viewer_grant_revocation();
        assert_eq!(revocation.revoke_user("someone-else"), 0);
        assert_eq!(revocation.revoke_user(USER), 1);
        assert!(producer
            .lifecycle()
            .grants()
            .grant_info(&issued.grant_id)
            .is_none());
    }

    #[test]
    fn revocation_handle_taken_before_install_is_bound_by_install() {
        let mut server = server();
        let early = server.browser_viewer_grant_revocation();
        assert_eq!(early.revoke_user(USER), 0, "inert before install");
        server
            .install_browser_live_runtime(Some(ORIGIN), BrowserSsrfGuard::default())
            .expect("valid origin");
        let (issuer, _) = server.browser_viewer.as_ref().expect("installed");
        let principal = IssuerIdentity::User {
            user_id: USER.into(),
        };
        issuer
            .grants
            .issue(
                principal.clone(),
                ViewerBinding {
                    principal,
                    task: ConversationBinding {
                        session_key: "s".into(),
                        run_id: "r".into(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".into(),
                        browser_session_id: "b".into(),
                        target_id: "t".into(),
                    },
                },
                "res",
                &[ViewerPermission::Frames],
                aleph_protocol::browser_live::ViewerOrigin {
                    scheme: "http".into(),
                    host: "127.0.0.1".into(),
                    port: 8787,
                },
                Duration::from_secs(600),
            )
            .expect("issue");
        assert_eq!(
            early.revoke_user(USER),
            1,
            "early clone sees the bound store"
        );
    }
}
