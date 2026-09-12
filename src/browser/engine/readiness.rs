//! The one readiness gate both engines pass through.
//!
//! **Ready ≠ healthy.** The launch-chain round paid for that distinction: a
//! Chromium that answers `/json/version` can still never answer a navigation
//! (the macOS keychain stall — `engine::chromium`'s `--use-mock-keychain`
//! comment). A sentinel that only probes the reachable half is a gate that
//! cannot go red (判据 §2), so this asks all three questions and refuses on
//! any of them.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use aleph_cdp::{CdpConnection, SessionId};

use crate::browser::error::BrowserError;

/// How long an engine has to answer all three probes. spec §6.2.
pub const READY_GATE_BUDGET: Duration = Duration::from_secs(5);

/// The page every readiness probe navigates to. Never a real URL: the SSRF
/// guard vets agent navigations, and a gate that browsed the network would be
/// a second, unguarded navigation path.
const READY_URL: &str = "about:blank";

/// `Browser.getVersion` **and** `Page.navigate about:blank` **and**
/// `Runtime.evaluate("1")`, all inside `budget`.
///
/// The budget wraps the whole sequence rather than each call: three calls each
/// just under the connection's own command timeout add up past any budget the
/// caller thought it set (判据 §13 — a limit's position decides what it limits).
pub async fn ready_gate(
    conn: &CdpConnection,
    session: &SessionId,
    budget: Duration,
) -> Result<(), BrowserError> {
    // Which probe is outstanding right now, so the timeout arm can name the one
    // that actually stalled.
    //
    // It used to name `Page.navigate` unconditionally, on the reasoning that
    // navigation is what stalls in the failure mode this gate exists for. That
    // is a probability, not an observation, and the arm stated it as a fact: an
    // engine wedged in `Browser.getVersion` was reported as a navigation stall,
    // which is the operator's whole diagnosis pointed at the wrong step. A wrong
    // label costs more than a missing one (判据 §17), and a single `AtomicU8` is
    // cheaper than the misdirection.
    let outstanding = AtomicU8::new(PROBE_VERSION);
    let probe = async {
        aleph_cdp::methods::browser::get_version(conn)
            .await
            .map_err(|e| not_ready("Browser.getVersion", &e.to_string()))?;
        outstanding.store(PROBE_NAVIGATE, Ordering::Relaxed);
        aleph_cdp::methods::page::navigate(conn, Some(session), READY_URL)
            .await
            .map_err(|e| not_ready("Page.navigate", &e.to_string()))?;
        outstanding.store(PROBE_EVALUATE, Ordering::Relaxed);
        let eval = aleph_cdp::methods::runtime::evaluate(conn, Some(session), "1", false)
            .await
            .map_err(|e| not_ready("Runtime.evaluate", &e.to_string()))?;
        if let Some(exception) = eval.exception {
            return Err(not_ready("Runtime.evaluate", &exception));
        }
        if eval.value.as_i64() != Some(1) {
            return Err(not_ready(
                "Runtime.evaluate",
                &format!("expected 1, got {}", eval.value),
            ));
        }
        Ok(())
    };
    match tokio::time::timeout(budget, probe).await {
        Ok(result) => result,
        Err(_) => Err(not_ready(
            probe_name(outstanding.load(Ordering::Relaxed)),
            &format!(
                "the engine did not finish the readiness probes within {}s",
                budget.as_secs_f64()
            ),
        )),
    }
}

const PROBE_VERSION: u8 = 0;
const PROBE_NAVIGATE: u8 = 1;
const PROBE_EVALUATE: u8 = 2;

/// The method the gate was waiting on when the budget expired.
///
/// The `_` arm cannot be reached — the only writers are the three constants
/// above — and it answers with a phrase that claims nothing rather than picking
/// a probe, because an unrecognised state word has to read as "I cannot
/// vouch for this" (判据 §17).
fn probe_name(outstanding: u8) -> &'static str {
    match outstanding {
        PROBE_VERSION => "Browser.getVersion",
        PROBE_NAVIGATE => "Page.navigate",
        PROBE_EVALUATE => "Runtime.evaluate",
        _ => "an unidentified readiness probe",
    }
}

/// One error shape for every way the gate can refuse, so the caller never has
/// to guess which step is being reported.
///
/// Every verb named here must EXIST. At HEAD `SessionAction` is `{Save, Load}`
/// (`browser_tools/session.rs`); this plan adds `SwitchEngine` (Task 19) and
/// `Capabilities` (Task 16), and nothing else. A fabricated recovery verb in a
/// fail-closed error is the wrong-label-costs-more shape (判据 §17).
fn not_ready(method: &str, detail: &str) -> BrowserError {
    BrowserError::LaunchFailed {
        stage: "cdp-ready",
        detail: format!(
            "the engine did not become ready: {method} — {detail}. The process \
             is running but unusable; retry, or switch engines with \
             browser_session{{action:\"switch_engine\"}}."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_cdp::{CdpConnection, ConnectOptions};
    use serde_json::json;
    use std::time::Duration;

    /// Answers everything the gate asks for.
    ///
    /// **Deliberately not the shared `engine_peer`** in
    /// [`crate::browser::testkit`]: that one models a whole engine a registry
    /// can launch, attach to and drive (`Target.*`, the three `enable`s); this
    /// answers only the three methods the gate itself asks. Folding them would
    /// give the gate's own tests a peer that also answers methods the gate must
    /// never send, and `version_only` below is a deliberate mutilation of THIS
    /// one — it has to be the smaller thing to stay readable as such.
    fn healthy(msg: &serde_json::Value) -> Responder {
        match msg.get("method").and_then(serde_json::Value::as_str) {
            Some("Browser.getVersion") => Responder::Reply(json!({
                "protocolVersion": "1.3",
                "product": "Chrome/131.0.0.0",
                "revision": "@fake",
                "userAgent": "fake-ua",
                "jsVersion": "13.1"
            })),
            Some("Page.navigate") => Responder::Reply(json!({"frameId": "F1", "loaderId": "L1"})),
            Some("Runtime.evaluate") => Responder::Reply(json!({
                "result": {"type": "number", "value": 1}
            })),
            Some(other) => Responder::Error {
                code: -32601,
                message: format!("fake peer does not implement {other}"),
            },
            None => Responder::Error {
                code: -32600,
                message: "not a request".to_string(),
            },
        }
    }

    /// Answers `/json/version`'s CDP twin and NOTHING else — the shape the
    /// spike found: the browser looks up, and the first navigation in every
    /// page is never answered. 判据 §2: a gate that only asks the reachable
    /// half of the question is a gate that cannot go red.
    ///
    /// `Drop`, not [`Responder::Hang`], and that is not a free choice:
    /// `FakeCdpServer::shutdown` refuses to report a clean stop while a
    /// connection task is still parked on a `Hang`, and says so by panicking
    /// (`shutdown_panics_naming_a_connection_task_that_will_not_end`, that
    /// crate's own `tests/smoke.rs`) — measured here, not assumed. `Drop`
    /// leaves the peer unable to answer just the same, so the gate still
    /// refuses at the step it names; the *slow* peer, which is the other half
    /// of "never became ready", is
    /// [`ready_gate_refuses_a_peer_that_is_slower_than_the_budget`] below.
    fn version_only(msg: &serde_json::Value) -> Responder {
        match msg.get("method").and_then(serde_json::Value::as_str) {
            Some("Browser.getVersion") => healthy(msg),
            _ => Responder::Drop,
        }
    }

    async fn connect(server: &FakeCdpServer) -> CdpConnection {
        CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_millis(150),
            },
        )
        .await
        .expect("the fake peer accepts a websocket")
    }

    #[tokio::test]
    async fn ready_gate_passes_when_the_engine_answers_version_navigate_and_evaluate() {
        let server = FakeCdpServer::start(healthy).await;
        let conn = connect(&server).await;
        let session = aleph_cdp::SessionId("S1".to_string());
        ready_gate(&conn, &session, Duration::from_secs(2))
            .await
            .expect("a peer answering all three is ready");
        // The gate must actually have asked all three — one that only sent
        // getVersion would pass the assertion above for the wrong reason.
        let asked: Vec<String> = server
            .received()
            .iter()
            .filter_map(|m| {
                m.get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert!(asked.iter().any(|m| m == "Browser.getVersion"), "{asked:?}");
        assert!(asked.iter().any(|m| m == "Page.navigate"), "{asked:?}");
        assert!(asked.iter().any(|m| m == "Runtime.evaluate"), "{asked:?}");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn ready_gate_fails_closed_when_only_the_version_probe_answers() {
        let server = FakeCdpServer::start(version_only).await;
        let conn = connect(&server).await;
        let session = aleph_cdp::SessionId("S1".to_string());
        let err = ready_gate(&conn, &session, Duration::from_millis(400))
            .await
            .expect_err("an engine that never navigates is NOT ready");
        let text = err.to_string();
        assert!(
            text.contains("cdp-ready"),
            "the stage must name the step that failed: {text}"
        );
        assert!(
            text.contains("Page.navigate"),
            "the detail must name the unanswered method, not just 'not ready': {text}"
        );
        // Every verb an error names must exist. `SessionAction` is `{Save, Load}`
        // at HEAD plus `SwitchEngine` (Task 19) and `Capabilities` (Task 16);
        // there is no `close`.
        assert!(
            !text.contains("action:\"close\""),
            "the error names a browser_session action that does not exist: {text}"
        );
        server.shutdown().await;
    }

    /// The timeout arm names the probe that was actually outstanding, not the
    /// one most likely to stall.
    ///
    /// Both directions, in one test, because the whole content of the fix is
    /// that the name VARIES: a version that hardcoded either string would pass
    /// one half and fail the other, and a test asserting only the navigate case
    /// is indistinguishable from the hardcoded arm it replaced (判据 §2).
    #[tokio::test]
    async fn the_timeout_arm_names_the_probe_that_was_actually_outstanding() {
        // 1. Wedged on the very first probe.
        let server = FakeCdpServer::start(|_: &serde_json::Value| Responder::Hang).await;
        let conn = CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_secs(30),
            },
        )
        .await
        .expect("connect");
        let session = aleph_cdp::SessionId("S1".to_string());
        let err = ready_gate(&conn, &session, Duration::from_millis(150))
            .await
            .expect_err("a peer that never answers is not ready");
        let text = err.to_string();
        assert!(
            text.contains("Browser.getVersion"),
            "the gate stalled on the version probe and blamed another step: {text}"
        );
        assert!(
            !text.contains("Page.navigate"),
            "it named a probe it never got to send: {text}"
        );
        drop(conn);

        // 2. Version answers, navigation never does — the other direction.
        let server2 = FakeCdpServer::start(|msg: &serde_json::Value| {
            match msg.get("method").and_then(serde_json::Value::as_str) {
                Some("Browser.getVersion") => healthy(msg),
                _ => Responder::Hang,
            }
        })
        .await;
        let conn2 = CdpConnection::connect(
            &server2.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_secs(30),
            },
        )
        .await
        .expect("connect");
        let err2 = ready_gate(&conn2, &session, Duration::from_millis(300))
            .await
            .expect_err("an engine that never navigates is not ready");
        let text2 = err2.to_string();
        assert!(
            text2.contains("Page.navigate"),
            "the gate stalled on the navigation and did not say so: {text2}"
        );
        assert!(
            !text2.contains("Browser.getVersion"),
            "it blamed a probe that had already answered: {text2}"
        );

        // `Hang` parks the peer's connection task forever, which is exactly
        // what `FakeCdpServer::shutdown` refuses to call a clean stop — so
        // these two servers are deliberately left to drop rather than shut
        // down (see `version_only`'s note above).
    }

    /// The budget is the gate's own, not the connection's: a peer that answers
    /// every method but slowly must still be refused. Without the outer
    /// `timeout`, three calls each just under `command_timeout` add up past any
    /// budget the caller thought it set.
    #[tokio::test]
    async fn ready_gate_refuses_a_peer_that_is_slower_than_the_budget() {
        let server = FakeCdpServer::start(|msg: &serde_json::Value| {
            Responder::Delay(Duration::from_millis(120), Box::new(healthy(msg)))
        })
        .await;
        let conn = CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_secs(5),
            },
        )
        .await
        .expect("connect");
        let session = aleph_cdp::SessionId("S1".to_string());
        let err = ready_gate(&conn, &session, Duration::from_millis(150))
            .await
            .expect_err("3 x 120ms does not fit in a 150ms budget");
        assert!(err.to_string().contains("cdp-ready"), "{err}");
        server.shutdown().await;
    }
}
