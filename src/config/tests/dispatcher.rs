//! Dispatcher configuration tests

use super::super::*;

#[test]
fn test_dispatcher_config_default() {
    let config = DispatcherConfigToml::default();

    assert!(config.enabled);
    assert!(config.l3_enabled);
    assert_eq!(config.l3_timeout_ms, 5000);
    assert!((config.confirmation_threshold - 0.7).abs() < 0.01);
    assert_eq!(config.confirmation_timeout_ms, 30000);
    assert!(config.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_validation_valid() {
    let config = DispatcherConfigToml::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_dispatcher_config_validation_threshold_negative() {
    let config = DispatcherConfigToml {
        confirmation_threshold: -0.5,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("confirmation_threshold must be >= 0.0"));
}

#[test]
fn test_dispatcher_config_validation_l3_timeout_zero() {
    let config = DispatcherConfigToml {
        l3_timeout_ms: 0,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("l3_timeout_ms must be > 0"));
}

#[test]
fn test_dispatcher_config_validation_confirmation_timeout_zero() {
    let config = DispatcherConfigToml {
        confirmation_timeout_ms: 0,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("confirmation_timeout_ms must be > 0"));
}

#[test]
fn test_dispatcher_config_threshold_above_one() {
    // Threshold > 1.0 is valid but disables confirmation
    let config = DispatcherConfigToml {
        confirmation_threshold: 1.5,
        ..Default::default()
    };
    // Should be valid (just warns)
    assert!(config.validate().is_ok());
}

#[test]
fn test_dispatcher_config_to_internal() {
    let toml_config = DispatcherConfigToml {
        enabled: true,
        l3_enabled: false,
        l3_timeout_ms: 3000,
        confirmation_threshold: 0.8,
        confirmation_timeout_ms: 20000,
        confirmation_enabled: true,
        agent: Default::default(),
    };

    let internal = toml_config.to_dispatcher_config();

    assert!(internal.enabled);
    assert!(!internal.l3_enabled);
    assert_eq!(internal.l3_timeout_ms, 3000);
    assert!((internal.l3_confidence_threshold - 0.8).abs() < 0.01);
    assert!(internal.confirmation.enabled);
    assert!((internal.confirmation.threshold - 0.8).abs() < 0.01);
    assert_eq!(internal.confirmation.timeout_ms, 20000);
}

#[test]
fn test_dispatcher_config_toml_parsing() {
    let toml_str = r#"
[dispatcher]
enabled = true
l3_enabled = false
l3_timeout_ms = 10000
confirmation_threshold = 0.5
confirmation_timeout_ms = 15000
confirmation_enabled = false
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");
    assert!(config.dispatcher.enabled);
    assert!(!config.dispatcher.l3_enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 10000);
    assert!((config.dispatcher.confirmation_threshold - 0.5).abs() < 0.01);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 15000);
    assert!(!config.dispatcher.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_toml_defaults_when_missing() {
    let toml_str = r#"
[general]
default_provider = "openai"
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // All dispatcher fields should have defaults
    assert!(config.dispatcher.enabled);
    assert!(config.dispatcher.l3_enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 5000);
    assert!((config.dispatcher.confirmation_threshold - 0.7).abs() < 0.01);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 30000);
    assert!(config.dispatcher.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_partial_toml() {
    let toml_str = r#"
[dispatcher]
l3_enabled = false
confirmation_threshold = 0.9
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // Specified values
    assert!(!config.dispatcher.l3_enabled);
    assert!((config.dispatcher.confirmation_threshold - 0.9).abs() < 0.01);

    // Defaults for unspecified
    assert!(config.dispatcher.enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 5000);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 30000);
    assert!(config.dispatcher.confirmation_enabled);
}
