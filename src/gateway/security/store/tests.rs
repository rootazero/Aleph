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

/// The v16 twin, and the one migration in this file with **rows already in the
/// table**. A fresh store cannot exercise it: `IDENTITY_SCHEMA` now carries
/// `principal`, so every from-scratch install gets the column for free and the
/// path that matters — an installation that has been recording since before it
/// existed — is exactly the one an isolated fixture never reaches.
///
/// Two things have to hold, and the second is the one worth the test: the
/// column arrives, AND the rows that were already there survive with NULL.
/// A `principal`-less row is what every pre-v16 record is, and `NULL` is the
/// value the preimage treats as "contributes no bytes" — so this is also what
/// keeps those rows verifying under the signatures they were written with.
#[test]
fn v16_adds_the_ledger_principal_column_to_a_store_that_already_has_records() {
    let store = SecurityStore::in_memory().unwrap();
    {
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        // The historical shape, inlined on purpose — it is what the constant
        // USED to say, so it cannot be sourced from the constant.
        conn.execute_batch(
            "DROP TABLE agent_ledger;
             CREATE TABLE agent_ledger (
                 agent_id   TEXT NOT NULL,
                 seq        INTEGER NOT NULL,
                 prev_hash  BLOB,
                 hash       BLOB NOT NULL,
                 signature  BLOB NOT NULL,
                 signer_fp  TEXT NOT NULL,
                 action     TEXT NOT NULL,
                 target     TEXT NOT NULL,
                 outcome    TEXT NOT NULL,
                 args_fp    TEXT,
                 detail     TEXT NOT NULL,
                 at_ms      INTEGER NOT NULL,
                 PRIMARY KEY (agent_id, seq)
             );
             INSERT INTO agent_ledger
                 (agent_id, seq, prev_hash, hash, signature, signer_fp, action, target,
                  outcome, args_fp, detail, at_ms)
             VALUES ('main', 1, NULL, x'00', x'00', 'fp', 'tool_call', 'bash',
                     'ok', NULL, 'pre-existing row', 1);",
        )
        .unwrap();
        assert!(
            conn.prepare("SELECT principal FROM agent_ledger LIMIT 0")
                .is_err(),
            "the fixture must start WITHOUT the column or this test proves nothing"
        );
    }
    store.set_schema_version(15).unwrap();

    store.migrate().unwrap();

    let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
    let (detail, principal): (String, Option<String>) = conn
        .query_row(
            "SELECT detail, principal FROM agent_ledger WHERE agent_id='main' AND seq=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("the row written before the column existed must still be there");
    assert_eq!(detail, "pre-existing row");
    assert_eq!(
        principal, None,
        "an existing row must read as naming nobody, which is what its signature covers"
    );
}

/// …and the other half, same shape as v15's: a store created from scratch
/// already has the column (an earlier arm ran `IDENTITY_SCHEMA`, which now
/// carries it), so the v16 arm must probe rather than trust the version gate.
/// Without the probe this is `duplicate column name` on every FIRST boot.
#[test]
fn v16_is_idempotent_when_the_ledger_column_is_already_there() {
    let store = SecurityStore::in_memory().unwrap();
    store.set_schema_version(15).unwrap();
    store
        .migrate()
        .expect("re-running v16 over an existing column must not be an error");
    assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
}

/// Proves the v17 arm actually runs `migrate()` from a pre-v17 state and
/// leaves a real, usable table behind — not just that a store already at
/// `SCHEMA_VERSION` happens to have one (a fresh `in_memory()` store would
/// pass that trivially even if this arm's SQL were never reached).
#[test]
fn v17_creates_the_spend_ledger_table_from_a_pre_v17_store() {
    let store = SecurityStore::in_memory().unwrap();
    store.set_schema_version(16).unwrap();
    {
        // `in_memory()` already ran every arm, including v17 — drop what it
        // built so this store is genuinely pre-v17, not just labeled as one.
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DROP TABLE spend_ledger", []).unwrap();
        assert!(
            conn.prepare("SELECT 1 FROM spend_ledger LIMIT 0").is_err(),
            "the fixture must start WITHOUT the table or this test proves nothing"
        );
    }

    store.migrate().unwrap();

    {
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO spend_ledger \
             (principal_id, period_start, usd, unpriced_calls, partial_calls, updated_at) \
             VALUES ('u-bob', 1000, 1.5, 0, 0, 1000)",
            [],
        )
        .expect("the v17 arm must have created spend_ledger");
        let usd: f64 = conn
            .query_row(
                "SELECT usd FROM spend_ledger WHERE principal_id = 'u-bob' AND period_start = 1000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(usd, 1.5);
        // `conn`'s guard must drop before `get_schema_version()` below takes
        // the same non-reentrant lock, or this test deadlocks itself.
    }
    assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
}

/// A store that has been pairing devices since before revocation existed must
/// gain `revoked_at`, or the deactivation sweep's fourth leg dies with
/// `no such column` — best-effort, so it would be logged once and otherwise
/// silent while every burned ticket stayed redeemable.
#[test]
fn v18_adds_the_ticket_revocation_column_to_a_pre_v18_store() {
    let store = SecurityStore::in_memory().unwrap();
    {
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        // The historical shape, inlined on purpose (as v15's fixture is): it
        // is what the table used to be, so it cannot be sourced from the
        // constant that describes what it is now.
        conn.execute_batch(
            "DROP TABLE bootstrap_tickets;
             CREATE TABLE bootstrap_tickets (
                 code                    TEXT PRIMARY KEY,
                 created_at              INTEGER NOT NULL,
                 expires_at              INTEGER NOT NULL,
                 consumed_at             INTEGER,
                 consumed_by_device_id   TEXT,
                 user_id                 TEXT
             );",
        )
        .unwrap();
        assert!(
            conn.prepare("SELECT revoked_at FROM bootstrap_tickets LIMIT 0")
                .is_err(),
            "the fixture must start WITHOUT the column or this test proves nothing"
        );
    }
    store.set_schema_version(17).unwrap();

    store.migrate().unwrap();

    store
        .create_bootstrap_ticket("bt-legacy", 60_000, Some("u-alice"))
        .unwrap();
    assert_eq!(
        store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
        1,
        "the v18 arm must have created revoked_at"
    );
    assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
}

/// …and the re-entrant half: `migrate()` re-run over a store that already has
/// the column must not abort with `duplicate column name`. Without the probe
/// this test is red and it takes every later arm down with it.
#[test]
fn v18_is_idempotent_when_the_ticket_column_is_already_there() {
    let store = SecurityStore::in_memory().unwrap();
    store.set_schema_version(17).unwrap();
    store
        .migrate()
        .expect("re-running v18 over an existing column must not be an error");
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
