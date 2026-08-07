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
            user_id: None,
        })
        .unwrap();

    let devices = store.list_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_name, "Test Device");
    assert_eq!(devices[0].fingerprint, "abc123");

    let by_fp = store.get_device_by_fingerprint("abc123").unwrap().unwrap();
    assert_eq!(by_fp.device_id, "dev-1");

    assert!(store.revoke_device("dev-1").unwrap());
    assert!(!store.is_device_approved("dev-1").unwrap());
}
