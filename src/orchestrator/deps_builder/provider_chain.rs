//! Failover provider chain assembly.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use crate::config::types::ProviderConfig;
use crate::config::Config;
use crate::providers::model_catalog::endpoint_kind_for_base_url;
use crate::providers::route_observe::{ChainCandidate, RouteObservability};
use crate::providers::route_policy::EndpointTier;
use crate::providers::{
    create_provider, AiProvider, DefaultProviderHandle, FailoverConfig, FailoverHealth,
    FailoverNode, FailoverProvider, LoadStats, ModelCooldown, ProviderCooldown, StaticDefault,
};
use crate::sandbox::exec_approval::gate::ApprovalRequester;

/// Sentinel name for the fallback node that wraps the whole global chain
/// inside a per-`provider_hint` override. Cannot collide with a real provider
/// (names come from `[providers]` toml keys), so `FailoverProvider`'s
/// primary-vs-fallback dedup never drops it.
const GLOBAL_CHAIN_NODE: &str = "__global_chain__";

/// Endpoint tier for one configured provider, used by the route policy.
///
/// Special-cases `protocol == "ollama"` with no `base_url` → [`Local`]
/// ([`OllamaProvider`](crate::providers::OllamaProvider) defaults to
/// `http://localhost:11434`). Otherwise defers to
/// [`endpoint_kind_for_base_url`], which keys on the `base_url` host so
/// `ollama.com` (Cloud) and a local Ollama (Local) are distinguished.
///
/// [`Local`]: EndpointTier::Local
pub(crate) fn provider_tier(pc: &ProviderConfig) -> EndpointTier {
    let is_native_ollama = pc
        .protocol
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("ollama"));
    if is_native_ollama && pc.base_url.as_deref().is_none_or(str::is_empty) {
        return EndpointTier::Local;
    }
    endpoint_kind_for_base_url(pc.base_url.as_deref()).into()
}

/// Provider routing assembled once at boot.
///
/// `default` is the global failover chain — wired as `deps.llm` for the main
/// harness and used as the subagent provider for agents without a
/// `provider_hint`. `agent_overrides` maps a `provider_hint` to a
/// `FailoverProvider` that pins that provider, then falls through the entire
/// global chain (the "pin + fall-through" semantics).
#[derive(Clone)]
pub struct ProviderChain {
    pub default: Arc<dyn DefaultProviderHandle>,
    pub agent_overrides: HashMap<String, Arc<dyn AiProvider>>,
    /// Live handles onto the chain's shared runtime state (breaker, cooldowns,
    /// load) plus the boot-time chain composition. The production boot path
    /// registers a clone as the process-global `route_observe` bundle so the
    /// `self_config` `route_status` action can render live diagnostics.
    pub observability: RouteObservability,
}

/// Build the provider routing: the global failover chain plus the
/// per-`provider_hint` override registry.
///
/// The global chain wraps `default_provider` in a [`FailoverProvider`] that
/// walks `[primary, ...[fallback_provider].chain]`. The primary slot stays
/// *live*: the `FailoverProvider` reads `default_provider.current()` on every
/// call, so a UI `set_default` swap takes effect on the next turn (hot-reload
/// preserved). Every configured provider's `models` list feeds the catalog,
/// so the chain can also fail over *between models* of one provider.
///
/// Each `agent_overrides` entry pins one configured provider as the primary
/// and adds a single fallback node = the whole global chain. All chains share
/// one [`FailoverHealth`], so one provider's outage is visible everywhere.
/// The primary provider itself gets no override entry — hinting it is
/// equivalent to not hinting at all.
///
/// A provider that fails to construct, and a chain entry that matches the
/// primary or is absent from `[providers]`, are skipped with a warn log.
pub fn build_failover_chain(
    config: &Config,
    primary_provider_key: &str,
    default_provider: Arc<dyn DefaultProviderHandle>,
    escalation_approval: Option<Arc<dyn ApprovalRequester>>,
    route_handle: Option<Arc<crate::providers::route_handle::RouteHandle>>,
) -> ProviderChain {
    // Local/cloud route preference (`[route]`). `Auto` (default) is a no-op,
    // so an unconfigured deployment is byte-identical to pre-route failover.
    let route_mode = config.route.mode;
    let allow_escalation = config.route.allow_cloud_escalation;
    // Catalog of every configured provider's model list, keyed by toml name.
    // Lets the live primary — and each fallback — fail over across models.
    let model_catalog: HashMap<String, Vec<String>> = config
        .providers
        .iter()
        // rust-doctor-disable-next-line excessive-clone
        .map(|(name, pc)| (name.clone(), pc.all_models().to_vec()))
        .collect();

    // Shared circuit-breaker health: the global chain and every per-hint
    // override see the same provider-outage picture.
    let health = FailoverHealth::default();

    // Shared per-model rate-limit cooldown: a model-specific 429 sidelines that
    // one model (not the whole provider) across every chain built here.
    let model_cooldown = ModelCooldown::default();

    // Shared per-provider rate-limit cooldown gate: after a provider 429s, the
    // next turn paces itself (waits out the recorded window) before re-dialing
    // it, instead of eating a fresh 429 and bouncing to a fallback — keeps a
    // single paid primary (e.g. Kimi) in use. Scoped exactly like `health`.
    let provider_cooldown = ProviderCooldown::default();

    // Operator-tunable in-place retry budget (`[fallback_provider].max_retries`),
    // falling back to the built-in default. Useful for single-provider setups
    // with no sibling to fail over to.
    let failover_config = FailoverConfig {
        max_retries: config
            .fallback_provider
            .as_ref()
            .and_then(|fb| fb.max_retries)
            .unwrap_or_else(|| FailoverConfig::default().max_retries),
        ..FailoverConfig::default()
    };

    // Shared runtime load registry: in-flight counts and observed latencies are
    // visible to every chain that might dial a given endpoint, so the
    // load-balancing strategy sees one consistent picture (scoped exactly like
    // `health` — one registry per boot-time chain assembly).
    let load = Arc::new(LoadStats::new());

    // Build every non-primary provider once, reused by both the fallback
    // chain and the per-hint override registry (no double construction).
    let mut built: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
    for (name, pc) in &config.providers {
        if name.eq_ignore_ascii_case(primary_provider_key) {
            continue; // the primary is already built behind `default_provider`
        }
        // rust-doctor-disable-next-line excessive-clone
        match create_provider(name, pc.clone()) {
            Ok(provider) => {
                // rust-doctor-disable-next-line excessive-clone
                built.insert(name.clone(), provider);
            }
            Err(e) => tracing::warn!(
                provider = %name,
                error = %e,
                "provider build failed; skipping"
            ),
        }
    }

    // Ordered fallback chain. Primary source is `[fallback_provider].chain`;
    // when that resolves to nothing usable (missing section, typo'd entry, or a
    // provider that failed to build) the helper auto-derives a chain from every
    // other enabled provider — so a throttled primary always has somewhere to
    // migrate instead of hard-failing the run on a 429.
    let (fallbacks, auto_derived) =
        assemble_fallbacks(config, primary_provider_key, &built, &model_catalog);

    tracing::info!(
        primary = %primary_provider_key,
        fallback_count = fallbacks.len(),
        override_count = built.len(),
        "failover chain assembled"
    );

    // Capture the chain composition + shared-state handles for the read-only
    // observability bundle before `fallbacks` moves into the provider. Each
    // handle clone shares the same live `Arc` map the chains mutate, so a
    // later snapshot always reads the current breaker/cooldown/load picture.
    let observability = RouteObservability {
        // rust-doctor-disable-next-line excessive-clone
        primary: default_provider.clone(),
        fallbacks: fallbacks
            .iter()
            .map(|n| ChainCandidate {
                // rust-doctor-disable-next-line excessive-clone
                name: n.name.clone(),
                // rust-doctor-disable-next-line excessive-clone
                models: n.models.clone(),
                tier: n.tier,
            })
            .collect(),
        // rust-doctor-disable-next-line excessive-clone
        health: health.clone(),
        // rust-doctor-disable-next-line excessive-clone
        model_cooldown: model_cooldown.clone(),
        // rust-doctor-disable-next-line excessive-clone
        provider_cooldown: provider_cooldown.clone(),
        // rust-doctor-disable-next-line excessive-clone
        load: load.clone(),
        // rust-doctor-disable-next-line excessive-clone
        route: route_handle.clone(),
    };

    let global_provider = FailoverProvider::new(
        default_provider,
        fallbacks,
        // rust-doctor-disable-next-line excessive-clone
        model_catalog.clone(),
        // rust-doctor-disable-next-line excessive-clone
        health.clone(),
        // rust-doctor-disable-next-line excessive-clone
        failover_config.clone(),
    )
    // rust-doctor-disable-next-line excessive-clone
    .with_route(route_mode, allow_escalation, escalation_approval.clone())
    // rust-doctor-disable-next-line excessive-clone
    .with_load_stats(load.clone())
    // rust-doctor-disable-next-line excessive-clone
    .with_model_cooldown(model_cooldown.clone())
    // rust-doctor-disable-next-line excessive-clone
    .with_provider_cooldown(provider_cooldown.clone());
    // A live handle (production) makes mode switches hot-apply; its absence
    // (tests) keeps the boot snapshot above — byte-identical to before.
    // rust-doctor-disable-next-line excessive-clone
    let global_provider = match route_handle.clone() {
        Some(h) => global_provider.with_route_live(h),
        None => global_provider,
    };
    // Auto-derived chains (no operator `[fallback_provider].chain`) derive the
    // fallback set live from the registry each turn, so a provider added or
    // removed at runtime joins/leaves the fallback set without a restart. An
    // explicit chain keeps the static snapshot built above.
    let global_provider = if auto_derived {
        global_provider.with_live_fallback_derivation()
    } else {
        global_provider
    };
    let global: Arc<dyn AiProvider> = Arc::new(global_provider);
    // rust-doctor-disable-next-line excessive-clone
    let default: Arc<dyn DefaultProviderHandle> = Arc::new(StaticDefault::new(global.clone()));

    // Per-`provider_hint` overrides: one FailoverProvider per non-primary
    // provider, pinning it as primary then falling through the global chain.
    let agent_overrides: HashMap<String, Arc<dyn AiProvider>> = built
        .into_iter()
        .map(|(name, provider)| {
            // The pinned provider's real tier, so a hard-guardrail route mode
            // (`AlwaysLocal`) routes an explicit cloud pin through the
            // borrow-cloud approval instead of silently allowing it. Unknown
            // configured tier defaults to Cloud (the conservative, gated side).
            let pin_tier = config
                .providers
                .get(&name)
                .map_or(EndpointTier::Cloud, provider_tier);
            let pinned = FailoverProvider::new(
                Arc::new(StaticDefault::new(provider)),
                vec![FailoverNode {
                    name: GLOBAL_CHAIN_NODE.to_string(),
                    models: Vec::new(),
                    // rust-doctor-disable-next-line excessive-clone
                    provider: global.clone(),
                    // The global-chain wrapper is itself route-shaped; tag it
                    // Unknown so the pinned chain's own route policy never drops
                    // it (the global chain already enforced the tier policy).
                    tier: EndpointTier::Unknown,
                }],
                // rust-doctor-disable-next-line excessive-clone
                model_catalog.clone(),
                // rust-doctor-disable-next-line excessive-clone
                health.clone(),
                // rust-doctor-disable-next-line excessive-clone
                failover_config.clone(),
            )
            // rust-doctor-disable-next-line excessive-clone
            .with_route(route_mode, allow_escalation, escalation_approval.clone())
            .with_primary_tier(pin_tier)
            // rust-doctor-disable-next-line excessive-clone
            .with_load_stats(load.clone())
            // rust-doctor-disable-next-line excessive-clone
            .with_model_cooldown(model_cooldown.clone())
            // rust-doctor-disable-next-line excessive-clone
            .with_provider_cooldown(provider_cooldown.clone());
            // rust-doctor-disable-next-line excessive-clone
            let pinned = match route_handle.clone() {
                Some(h) => pinned.with_route_live(h),
                None => pinned,
            };
            (name, Arc::new(pinned) as Arc<dyn AiProvider>)
        })
        .collect();

    ProviderChain {
        default,
        agent_overrides,
        observability,
    }
}

/// Assemble the ordered fallback chain for the global failover provider.
///
/// Source of truth is `[fallback_provider].chain` (with the back-compat
/// `provider` field folded in by [`FallbackProviderToml::resolved_chain`]),
/// resolved against the already-built non-primary provider set. Two hardening
/// rules close the gap that let a 429 hard-fail despite the provider-layer
/// retry/cooldown machinery — the failure was never "retry too shallow", it
/// was "the chain to migrate into is empty":
///
/// 1. A chain entry naming a provider absent from `[providers]` (a typo, or one
///    that failed to build) is logged at ERROR — loud enough to actually notice
///    in the boot log — then skipped, instead of the old silent WARN.
/// 2. If the resolved chain ends up empty but other enabled providers exist,
///    derive the chain from all of them (deterministic, name-sorted). A missing
///    or fully-invalid `[fallback_provider]` therefore no longer strands the
///    primary with nowhere to fail over to: model fallback works by default.
///
/// An explicit, non-empty chain is always honored verbatim — auto-derivation
/// only fills a vacuum, it never overrides operator intent.
/// Returns the ordered fallback nodes plus whether they were *auto-derived*
/// (`true`) rather than taken from an explicit operator chain. The global chain
/// uses the flag to enable live, registry-backed fallback derivation so a
/// provider added/removed at runtime is reflected without a restart; an explicit
/// chain stays a static snapshot.
fn assemble_fallbacks(
    config: &Config,
    primary_provider_key: &str,
    built: &HashMap<String, Arc<dyn AiProvider>>,
    model_catalog: &HashMap<String, Vec<String>>,
) -> (Vec<FailoverNode>, bool) {
    // Build one node from a name + its already-constructed provider. Tier comes
    // from the provider's base_url host so the route policy can order/gate this
    // fallback by local-vs-cloud.
    let node_for = |name: &str, provider: &Arc<dyn AiProvider>| FailoverNode {
        name: name.to_string(),
        models: model_catalog.get(name).cloned().unwrap_or_default(),
        tier: config
            .providers
            .get(name)
            .map_or(EndpointTier::Cloud, provider_tier),
        // rust-doctor-disable-next-line excessive-clone
        provider: provider.clone(),
    };

    // Effective protocol of a provider ("anthropic" / "openai" / ...), used to
    // prefer *same-protocol* fallbacks. Cross-protocol failover (e.g. an
    // Anthropic-protocol primary like Kimi migrating to an OpenAI-compatible
    // endpoint) has to convert the request shape, and some OpenAI-compat Claude
    // shims reject the conversion outright (the -10003 "bad parameter" gap). A
    // homogeneous chain sends the same format the primary already proved valid.
    let protocol_of = |name: &str| -> Option<String> {
        config
            .providers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, pc)| pc.protocol())
    };
    let primary_protocol = protocol_of(primary_provider_key);

    let mut fallbacks: Vec<FailoverNode> = Vec::new();
    if let Some(fb) = config.fallback_provider.as_ref() {
        for name in fb.resolved_chain() {
            if name.eq_ignore_ascii_case(primary_provider_key) {
                tracing::warn!(provider = %name, "failover chain: entry matches primary; skipping");
                continue;
            }
            let Some(provider) = built.get(&name) else {
                tracing::error!(
                    provider = %name,
                    "failover chain: '{name}' is not defined under [providers] (or failed to \
                     build) — fix the [fallback_provider].chain entry; skipping it",
                );
                continue;
            };
            // Loud diagnostic for a cross-protocol fallback: it still runs (the
            // operator configured it on purpose), but a protocol mismatch is the
            // root of the request-conversion rejection class, so make it visible.
            if let (Some(prim), Some(fbp)) = (&primary_protocol, protocol_of(&name)) {
                if !fbp.eq_ignore_ascii_case(prim) {
                    tracing::warn!(
                        primary_protocol = %prim,
                        fallback = %name,
                        fallback_protocol = %fbp,
                        "failover chain: fallback protocol differs from primary — cross-protocol \
                         failover converts the request shape and some OpenAI-compatible endpoints \
                         reject it (e.g. -10003); prefer a same-protocol fallback",
                    );
                }
            }
            fallbacks.push(node_for(&name, provider));
        }
    }

    // Self-heal: a primary with no usable configured fallback still gets one,
    // derived from every other enabled provider. Same-protocol providers are
    // preferred (listed first) so an auto-derived chain stays homogeneous when
    // possible; name-sort breaks ties for determinism.
    let mut auto_derived = false;
    if fallbacks.is_empty() && !built.is_empty() {
        auto_derived = true;
        let mut names: Vec<&String> = built.keys().collect();
        names.sort();
        if let Some(prim) = &primary_protocol {
            // Stable sort: within each protocol group the name order is kept,
            // and same-as-primary (`false`) sorts before different (`true`).
            names.sort_by_key(|n| !protocol_of(n).is_some_and(|p| p.eq_ignore_ascii_case(prim)));
        }
        for name in names {
            if let Some(provider) = built.get(name) {
                fallbacks.push(node_for(name, provider));
            }
        }
        tracing::warn!(
            derived_count = fallbacks.len(),
            "failover chain: no usable [fallback_provider].chain — auto-derived fallback from all \
             enabled providers (same-protocol first) so the primary can fail over on \
             rate-limit/outage",
        );
    }

    (fallbacks, auto_derived)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::FallbackProviderToml;
    use crate::orchestrator::deps_builder::common::cfg_with_fallback;
    use crate::providers::StaticDefault;
    use crate::ProviderConfig;

    fn mock_provider_config() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some("mock".to_string());
        pc
    }

    fn provider_config_with_protocol(proto: &str) -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some(proto.to_string());
        pc
    }

    fn mock_handle(name: &str) -> Arc<dyn DefaultProviderHandle> {
        let provider = create_provider(name, mock_provider_config()).expect("mock provider");
        Arc::new(StaticDefault::new(provider))
    }

    /// The already-built non-primary provider set, exactly as
    /// `build_failover_chain` constructs it before assembling the chain.
    fn built_map(names: &[&str]) -> HashMap<String, Arc<dyn AiProvider>> {
        names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    create_provider(n, mock_provider_config()).expect("mock provider"),
                )
            })
            .collect()
    }

    fn fallback_names(nodes: &[FailoverNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn assemble_fallbacks_auto_derives_when_no_chain_configured() {
        // No `[fallback_provider]` at all, but two other enabled providers
        // exist → they become the fallback chain (sorted) so the primary can
        // still fail over.
        let cfg = cfg_with_fallback(
            None,
            vec![
                ("primary", mock_provider_config()),
                ("aux2", mock_provider_config()),
                ("aux1", mock_provider_config()),
            ],
        );
        let built = built_map(&["aux1", "aux2"]);
        let (nodes, auto_derived) = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        assert!(auto_derived, "no configured chain → auto-derived");
        assert_eq!(fallback_names(&nodes), vec!["aux1", "aux2"]);
    }

    #[test]
    fn assemble_fallbacks_auto_derives_when_chain_entries_all_invalid() {
        // The real-world bug: the chain names providers that were never defined
        // under `[providers]` (`minimax`/`chatgpt`), so every entry skips. The
        // one healthy alternative is auto-derived instead of leaving the chain
        // empty (which previously made a 429 hard-fail with nowhere to migrate).
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["minimax".to_string(), "chatgpt".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("kimi", mock_provider_config()),
                ("x302", mock_provider_config()),
            ],
        );
        // `kimi` is the primary, so only `x302` is in the built set.
        let built = built_map(&["x302"]);
        let (nodes, auto_derived) = assemble_fallbacks(&cfg, "kimi", &built, &HashMap::new());
        assert!(auto_derived, "all chain entries invalid → auto-derived");
        assert_eq!(fallback_names(&nodes), vec!["x302"]);
    }

    #[test]
    fn assemble_fallbacks_prefers_same_protocol_in_auto_derive() {
        // Primary is anthropic-protocol (e.g. Kimi). Auto-derive must list the
        // same-protocol fallback first even though its name sorts LATER, so the
        // derived chain stays homogeneous and avoids the cross-protocol request
        // conversion that an OpenAI-compat endpoint rejects (-10003).
        let cfg = cfg_with_fallback(
            None,
            vec![
                ("primary", provider_config_with_protocol("anthropic")),
                ("a_openai", provider_config_with_protocol("openai")),
                ("z_anthropic", provider_config_with_protocol("anthropic")),
            ],
        );
        let built = built_map(&["a_openai", "z_anthropic"]);
        let (nodes, auto_derived) = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        assert!(auto_derived, "no configured chain → auto-derived");
        // Same-protocol (anthropic) first despite the later name, then the
        // cross-protocol one — protocol affinity beats the name-sort tiebreak.
        assert_eq!(fallback_names(&nodes), vec!["z_anthropic", "a_openai"]);
    }

    #[test]
    fn assemble_fallbacks_honors_explicit_chain_without_auto_deriving() {
        // A valid explicit chain is used verbatim; auto-derivation must NOT
        // append other enabled providers (`aux`) behind operator intent.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["fb".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("fb", mock_provider_config()),
                ("aux", mock_provider_config()),
            ],
        );
        let built = built_map(&["fb", "aux"]);
        let (nodes, auto_derived) = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        assert!(
            !auto_derived,
            "explicit chain → honored static, not auto-derived"
        );
        assert_eq!(fallback_names(&nodes), vec!["fb"]);
    }

    #[test]
    fn failover_chain_wraps_primary_even_without_fallback_section() {
        // No `[fallback_provider]` — the primary is still wrapped so it gains
        // model-level fallback, transient retry, and a circuit breaker.
        let cfg = cfg_with_fallback(None, vec![("primary", mock_provider_config())]);
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
        // Only the primary is configured → no per-hint overrides.
        assert!(chain.agent_overrides.is_empty());
    }

    #[test]
    fn failover_chain_builds_with_configured_chain() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["fb".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("fb", mock_provider_config()),
            ],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
        // `fb` is a non-primary configured provider → it gets a pinned override;
        // the primary never gets one (hinting it == no hint).
        assert!(chain.agent_overrides.contains_key("fb"));
        assert!(!chain.agent_overrides.contains_key("primary"));
        assert_eq!(chain.agent_overrides["fb"].name(), "failover");
    }

    #[test]
    fn failover_chain_skips_self_reference_and_unknown_providers() {
        // `Primary` (self, case-insensitive) and `ghost` (absent from
        // `[providers]`) are both skipped with a warn log; the build still
        // succeeds and yields a failover handle.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["Primary".to_string(), "ghost".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![("primary", mock_provider_config())],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
    }

    #[test]
    fn agent_overrides_cover_every_non_primary_provider() {
        // Three providers configured; `aux1` / `aux2` are not even in the
        // fallback chain, but per-hint overrides still cover them so any agent
        // can pin any configured provider.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["aux1".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("aux1", mock_provider_config()),
                ("aux2", mock_provider_config()),
            ],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.agent_overrides.len(), 2);
        assert!(chain.agent_overrides.contains_key("aux1"));
        assert!(chain.agent_overrides.contains_key("aux2"));
        assert!(!chain.agent_overrides.contains_key("primary"));
    }
}
