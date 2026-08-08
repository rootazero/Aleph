use super::*;

#[test]
fn test_schema_migration() {
    let store = SecurityStore::in_memory().unwrap();
    assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
}

/// The half a fresh store can never exercise: an EXISTING `security_audit_log`
/// has to gain `actor_user`, or every `ScopedContentRead` the trace handlers
/// emit dies in the drain with `no such column` — logged once at `error!` and
/// otherwise silent, which is precisely the failure mode an audit trail must
/// not have.
#[test]
fn v15_adds_the_audit_actor_column_to_a_pre_v15_store() {
    let store = SecurityStore::in_memory().unwrap();
    {
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        // The historical shape, inlined on purpose: it is what the constant
        // USED to say, so it cannot be sourced from the constant.
        conn.execute_batch(
            "DROP TABLE security_audit_log;
             CREATE TABLE security_audit_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                 event_type TEXT NOT NULL,
                 severity TEXT NOT NULL,
                 source_ip TEXT,
                 session_id TEXT,
                 detail TEXT NOT NULL
             );",
        )
        .unwrap();
        assert!(
            conn.prepare("SELECT actor_user FROM security_audit_log LIMIT 0")
                .is_err(),
            "the fixture must start WITHOUT the column or this test proves nothing"
        );
    }
    store.set_schema_version(14).unwrap();

    store.migrate().unwrap();

    store
        .insert_audit_entry(&crate::security::audit::AuditEntry::scoped_content_read(
            "u-bob",
            None,
            "trace.get",
        ))
        .unwrap();
    let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
    let actor: Option<String> = conn
        .query_row("SELECT actor_user FROM security_audit_log", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(actor.as_deref(), Some("u-bob"));
}

/// …and the other half: a store created from scratch already has the column
/// (v7 builds the table from the current `AUDIT_LOG_SCHEMA`), so the v15 arm
/// must probe rather than trust the version gate. Without the probe this is a
/// `duplicate column name` on every FIRST boot — the version gate alone is
/// what v14's two ALTERs could rely on and this one cannot.
#[test]
fn v15_is_idempotent_when_the_column_is_already_there() {
    let store = SecurityStore::in_memory().unwrap();
    store.set_schema_version(14).unwrap();
    store
        .migrate()
        .expect("re-running v15 over an existing column must not be an error");
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
