use super::*;

#[test]
fn test_schema_migration() {
    let store = SecurityStore::in_memory().unwrap();
    assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn test_device_crud() {
    let store = SecurityStore::in_memory().unwrap();

    store
        .upsert_device(&DeviceUpsertData {
            device_id: "dev-1",
            device_name: "Test Device",
            device_type: Some("macos"),
            public_key: &[1u8; 32],
            fingerprint: "abc123",
            role: "operator",
            scopes: &["*".to_string()],
        })
        .unwrap();

    let device = store.get_device("dev-1").unwrap().unwrap();
    assert_eq!(device.device_name, "Test Device");
    assert_eq!(device.fingerprint, "abc123");

    let devices = store.list_devices().unwrap();
    assert_eq!(devices.len(), 1);

    assert!(store.revoke_device("dev-1").unwrap());
    assert!(!store.is_device_approved("dev-1").unwrap());
}

#[test]
fn test_token_crud() {
    let store = SecurityStore::in_memory().unwrap();

    store
        .upsert_device(&DeviceUpsertData {
            device_id: "dev-1",
            device_name: "Test",
            device_type: None,
            public_key: &[1u8; 32],
            fingerprint: "fp",
            role: "operator",
            scopes: &[],
        })
        .unwrap();

    let expires = current_timestamp_ms() + 3600000;

    store
        .insert_token(
            "tok-1",
            "dev-1",
            "hash123",
            "operator",
            &["*".to_string()],
            expires,
        )
        .unwrap();

    let token = store.get_token_by_hash("hash123").unwrap().unwrap();
    assert_eq!(token.device_id, "dev-1");

    assert!(store.revoke_token("tok-1").unwrap());
    assert!(store.get_token_by_hash("hash123").unwrap().is_none());
}

#[test]
fn test_pairing_request_crud() {
    let store = SecurityStore::in_memory().unwrap();

    let expires = current_timestamp_ms() + 300000;

    store
        .insert_pairing_request(&PairingRequestData {
            request_id: "req-1",
            code: "A3B7K9M2",
            pairing_type: "device",
            device_name: Some("iPhone"),
            device_type: Some("ios"),
            public_key: Some(&[1u8; 32]),
            channel: None,
            sender_id: None,
            remote_addr: Some("192.168.1.1"),
            expires_at: expires,
        })
        .unwrap();

    let req = store.get_pairing_request("A3B7K9M2").unwrap().unwrap();
    assert_eq!(req.device_name, Some("iPhone".to_string()));
    assert!(req.remaining_secs() > 0);

    assert!(store.delete_pairing_request("A3B7K9M2").unwrap());
    assert!(store.get_pairing_request("A3B7K9M2").unwrap().is_none());
}

#[test]
fn test_channel_dm_policy_crud() {
    let store = SecurityStore::in_memory().unwrap();

    assert!(store.get_channel_dm_policy("telegram").unwrap().is_none());

    store
        .set_channel_dm_policy("telegram", "pairing", None)
        .unwrap();
    let (policy, allowlist) = store.get_channel_dm_policy("telegram").unwrap().unwrap();
    assert_eq!(policy, "pairing");
    assert!(allowlist.is_none());

    store
        .set_channel_dm_policy("discord", "allowlist", Some("[\"user1\",\"user2\"]"))
        .unwrap();
    let (policy, allowlist) = store.get_channel_dm_policy("discord").unwrap().unwrap();
    assert_eq!(policy, "allowlist");
    assert_eq!(allowlist.unwrap(), "[\"user1\",\"user2\"]");

    store
        .set_channel_dm_policy("telegram", "open", None)
        .unwrap();
    let (policy, _) = store.get_channel_dm_policy("telegram").unwrap().unwrap();
    assert_eq!(policy, "open");
}

#[test]
fn test_sender_approval() {
    let store = SecurityStore::in_memory().unwrap();

    store.approve_sender("telegram", "user123").unwrap();
    assert!(store.is_sender_approved("telegram", "user123").unwrap());

    let senders = store.list_senders("telegram").unwrap();
    assert_eq!(senders.len(), 1);

    assert!(store.revoke_sender("telegram", "user123").unwrap());
    assert!(!store.is_sender_approved("telegram", "user123").unwrap());
}
