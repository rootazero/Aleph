//! `Config::validate` rule coverage.
//!
//! Ported from `tests/features/config/validation.feature`. This module's
//! parent claimed these tests had "been migrated to BDD cucumber tests", but
//! no test target ever compiled `tests/steps/`, so the rules below spent eight
//! months with no executable coverage at all.
//!
//! The port is not literal, because the rules moved underneath the snapshot:
//!
//! - Scenarios whose subject no longer exists are dropped —
//!   `memory.max_context_items` and `memory.graph_decay` are both gone from
//!   `MemoryConfig`. The latter's successor, `memory.memory_decay`, is covered
//!   in its place.
//! - The one scenario whose verdict *inverted* — a provider with no `api_key`
//!   was expected to fail — is kept, flipped, as a characterization of the
//!   current deliberate behaviour.

use super::super::*;
use crate::sync_primitives::Arc;

/// Valid baseline: a single `openai` provider, selected as the default.
fn config_with_openai() -> Config {
    let mut config = Config::default();
    config
        .providers
        .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
    config.general.default_provider = Some("openai".to_string());
    config
}

/// [`config_with_openai`] with the provider tweaked before validation.
fn config_with_openai_tweaked(mutate: impl FnOnce(&mut ProviderConfig)) -> Config {
    let mut config = config_with_openai();
    mutate(
        config
            .providers
            .get_mut("openai")
            .expect("config_with_openai inserts it"),
    );
    config
}

/// [`config_with_openai`] plus one command rule carrying `regex`.
fn config_with_rule(regex: &str) -> Config {
    let mut config = config_with_openai();
    config
        .rules
        .push(RoutingRuleConfig::command(regex, "openai", None));
    config
}

fn expect_invalid(config: &Config, needle: &str) {
    let err = config
        .validate()
        .expect_err("expected validation to reject this config")
        .to_string();
    assert!(
        err.contains(needle),
        "error {err:?} does not mention {needle:?}"
    );
}

// ═══ Providers ═══

#[test]
fn test_valid_provider_passes() {
    assert!(config_with_openai().validate().is_ok());
}

#[test]
fn test_unknown_default_provider_fails() {
    let mut config = Config::default();
    config.general.default_provider = Some("nonexistent".to_string());

    expect_invalid(&config, "not found in providers");
}

#[test]
fn test_missing_api_key_is_not_a_validation_error() {
    // `api_key` is a runtime-only field injected from the encrypted vault at
    // startup, so config-load time cannot see it and must not reject a
    // provider that omits it. The Gherkin snapshot expected the opposite.
    let config = config_with_openai_tweaked(|provider| provider.api_key = None);

    assert!(config.validate().is_ok());
}

#[test]
fn test_temperature_above_protocol_range_fails() {
    // No explicit protocol means "openai", whose accepted range is 0.0..=2.0.
    let config = config_with_openai_tweaked(|provider| provider.temperature = Some(3.0));

    expect_invalid(&config, "temperature must be between");
}

#[test]
fn test_zero_timeout_fails() {
    let config = config_with_openai_tweaked(|provider| provider.timeout_seconds = 0);

    expect_invalid(&config, "timeout must be greater than 0");
}

#[test]
fn test_ollama_without_api_key_passes() {
    let config = config_with_openai_tweaked(|provider| {
        provider.protocol = Some("ollama".to_string());
        provider.api_key = None;
    });

    assert!(config.validate().is_ok());
}

// ═══ Routing rules ═══

#[test]
fn test_valid_regex_patterns_pass() {
    for pattern in [".*", "^/code", r"\d+", "[a-zA-Z]+", "^test$", "hello|world"] {
        assert!(
            config_with_rule(pattern).validate().is_ok(),
            "pattern {pattern:?} should be accepted"
        );
    }
}

/// Keyword rules are retired. Two halves, and either alone is a false green:
/// the daemon must still **boot** (an operator's existing file keeps parsing —
/// `Config` has no `deny_unknown_fields` and these are declared fields), and
/// the operator must be **told which rule** stopped working, or the cut is
/// fail-dead rather than fail-closed.
///
/// Note the second rule carries a `system_prompt`. The warning this replaced
/// fired only when one was *missing*, which read as "a keyword rule with a
/// prompt is fine" — backwards, since the prompt is the part that reaches
/// nothing. This rule would have been silent under the old predicate.
#[test]
fn a_retired_keyword_rule_is_named_and_still_boots() {
    let mut config = config_with_openai();
    config
        .rules
        .push(RoutingRuleConfig::command("^/draw", "openai", None));
    config.rules.push(RoutingRuleConfig {
        regex: "translate to English".to_string(),
        system_prompt: Some("Translate the target language to English".to_string()),
        ..Default::default()
    });

    assert!(
        config.validate().is_ok(),
        "a retired keyword rule must not stop the daemon booting"
    );
    assert_eq!(
        config.retired_keyword_rules(),
        vec![(2, "translate to English")],
        "the command rule must not be named, and the keyword rule must be"
    );
}

/// The label is not the authority. A rule declared `rule_type = "command"`
/// whose regex lacks `^/` is skipped by `register_custom_commands` all the
/// same, so it belongs in the retirement report — a warning keyed on
/// `rule_type` would miss exactly this rule.
#[test]
fn a_rule_labelled_command_without_the_prefix_is_still_retired() {
    let mut config = config_with_openai();
    config
        .rules
        .push(RoutingRuleConfig::command("translate", "openai", None));

    assert_eq!(config.retired_keyword_rules(), vec![(1, "translate")]);
}

/// Bounded `tracing_subscriber` layer that captures the formatted message
/// of every event passing through. Local to this test module so it cannot
/// leak into other test modules or production code; uses `set_default` (a
/// thread-local subscriber), not `init` (a process-global one), so the
/// guard returned by `set_default` scopes capture to this test only.
#[derive(Clone, Default)]
struct CapturedWarnMessages(Arc<std::sync::Mutex<Vec<String>>>);

struct WarnMessageVisitor(String);

impl tracing::field::Visit for WarnMessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedWarnMessages {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = WarnMessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(visitor.0);
    }
}

/// Retired rule boots: the previous `test_rule_referencing_unknown_provider_fails`
/// asserted a hard rejection on the unknown-provider path. The brief requires
/// preserving that original fixture — `command(".*", "nonexistent", None)` —
/// while changing only the test semantics from reject to retired warning +
/// boot, so the provider is left as `"nonexistent"` and the regex as `".*"`.
/// The rule's regex `.*` does not start with `^/`, so the retired-detection
/// arm classifies it as a **keyword rule** and fires BEFORE the
/// unknown-provider arm in `validate_rules`; the daemon therefore still
/// boots, the rule is listed in `retired_keyword_rules()` at position 1, and
/// a captured `tracing` warn matches the exact retired-rule text. The
/// baseline `config_with_openai()` keeps `default_provider = "openai"` so
/// the surrounding config stays valid and the retired arm is reached.
#[test]
fn a_retired_keyword_rule_with_unknown_provider_boots_and_warns_exact_text() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let mut config = config_with_openai();
    config
        .rules
        .push(RoutingRuleConfig::command(".*", "nonexistent", None));

    let retired = config.retired_keyword_rules();
    assert_eq!(
        retired,
        vec![(1, ".*")],
        "rule at position 1 is a keyword rule (no `^/` prefix) and must be listed"
    );

    let captured = CapturedWarnMessages::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let _guard = subscriber.set_default();

    assert!(
        config.validate().is_ok(),
        "a retired keyword rule must not stop the daemon booting"
    );

    drop(_guard);
    let messages = captured.0.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let expected = "Keyword routing rules are retired and reach no code; this rule does nothing. \
                    Give it a `^/`-prefixed regex to make it a slash command, or delete it.";
    assert!(
        messages.iter().any(|m| m == expected),
        "exact retired-rule warning missing; got: {messages:?}"
    );
}

/// Retired rule boots: the previous `test_invalid_regex_patterns_fail` asserted
/// a hard `"invalid regex"` rejection that `Config::validate` never produced
/// — the regex bytes are not compiled during `validate`, so the four patterns
/// below are all classified as retired keyword rules (none starts with `^/`)
/// and they all BOOT. The test is renamed to describe what the fixture
/// actually is, asserts each pattern's position and regex appear in
/// `retired_keyword_rules()`, asserts the daemon still boots, and captures
/// the exact warning text via a scoped `tracing_subscriber` layer so a
/// regression that quietly drops the warn — or rewrites the message — is
/// visible.
#[test]
fn malformed_keyword_rules_boot_and_are_listed_as_retired() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let patterns = ["[invalid(", "(unclosed", "**", "[z-a]"];
    let mut config = config_with_openai();
    for p in patterns {
        config
            .rules
            .push(RoutingRuleConfig::command(p, "openai", None));
    }

    let retired = config.retired_keyword_rules();
    let expected: Vec<(usize, &str)> = patterns
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, *p))
        .collect();
    assert_eq!(
        retired, expected,
        "every malformed pattern is a keyword rule (no `^/` prefix) and must be listed at its 1-based position"
    );

    let captured = CapturedWarnMessages::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let _guard = subscriber.set_default();

    assert!(
        config.validate().is_ok(),
        "a retired keyword rule must not stop the daemon booting"
    );

    drop(_guard);
    let messages = captured.0.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let expected_text =
        "Keyword routing rules are retired and reach no code; this rule does nothing. \
                          Give it a `^/`-prefixed regex to make it a slash command, or delete it.";
    let matching = messages.iter().filter(|m| *m == expected_text).count();
    assert_eq!(
        matching,
        patterns.len(),
        "expected one exact retired-rule warning per retired rule ({}); got {} matches in {messages:?}",
        patterns.len(),
        matching
    );
}

// ═══ Memory ═══

#[test]
fn test_invalid_dreaming_window_start_fails() {
    let mut config = Config::default();
    config.memory.dreaming.window_start_local = "25:00".to_string();

    expect_invalid(&config, "window_start_local must be HH:MM");
}

#[test]
fn test_memory_decay_bounds_are_enforced() {
    // Successor of the feature file's `graph_decay.node_decay_per_day` scenario.
    let mut config = Config::default();
    config.memory.memory_decay.half_life_days = 0.0;
    expect_invalid(&config, "half_life_days must be greater than 0");

    let mut config = Config::default();
    config.memory.memory_decay.min_strength = 1.5;
    expect_invalid(&config, "min_strength must be between 0.0 and 1.0");
}
