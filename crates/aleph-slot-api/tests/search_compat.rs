//! Task 3 — search/embedding contract compatibility pins (API side).
//!
//! These tests pin the *public contract surface* that moved from
//! `alephcore` (`src/search/{provider,options,result}.rs`,
//! `src/memory/embedding_provider.rs`) to this crate in Task 3 of the P0
//! modularization. They are the API-path half of the migration proof; the
//! type-identity half (old Core path == new API path, same `TypeId`) lives
//! in `alephcore`'s `tests/modularization_contracts.rs`.
//!
//! What is pinned here (per the Task 3 plan, item a):
//!
//! * `SearchCapabilities::default()` is three `false` bits — a provider
//!   that declares nothing is invisible to dimension-aware routing, which
//!   is the safe direction.
//! * `SearchOptions` legacy JSON defaults and the `Recency` serde
//!   vocabulary are unchanged (wire compat with pre-migration callers).
//! * `SearchResult::new(title, url, snippet)` is the exact constructor.
//! * `Recency::Year` must NOT be claimed as Bing-supported: the
//!   `bing_freshness` mapper answers `None` for it, and no capability bit
//!   may say otherwise.

use aleph_slot_api::embedding::EmbeddingProvider;
use aleph_slot_api::search::{
    Recency, SearchCapabilities, SearchOptions, SearchProvider, SearchResult, MAX_SEARCH_RESULTS,
};

/// The all-`false` default is the fail-safe: a provider that forgets to
/// declare a capability is *invisible* to dimension-aware routing. The
/// reverse default would make every provider claim everything.
#[test]
fn search_capabilities_default_is_all_false() {
    let caps = SearchCapabilities::default();
    assert!(!caps.domain_filter);
    assert!(!caps.recency);
    assert!(!caps.full_content);
    assert_eq!(
        caps,
        SearchCapabilities {
            domain_filter: false,
            recency: false,
            full_content: false,
        }
    );
}

/// The 11-field default shape, pinned field by field. These are the values
/// pre-migration callers observed from `SearchOptions::default()`.
#[test]
fn search_options_default_matches_pre_migration_shape() {
    let o = SearchOptions::default();
    assert_eq!(o.language, None);
    assert_eq!(o.region, None);
    assert_eq!(o.recency, None);
    assert!(o.safe_search);
    assert_eq!(o.max_results, 5);
    assert_eq!(o.timeout_seconds, 10);
    assert!(!o.include_full_content);
    assert!(o.include_domains.is_empty());
    assert!(o.exclude_domains.is_empty());
    assert!(o.providers.is_empty());
    assert_eq!(o.snippet_budget_chars, None);
}

/// Legacy wire compat: an empty JSON object decodes to the same defaults,
/// and a partial legacy object fills the rest from defaults. This is the
/// behaviour every pre-migration serialized caller relied on.
#[test]
fn search_options_legacy_json_still_decodes_to_defaults() {
    let from_empty: SearchOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(from_empty.max_results, 5);
    assert_eq!(from_empty.timeout_seconds, 10);
    assert!(from_empty.safe_search);

    let partial: SearchOptions =
        serde_json::from_str(r#"{"max_results": 10, "recency": "week"}"#).unwrap();
    assert_eq!(partial.max_results, 10);
    assert_eq!(partial.recency, Some(Recency::Week));
    // Untouched fields still default.
    assert_eq!(partial.timeout_seconds, 10);
    assert!(partial.safe_search);
}

/// The serde vocabulary of `Recency` is the wire contract: four lowercase
/// tokens, anything else rejected *naming the legal values* (not silently
/// dropped the way the pre-enum mappers dropped them).
#[test]
fn recency_serde_vocabulary_unchanged() {
    for (variant, token) in [
        (Recency::Day, "day"),
        (Recency::Week, "week"),
        (Recency::Month, "month"),
        (Recency::Year, "year"),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, format!("\"{token}\""));
        let back: Recency = serde_json::from_str(&json).unwrap();
        assert_eq!(back, variant);
    }

    let err = serde_json::from_str::<Recency>("\"7d\"").unwrap_err();
    assert!(
        err.to_string().contains("day"),
        "the error must list the legal values: {err}"
    );
}

/// `SearchResult::new(title, url, snippet)` is the exact constructor: the
/// three required fields set, every `Option` field `None`.
#[test]
fn search_result_new_constructor_exact() {
    let r = SearchResult::new(
        "Test Title".to_string(),
        "https://example.com".to_string(),
        "Test snippet".to_string(),
    );
    assert_eq!(r.title, "Test Title");
    assert_eq!(r.url, "https://example.com");
    assert_eq!(r.snippet, "Test snippet");
    assert_eq!(r.relevance_score, None);
    assert_eq!(r.full_content, None);
    assert_eq!(r.published_date, None);
    assert_eq!(r.provider, None);

    // `impl Into<String>` parameters accept borrowed strings too.
    let borrowed = SearchResult::new("t", "u", "s");
    assert_eq!(borrowed.title, "t");
}

/// `Recency::Year` has no Bing `freshness` spelling. The mapper must answer
/// `None` for it — and therefore nothing may claim Bing carries a Year
/// constraint. (The runtime half of this pin is
/// `bing.rs::the_recency_bit_is_false_for_the_one_bucket_bing_cannot_express`,
/// which stays in Core with the provider.)
#[test]
fn bing_freshness_has_no_year_bucket() {
    let with = |r: Recency| SearchOptions {
        recency: Some(r),
        ..Default::default()
    };
    assert_eq!(with(Recency::Day).bing_freshness(), Some("Day"));
    assert_eq!(with(Recency::Week).bing_freshness(), Some("Week"));
    assert_eq!(with(Recency::Month).bing_freshness(), Some("Month"));
    // Year must NOT be claimed as Bing-supported.
    assert_eq!(with(Recency::Year).bing_freshness(), None);
}

/// The single `max_results` ceiling stays 100, and the request-side clamp
/// holds at both ends.
#[test]
fn max_search_results_ceiling_and_clamp() {
    assert_eq!(MAX_SEARCH_RESULTS, 100);
    let clamped_high = SearchOptions {
        max_results: 500,
        ..Default::default()
    };
    assert_eq!(clamped_high.validated_max_results(), MAX_SEARCH_RESULTS);
    let clamped_low = SearchOptions {
        max_results: 0,
        ..Default::default()
    };
    assert_eq!(clamped_low.validated_max_results(), 1);
}

/// The trait's `capabilities` default is the all-false bitset: a provider
/// that does not override it declares nothing.
#[test]
fn provider_trait_default_capabilities_is_all_false() {
    struct Stub;

    #[async_trait::async_trait]
    impl SearchProvider for Stub {
        async fn search(
            &self,
            _query: &str,
            _options: &SearchOptions,
        ) -> aleph_slot_api::Result<Vec<SearchResult>> {
            Ok(vec![])
        }

        fn name(&self) -> &str {
            "stub"
        }

        fn is_available(&self) -> bool {
            false
        }
    }

    let stub = Stub;
    // Object safety: the trait must remain usable behind `dyn`.
    let obj: &dyn SearchProvider = &stub;
    let caps = obj.capabilities(&SearchOptions::default());
    assert_eq!(caps, SearchCapabilities::default());
    assert!(!caps.domain_filter && !caps.recency && !caps.full_content);
}

/// The `EmbeddingProvider` contract surface: five methods, exact
/// signatures, object-safe. The async methods are only *implemented* here
/// (no runtime in this crate); the sync ones are called.
#[test]
fn embedding_provider_trait_signature_pin() {
    struct StubEmbed;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbed {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, aleph_slot_api::AlephError> {
            Ok(vec![0.0; 3])
        }

        async fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, aleph_slot_api::AlephError> {
            Ok(texts.iter().map(|_| vec![0.0; 3]).collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn model_name(&self) -> &str {
            "stub-model"
        }

        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    let stub = StubEmbed;
    let obj: &dyn EmbeddingProvider = &stub;
    assert_eq!(obj.dimensions(), 3);
    assert_eq!(obj.model_name(), "stub-model");
    assert_eq!(obj.provider_id(), "stub");
}
