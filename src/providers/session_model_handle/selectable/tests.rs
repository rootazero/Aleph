use super::*;
use crate::config::ProviderConfig;

fn configured() -> Config {
    let mut config = Config::default();
    config.providers.clear();
    let mut provider = ProviderConfig::test_config("tenant-model, other-model");
    provider.base_url = Some("https://tenant.invalid/v1".into());
    provider
        .model_defs
        .push(serde_json::from_value(serde_json::json!({"id": "definition-only"})).unwrap());
    config.providers.insert("tenant".into(), provider);
    config
}

fn roster(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).into()).collect()
}

#[test]
fn configured_custom_and_model_defs_are_authoritative() {
    let config = configured();
    let snapshot = SelectableModelsSnapshot::from_config(
        &config,
        roster(&["tenant"]),
        &roster(&["tenant"]),
        |_| true,
    );
    for id in ["tenant-model", "other-model", "definition-only"] {
        let expected = SessionModelPref {
            provider: Some("tenant".into()),
            model: id.into(),
        };
        assert_eq!(snapshot.resolve(None, id), Ok(expected.clone()));
        assert_eq!(snapshot.resolve(Some("tenant"), id), Ok(expected.clone()));
        assert_eq!(
            snapshot.resolve(None, &format!("tenant/{id}")),
            Ok(expected)
        );
    }
    assert!(matches!(
        snapshot.resolve(None, "not-configured"),
        Err(ModelSelectionError::Unknown { .. })
    ));
    assert!(matches!(
        snapshot.resolve(Some("tenant"), "not-configured"),
        Err(ModelSelectionError::Unsupported { .. })
    ));
    assert!(matches!(
        snapshot.resolve(Some("absent"), "tenant-model"),
        Err(ModelSelectionError::Unknown { .. })
    ));
}

#[test]
fn shared_model_is_ambiguous_even_when_one_provider_is_unavailable() {
    let mut config = configured();
    let mut other = ProviderConfig::test_config("tenant-model");
    other.enabled = false;
    config.providers.insert("other".into(), other);
    let snapshot = SelectableModelsSnapshot::from_config(
        &config,
        roster(&["tenant", "other"]),
        &roster(&["tenant"]),
        |_| true,
    );
    assert_eq!(
        snapshot.resolve(None, "tenant-model"),
        Err(ModelSelectionError::Ambiguous {
            model: "tenant-model".into(),
            providers: vec!["other".into(), "tenant".into()],
        })
    );
    assert!(snapshot.resolve(Some("tenant"), "tenant-model").is_ok());
}

#[test]
fn authentication_runtime_and_routing_are_independent_fail_closed_gates() {
    let config = configured();
    let names = roster(&["tenant"]);
    let no_auth = SelectableModelsSnapshot::from_config(&config, names.clone(), &names, |_| false);
    assert_eq!(
        no_auth.resolve(None, "tenant-model"),
        Err(ModelSelectionError::Unauthenticated {
            provider: "tenant".into()
        })
    );
    for (routable, available) in [
        (BTreeSet::new(), names.clone()),
        (names.clone(), BTreeSet::new()),
    ] {
        let snapshot =
            SelectableModelsSnapshot::from_config(&config, routable, &available, |_| true);
        assert!(matches!(
            snapshot.resolve(Some("tenant"), "tenant-model"),
            Err(ModelSelectionError::Unsupported { .. })
        ));
    }
    let mut disabled = config;
    disabled.providers.get_mut("tenant").unwrap().enabled = false;
    let snapshot =
        SelectableModelsSnapshot::from_config(&disabled, names.clone(), &names, |_| true);
    assert!(matches!(
        snapshot.resolve(None, "tenant-model"),
        Err(ModelSelectionError::Unsupported { .. })
    ));
}

#[test]
fn reload_projection_replaces_roster_without_mutating_held_snapshot() {
    let mut config = configured();
    let names = roster(&["tenant"]);
    let previous = std::sync::Arc::new(SelectableModelsSnapshot::from_config(
        &config,
        names.clone(),
        &names,
        |_| true,
    ));
    let pc = config.providers.get_mut("tenant").unwrap();
    pc.models = vec!["new-model".into()];
    pc.model_defs.clear();
    config.providers.insert(
        "new-provider".into(),
        ProviderConfig::test_config("new-provider-model"),
    );
    let refreshed = SelectableModelsSnapshot::from_config(
        &config,
        previous.routable.clone(),
        &roster(&["tenant", "new-provider"]),
        |_| true,
    );
    assert!(previous.resolve(None, "tenant-model").is_ok());
    assert!(matches!(
        previous.resolve(None, "new-model"),
        Err(ModelSelectionError::Unknown { .. })
    ));
    assert!(refreshed.resolve(None, "new-model").is_ok());
    assert!(matches!(
        refreshed.resolve(Some("tenant"), "tenant-model"),
        Err(ModelSelectionError::Unsupported { .. })
    ));
    assert!(matches!(
        refreshed.resolve(Some("new-provider"), "new-provider-model"),
        Err(ModelSelectionError::Unsupported { .. })
    ));
    config.providers.remove("tenant");
    let removed = SelectableModelsSnapshot::from_config(&config, names.clone(), &names, |_| true);
    assert!(matches!(
        removed.resolve(Some("tenant"), "new-model"),
        Err(ModelSelectionError::Unknown { .. })
    ));
}

#[tokio::test]
async fn explicit_test_publication_and_missing_publication_use_the_real_resolver() {
    use crate::providers::session_model_handle::{
        resolve_selectable_model, with_test_selectable_models,
    };
    let names = roster(&["tenant"]);
    let snapshot =
        SelectableModelsSnapshot::from_config(&configured(), names.clone(), &names, |_| true);
    with_test_selectable_models(Some(snapshot), async {
        assert!(resolve_selectable_model(None, "tenant-model").is_ok());
        with_test_selectable_models(None, async {
            assert_eq!(
                resolve_selectable_model(None, "tenant-model"),
                Err(ModelSelectionError::Unavailable)
            );
        })
        .await;
        assert!(resolve_selectable_model(None, "tenant-model").is_ok());
    })
    .await;
}
