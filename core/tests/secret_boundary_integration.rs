//! End-to-end integration tests for the IronClaw secret boundary pipeline.
//!
//! Tests the full flow from placeholder extraction through vault resolution,
//! secret injection, leak detection, and EVM signing. Ensures that secrets
//! never leak across the trust boundary.

use std::hash::{Hash, Hasher};

use alephcore::secrets::{
    extract_secret_refs, render_with_secrets, EvmSigner, LeakDecision, LeakDetector, SecretError,
    SecretVault, SignIntent,
};
use alephcore::secrets::types::EntryMetadata;
use tempfile::TempDir;

/// Helper: create a vault with a stored secret.
fn vault_with_secret(dir: &TempDir, name: &str, value: &str) -> SecretVault {
    let path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(&path, "integration-master-key").unwrap();
    vault
        .set(name, value, EntryMetadata::default())
        .unwrap();
    vault
}

/// Helper: compute the SipHash-2-4 of a value (same algorithm as InjectedSecret).
fn siphash_value(value: &str) -> u64 {
    let mut hasher = siphasher::sip::SipHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

// ─── Test 1: Green Path ─────────────────────────────────────────────────────

/// Full pipeline: create vault -> store secret -> extract_secret_refs ->
/// render_with_secrets -> scan_inbound safe response -> Allow
#[tokio::test]
async fn test_green_path_placeholder_to_injection() {
    let dir = TempDir::new().unwrap();
    let api_key = "my-custom-safe-api-key-value-1234";
    let vault = vault_with_secret(&dir, "my_api_key", api_key);

    // Step 1: Template with placeholder
    let template = "Authorization: Bearer {{secret:my_api_key}}";

    // Step 2: Extract refs to confirm parsing
    let refs = extract_secret_refs(template).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "my_api_key");
    assert_eq!(refs[0].raw, "{{secret:my_api_key}}");

    // Step 3: Render via vault (SecretVault implements AsyncSecretResolver)
    let (rendered, injected) = render_with_secrets(template, &vault).await.unwrap();
    assert_eq!(
        rendered,
        format!("Authorization: Bearer {}", api_key)
    );
    assert_eq!(injected.len(), 1);
    assert_eq!(injected[0].name, "my_api_key");
    assert_eq!(injected[0].value_len, api_key.len());
    assert_eq!(injected[0].value_hash, siphash_value(api_key));

    // Step 4: Register injected secrets with the leak detector
    let mut detector = LeakDetector::new();
    detector.register_injected(&injected, &[api_key]);

    // Step 5: Scan a safe inbound response (no echo of the secret)
    let safe_response = "Request successful. Status 200 OK. Data returned.";
    let decision = detector.scan_inbound(safe_response);
    assert!(
        !decision.is_blocked(),
        "Safe response should be allowed, got: {:?}",
        decision
    );
    assert!(matches!(decision, LeakDecision::Allow));
}

// ─── Test 2: Red Path ───────────────────────────────────────────────────────

/// Full pipeline: inject secret -> response echoes it -> scan_inbound -> Block
/// with redacted content
#[tokio::test]
async fn test_red_path_response_echoes_injected_secret() {
    let dir = TempDir::new().unwrap();
    let secret_value = "super-secret-token-abcdefghij";
    let vault = vault_with_secret(&dir, "token", secret_value);

    // Render the template
    let template = "Token: {{secret:token}}";
    let (rendered, injected) = render_with_secrets(template, &vault).await.unwrap();
    assert_eq!(rendered, format!("Token: {}", secret_value));

    // Register with leak detector
    let mut detector = LeakDetector::new();
    detector.register_injected(&injected, &[secret_value]);

    // Simulate an inbound response that echoes the injected secret
    let malicious_response = format!(
        "I found your token: {}. Let me store it for you.",
        secret_value
    );
    let decision = detector.scan_inbound(&malicious_response);

    assert!(
        decision.is_blocked(),
        "Response echoing injected secret should be blocked"
    );

    if let LeakDecision::Block {
        reason,
        redacted_content,
    } = &decision
    {
        // The reason should indicate an injected secret was echoed (exact-value path)
        assert!(
            reason.contains("injected") && reason.contains("Inbound"),
            "Reason should mention both 'Inbound' and 'injected' for exact-value echo path, got: {}",
            reason
        );
        // The redacted content should NOT contain the original secret
        assert!(
            !redacted_content.contains(secret_value),
            "Redacted content must not contain the original secret"
        );
        // The redacted content SHOULD contain the redaction marker
        assert!(
            redacted_content.contains("REDACTED"),
            "Redacted content should contain REDACTED marker, got: {}",
            redacted_content
        );
    } else {
        panic!("Expected Block decision");
    }
}

// ─── Test 3: Outbound Leak Detection ────────────────────────────────────────

/// Agent tries to pass an API key pattern in tool params -> scan_outbound -> Block
#[test]
fn test_outbound_leak_detection_blocks_api_key_pattern() {
    let detector = LeakDetector::new();

    // Test various known API key patterns
    let test_cases = vec![
        (
            "Anthropic key",
            "Use this: sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
        ),
        (
            "OpenAI key",
            "Key: sk-abcdefghijklmnopqrstuvwxyz1234567890abcd",
        ),
        (
            "AWS key",
            "Credentials: AKIAIOSFODNN7EXAMPLE",
        ),
        (
            "GitHub token",
            "Auth: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl",
        ),
        (
            "Private key block",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...",
        ),
    ];

    for (label, content) in &test_cases {
        let decision = detector.scan_outbound(content);
        assert!(
            decision.is_blocked(),
            "{} should be blocked by outbound scan, got Allow",
            label
        );

        if let LeakDecision::Block {
            reason,
            redacted_content,
        } = &decision
        {
            assert!(
                !reason.is_empty(),
                "{}: reason should not be empty",
                label
            );
            assert!(
                redacted_content.contains("***LEAKED_REDACTED***"),
                "{}: should have LEAKED_REDACTED marker, got: {}",
                label,
                redacted_content
            );
        }
    }

    // Normal content should pass
    let safe_content = "Please search for 'rust async programming patterns'";
    let decision = detector.scan_outbound(safe_content);
    assert!(
        !decision.is_blocked(),
        "Normal content should be allowed through outbound scan"
    );
}

// ─── Test 4: EVM Signing Never Leaks Private Key ────────────────────────────

/// Sign with vault-stored key -> Debug/Display/signature hex never contain private key
#[test]
fn test_evm_signing_never_leaks_private_key() {
    let dir = TempDir::new().unwrap();

    // Hardhat default account #0 private key
    let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    let private_key_hex = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    let vault = vault_with_secret(&dir, "eth_wallet", private_key);
    let signer = EvmSigner::new(&vault);

    // Sign a personal message
    let intent = SignIntent::PersonalSign {
        message: b"Hello IronClaw Integration Test".to_vec(),
    };
    let result = signer.sign("eth_wallet", &intent).unwrap();

    // Verify signature is valid (64 bytes = r + s)
    assert_eq!(result.signature.len(), 64);
    assert!(result.recovery_id <= 1);

    // Verify signer address matches Hardhat account #0
    let expected_addr = hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
    assert_eq!(
        result.signer_address,
        expected_addr.as_slice(),
        "Signer address should match Hardhat account #0"
    );

    // CRITICAL: Debug output must never contain the private key
    let debug_str = format!("{:?}", result);
    assert!(
        !debug_str.contains(private_key_hex),
        "Debug output must never contain private key hex"
    );
    assert!(
        !debug_str.contains(private_key),
        "Debug output must never contain private key with 0x prefix"
    );

    // CRITICAL: Display output must never contain the private key
    let display_str = format!("{}", result);
    assert!(
        !display_str.contains(private_key_hex),
        "Display output must never contain private key hex"
    );

    // CRITICAL: The raw signature bytes hex-encoded must not contain the private key
    let sig_hex = hex::encode(&result.signature);
    assert!(
        !sig_hex.contains(private_key_hex),
        "Signature hex must not contain private key"
    );

    // Test with TypedData signing too
    let typed_intent = SignIntent::TypedData {
        domain_hash: [0xAA; 32],
        struct_hash: [0xBB; 32],
    };
    let typed_result = signer.sign("eth_wallet", &typed_intent).unwrap();
    let typed_debug = format!("{:?}", typed_result);
    assert!(
        !typed_debug.contains(private_key_hex),
        "TypedData signature debug must not contain private key"
    );
    let typed_display = format!("{}", typed_result);
    assert!(
        !typed_display.contains(private_key_hex),
        "TypedData signature display must not contain private key"
    );

    // Test with Transaction signing
    let tx_intent = SignIntent::Transaction {
        chain_id: 1,
        to: [0x42; 20],
        value: [0; 32],
        data: vec![],
        nonce: 0,
        gas_limit: 21000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
    };
    let tx_result = signer.sign("eth_wallet", &tx_intent).unwrap();
    let tx_debug = format!("{:?}", tx_result);
    assert!(
        !tx_debug.contains(private_key_hex),
        "Transaction signature debug must not contain private key"
    );
    let tx_display = format!("{}", tx_result);
    assert!(
        !tx_display.contains(private_key_hex),
        "Transaction signature display must not contain private key"
    );
}

// ─── Test 5: Vault Persistence ──────────────────────────────────────────────

/// Store secrets -> reopen vault -> render_with_secrets still works -> wrong master key fails
#[tokio::test]
async fn test_vault_persistence_across_operations() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("persist.vault");
    let master_key = "correct-master-key";
    let secret_value = "persistent-api-key-xyz-12345678";

    // Phase 1: Store secrets in the vault
    {
        let mut vault = SecretVault::open(&vault_path, master_key).unwrap();
        vault
            .set(
                "persistent_key",
                secret_value,
                EntryMetadata {
                    description: Some("Integration test key".into()),
                    provider: Some("test-provider".into()),
                },
            )
            .unwrap();
        vault
            .set(
                "secondary_key",
                "secondary-value-abcdefghij",
                EntryMetadata::default(),
            )
            .unwrap();
        assert_eq!(vault.len(), 2);
    }
    // vault is dropped here, file is persisted

    // Phase 2: Reopen vault with correct key -> render_with_secrets works
    {
        let vault = SecretVault::open(&vault_path, master_key).unwrap();

        // Verify secrets survived reopening
        assert_eq!(vault.len(), 2);
        assert!(vault.exists("persistent_key"));
        assert!(vault.exists("secondary_key"));

        // Verify decryption works
        let decrypted = vault.get("persistent_key").unwrap();
        assert_eq!(decrypted.expose(), secret_value);

        // Verify render_with_secrets works with the reopened vault
        let template = "Key: {{secret:persistent_key}}";
        let (rendered, injected) = render_with_secrets(template, &vault).await.unwrap();
        assert_eq!(rendered, format!("Key: {}", secret_value));
        assert_eq!(injected.len(), 1);
        assert_eq!(injected[0].name, "persistent_key");
        assert_eq!(injected[0].value_len, secret_value.len());

        // Verify metadata survived
        let list = vault.list();
        let (_, meta) = list
            .iter()
            .find(|(n, _)| n == "persistent_key")
            .expect("persistent_key should be in the list");
        assert_eq!(meta.description.as_deref(), Some("Integration test key"));
        assert_eq!(meta.provider.as_deref(), Some("test-provider"));
    }

    // Phase 3: Wrong master key -> open succeeds but decryption fails
    {
        let vault = SecretVault::open(&vault_path, "wrong-master-key").unwrap();

        // The vault opens (deserialization works) but decryption should fail
        assert_eq!(vault.len(), 2); // entries are still there (encrypted)

        let result = vault.get("persistent_key");
        assert!(
            result.is_err(),
            "Decrypting with wrong master key should fail"
        );
        assert!(
            matches!(result, Err(SecretError::DecryptionFailed)),
            "Error should be DecryptionFailed, got: {:?}",
            result
        );

        // render_with_secrets should also fail since it calls resolve -> get
        let template = "Key: {{secret:persistent_key}}";
        let render_result = render_with_secrets(template, &vault).await;
        assert!(
            render_result.is_err(),
            "render_with_secrets with wrong master key should fail"
        );
    }

    // Phase 4: Correct key still works after wrong key attempt
    {
        let vault = SecretVault::open(&vault_path, master_key).unwrap();
        let decrypted = vault.get("persistent_key").unwrap();
        assert_eq!(
            decrypted.expose(),
            secret_value,
            "Correct key should still work after a failed attempt with wrong key"
        );
    }
}
