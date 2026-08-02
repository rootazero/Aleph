//! Chain verification and portable export, exercised the way the threat model
//! describes them.
//!
//! These live in `tests/` on purpose rather than beside the module:
//!
//! * The tamper cases open **their own** `rusqlite` connection to the same
//!   database file. That is the adversary the design names — someone with write
//!   access to `security.db` — rather than the module reaching into its own
//!   `pub(crate)` handle.
//! * The export verifier's entire contract is "checks out with nothing but the
//!   document". A test that can only touch the public API is the honest way to
//!   assert that; anything reachable only from inside the crate would not
//!   prove it.

use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::identity::{
    export_chain, verify_export, AgentKeystore, AgentLedger, ChainExport, ChainFault, ExportError,
    LedgerAction, LedgerOutcome, NewRecord,
};
use alephcore::sync_primitives::Arc;
use tempfile::TempDir;

struct Fixture {
    ledger: AgentLedger,
    db: std::path::PathBuf,
    _dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("security.db");
        let store = Arc::new(SecurityStore::open(&db).unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store.clone(),
            dir.path().join("t.vault"),
        ));
        vault.generate_token().unwrap();
        let keys = Arc::new(AgentKeystore::new(store, vault));
        Self {
            ledger: AgentLedger::new(keys),
            db,
            _dir: dir,
        }
    }

    fn append(&self, agent: &str, target: &str) {
        self.ledger
            .append(&NewRecord {
                agent_id: agent.to_string(),
                action: LedgerAction::ToolCall,
                target: target.to_string(),
                outcome: LedgerOutcome::Ok,
                args_fp: Some("fp".into()),
                detail: format!("{target}: did a thing"),
            })
            .unwrap();
    }

    /// Someone with write access to the database, and nothing else.
    fn tamper(&self, sql: &str) {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute(sql, []).unwrap();
    }

    fn active_fingerprint(&self, agent: &str) -> String {
        self.ledger
            .keys()
            .identity(agent)
            .unwrap()
            .unwrap()
            .active_fingerprint
    }
}

// ── the identity row is one mutable row ─────────────────────────────────────

#[test]
fn deleting_the_identity_row_does_not_remove_the_chain_from_the_verdict() {
    // Driving verification off the identity table meant deleting one row
    // produced a clean "all chains OK" that had simply stopped looking at this
    // one — the structural blindness the anchor closes for a truncated tail,
    // moved one table over.
    let f = Fixture::new();
    f.append("main", "a");
    f.append("trader", "b");
    f.tamper("DELETE FROM agent_identities WHERE agent_id='main'");

    assert_eq!(f.ledger.orphan_chains().unwrap(), vec!["main".to_string()]);

    let reports = f.ledger.verify_all().unwrap();
    let main = reports.iter().find(|r| r.agent_id == "main").expect(
        "a chain with records must be verified even with no identity row — \
         that deletion is the attack, not a reason to stop looking",
    );
    assert!(!main.ok);
    assert!(main.faults.contains(&ChainFault::IdentityMissing));
    assert!(reports.iter().any(|r| r.agent_id == "trader" && r.ok));
}

#[test]
fn a_key_the_chain_never_introduced_is_detected() {
    // The self-healing form of the same attack: delete the identity row, let
    // the agent act once more, and the keystore mints a fresh key and continues
    // the chain under it. Every link valid, every signature valid, the new key
    // genuinely this agent's — the only thing wrong is that the chain never
    // says where it came from.
    let f = Fixture::new();
    f.append("main", "before");
    let original = f.active_fingerprint("main");
    f.tamper("DELETE FROM agent_identities WHERE agent_id='main'");
    f.append("main", "after");

    let report = f.ledger.verify("main").unwrap();
    assert!(!report.ok, "a silent re-mint must not verify clean");
    let undeclared: Vec<_> = report
        .faults
        .iter()
        .filter_map(|fault| match fault {
            ChainFault::UndeclaredSigner { fingerprint, .. } => Some(fingerprint.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(undeclared.len(), 1, "{:?}", report.faults);
    assert_ne!(undeclared[0], original, "the new key is the undeclared one");
    // Ownership alone says nothing here: the key really is main's.
    assert!(!report
        .faults
        .iter()
        .any(|fault| matches!(fault, ChainFault::ForeignSigner { .. })));
}

#[test]
fn a_recorded_rotation_declares_its_key_whatever_order_it_lands_in() {
    // Records are enqueued asynchronously, so a call issued just before a
    // rotation can be appended AFTER it and be signed by the incoming key.
    // Declaration is therefore set membership, not adjacency — an adjacency
    // rule would report that race as tampering.
    let f = Fixture::new();
    f.append("main", "before");
    let old = f.active_fingerprint("main");
    let new = f.ledger.keys().rotate("main").unwrap().active_fingerprint;
    f.append("main", "raced"); // already signed by the new key…
    f.ledger
        .append(&NewRecord::identity_rotated("main", &new, Some(&old)))
        .unwrap(); // …and the rotation record lands after it.

    let report = f.ledger.verify("main").unwrap();
    assert!(report.ok, "{:?}", report.faults);
}

// ── portable export ─────────────────────────────────────────────────────────

#[test]
fn an_exported_chain_verifies_off_box() {
    let f = Fixture::new();
    for i in 0..4 {
        f.append("main", &format!("t{i}"));
    }
    let json = serde_json::to_string(&export_chain(&f.ledger, "main").unwrap()).unwrap();
    // From here on, nothing but the document.
    let parsed: ChainExport = serde_json::from_str(&json).unwrap();

    let report = verify_export(&parsed, &[]).unwrap();
    assert!(report.ok, "{:?}", report.faults);
    assert_eq!(report.records, 5, "4 calls plus the opening record");
    assert_eq!(report.first_seq, 1);
    assert_eq!(report.root_pinned, None, "no pin supplied");
}

#[test]
fn a_written_file_verifies_after_a_round_trip_through_disk() {
    // The shape `aleph-server identity export --out` writes and
    // `identity verify --input` reads. Pretty-printed, because that is what
    // the CLI emits and whitespace must not matter.
    let f = Fixture::new();
    f.append("main", "t");
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("chain.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&export_chain(&f.ledger, "main").unwrap()).unwrap(),
    )
    .unwrap();

    let doc: ChainExport = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let root = doc.records[0].signer_fp.clone();
    assert!(verify_export(&doc, &[root]).unwrap().ok);
}

#[test]
fn the_private_key_never_leaves() {
    let f = Fixture::new();
    f.append("main", "bash");
    let json = serde_json::to_string(&export_chain(&f.ledger, "main").unwrap()).unwrap();
    assert!(!json.contains("agent-key:"), "no vault entry name");
    assert!(json.contains("public_key"), "public halves only");
}

#[test]
fn editing_an_exported_row_is_detected() {
    let f = Fixture::new();
    f.append("main", "bash");
    let mut doc = export_chain(&f.ledger, "main").unwrap();
    doc.records[1].target = "harmless".into();

    let report = verify_export(&doc, &[]).unwrap();
    assert!(!report.ok);
    assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 2 }));
}

#[test]
fn a_fabricated_chain_fails_the_pin() {
    // The threat the pin exists for: whoever produced the document also chose
    // the keys in it, so a chain minted from scratch verifies perfectly on its
    // own terms. Only a root pinned out-of-band separates it from the real one.
    let real = Fixture::new();
    real.append("main", "real");
    let genuine = export_chain(&real.ledger, "main").unwrap();
    let real_root = genuine.records[0].signer_fp.clone();

    let other = Fixture::new();
    other.append("main", "fabricated");
    let forged = export_chain(&other.ledger, "main").unwrap();

    assert!(
        verify_export(&forged, &[]).unwrap().ok,
        "internally consistent — that is exactly the problem"
    );
    let pinned = verify_export(&forged, std::slice::from_ref(&real_root)).unwrap();
    assert_eq!(pinned.root_pinned, Some(false));
    assert!(!pinned.ok);
    assert!(verify_export(&genuine, &[real_root]).unwrap().ok);
}

#[test]
fn a_rotation_keeps_the_pinned_root() {
    // Rotation must not break a pin: the chain still opens under the pinned
    // key, and the incoming key is introduced by a record signed inside it.
    let f = Fixture::new();
    f.append("main", "before");
    let old = f.active_fingerprint("main");
    let new = f.ledger.keys().rotate("main").unwrap().active_fingerprint;
    f.ledger
        .append(&NewRecord::identity_rotated("main", &new, Some(&old)))
        .unwrap();
    f.append("main", "after");

    let doc = export_chain(&f.ledger, "main").unwrap();
    let report = verify_export(&doc, &[doc.records[0].signer_fp.clone()]).unwrap();
    assert!(report.ok, "{:?}", report.faults);
    assert_eq!(report.root_pinned, Some(true));
    assert_eq!(report.keys.len(), 2, "the retired key travels too");
}

#[test]
fn dropping_a_key_from_the_document_makes_its_rows_unverifiable() {
    let f = Fixture::new();
    f.append("main", "t");
    let mut doc = export_chain(&f.ledger, "main").unwrap();
    doc.keys.clear();

    let report = verify_export(&doc, &[]).unwrap();
    assert!(!report.ok);
    assert!(report
        .faults
        .iter()
        .any(|fault| matches!(fault, ChainFault::UnknownSigner { seq: 1, .. })));
}

#[test]
fn a_foreign_format_is_refused_rather_than_guessed() {
    let f = Fixture::new();
    f.append("main", "t");
    let mut doc = export_chain(&f.ledger, "main").unwrap();
    doc.format = "something-else".into();
    assert!(matches!(
        verify_export(&doc, &[]),
        Err(ExportError::UnknownFormat(_))
    ));
}

#[test]
fn a_malformed_row_is_an_error_not_a_pass() {
    let f = Fixture::new();
    f.append("main", "t");
    let mut doc = export_chain(&f.ledger, "main").unwrap();
    doc.records[0].action = "definitely_not_an_action".into();
    assert!(matches!(
        verify_export(&doc, &[]),
        Err(ExportError::Malformed { .. })
    ));
}

#[test]
fn the_export_carries_the_loss_count_into_the_verdict() {
    // A chain says nothing about records that were never written, so the count
    // has to travel with the document or the off-box reader cannot know.
    let f = Fixture::new();
    f.append("main", "t");
    f.ledger.keys().store().ledger_note_lost().unwrap();

    let doc = export_chain(&f.ledger, "main").unwrap();
    assert_eq!(doc.failed_appends, 1);
    assert_eq!(verify_export(&doc, &[]).unwrap().failed_appends, 1);
}
