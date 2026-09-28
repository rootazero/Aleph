//! Whether a run's provider asks the server to clear old tool results — read
//! through the decorator stack production puts in front of an `HttpProvider`,
//! because the local passes stand down on what the *run's* provider answers.

use super::HttpProvider;
use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::adapter::ProtocolAdapter;
use crate::providers::protocols::AnthropicProtocol;
use crate::providers::think_level_provider::ThinkLevelProvider;
use crate::providers::{
    AiProvider, FailoverConfig, FailoverHealth, FailoverProvider, MeteringProvider,
    ModelOverrideProvider, StaticDefault,
};
use crate::sync_primitives::Arc;
use std::collections::HashMap;

fn anthropic_provider(
    adapter: Arc<dyn ProtocolAdapter>,
    base_url: Option<&str>,
    enabled: bool,
) -> Arc<dyn AiProvider> {
    let mut config = ProviderConfig::test_config("claude-sonnet-4-6");
    config.base_url = base_url.map(str::to_string);
    config.server_context_editing.enabled = enabled;
    Arc::new(HttpProvider::new("claude".into(), config, adapter).expect("provider"))
}

fn anthropic() -> Arc<dyn ProtocolAdapter> {
    Arc::new(AnthropicProtocol::new(reqwest::Client::new()))
}

/// Failover → Metering → ThinkLevel → ModelOverride → `inner`.
fn production_stack(inner: Arc<dyn AiProvider>) -> Arc<dyn AiProvider> {
    let pinned: Arc<dyn AiProvider> =
        Arc::new(ModelOverrideProvider::new(inner, "claude-opus-4-8"));
    let think: Arc<dyn AiProvider> = Arc::new(ThinkLevelProvider::new(pinned, ThinkLevel::High));
    let metered: Arc<dyn AiProvider> = Arc::new(MeteringProvider::new(think, None, "main"));
    Arc::new(FailoverProvider::new(
        Arc::new(StaticDefault::new(metered)),
        Vec::new(),
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    ))
}

#[test]
fn the_answer_survives_every_wrapper_in_front_of_the_provider() {
    let on = production_stack(anthropic_provider(anthropic(), None, true));
    assert!(on.clears_tool_results_server_side());

    let off = production_stack(anthropic_provider(anthropic(), None, false));
    assert!(!off.clears_tool_results_server_side());
}

/// Enabled where the request cannot carry it is not "on": the answer is the
/// wire's, so the local passes keep running.
#[test]
fn enabled_on_a_host_without_the_feature_does_not_stand_the_passes_down() {
    let bedrock = anthropic_provider(
        anthropic(),
        Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
        true,
    );
    assert!(!bedrock.clears_tool_results_server_side());
}

/// A YAML protocol that `extends: anthropic` builds the Anthropic request, so
/// it answers as Anthropic — the shape of the `wire_family` regression.
#[test]
fn a_yaml_protocol_answers_as_the_protocol_it_extends() {
    use crate::providers::protocols::definition::ProtocolDefinition;
    use crate::providers::protocols::{ConfigurableProtocol, ProtocolRegistry};
    ProtocolRegistry::global().register_builtin();
    let definition = ProtocolDefinition {
        name: "my-proxy".into(),
        extends: Some("anthropic".into()),
        base_url: None,
        differences: None,
        custom: None,
    };
    let adapter: Arc<dyn ProtocolAdapter> =
        Arc::new(ConfigurableProtocol::new(definition, reqwest::Client::new()).expect("protocol"));
    assert!(anthropic_provider(adapter, None, true).clears_tool_results_server_side());
}

/// The predicate behind the construction-time warning: enabled but not
/// honoured on another host or on the OAuth path, quiet when honoured or not
/// enabled. The `tracing::warn!` line itself is NOT covered here — a WARN
/// capture through a scoped subscriber proved flaky under the parallel test
/// runner (the event was missed on one of two runs), and a flaky guard is
/// worse than a named gap.
#[test]
fn the_unhonoured_predicate_follows_host_and_auth() {
    let provider = |base_url: Option<&str>, key: Option<&str>, enabled: bool| {
        let mut c = ProviderConfig::test_config("claude-sonnet-4-6");
        c.base_url = base_url.map(str::to_string);
        if let Some(key) = key {
            c.api_key = Some(key.to_string());
        }
        c.server_context_editing.enabled = enabled;
        HttpProvider::new("claude".into(), c, anthropic()).expect("provider")
    };
    assert!(!provider(None, None, true).server_context_editing_unhonoured());
    assert!(provider(
        Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
        None,
        true
    )
    .server_context_editing_unhonoured());
    assert!(
        provider(None, Some("sk-ant-oat01-test-token"), true).server_context_editing_unhonoured()
    );
    assert!(!provider(None, None, false).server_context_editing_unhonoured());
}
