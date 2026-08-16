//! Real-API prompt-cache hit contract (e2e).
//!
//! The wire-level contract tests (`adapter_tests::prefix_stability`) prove the
//! request *bytes* are shaped for a cache hit; this proves the *provider
//! actually honours them*: two calls sharing a >1024-token stable prefix, the
//! second must report `cache_read_tokens > 0`. Without a live assertion like
//! this, a silent break in the breakpoint layout only shows up on the bill.
//!
//! `#[ignore]`d by default — compile-time zero cost in the suite. Requires:
//! - `ANTHROPIC_API_KEY` in the environment
//! - network access to `api.anthropic.com`
//! - optional `ALEPH_CACHE_E2E_MODEL` (default `claude-haiku-4-5`) — the
//!   smallest cacheable model keeps the probe cheap; 1024 tokens is the
//!   prompt-cache minimum for every current Claude model.
//!
//! Run manually:
//! `cargo test -p alephcore --test cache_hit_e2e -- --ignored`

use alephcore::providers::adapter::RequestPayload;
use alephcore::providers::message::UnifiedMessage;
use alephcore::providers::{create_provider, AiProvider};
use alephcore::thinker::prompt_builder::SystemPromptPart;
use alephcore::ProviderConfig;

/// A stable prefix comfortably past the 1024-token cache minimum
/// (~4 chars/token → 6000+ chars of varied prose).
fn stable_prefix() -> String {
    let mut s = String::from(
        "You are a precise code-review assistant. The following standing rules \
         govern every review you perform.\n",
    );
    for i in 0..120 {
        s.push_str(&format!(
            "Rule {i}: when examining change {i}, consider correctness, cache \
             locality, error propagation, naming clarity, and the blast radius \
             of the abstraction before suggesting any refactor.\n"
        ));
    }
    s
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY and network access"]
async fn second_request_with_shared_prefix_reads_cache() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("ANTHROPIC_API_KEY not set — skipping live cache-hit probe");
        return;
    };
    let model =
        std::env::var("ALEPH_CACHE_E2E_MODEL").unwrap_or_else(|_| "claude-haiku-4-5".to_string());

    let mut config = ProviderConfig::test_config(&model);
    config.api_key = Some(api_key);
    let provider: std::sync::Arc<dyn AiProvider> =
        create_provider("claude", config).expect("provider builds");

    let parts = [SystemPromptPart {
        content: stable_prefix(),
        cache: true,
    }];

    // Call 1: cold — establishes the cache entry (creation > 0, or an
    // immediate read if a previous probe run is still within the 5m TTL).
    let msgs1 = [UnifiedMessage::user("Reply with exactly: ok")];
    let payload1 = RequestPayload::new(&msgs1).with_system_blocks(Some(&parts));
    let resp1 = provider
        .process(payload1)
        .await
        .expect("first live call succeeds");
    let usage1 = resp1.usage.expect("live API always reports usage");

    // Call 2: same stable prefix, different trailing user turn. The shared
    // prefix must come back as cache READS — this is the assertion the whole
    // domain exists to keep true.
    let msgs2 = [
        UnifiedMessage::user("Reply with exactly: ok"),
        UnifiedMessage::assistant("ok"),
        UnifiedMessage::user("Reply with exactly: done"),
    ];
    let payload2 = RequestPayload::new(&msgs2).with_system_blocks(Some(&parts));
    let resp2 = provider
        .process(payload2)
        .await
        .expect("second live call succeeds");
    let usage2 = resp2.usage.expect("live API always reports usage");

    eprintln!(
        "call1: input={} read={:?} creation={:?} | call2: input={} read={:?} creation={:?}",
        usage1.input_tokens,
        usage1.cache_read_tokens,
        usage1.cache_creation_tokens,
        usage2.input_tokens,
        usage2.cache_read_tokens,
        usage2.cache_creation_tokens,
    );
    assert!(
        usage2.cache_read_tokens.unwrap_or(0) > 0,
        "second request over a shared >1024-token prefix must hit the prompt cache \
         (got cache_read_tokens={:?}); a miss here means the breakpoint layout \
         regressed — the bytes ahead of the marker are not stable",
        usage2.cache_read_tokens
    );
    // Sanity: the cached share should be the bulk of the prompt, i.e. the
    // canonical ratio is well above zero.
    let ratio = usage2
        .cache_hit_ratio()
        .expect("cache reads reported ⇒ ratio defined");
    assert!(
        ratio > 0.5,
        "expected the stable prefix to dominate the prompt (ratio {ratio:.2})"
    );
}
