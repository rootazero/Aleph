// Token-Vault Integration Tests
//
// 19 tests across 4 sections verifying the end-to-end behavior of
// SharedTokenManager + SecretVault + SecretsCrypto working together.

use std::sync::Arc;

use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::secrets::vault::SecretVault;

/// Helper: create an in-memory store + manager with a unique vault path inside `dir`.
fn make_manager(dir: &tempfile::TempDir) -> (Arc<SecurityStore>, SharedTokenManager) {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("secrets.vault");
    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    (store, mgr)
}

/// Helper: create a manager reusing an existing store (simulates restart).
fn restart_manager(
    store: Arc<SecurityStore>,
    vault_path: &std::path::Path,
) -> SharedTokenManager {
    SharedTokenManager::new(store, vault_path)
}

// ============================================================================
// Section 1: End-to-End Lifecycle (5 tests)
// ============================================================================

#[test]
fn full_lifecycle_startup_configure_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let token_file = dir.path().join("token");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    // First boot: generate token, store 3 provider keys
    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    let token = mgr.generate_token().unwrap();
    std::fs::write(&token_file, &token).unwrap();

    mgr.store_secret("anthropic", "sk-ant-key1").unwrap();
    mgr.store_secret("openai", "sk-openai-key1").unwrap();
    mgr.store_secret("google", "sk-google-key1").unwrap();
    drop(mgr);

    // Second boot: recreate from same store + vault path, load token from file
    let mgr2 = restart_manager(store, &vault_path);
    let loaded = mgr2.try_load_token_from_file(&token_file);
    assert!(loaded.is_some(), "Should load token from file");

    // Verify all 3 keys readable
    assert_eq!(mgr2.get_secret("anthropic").unwrap().unwrap().expose(), "sk-ant-key1");
    assert_eq!(mgr2.get_secret("openai").unwrap().unwrap().expose(), "sk-openai-key1");
    assert_eq!(mgr2.get_secret("google").unwrap().unwrap().expose(), "sk-google-key1");
}

#[test]
fn lifecycle_add_update_delete_across_restarts() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let token_file = dir.path().join("token");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    // Boot1: store "anthropic" key
    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    let token = mgr.generate_token().unwrap();
    std::fs::write(&token_file, &token).unwrap();
    mgr.store_secret("anthropic", "sk-v1").unwrap();
    drop(mgr);

    // Boot2: update "anthropic" to new value, add "openai"
    let mgr = restart_manager(store.clone(), &vault_path);
    mgr.try_load_token_from_file(&token_file);
    mgr.store_secret("anthropic", "sk-v2").unwrap();
    mgr.store_secret("openai", "sk-openai").unwrap();
    assert_eq!(mgr.get_secret("anthropic").unwrap().unwrap().expose(), "sk-v2");
    drop(mgr);

    // Boot3: delete "anthropic"
    let mgr = restart_manager(store.clone(), &vault_path);
    mgr.try_load_token_from_file(&token_file);
    assert!(mgr.delete_secret("anthropic").unwrap());
    drop(mgr);

    // Boot4: verify "anthropic" gone, "openai" still there
    let mgr = restart_manager(store, &vault_path);
    mgr.try_load_token_from_file(&token_file);
    assert!(mgr.get_secret("anthropic").unwrap().is_none());
    assert_eq!(mgr.get_secret("openai").unwrap().unwrap().expose(), "sk-openai");
}

#[test]
fn lifecycle_reset_token_then_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let token_file = dir.path().join("token");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    // Store secrets, reset token
    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    let old_token = mgr.generate_token().unwrap();
    mgr.store_secret("key_a", "val_a").unwrap();
    mgr.store_secret("key_b", "val_b").unwrap();

    let new_token = mgr.reset_token().unwrap();
    assert_ne!(old_token, new_token);
    std::fs::write(&token_file, &new_token).unwrap();
    drop(mgr);

    // Restart with new token
    let mgr2 = restart_manager(store.clone(), &vault_path);
    let loaded = mgr2.try_load_token_from_file(&token_file);
    assert!(loaded.is_some());

    // All secrets readable
    assert_eq!(mgr2.get_secret("key_a").unwrap().unwrap().expose(), "val_a");
    assert_eq!(mgr2.get_secret("key_b").unwrap().unwrap().expose(), "val_b");

    // Old token no longer validates
    assert!(!mgr2.validate(&old_token).unwrap());
}

#[test]
fn lifecycle_multiple_resets() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store; // keep store alive

    let _token = mgr.generate_token().unwrap();
    mgr.store_secret("epoch0_key", "epoch0_val").unwrap();

    // Reset 3 times, adding a new secret each time
    let _t1 = mgr.reset_token().unwrap();
    mgr.store_secret("epoch1_key", "epoch1_val").unwrap();

    let _t2 = mgr.reset_token().unwrap();
    mgr.store_secret("epoch2_key", "epoch2_val").unwrap();

    let _t3 = mgr.reset_token().unwrap();
    mgr.store_secret("epoch3_key", "epoch3_val").unwrap();

    // All secrets from all epochs readable
    assert_eq!(mgr.get_secret("epoch0_key").unwrap().unwrap().expose(), "epoch0_val");
    assert_eq!(mgr.get_secret("epoch1_key").unwrap().unwrap().expose(), "epoch1_val");
    assert_eq!(mgr.get_secret("epoch2_key").unwrap().unwrap().expose(), "epoch2_val");
    assert_eq!(mgr.get_secret("epoch3_key").unwrap().unwrap().expose(), "epoch3_val");

    let names = mgr.list_secret_names().unwrap();
    assert_eq!(names.len(), 4);
}

#[test]
fn lifecycle_reset_preserves_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    let mgr = SharedTokenManager::new(store, &vault_path);
    let _token = mgr.generate_token().unwrap();

    // Store a secret — this sets metadata.provider = name
    mgr.store_secret("anthropic", "sk-ant").unwrap();

    // Read created_at from the vault file directly
    let vault = SecretVault::open(&vault_path).unwrap();
    let entry_before = vault.get("anthropic").unwrap();
    let created_at_before = entry_before.created_at;
    let provider_before = entry_before.metadata.provider.clone();
    drop(vault);

    // Small delay so timestamps differ
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Reset token
    let _new_token = mgr.reset_token().unwrap();

    // Re-read vault to check metadata preservation
    let vault = SecretVault::open(&vault_path).unwrap();
    let entry_after = vault.get("anthropic").unwrap();

    // created_at should be preserved
    assert_eq!(entry_after.created_at, created_at_before);
    // updated_at should be >= created_at
    assert!(entry_after.updated_at >= created_at_before);
    // metadata.provider should be preserved
    assert_eq!(entry_after.metadata.provider, provider_before);
}

// ============================================================================
// Section 2: Concurrency Safety (4 tests)
// ============================================================================

#[test]
fn concurrent_reads() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    // Store 10 secrets
    for i in 0..10 {
        mgr.store_secret(&format!("key_{}", i), &format!("val_{}", i)).unwrap();
    }

    let mgr = Arc::new(mgr);
    let mut handles = Vec::new();

    // Spawn 20 threads each reading a key
    for t in 0..20 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            let key = format!("key_{}", t % 10);
            let expected = format!("val_{}", t % 10);
            let secret = mgr.get_secret(&key).unwrap().unwrap();
            assert_eq!(secret.expose(), expected);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn concurrent_writes() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    let mgr = Arc::new(mgr);
    let mut handles = Vec::new();

    // 10 threads, each storing a uniquely-named secret
    for i in 0..10 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            mgr.store_secret(&format!("writer_{}", i), &format!("value_{}", i)).unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify all 10 entries
    let mut names = mgr.list_secret_names().unwrap();
    names.sort();
    assert_eq!(names.len(), 10);

    for i in 0..10 {
        let s = mgr.get_secret(&format!("writer_{}", i)).unwrap().unwrap();
        assert_eq!(s.expose(), format!("value_{}", i));
    }
}

#[test]
fn concurrent_read_write_mixed() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    // Pre-store 5 secrets
    for i in 0..5 {
        mgr.store_secret(&format!("pre_{}", i), &format!("pre_val_{}", i)).unwrap();
    }

    let mgr = Arc::new(mgr);
    let mut handles = Vec::new();

    // 10 readers reading existing keys
    for t in 0..10 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            let key = format!("pre_{}", t % 5);
            let expected = format!("pre_val_{}", t % 5);
            let secret = mgr.get_secret(&key).unwrap().unwrap();
            assert_eq!(secret.expose(), expected);
        }));
    }

    // 5 writers writing new keys
    for i in 0..5 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            mgr.store_secret(&format!("new_{}", i), &format!("new_val_{}", i)).unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify all 10 keys readable
    let names = mgr.list_secret_names().unwrap();
    assert_eq!(names.len(), 10);
}

#[test]
fn concurrent_reset_during_reads() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    // Store 5 secrets
    for i in 0..5 {
        mgr.store_secret(&format!("s_{}", i), &format!("v_{}", i)).unwrap();
    }

    let mgr = Arc::new(mgr);
    let mut handles = Vec::new();

    // 10 reader threads
    for t in 0..10 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            // Read a few times in a loop — some may hit the reset window
            for _ in 0..5 {
                let key = format!("s_{}", t % 5);
                // During reset, reads might temporarily fail; that's okay
                let _ = mgr.get_secret(&key);
            }
        }));
    }

    // 1 resetter thread
    {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            let _new_token = mgr.reset_token().unwrap();
        }));
    }

    // No thread should panic
    for h in handles {
        h.join().expect("Thread panicked during concurrent reset");
    }

    // After reset: new token is set and all secrets are still readable
    let current = mgr.get_current_token();
    assert!(current.is_some(), "Should have a token after reset");

    for i in 0..5 {
        let s = mgr.get_secret(&format!("s_{}", i)).unwrap().unwrap();
        assert_eq!(s.expose(), format!("v_{}", i));
    }
}

// ============================================================================
// Section 3: Security Properties (6 tests)
// ============================================================================

#[test]
fn different_tokens_produce_different_ciphertext() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_a = dir.path().join("a.vault");
    let vault_b = dir.path().join("b.vault");

    let store_a = Arc::new(SecurityStore::in_memory().unwrap());
    let store_b = Arc::new(SecurityStore::in_memory().unwrap());

    let mgr_a = SharedTokenManager::new(store_a, &vault_a);
    let mgr_b = SharedTokenManager::new(store_b, &vault_b);

    let _token_a = mgr_a.generate_token().unwrap();
    let _token_b = mgr_b.generate_token().unwrap();

    // Store same secret name + value
    mgr_a.store_secret("shared", "same_value").unwrap();
    mgr_b.store_secret("shared", "same_value").unwrap();

    // Read raw vault entries
    let va = SecretVault::open(&vault_a).unwrap();
    let vb = SecretVault::open(&vault_b).unwrap();

    let ea = va.get("shared").unwrap();
    let eb = vb.get("shared").unwrap();

    // Ciphertext should differ (different tokens = different encryption keys)
    assert_ne!(ea.ciphertext, eb.ciphertext);
}

#[test]
fn cross_token_isolation() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("shared.vault");

    let store_a = Arc::new(SecurityStore::in_memory().unwrap());
    let mgr_a = SharedTokenManager::new(store_a, &vault_path);
    let _token_a = mgr_a.generate_token().unwrap();
    mgr_a.store_secret("private_key", "secret_data").unwrap();
    drop(mgr_a);

    // Manager B with different store (different token), same vault file
    let store_b = Arc::new(SecurityStore::in_memory().unwrap());
    let mgr_b = SharedTokenManager::new(store_b, &vault_path);
    let _token_b = mgr_b.generate_token().unwrap();

    // Decryption should fail (wrong key)
    let result = mgr_b.get_secret("private_key");
    assert!(result.is_err(), "Decryption with wrong token should fail");
}

#[test]
fn reset_changes_all_ciphertext() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    let mgr = SharedTokenManager::new(store, &vault_path);
    let _token = mgr.generate_token().unwrap();

    mgr.store_secret("key1", "val1").unwrap();
    mgr.store_secret("key2", "val2").unwrap();
    mgr.store_secret("key3", "val3").unwrap();

    // Read raw entries before reset
    let v_before = SecretVault::open(&vault_path).unwrap();
    let ct1_before = v_before.get("key1").unwrap().ciphertext.clone();
    let ct2_before = v_before.get("key2").unwrap().ciphertext.clone();
    let ct3_before = v_before.get("key3").unwrap().ciphertext.clone();
    let salt1_before = v_before.get("key1").unwrap().salt;
    let salt2_before = v_before.get("key2").unwrap().salt;
    let salt3_before = v_before.get("key3").unwrap().salt;
    drop(v_before);

    // Reset
    let _new_token = mgr.reset_token().unwrap();

    // Read raw entries after reset
    let v_after = SecretVault::open(&vault_path).unwrap();
    let ct1_after = v_after.get("key1").unwrap().ciphertext.clone();
    let ct2_after = v_after.get("key2").unwrap().ciphertext.clone();
    let ct3_after = v_after.get("key3").unwrap().ciphertext.clone();
    let salt1_after = v_after.get("key1").unwrap().salt;
    let salt2_after = v_after.get("key2").unwrap().salt;
    let salt3_after = v_after.get("key3").unwrap().salt;

    // All ciphertexts and salts should differ
    assert_ne!(ct1_before, ct1_after, "key1 ciphertext should change");
    assert_ne!(ct2_before, ct2_after, "key2 ciphertext should change");
    assert_ne!(ct3_before, ct3_after, "key3 ciphertext should change");
    assert_ne!(salt1_before, salt1_after, "key1 salt should change");
    assert_ne!(salt2_before, salt2_after, "key2 salt should change");
    assert_ne!(salt3_before, salt3_after, "key3 salt should change");
}

#[test]
fn tampered_vault_file_detected() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let token_file = dir.path().join("token");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    let token = mgr.generate_token().unwrap();
    std::fs::write(&token_file, &token).unwrap();
    mgr.store_secret("important", "data").unwrap();
    drop(mgr);

    // Tamper with vault file: read bytes, flip some, write back
    let mut bytes = std::fs::read(&vault_path).unwrap();
    // Flip bytes in the middle (where ciphertext likely lives)
    let mid = bytes.len() / 2;
    if mid + 4 < bytes.len() {
        for i in mid..mid + 4 {
            bytes[i] ^= 0xFF;
        }
    }
    std::fs::write(&vault_path, &bytes).unwrap();

    // Recreate manager, try to read — should either fail to deserialize vault
    // or fail to decrypt
    let mgr2 = restart_manager(store, &vault_path);
    let loaded = mgr2.try_load_token_from_file(&token_file);

    if loaded.is_some() {
        // Vault loaded (deserialization succeeded) but decryption should fail
        let result = mgr2.get_secret("important");
        // It should either error or return wrong data
        match result {
            Err(_) => {} // Expected: decryption error
            Ok(None) => {} // Vault may have failed to parse the entry
            Ok(Some(_)) => {
                panic!("Tampered ciphertext should never decrypt successfully with AES-GCM");
            }
        }
    }
    // If loaded is None, the vault file was too corrupted to parse — also acceptable
}

#[test]
fn empty_and_special_values() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    // Empty string
    mgr.store_secret("empty", "").unwrap();
    assert_eq!(mgr.get_secret("empty").unwrap().unwrap().expose(), "");

    // Unicode
    let unicode_val = "密钥🔑";
    mgr.store_secret("unicode", unicode_val).unwrap();
    assert_eq!(mgr.get_secret("unicode").unwrap().unwrap().expose(), unicode_val);

    // 10KB string
    let large = "x".repeat(10_240);
    mgr.store_secret("large", &large).unwrap();
    assert_eq!(mgr.get_secret("large").unwrap().unwrap().expose(), large);
}

#[test]
fn same_name_overwrite_encrypts_differently() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    let mgr = SharedTokenManager::new(store, &vault_path);
    let _token = mgr.generate_token().unwrap();

    // Store same name+value
    mgr.store_secret("anthropic", "key1").unwrap();

    let v1 = SecretVault::open(&vault_path).unwrap();
    let ct1 = v1.get("anthropic").unwrap().ciphertext.clone();
    let salt1 = v1.get("anthropic").unwrap().salt;
    drop(v1);

    // Store same name+value again
    mgr.store_secret("anthropic", "key1").unwrap();

    let v2 = SecretVault::open(&vault_path).unwrap();
    let ct2 = v2.get("anthropic").unwrap().ciphertext.clone();
    let salt2 = v2.get("anthropic").unwrap().salt;

    // Different random salt/nonce should produce different ciphertext
    assert_ne!(ct1, ct2, "Same value should encrypt differently each time");
    assert_ne!(salt1, salt2, "Salt should differ between encryptions");
}

// ============================================================================
// Section 4: Boundary & Error Recovery (4 tests)
// ============================================================================

#[test]
fn vault_file_missing_recreates_empty() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");
    let token_file = dir.path().join("token");
    let store = Arc::new(SecurityStore::in_memory().unwrap());

    // Store a secret
    let mgr = SharedTokenManager::new(store.clone(), &vault_path);
    let token = mgr.generate_token().unwrap();
    std::fs::write(&token_file, &token).unwrap();
    mgr.store_secret("will_be_lost", "value").unwrap();
    drop(mgr);

    // Delete vault file
    std::fs::remove_file(&vault_path).unwrap();

    // Recreate manager — vault should be empty but functional
    let mgr2 = restart_manager(store, &vault_path);
    mgr2.try_load_token_from_file(&token_file);

    assert!(mgr2.get_secret("will_be_lost").unwrap().is_none());
    let names = mgr2.list_secret_names().unwrap();
    assert!(names.is_empty());

    // Can store new secrets
    mgr2.store_secret("new_key", "new_val").unwrap();
    assert_eq!(mgr2.get_secret("new_key").unwrap().unwrap().expose(), "new_val");
}

#[test]
fn many_entries_performance() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    let start = std::time::Instant::now();

    // Store 100 secrets
    for i in 0..100 {
        mgr.store_secret(&format!("perf_key_{:03}", i), &format!("perf_val_{:03}", i))
            .unwrap();
    }

    // Reset token (re-encrypts all 100)
    let _new_token = mgr.reset_token().unwrap();

    // Verify all 100 readable
    for i in 0..100 {
        let s = mgr
            .get_secret(&format!("perf_key_{:03}", i))
            .unwrap()
            .unwrap();
        assert_eq!(s.expose(), format!("perf_val_{:03}", i));
    }

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs() < 2,
        "100-entry store+reset+verify took {:?}, expected < 2s",
        elapsed
    );
}

#[test]
fn secret_names_with_special_characters() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    let names_values = [
        ("a/b", "slash"),
        ("a.b", "dot"),
        ("a-b", "dash"),
        ("a_b", "underscore"),
        ("a b", "space"),
        ("中文key", "chinese"),
        ("emoji🔑", "emoji"),
    ];

    // Store all
    for (name, val) in &names_values {
        mgr.store_secret(name, val).unwrap();
    }

    // Get all
    for (name, val) in &names_values {
        let s = mgr.get_secret(name).unwrap().unwrap();
        assert_eq!(s.expose(), *val, "Failed for name: {}", name);
    }

    // List all
    let mut listed = mgr.list_secret_names().unwrap();
    listed.sort();
    let mut expected: Vec<&str> = names_values.iter().map(|(n, _)| *n).collect();
    expected.sort();
    assert_eq!(listed, expected);
}

#[test]
fn double_delete_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, mgr) = make_manager(&dir);
    let _ = store;
    let _token = mgr.generate_token().unwrap();

    mgr.store_secret("x", "value").unwrap();

    // First delete returns true
    assert!(mgr.delete_secret("x").unwrap());
    // Second delete returns false, no error
    assert!(!mgr.delete_secret("x").unwrap());

    // Still works
    assert!(mgr.get_secret("x").unwrap().is_none());
}
