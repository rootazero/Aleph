// Browser click tool — clicks an element on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::profile::BrowserDriver;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_click` tool.
///
/// At least one targeting method must be provided: `ref_id` or coordinates (`x`/`y`).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Accessibility `ref_id` from a previous snapshot.
    pub ref_id: Option<String>,
    /// X coordinate for coordinate-based clicking.
    pub x: Option<f64>,
    /// Y coordinate for coordinate-based clicking.
    pub y: Option<f64>,
    /// Double-click instead of single-click. With `x`/`y` this needs a
    /// `driver = "cdp"` profile; the other two drivers double-click only by
    /// `ref_id`.
    #[serde(default)]
    pub double: bool,
}

/// Output from the `browser_click` tool.
#[derive(Debug, Serialize)]
pub struct BrowserClickOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Clicks an element on the page by `ref_id` or page coordinates — the space
/// `browser_snapshot` prints element geometry in.
#[derive(Clone)]
pub struct BrowserClickTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserClickTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate clicks behind a user-defined approval policy. With no policy wired
    /// the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// Lower the model's targeting arguments to an [`ActionTarget`].
///
/// Returns the contract as a message rather than an error: a malformed request
/// degrades to `success:false` with the contract spelled out, never a hard
/// `Err` — the convention `exec.rs` and `wait_for.rs` already state in prose,
/// and previously the one place this family disagreed with itself (click and
/// select hard-errored while type and fill_form did not, so the same mistake
/// reached the model two different ways).
///
/// The `driver` is threaded in as a PARAMETER rather than fetched here: this is
/// a free function with no `self` and therefore no manager, and keeping it a
/// total function of its inputs is what makes the resolution testable and keeps
/// it ahead of the approval gate.
fn resolve_target(
    args: &BrowserClickArgs,
    driver: Option<BrowserDriver>,
) -> std::result::Result<ActionTarget, String> {
    if let Some(ref rid) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: rid.clone(),
        })
    } else if let (Some(x), Some(y)) = (args.x, args.y) {
        // Driver-dependent, not universal: the CDP backend dispatches two
        // press/release pairs at a point, so a coordinate double-click is a
        // real call on a `driver = "cdp"` profile. The other two have only a
        // ref-taking native primitive and refuse a coordinate outright. Still
        // refused BEFORE the approval gate for the drivers that cannot serve it
        // — a call rejected by construction must not spend a user approval —
        // and an unknown profile (`None`) refuses too, which is the fail-closed
        // direction.
        if args.double && driver != Some(BrowserDriver::Cdp) {
            return Err(
                "browser_click double=true by coordinates needs a driver=\"cdp\" profile; \
                 this profile's driver double-clicks only by ref_id. Call browser_snapshot \
                 and pass the ref_id it reports, or use a cdp profile."
                    .into(),
            );
        }
        Ok(ActionTarget::Coordinates { x, y })
    } else {
        Err(
            "browser_click requires at least one targeting method: ref_id (from \
             browser_snapshot) or x/y coordinates"
                .into(),
        )
    }
}

#[async_trait]
impl AlephTool for BrowserClickTool {
    const NAME: &'static str = "browser_click";
    const DESCRIPTION: &'static str =
        "Click an element on the page by accessibility ref_id or coordinates; \
         set double=true for a double-click (by coordinates only on a \
         driver=\"cdp\" profile)";
    type Args = BrowserClickArgs;
    type Output = BrowserClickOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate before the approval check: a malformed call is a model
        // mistake and must not consume a user approval or touch the page.
        let driver = self.manager.get_driver(&args.profile);
        let target = match resolve_target(&args, driver) {
            Ok(t) => t,
            Err(message) => {
                return Ok(BrowserClickOutput {
                    success: false,
                    message: Some(message),
                });
            }
        };
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserClick,
            "click",
            &format!("{target:?}"),
        )
        .await
        {
            return Ok(BrowserClickOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                let result = if args.double {
                    backend.dblclick(&tab_id, target).await
                } else {
                    backend.click(&tab_id, target).await
                };
                match result {
                    Ok(()) => Ok(BrowserClickOutput {
                        success: true,
                        message: Some(format!(
                            "{} in profile '{}'",
                            if args.double {
                                "Double-clicked"
                            } else {
                                "Clicked"
                            },
                            args.profile
                        )),
                    }),
                    Err(e) => Ok(BrowserClickOutput {
                        success: false,
                        message: Some(format!(
                            "Click failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                    }),
                }
            }
            Err(e) => Ok(BrowserClickOutput {
                success: false,
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_click_with_coordinates() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: Some(100.0),
                y: Some(200.0),
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_click_no_target_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        // A malformed call degrades to success:false with the contract spelled
        // out — the same shape type/fill_form/batch/wait_for already use.
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("ref_id")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_click_malformed_call_does_not_consume_approval() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        // Deny clicks outright: a targetless call must still report the
        // targeting contract, proving validation ran before the gate.
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserClick, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserClickTool::new(manager).with_approval_policy(policy);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("ref_id") && !m.contains("denied")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_double_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: Some("e7".into()),
                x: None,
                y: None,
                double: true,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    /// `double: true` with coordinates is a call **this profile's driver**
    /// cannot serve — the managed driver's `dblclick` takes a ref only — so it
    /// must be refused with the contract, and refused before the approval gate,
    /// since a rejected-by-construction call must not spend a user approval.
    ///
    /// The condition narrowed when the CDP backend gained a real coordinate
    /// double-click: the refusal is no longer universal, so the fixture's
    /// driver is load-bearing and is named in the assertion below.
    ///
    /// ⚠️ The fixture **configures `managed` explicitly** rather than taking
    /// `ProfileConfig::default()`. It used to take the default, which was
    /// `managed`; the dual-engine flip made the default `cdp`, i.e. the one
    /// driver for which this call is legal. The precondition assertion its
    /// author installed is what caught that — a fixture that silently became
    /// the opposite of the subject would otherwise have passed this test for
    /// the opposite reason.
    #[tokio::test]
    async fn double_click_by_coordinates_is_refused_before_the_approval_gate() {
        use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingAllow(Arc<AtomicUsize>);
        #[async_trait]
        impl ApprovalPolicy for CountingAllow {
            async fn check(&self, _req: &ActionRequest) -> ApprovalDecision {
                self.0.fetch_add(1, Ordering::SeqCst);
                ApprovalDecision::Allow
            }
            async fn record(&self, _req: &ActionRequest, _dec: &ApprovalDecision) {}
        }

        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            crate::browser::profile::ProfileConfig {
                driver: BrowserDriver::Managed,
                ..crate::browser::profile::ProfileConfig::default()
            },
        );
        let manager = Arc::new(ProfileManager::new(config));
        // The premise the refusal rests on, asserted rather than assumed:
        // with a `cdp` profile this call is legal, so a fixture that had
        // drifted to one would make this test pass for the opposite reason.
        assert_eq!(
            manager.get_driver("default"),
            Some(BrowserDriver::Managed),
            "precondition: the refusal below holds only for a non-cdp driver"
        );
        let asked = Arc::new(AtomicUsize::new(0));
        let tool = BrowserClickTool::new(manager)
            .with_approval_policy(
                Arc::new(CountingAllow(Arc::clone(&asked))) as Arc<dyn ApprovalPolicy>
            );

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: Some(10.0),
                y: Some(20.0),
                double: true,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("needs a driver=\"cdp\" profile")),
            "got: {:?}",
            result.message
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "a call the backend cannot serve must not consume an approval"
        );
    }

    /// The `DESCRIPTION` and the guard state the same fact, so they have to be
    /// pinned to each other: the sentence names the driver that CAN do it, and
    /// the guard lets exactly that driver through. A `DESCRIPTION` edit that
    /// dropped the driver name would leave the model told "coordinates only" on
    /// a profile where coordinates work (判据 §1, and 判据 §17 — the wrong
    /// label costs more than the vague one).
    #[test]
    fn the_description_names_the_driver_that_supports_a_coordinate_double_click() {
        let d = BrowserClickTool::DESCRIPTION;
        assert!(d.contains("double=true"), "still documents the flag: {d}");
        assert!(
            d.contains("cdp"),
            "must name the driver a coordinate double-click needs: {d}"
        );
        assert!(
            !d.contains("ref_id only"),
            "the old universal restriction must be gone, not merely added to: {d}"
        );
    }

    /// The permissive half of the narrowed predicate, at the only layer that
    /// can be reached without a manager wired to a live engine: a `cdp` profile
    /// lowers a coordinate `double: true` to a real `Coordinates` target
    /// instead of refusing it.
    ///
    /// Written against `resolve_target` rather than through `call` because the
    /// end-to-end version needs Task 14's manager wiring (a `driver = "cdp"`
    /// profile plus a registry seeded through `insert_for_test`). Without
    /// something here, reverting the predicate to the unconditional
    /// `if args.double` would leave every test in this file green — the guard
    /// would be 恒红 for the cdp case and nothing would say so (判据 §2).
    #[test]
    fn a_cdp_profile_lowers_a_coordinate_double_click_instead_of_refusing_it() {
        let args = BrowserClickArgs {
            profile: "default".into(),
            ref_id: None,
            x: Some(10.0),
            y: Some(20.0),
            double: true,
        };
        match resolve_target(&args, Some(BrowserDriver::Cdp)) {
            Ok(ActionTarget::Coordinates { x, y }) => {
                assert!((x - 10.0).abs() < f64::EPSILON && (y - 20.0).abs() < f64::EPSILON);
            }
            other => panic!("a cdp profile must serve this call, got {other:?}"),
        }
        // Every non-cdp driver, plus the unknown profile, refuses — the
        // fail-closed direction. Derived from `BrowserDriver::ALL`, which
        // exists for exactly this ("an enumerator reaches a new one by adding a
        // variant here rather than by being remembered elsewhere"). The list
        // used to be hand-written under a comment claiming it enumerated, so a
        // fourth variant would have inherited the permission with this test
        // green and still saying it covered the case (判据 §5 + §1, and the
        // comment was the lying half).
        let others: Vec<Option<BrowserDriver>> = BrowserDriver::ALL
            .into_iter()
            .filter(|d| *d != BrowserDriver::Cdp)
            .map(Some)
            .chain(std::iter::once(None))
            .collect();
        assert_eq!(
            others.len(),
            BrowserDriver::ALL.len(),
            "precondition: exactly one variant is cdp, so this list is every \
             other driver plus the unknown profile"
        );
        for driver in others {
            assert!(
                resolve_target(&args, driver).is_err(),
                "a coordinate double-click must be refused for {driver:?}"
            );
        }
    }

    /// The other half of the narrowed predicate, end to end, on the wire.
    ///
    /// `a_cdp_profile_lowers_a_coordinate_double_click_instead_of_refusing_it`
    /// above proves the RESOLUTION; it says nothing about whether the call
    /// reaches a page, because `resolve_target` never touches one. This is the
    /// half that needed Task 14's manager wiring: a `driver = "cdp"` profile
    /// routed to a real `CdpBackend`, and a registry seeded with a handle so no
    /// browser is launched.
    ///
    /// Asserted by EFFECT, on the wire: six `Input.dispatchMouseEvent` frames
    /// in the order `dblclick` dispatches them, at the requested point. "The
    /// tool returned success" is equally true of a backend that sent nothing
    /// (判据 §4).
    #[tokio::test]
    async fn a_coordinate_double_click_reaches_the_page_on_a_cdp_profile() {
        use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
        use crate::browser::cdp_backend::test_support::wire_session;
        use crate::browser::engine::{Engine, EngineHandle};
        use crate::browser::profile::ProfileConfig;
        use aleph_cdp::testkit::{FakeCdpServer, Responder};
        use aleph_cdp::{CdpConnection, ConnectOptions, TargetId};
        use async_trait::async_trait;
        use serde_json::json;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingAllow(Arc<AtomicUsize>);
        #[async_trait]
        impl ApprovalPolicy for CountingAllow {
            async fn check(&self, _req: &ActionRequest) -> ApprovalDecision {
                self.0.fetch_add(1, Ordering::SeqCst);
                ApprovalDecision::Allow
            }
            async fn record(&self, _req: &ActionRequest, _dec: &ApprovalDecision) {}
        }

        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        // Scroll offset 0 keeps the page→viewport arithmetic out of THIS claim;
        // the conversion has its own test, and one that is deliberately
        // scrolled:
        // `cdp_backend::actions::tests::a_coordinate_click_is_converted_from_page_space_to_the_viewport`.
        // Asserted below rather than assumed, so "the point arrived" cannot
        // quietly become "the point happened to survive a conversion".
        server.on(
            "Page.getLayoutMetrics",
            Responder::Reply(json!({
                "cssVisualViewport": {
                    "pageX": 0.0, "pageY": 0.0,
                    "clientWidth": 1280.0, "clientHeight": 800.0, "scale": 1.0
                },
                "cssContentSize": { "width": 1280.0, "height": 800.0 }
            })),
        );
        server.on("Input.dispatchMouseEvent", Responder::Reply(json!({})));

        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "cdp".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                // Explicit, so routing never derives a path from `ALEPH_HOME`.
                user_data_dir: Some("/nonexistent/aleph-click-test".into()),
                ..Default::default()
            },
        );
        let manager = Arc::new(ProfileManager::new(config));
        let conn = CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: std::time::Duration::from_millis(400),
            },
        )
        .await
        .expect("the fake server accepts a websocket");
        let handle = Arc::new(EngineHandle::for_test(Engine::Chromium, "cdp", conn));
        handle
            .attach_tab(&TargetId("T1".into()))
            .await
            .expect("attach");
        manager.engines().insert_for_test("cdp", handle).await;

        let asked = Arc::new(AtomicUsize::new(0));
        let tool = BrowserClickTool::new(Arc::clone(&manager))
            .with_approval_policy(
                Arc::new(CountingAllow(Arc::clone(&asked))) as Arc<dyn ApprovalPolicy>
            );

        let result = tool
            .call(BrowserClickArgs {
                profile: "cdp".into(),
                ref_id: None,
                x: Some(120.0),
                y: Some(48.0),
                double: true,
            })
            .await
            .unwrap();

        assert!(
            result.success,
            "the guard must let a cdp profile through: {:?}",
            result.message
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "the call reached the approval gate exactly once — the guard did \
             not refuse it beforehand, and did not skip the gate either"
        );

        let mouse: Vec<(String, f64, f64, u64)> = server
            .received()
            .iter()
            .filter(|m| m["method"].as_str() == Some("Input.dispatchMouseEvent"))
            .map(|m| {
                (
                    m["params"]["type"].as_str().unwrap_or_default().to_string(),
                    m["params"]["x"].as_f64().unwrap_or_default(),
                    m["params"]["y"].as_f64().unwrap_or_default(),
                    m["params"]["clickCount"].as_u64().unwrap_or_default(),
                )
            })
            .collect();

        // Two press/release pairs, each preceded by its own move — the sequence
        // `actions::dblclick` builds. A single `clickCount: 2` press does not
        // produce a `dblclick` event in Chromium, which is why there are two
        // pairs rather than one.
        let shape: Vec<(&str, u64)> = mouse.iter().map(|(t, _, _, c)| (t.as_str(), *c)).collect();
        assert_eq!(
            shape,
            vec![
                ("mouseMoved", 0),
                ("mousePressed", 1),
                ("mouseReleased", 1),
                ("mouseMoved", 0),
                ("mousePressed", 2),
                ("mouseReleased", 2),
            ],
            "got {mouse:?}"
        );
        assert!(
            mouse
                .iter()
                .all(|(_, x, y, _)| (*x - 120.0).abs() < f64::EPSILON
                    && (*y - 48.0).abs() < f64::EPSILON),
            "every event lands on the requested point: {mouse:?}"
        );
    }

    #[tokio::test]
    async fn test_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: Some("ref-42".into()),
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}
