//! End-to-end driver for the pairing wizard via the JSON-RPC handler surface.
//!
//! Drives wizard.start("pairing") → wizard.answer(welcome) → wizard.answer(approve)
//! → asserts the response contains a token payload in `data.token`.

use std::sync::Arc;

use alephcore::gateway::device_store::DeviceStore;
use alephcore::gateway::handlers::wizard::{
    handle_answer, handle_start, WizardFlowFactory, WizardSessionManager,
};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::security::{PairingManager, SecurityStore, TokenManager};
use alephcore::wizard::{PairingFlow, WizardFlow};
use serde_json::json;

fn make_manager() -> Arc<WizardSessionManager> {
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    let devices = Arc::new(DeviceStore::in_memory().unwrap());
    let pairing = Arc::new(PairingManager::new(security.clone()));
    let tokens = Arc::new(TokenManager::new(security.clone()));

    let security_f = security.clone();
    let pairing_f = pairing.clone();
    let devices_f = devices.clone();
    let tokens_f = tokens.clone();

    let factory: WizardFlowFactory =
        Arc::new(move |wizard_type: &str, _initial: Option<serde_json::Value>| {
            if wizard_type == "pairing" {
                Some(Box::new(PairingFlow::new(
                    "E2E Mac",
                    pairing_f.clone(),
                    security_f.clone(),
                    devices_f.clone(),
                    tokens_f.clone(),
                )) as Box<dyn WizardFlow>)
            } else {
                None
            }
        });

    Arc::new(WizardSessionManager::new(factory))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_wizard_round_trip_returns_token() {
    let manager = make_manager();

    // ── 1. wizard.start ──────────────────────────────────────────────────────
    let req = JsonRpcRequest::new(
        "wizard.start",
        Some(json!({ "wizard_type": "pairing" })),
        Some(json!(1)),
    );
    let resp = handle_start(req, manager.clone()).await;
    assert!(
        resp.is_success(),
        "wizard.start failed: {:?}",
        resp.error
    );

    let start_body: serde_json::Value = resp.result.unwrap();
    let session_id = start_body["session_id"]
        .as_str()
        .expect("session_id must be a string")
        .to_string();

    // The first step is the welcome note (prompt_no_wait → auto step ID).
    let first_step = start_body["step"].as_object().expect("step must be present");
    let welcome_step_id = first_step["id"]
        .as_str()
        .expect("step.id must be a string")
        .to_string();
    // The welcome note is a prompt_no_wait — it does NOT have a pending answer slot.
    // Verify the step is of type "note" (or at least is not "pairing-approve" yet).
    assert_ne!(
        welcome_step_id, "pairing-approve",
        "first step should be the welcome note, not the confirm step"
    );

    // ── 2. wizard.answer(welcome note, null) ─────────────────────────────────
    // Acknowledging a note step: since prompt_no_wait does not register a pending
    // answer, the session.answer call will fail with StepNotFound (the note step
    // has no pending answer). Instead we call handle_answer and expect it to
    // return the next step (the confirm step), or we use handle_next if answer fails.
    //
    // The actual protocol: handle_answer calls manager.answer() then manager.next().
    // For a note step (no pending answer registered), manager.answer returns an
    // error because current_step != None but no PendingAnswer exists.
    // We therefore call wizard.answer with the confirm step directly by first
    // calling handle_next to surface the confirm step.
    //
    // Use handle_next to advance past the welcome note (which was emitted via
    // prompt_no_wait and has no answer slot).
    let req_next = JsonRpcRequest::new(
        "wizard.next",
        Some(json!({ "session_id": session_id })),
        Some(json!(2)),
    );
    use alephcore::gateway::handlers::wizard::handle_next;
    let resp_next = handle_next(req_next, manager.clone()).await;
    assert!(
        resp_next.is_success(),
        "wizard.next failed: {:?}",
        resp_next.error
    );
    let next_body: serde_json::Value = resp_next.result.unwrap();
    let approve_step = next_body["step"].as_object().expect("confirm step must be present");
    assert_eq!(
        approve_step["id"].as_str().unwrap(),
        "pairing-approve",
        "second step must be pairing-approve"
    );

    // ── 3. wizard.answer(pairing-approve, true) ───────────────────────────────
    let req_approve = JsonRpcRequest::new(
        "wizard.answer",
        Some(json!({
            "session_id": session_id,
            "step_id": "pairing-approve",
            "value": true,
        })),
        Some(json!(3)),
    );
    let resp_approve = handle_answer(req_approve, manager.clone()).await;
    assert!(
        resp_approve.is_success(),
        "wizard.answer(approve) failed: {:?}",
        resp_approve.error
    );
    let approve_body: serde_json::Value = resp_approve.result.unwrap();

    // ── 4. If not yet done, call wizard.next once more ─────────────────────────
    let final_body = if approve_body["done"].as_bool().unwrap_or(false) {
        approve_body
    } else {
        let req_final = JsonRpcRequest::new(
            "wizard.next",
            Some(json!({ "session_id": session_id })),
            Some(json!(4)),
        );
        let resp_final = handle_next(req_final, manager.clone()).await;
        assert!(
            resp_final.is_success(),
            "final wizard.next failed: {:?}",
            resp_final.error
        );
        resp_final.result.unwrap()
    };

    // ── 5. Assert token payload ───────────────────────────────────────────────
    assert_eq!(
        final_body["done"].as_bool(),
        Some(true),
        "wizard must be done after approve; got {:?}",
        final_body
    );
    let data = final_body["data"].as_object().expect("data must be present");
    let token = data["token"].as_str().expect("token must be a string");
    assert!(
        token.contains(':'),
        "token must be in <body>:<sig> format, got: {token}"
    );
    assert_eq!(
        data["device_name"].as_str(),
        Some("E2E Mac"),
        "device_name must match"
    );
    assert!(
        data["device_id"].as_str().is_some(),
        "device_id must be present"
    );
}
