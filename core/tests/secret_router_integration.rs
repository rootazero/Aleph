//! Integration tests for the secret routing system.
//!
//! Tests the full flow: Config -> SecretRouter -> Provider -> Resolution.

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::secrets::{
    render_with_secrets, AsyncSecretResolver, EntryMetadata, LocalVaultProvider, SecretMapping,
    SecretProvider, SecretRouter, SecretVault, Sensitivity,
};
use tempfile::TempDir;

// =============================================================================
// Helpers
// =============================================================================

/// Create an isolated vault in a temp dir, pre-populated with the given entries.
fn create_vault(dir: &TempDir, entries: &[(&str, &str)]) -> SecretVault {
    let path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(path, "integration-test-key").unwrap();
    for (name, value) in entries {
        vault
            .set(name, value, EntryMetadata::default())
            .unwrap();
    }
    vault
}

/// Build a `SecretRouter` backed by a single `LocalVaultProvider`.
fn build_router(
    vault: SecretVault,
    mappings: Vec<(&str, SecretMapping)>,
) -> SecretRouter {
    let provider: Arc<dyn SecretProvider> = Arc::new(LocalVaultProvider::new(vault));
    let mut providers = HashMap::new();
    providers.insert("local".to_string(), provider);

    let mapping_map: HashMap<String, SecretMapping> = mappings
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

    SecretRouter::new(mapping_map, providers, "local".to_string())
}

// =============================================================================
// 1. Full resolution flow — mapped Standard + High
// =============================================================================

#[tokio::test]
async fn test_full_resolution_flow() {
    let dir = TempDir::new().unwrap();
    let vault = create_vault(&dir, &[
        ("anthropic_key", "sk-ant-integration-123"),
        ("bank_password", "super-secret-pw"),
    ]);

    let router = build_router(
        vault,
        vec![
            (
                "ANTHROPIC_KEY",
                SecretMapping {
                    provider: "local".into(),
                    reference: Some("anthropic_key".into()),
                    sensitivity: Sensitivity::Standard,
                    ttl: 3600,
                },
            ),
            (
                "BANK_PASSWORD",
                SecretMapping {
                    provider: "local".into(),
                    reference: Some("bank_password".into()),
                    sensitivity: Sensitivity::High,
                    ttl: 0,
                },
            ),
        ],
    );

    // Resolve the Standard-sensitivity secret
    let standard = router.resolve("ANTHROPIC_KEY").await.unwrap();
    assert_eq!(standard.expose(), "sk-ant-integration-123");

    // Resolve the High-sensitivity secret
    let high = router.resolve("BANK_PASSWORD").await.unwrap();
    assert_eq!(high.expose(), "super-secret-pw");
}

// =============================================================================
// 2. Unmapped secret fallback — resolves via default provider
// =============================================================================

#[tokio::test]
async fn test_unmapped_secret_fallback() {
    let dir = TempDir::new().unwrap();
    // The vault contains a secret called "SOME_TOKEN" directly.
    // No explicit mapping is provided, so the router should fall back
    // to the default provider using "SOME_TOKEN" as the reference.
    let vault = create_vault(&dir, &[("SOME_TOKEN", "tok-fallback-789")]);

    let router = build_router(vault, vec![]);

    let secret = router.resolve("SOME_TOKEN").await.unwrap();
    assert_eq!(secret.expose(), "tok-fallback-789");
}

// =============================================================================
// 3. render_with_secrets integration — placeholder replacement
// =============================================================================

#[tokio::test]
async fn test_render_with_secrets_integration() {
    let dir = TempDir::new().unwrap();
    let vault = create_vault(&dir, &[("my_api_key", "sk-rendered-456")]);

    let router = build_router(
        vault,
        vec![(
            "my_api_key",
            SecretMapping {
                provider: "local".into(),
                reference: Some("my_api_key".into()),
                sensitivity: Sensitivity::Standard,
                ttl: 3600,
            },
        )],
    );

    let input = "Authorization: Bearer {{secret:my_api_key}}";
    let (rendered, injected) = render_with_secrets(input, &router).await.unwrap();

    assert_eq!(rendered, "Authorization: Bearer sk-rendered-456");
    assert_eq!(injected.len(), 1);
    assert_eq!(injected[0].name, "my_api_key");
    // Injected record must never contain the plaintext value
    assert_eq!(injected[0].value_len, "sk-rendered-456".len());
}

// =============================================================================
// 4. Non-existent secret returns error
// =============================================================================

#[tokio::test]
async fn test_nonexistent_secret_returns_error() {
    let dir = TempDir::new().unwrap();
    // Empty vault — nothing stored
    let vault = create_vault(&dir, &[]);

    let router = build_router(vault, vec![]);

    let err = router.resolve("DOES_NOT_EXIST").await.unwrap_err();
    assert!(
        matches!(err, alephcore::secrets::SecretError::NotFound(ref name) if name == "DOES_NOT_EXIST"),
        "expected NotFound(\"DOES_NOT_EXIST\"), got: {err:?}"
    );
}
