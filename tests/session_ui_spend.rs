//! Isolated real spend-admission integration coverage.
#![allow(clippy::needless_borrow)]

#[path = "../src/bin/aleph-server/server_init.rs"]
mod server_init;
#[path = "session_ui_readiness/support.rs"]
mod support;

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use alephcore::spend::{self, Delta, InMemorySpendLedger, Principal, SpendLedger};
use futures::FutureExt;
use support::ReadinessRig;

#[tokio::test]
async fn later_spend_rejection_keeps_legitimate_row_without_model_effect() {
    // This integration target is intentionally the only owner of these
    // first-writer globals in its process.
    let ledger = Arc::new(InMemorySpendLedger::default());
    spend::install_ledger(ledger.clone());
    spend::install_policy(
        serde_json::from_value(serde_json::json!({
            "per_user_usd": 0.50,
            "total_usd": null,
            "telecom_public_usd": null,
            "period": "month"
        }))
        .expect("real spend policy must deserialize"),
    );

    let user = "u-r2c-isolated-spend";
    let key = "agent:main:r2c-isolated-spend";
    let mut request_metadata = HashMap::new();
    request_metadata.insert(
        alephcore::gateway::execution_engine::AUTHOR_USER_KEY.to_owned(),
        user.to_owned(),
    );
    let principal = spend::principal_from_metadata(&request_metadata);
    assert_eq!(principal, Principal::user(user));
    let period_start = spend::period::period_start_ms(
        chrono::Utc::now().timestamp_millis(),
        serde_json::from_value(serde_json::json!("month")).expect("real period must deserialize"),
    );
    ledger
        .record(&principal, period_start, Delta::Usd(1.0))
        .expect("real seed spend must be recorded");

    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let before = rig.precreate(user, key).await.expect("precreate real row");
            assert_eq!(before.owner_user_id.as_deref(), Some(user));
            assert_eq!(
                before.scope_id.as_deref(),
                Some("personal:u-r2c-isolated-spend")
            );

            rig.release_prepare();
            rig.release_committed_prepare();
            let response = rig.chat_send(user, key).await;
            assert!(
                response.error.is_none(),
                "keyed success must be returned: {response:?}"
            );
            let result = response.result.expect("keyed success carries a result");
            assert_eq!(result["session_key"], key);
            let run_id = result["run_id"].as_str().expect("keyed success run id");

            let terminal = rig.wait_run_error(run_id).await;
            assert_eq!(terminal["params"]["run_id"], run_id);
            assert_eq!(terminal["params"]["session_key"], key);
            assert_eq!(terminal["params"]["error_code"], "SPEND_EXHAUSTED");
            assert!(terminal["params"]["error"].as_str().is_some());

            let after = rig
                .metadata(key)
                .await
                .expect("metadata read")
                .expect("canonical row remains after terminal denial");
            assert_eq!(after.owner_user_id, before.owner_user_id);
            assert_eq!(after.scope_id, before.scope_id);
            let committed = rig.committed_metadata();
            assert!(
                !committed.is_empty(),
                "real metadata commit must be observed"
            );
            assert!(committed.iter().all(|row| {
                row.owner_user_id.as_deref() == Some(user)
                    && row.scope_id.as_deref() == Some("personal:u-r2c-isolated-spend")
            }));

            assert_eq!(rig.local_provider_effects(), 0);
            assert_eq!(rig.external_provider_effects(), 0);
            assert_eq!(rig.tool_effects(), 0);
            assert_eq!(rig.active_run_id(key), None);
        })
        .await
        .expect("isolated spend case must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R2-C cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("owned fixture runtime and store calls must be drained");
}
