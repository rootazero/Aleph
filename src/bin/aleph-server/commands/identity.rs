//! `aleph-server identity` — offline reader for agent signing identities and
//! the signed operation ledger.
//!
//! **Read-only on purpose.** Minting, rotating and revoking keys live in the
//! `agent_identity` tool, inside the running daemon: those mutate state the
//! daemon caches in memory, and doing them behind its back would leave the two
//! disagreeing about which key signs.
//!
//! Reading, by contrast, is exactly what must work when the daemon is *not*
//! trusted — or not running. This opens `security.db` directly (WAL, so a live
//! daemon is unaffected), takes no instance lock and starts no runtime, the
//! same shape as `secret` / `bootstrap-token`. `identity verify` therefore
//! answers "was this record trail tampered with" without asking the process
//! that wrote it.

use std::error::Error;

use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
use alephcore::identity::{AgentKeystore, AgentLedger};
use alephcore::sync_primitives::Arc;
use alephcore::utils::paths;

use crate::cli::IdentityAction;

/// Build a ledger over the on-disk stores, without a runtime or a lock.
///
/// The vault master key is required even for reads: verifying a signature
/// needs only the public key (which is in the DB), but building the keystore
/// shares one constructor with the signing path. A missing token is reported
/// as such rather than as an empty ledger.
fn open_ledger() -> Result<AgentLedger, Box<dyn Error>> {
    let db = paths::get_security_db_path()
        .map_err(|e| format!("Failed to resolve security DB path: {e}"))?;
    let store = Arc::new(
        SecurityStore::open(&db).map_err(|e| format!("Failed to open security store: {e}"))?,
    );
    let data_dir =
        paths::get_data_dir().map_err(|e| format!("Cannot determine data directory: {e}"))?;
    let vault = Arc::new(SharedTokenManager::new(
        store.clone(),
        data_dir.join("secrets.vault"),
    ));
    if vault.try_load_token_from_db().is_none() {
        return Err("No vault master token found. Start the server at least once.".into());
    }
    Ok(AgentLedger::new(Arc::new(AgentKeystore::new(store, vault))))
}

fn ms(v: i64) -> String {
    chrono::DateTime::from_timestamp_millis(v).map_or_else(
        || v.to_string(),
        |d| d.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

/// Verify an exported chain with nothing but the file.
///
/// Split out and dispatched **before** [`open_ledger`] so it genuinely needs no
/// database, no vault and no Aleph state — this is the form of the check that
/// runs on the auditor's machine, and it would be worth very little if it
/// quietly required the host it is checking.
fn verify_exported_file(path: &str, pins: &[String]) -> Result<(), Box<dyn Error>> {
    let body = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc: alephcore::identity::ChainExport =
        serde_json::from_str(&body).map_err(|e| format!("{path} is not a chain export: {e}"))?;
    let report = alephcore::identity::verify_export(&doc, pins)?;

    println!("agent      {}", report.agent_id);
    println!(
        "records    {} (seq {}..{})",
        report.records, report.first_seq, report.last_seq
    );
    println!(
        "root key   {}",
        report.root_fingerprint.as_deref().unwrap_or("(none)")
    );
    println!("head hash  {}", report.last_hash.as_deref().unwrap_or("-"));
    println!("keys       {}", report.keys.join(", "));
    match report.root_pinned {
        Some(true) => println!("pin        OK — the chain opens under a pinned key"),
        Some(false) => println!(
            "pin        FAILED — the chain opens under {}, which is not pinned",
            report.root_fingerprint.as_deref().unwrap_or("(nothing)")
        ),
        // Said out loud every time, because the reassuring half of this
        // sentence is the half people remember.
        None => println!(
            "pin        none supplied — a clean result proves internal consistency only; \
             whoever produced this file also chose the keys in it"
        ),
    }
    if report.failed_appends > 0 {
        println!(
            "WARNING    {} record(s) were lost before they were ever written on the \
             installation that produced this file. No chain check can see them.",
            report.failed_appends
        );
    }
    if report.faults.is_empty() {
        println!("result     OK");
    } else {
        println!("result     FAULT");
        for f in &report.faults {
            println!("        {f:?}");
        }
    }
    if !report.ok {
        return Err("export verification failed".into());
    }
    Ok(())
}

pub fn handle_identity_command(action: IdentityAction) -> Result<(), Box<dyn Error>> {
    // The one path that must not touch local state.
    if let IdentityAction::Verify {
        input: Some(ref path),
        ref pin,
        ..
    } = action
    {
        return verify_exported_file(path, pin);
    }

    let ledger = open_ledger()?;

    match action {
        IdentityAction::List => {
            let rows = ledger.keys().list()?;
            if rows.is_empty() {
                println!("No agent identities yet — none has taken a recorded action.");
                return Ok(());
            }
            // "STATE" is inlined rather than passed as an argument: its column is
            // unpadded in the row below too, so there is no width to keep in sync.
            println!(
                "{:<24} {:<18} {:>8}  {:<20} STATE",
                "AGENT", "FINGERPRINT", "RECORDS", "CREATED"
            );
            for r in rows {
                println!(
                    "{:<24} {:<18} {:>8}  {:<20} {}",
                    r.agent_id,
                    r.active_fingerprint,
                    r.head_seq,
                    ms(r.created_at),
                    r.revoked_at
                        .map_or("active".into(), |t| format!("revoked {}", ms(t)))
                );
            }
            // A chain whose identity row was deleted cannot appear in the
            // listing above — that is precisely why deleting it is worth doing.
            // Printing the names here keeps the table from being narrower than
            // the evidence without pretending they are ordinary identities.
            let orphans = ledger.orphan_chains()?;
            if !orphans.is_empty() {
                println!(
                    "\nWARNING: {} chain(s) have records but no identity row — their anchor \
                     is gone, so a truncated tail cannot be detected for them: {}",
                    orphans.len(),
                    orphans.join(", ")
                );
            }
        }

        IdentityAction::Ledger { agent, limit } => {
            let records = ledger.recent(agent.as_deref(), limit)?;
            if records.is_empty() {
                println!("No records.");
                return Ok(());
            }
            for r in records {
                println!(
                    "{} #{:<6} {:<20} {:<18} {:<8} {}",
                    ms(r.at_ms),
                    r.seq,
                    r.agent_id,
                    r.action,
                    r.outcome,
                    r.detail
                );
            }
        }

        IdentityAction::Export { agent, out } => {
            let doc = alephcore::identity::export_chain(&ledger, &agent)?;
            let body = serde_json::to_string_pretty(&doc)?;
            match out {
                Some(path) => {
                    std::fs::write(&path, body).map_err(|e| format!("cannot write {path}: {e}"))?;
                    let root = doc.records.first().map(|r| r.signer_fp.as_str());
                    println!(
                        "Wrote {} ({} records) to {path}",
                        doc.agent_id,
                        doc.records.len()
                    );
                    println!("Pin this root fingerprint off-box: {}", root.unwrap_or("-"));
                }
                None => println!("{body}"),
            }
        }

        IdentityAction::Verify { agent, .. } => {
            let reports = match agent {
                Some(a) => vec![ledger.verify(&a)?],
                None => ledger.verify_all()?,
            };
            if reports.is_empty() {
                println!("No agent identities to verify.");
                return Ok(());
            }
            let mut all_ok = true;
            for r in &reports {
                if r.ok {
                    println!(
                        "OK    {:<24} {} records, head #{}",
                        r.agent_id, r.records, r.last_seq
                    );
                } else {
                    all_ok = false;
                    println!(
                        "FAULT {:<24} {} records, head #{} (anchor #{})",
                        r.agent_id, r.records, r.last_seq, r.anchor_seq
                    );
                    for f in &r.faults {
                        println!("        {f:?}");
                    }
                }
            }
            // Records lost before they were ever written are invisible to any
            // chain check, so a clean verdict must never be printed alone.
            let lost = ledger.lost();
            if lost > 0 {
                println!(
                    "\nWARNING: {lost} record(s) failed to append on this installation. \
                     They are missing from the chains above and no verification can see them."
                );
            }
            if !all_ok {
                return Err("ledger verification failed".into());
            }
        }
    }
    Ok(())
}
