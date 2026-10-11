//! Public telecom run -> formal spend ledger (public session isolation
//! contract §8). Drives the real `HarnessRunner::run_public_text` and checks
//! the run's usage lands on `Principal::TelecomPublic`, never on the ambient
//! owner, and that a public cap refuses before any provider call.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::types::policies::{SpendPeriod, SpendPolicy};
use crate::error::Result as AlephResult;
use crate::orchestrator::dispatch::{HarnessRunner, PublicRunRequest, PUBLIC_PROMPT_VERSION};
use crate::providers::adapter::{ProviderResponse, RequestPayload, StopReason, TokenUsage};
use crate::providers::AiProvider;
use crate::spend::{Principal, Spent};
use crate::sync_primitives::Arc;

/// Provider that answers with real usage so the metering decorator has
/// something to record. Its name is not in the price table: the call is
/// recorded as `unpriced`, which is enough to prove *where* it was charged.
struct UsageProvider;

impl AiProvider for UsageProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            Ok(ProviderResponse {
                text: Some("public reply".to_string()),
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 40,
                    output_tokens: 10,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    thinking_tokens: None,
                    cost: None,
                }),
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "public-spend-canary-provider"
    }
    fn color(&self) -> &str {
        "#000"
    }
}

fn request() -> PublicRunRequest {
    PublicRunRequest {
        ctx: crate::gateway::telecom::public_context::test_support::sample_public_context("bind-a"),
        text: "hello".to_string(),
        run_id: "run-spend-1".into(),
        prompt_version: PUBLIC_PROMPT_VERSION,
    }
}

/// Force the process-wide policy/ledger to a known enabled state (the same
/// ensure-not-install dance as `providers::metering`'s test helper).
fn install_policy(policy: SpendPolicy) {
    crate::spend::install_policy(policy.clone());
    assert!(crate::spend::update_policy(policy));
    crate::spend::install_ledger(Arc::new(crate::spend::InMemorySpendLedger::default()));
}

fn spent_now(principal: &Principal) -> Spent {
    let policy = crate::spend::current_policy();
    let start =
        crate::spend::period::period_start_ms(chrono::Utc::now().timestamp_millis(), policy.period);
    crate::spend::global_ledger()
        .spent_for(principal, start)
        .unwrap()
}

/// Canary: a real public run, started under a live owner scope, is metered
/// under `TelecomPublic` and leaves the owner's row (and the unattributed
/// row) alone.
#[tokio::test]
#[serial_test::serial(spend_global_policy)]
async fn public_run_meters_under_telecom_public_not_the_ambient_owner() {
    install_policy(SpendPolicy {
        per_user_usd: Some(1_000_000.0),
        period: SpendPeriod::Month,
        ..SpendPolicy::default()
    });
    let owner = Principal::User("u-public-spend-owner".to_string());
    let (runner, _owner_session) =
        super::tests::runner_with_provider_for_public(Arc::new(UsageProvider), "OWNER-CANARY")
            .await;
    let public_before = spent_now(&Principal::TelecomPublic);
    let owner_before = spent_now(&owner);
    let unattributed_before = spent_now(&Principal::Unattributed);

    let (tx, _rx) = broadcast::channel(4);
    let out = crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal(
            "u-public-spend-owner",
        )),
        async {
            runner
                .run_public_text(request(), CancellationToken::new(), tx)
                .await
        },
    )
    .await
    .expect("public run ok");
    assert_eq!(out.final_text, "public reply");

    assert!(
        spent_now(&Principal::TelecomPublic).unpriced_calls > public_before.unpriced_calls,
        "the public run's LLM call must be recorded under TelecomPublic"
    );
    assert_eq!(
        spent_now(&owner).unpriced_calls,
        owner_before.unpriced_calls,
        "ambient owner must not be charged for a public run"
    );
    assert_eq!(
        spent_now(&Principal::Unattributed).unpriced_calls,
        unattributed_before.unpriced_calls
    );
}

/// With a public cap of $0 the run is refused at the provider floor: the
/// provider is never reached and no owner principal is involved.
#[tokio::test]
#[serial_test::serial(spend_global_policy)]
async fn public_run_over_public_cap_is_refused_before_any_provider_call() {
    install_policy(SpendPolicy {
        per_user_usd: Some(1_000_000.0),
        telecom_public_usd: Some(0.0),
        period: SpendPeriod::Month,
        ..SpendPolicy::default()
    });
    let (runner, _owner_session) =
        super::tests::runner_with_provider_for_public(Arc::new(UsageProvider), "OWNER-CANARY")
            .await;
    let public_before = spent_now(&Principal::TelecomPublic);

    let (tx, _rx) = broadcast::channel(4);
    let result = runner
        .run_public_text(request(), CancellationToken::new(), tx)
        .await;

    // Restore a non-capping policy for the next test in the group.
    install_policy(SpendPolicy {
        per_user_usd: Some(1_000_000.0),
        period: SpendPeriod::Month,
        ..SpendPolicy::default()
    });
    assert!(
        result.is_err(),
        "capped public run must not produce a reply"
    );
    assert_eq!(
        spent_now(&Principal::TelecomPublic).unpriced_calls,
        public_before.unpriced_calls,
        "a refused call records nothing"
    );
}
