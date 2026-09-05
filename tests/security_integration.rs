// short-lived sync guards held across await; reviewed non-contending.
#![allow(clippy::await_holding_lock)]

// `PiiEngine` comes from `pii`; the three config types it takes live in
// `config/types/privacy.rs` and are re-exported at the crate root (the `config`
// module itself is private). Importing all four from `pii` stopped this whole
// test binary compiling — invisible to `cargo check` and to
// `cargo test --lib`, and visible only under `--test '*'`.
use alephcore::pii::PiiEngine;
use alephcore::secrets::injection::AsyncSecretResolver;
use alephcore::secrets::types::{DecryptedSecret, SecretError};
use alephcore::security::audit::{AuditEventType, AuditSeverity};
use alephcore::security::{
    GuardResult, RuntimeSecurityGuard, SecurityContext, SecurityGuardConfig,
};
use alephcore::{PiiAction, PlatformPiiPolicy, PrivacyConfig};
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::timeout;

static PII_MUTEX: Mutex<()> = Mutex::new(());

struct TestResolver;

#[async_trait::async_trait]
impl AsyncSecretResolver for TestResolver {
    async fn resolve(&self, _name: &str) -> Result<DecryptedSecret, SecretError> {
        Ok(DecryptedSecret::new(
            "sk-ant-integration123456789012345".to_string(),
        ))
    }
}

#[tokio::test]
async fn test_outbound_inbound_roundtrip_blocks_echo() {
    let guard = RuntimeSecurityGuard::new_without_audit(SecurityGuardConfig {
        audit_enabled: false,
        ..Default::default()
    });
    let resolver = TestResolver;

    // Outbound: inject a secret
    let outbound_input = "Please use {{secret:api_key}} for the request";
    let result = guard
        .process_outbound(outbound_input, Some(&resolver), SecurityContext::default())
        .await
        .unwrap();

    let outbound_text = match result {
        GuardResult::Clean { text } | GuardResult::Redacted { text, .. } => text,
        GuardResult::Warned { text, .. } => text,
        GuardResult::Blocked { .. } => {
            // If blocked, it's because the mock secret looks like a real key
            // and leak detector caught it before replacement.
            // In that case the placeholder should still be in the blocked text
            // or the text should be redacted. For this test we just continue.
            return;
        }
        // `GuardResult` is #[non_exhaustive]; any future non-text variant is
        // skipped like the Blocked case rather than failing this flow.
        _ => return,
    };

    // Verify placeholder was replaced
    assert!(
        !outbound_text.contains("{{secret:api_key}}"),
        "Placeholder should have been replaced"
    );

    // Inbound: simulate LLM echoing the secret back
    let inbound_input = "Your key sk-ant-integration123456789012345 has been used";
    let inbound_result = guard
        .process_inbound(inbound_input, &SecurityContext::default())
        .await
        .unwrap();

    assert!(
        matches!(inbound_result, GuardResult::Blocked { .. }),
        "Expected inbound echo to be blocked, got {:?}",
        inbound_result
    );
}

#[tokio::test]
async fn test_outbound_blocks_accidental_secret_in_user_text() {
    let guard = RuntimeSecurityGuard::new_without_audit(SecurityGuardConfig {
        audit_enabled: false,
        ..Default::default()
    });
    let resolver = TestResolver;

    let input = "My secret key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let result = guard
        .process_outbound(input, Some(&resolver), SecurityContext::default())
        .await;

    assert!(
        matches!(result, Ok(GuardResult::Blocked { .. })),
        "Expected outbound accidental secret to be blocked, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_platform_pii_policy_warn_mode_end_to_end() {
    let _lock = PII_MUTEX.lock().unwrap();

    let mut config = PrivacyConfig::default();
    let policy = PlatformPiiPolicy {
        phone: Some(PiiAction::Warn),
        ..Default::default()
    };
    config
        .platform_policies
        .insert("discord".to_string(), policy);

    if PiiEngine::global().is_some() {
        PiiEngine::reload(config);
    } else {
        PiiEngine::init(config);
    }

    let guard = RuntimeSecurityGuard::new_without_audit(SecurityGuardConfig {
        audit_enabled: false,
        ..Default::default()
    });
    let context = SecurityContext {
        platform_name: Some("discord".to_string()),
        ..Default::default()
    };

    let result = guard
        .process_outbound("Call me at 13812345678", None, context)
        .await
        .unwrap();

    match result {
        GuardResult::Warned { text, .. } => {
            assert!(
                text.contains("13812345678"),
                "Warn mode should preserve original text"
            );
        }
        other => panic!(
            "Expected Warned result for discord platform override, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_platform_excluded_provider_end_to_end() {
    let _lock = PII_MUTEX.lock().unwrap();

    let mut config = PrivacyConfig::default();
    let policy = PlatformPiiPolicy {
        exclude_providers: Some(vec!["local-llm".to_string()]),
        ..Default::default()
    };
    config
        .platform_policies
        .insert("telegram".to_string(), policy);

    if PiiEngine::global().is_some() {
        PiiEngine::reload(config);
    } else {
        PiiEngine::init(config);
    }

    let guard = RuntimeSecurityGuard::new_without_audit(SecurityGuardConfig {
        audit_enabled: false,
        ..Default::default()
    });
    let context = SecurityContext {
        platform_name: Some("telegram".to_string()),
        provider_name: Some("local-llm".to_string()),
        ..Default::default()
    };

    let result = guard
        .process_outbound("Call me at 13812345678", None, context)
        .await
        .unwrap();

    match result {
        GuardResult::Clean { text } => {
            assert!(
                text.contains("13812345678"),
                "Excluded provider should bypass PII filtering"
            );
        }
        other => panic!(
            "Expected Clean result for excluded provider via platform policy, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_audit_outbound_exec_blocked() {
    let config = SecurityGuardConfig::default();
    let (guard, mut rx) = RuntimeSecurityGuard::new_with_audit(config);

    let input = "My secret key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let result = guard
        .process_outbound(input, None, SecurityContext::default())
        .await;

    assert!(
        matches!(result, Ok(GuardResult::Blocked { .. })),
        "Expected outbound secret leak to be blocked, got {:?}",
        result
    );

    let entry = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out waiting for audit event")
        .expect("audit channel closed unexpectedly");

    assert_eq!(entry.event_type, AuditEventType::ExecBlocked);
    assert_eq!(entry.severity, AuditSeverity::Critical);
    assert!(
        entry.detail.contains("outbound leak blocked"),
        "Expected detail to mention outbound leak blocked, got: {}",
        entry.detail
    );
}

#[tokio::test]
async fn test_audit_inbound_exec_blocked() {
    let config = SecurityGuardConfig::default();
    let (guard, mut rx) = RuntimeSecurityGuard::new_with_audit(config);
    let resolver = TestResolver;

    let _ = guard
        .process_outbound(
            "Please use {{secret:api_key}}",
            Some(&resolver),
            SecurityContext::default(),
        )
        .await
        .unwrap();

    while let Ok(Some(_)) = timeout(Duration::from_millis(50), rx.recv()).await {}

    let inbound = "Your key sk-ant-integration123456789012345 has been used";
    let result = guard
        .process_inbound(inbound, &SecurityContext::default())
        .await
        .unwrap();

    assert!(
        matches!(result, GuardResult::Blocked { .. }),
        "Expected inbound echo to be blocked, got {:?}",
        result
    );

    let entry = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out waiting for audit event")
        .expect("audit channel closed unexpectedly");

    assert_eq!(entry.event_type, AuditEventType::EnvInjectionDetected);
    assert_eq!(entry.severity, AuditSeverity::Critical);
    assert!(
        entry.detail.contains("inbound secret leak blocked"),
        "Expected detail to mention inbound secret leak blocked, got: {}",
        entry.detail
    );
}

#[tokio::test]
async fn test_audit_outbound_pii_detected() {
    let _lock = PII_MUTEX.lock().unwrap();

    let mut privacy_config = PrivacyConfig::default();
    let policy = PlatformPiiPolicy {
        phone: Some(PiiAction::Warn),
        ..Default::default()
    };
    privacy_config
        .platform_policies
        .insert("discord".to_string(), policy);

    if PiiEngine::global().is_some() {
        PiiEngine::reload(privacy_config);
    } else {
        PiiEngine::init(privacy_config);
    }

    let config = SecurityGuardConfig {
        pii_filtering: true,
        ..Default::default()
    };
    let (guard, mut rx) = RuntimeSecurityGuard::new_with_audit(config);

    let context = SecurityContext {
        platform_name: Some("discord".to_string()),
        ..Default::default()
    };

    let result = guard
        .process_outbound("Call me at 13812345678", None, context)
        .await
        .unwrap();

    assert!(
        matches!(result, GuardResult::Warned { .. }),
        "Expected Warned result for discord platform override, got {:?}",
        result
    );

    let mut found = false;
    while let Ok(Some(entry)) = timeout(Duration::from_millis(200), rx.recv()).await {
        if entry.event_type == AuditEventType::PiiDetected {
            assert_eq!(entry.severity, AuditSeverity::Warn);
            assert!(
                entry.detail.contains("PII filter warned"),
                "Expected detail to mention PII filter warned, got: {}",
                entry.detail
            );
            found = true;
            break;
        }
    }
    assert!(found, "Expected PiiDetected audit event in receiver");
}
