//! Agent identity + signed operation ledger persistence.
//!
//! Pure I/O (R4): every digest and every signature is computed in
//! [`crate::identity`]; this module only reads and writes rows. Mirrors the
//! `devices.rs` / `tokens.rs` split in this directory.

use rusqlite::{params, Result as SqliteResult};

use super::{current_timestamp_ms, SecurityStore};
use crate::identity::keystore::{AgentIdentityRow, AgentKeyRow};
use crate::identity::record::{row_to_record, LedgerRecord};
use crate::identity::schema::LEDGER_COLUMNS;

/// Columns selected by every `agent_keys` read, in `AgentKeyRow` order.
const KEY_COLUMNS: &str = "fingerprint, agent_id, public_key, created_at, retired_at";
/// Columns selected by every `agent_identities` read, in `AgentIdentityRow` order.
const IDENTITY_COLUMNS: &str =
    "agent_id, active_fingerprint, created_at, revoked_at, head_seq, head_hash";

fn key_from_row(row: &rusqlite::Row<'_>) -> SqliteResult<AgentKeyRow> {
    Ok(AgentKeyRow {
        fingerprint: row.get(0)?,
        agent_id: row.get(1)?,
        public_key: row.get(2)?,
        created_at: row.get(3)?,
        retired_at: row.get(4)?,
    })
}

fn identity_from_row(row: &rusqlite::Row<'_>) -> SqliteResult<AgentIdentityRow> {
    Ok(AgentIdentityRow {
        agent_id: row.get(0)?,
        active_fingerprint: row.get(1)?,
        created_at: row.get(2)?,
        revoked_at: row.get(3)?,
        head_seq: row.get(4)?,
        head_hash: row.get(5)?,
    })
}

impl SecurityStore {
    // ── keys ────────────────────────────────────────────────────────────────

    /// Record a newly minted public key. Keys are never updated in place: a
    /// rotation inserts a new row and retires the old one, because records
    /// signed under the old key must stay verifiable.
    pub fn insert_agent_key(
        &self,
        fingerprint: &str,
        agent_id: &str,
        public_key: &[u8],
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR IGNORE INTO agent_keys (fingerprint, agent_id, public_key, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![fingerprint, agent_id, public_key, current_timestamp_ms()],
        )?;
        Ok(())
    }

    /// Look a key up by fingerprint, **including retired ones** — verification
    /// of an old record needs the key that actually signed it.
    pub fn get_agent_key(&self, fingerprint: &str) -> SqliteResult<Option<AgentKeyRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {KEY_COLUMNS} FROM agent_keys WHERE fingerprint = ?1"
        ))?;
        match stmt.query_row(params![fingerprint], key_from_row) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every key an agent has ever held, newest first.
    pub fn list_agent_keys(&self, agent_id: &str) -> SqliteResult<Vec<AgentKeyRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {KEY_COLUMNS} FROM agent_keys WHERE agent_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![agent_id], key_from_row)?;
        rows.collect()
    }

    /// Stop a key from signing. Idempotent — an already-retired key keeps its
    /// original `retired_at` so the retirement instant stays honest.
    pub fn retire_agent_key(&self, fingerprint: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE agent_keys SET retired_at = ?2 WHERE fingerprint = ?1 AND retired_at IS NULL",
            params![fingerprint, current_timestamp_ms()],
        )?;
        Ok(())
    }

    // ── identities ──────────────────────────────────────────────────────────

    /// Bind `agent_id` to its active signing key.
    ///
    /// Clears `revoked_at`: re-keying a revoked agent is an operator action
    /// that deliberately brings it back. The anchor columns are **not** touched
    /// — a rotation must never reset the chain head, or rotating would become a
    /// way to erase history undetectably.
    pub fn upsert_agent_identity(&self, agent_id: &str, fingerprint: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO agent_identities (agent_id, active_fingerprint, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(agent_id) DO UPDATE SET
               active_fingerprint = excluded.active_fingerprint,
               revoked_at = NULL",
            params![agent_id, fingerprint, current_timestamp_ms()],
        )?;
        Ok(())
    }

    pub fn get_agent_identity(&self, agent_id: &str) -> SqliteResult<Option<AgentIdentityRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {IDENTITY_COLUMNS} FROM agent_identities WHERE agent_id = ?1"
        ))?;
        match stmt.query_row(params![agent_id], identity_from_row) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// All identities, including revoked ones — a revoked agent's chain is
    /// still evidence and must stay listable.
    pub fn list_agent_identities(&self) -> SqliteResult<Vec<AgentIdentityRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {IDENTITY_COLUMNS} FROM agent_identities ORDER BY agent_id"
        ))?;
        let rows = stmt.query_map([], identity_from_row)?;
        rows.collect()
    }

    /// Revoke an identity and retire its active key in one transaction.
    /// Returns `false` when there is no such identity.
    pub fn revoke_agent_identity(&self, agent_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;
        let now = current_timestamp_ms();
        let changed = tx.execute(
            "UPDATE agent_identities SET revoked_at = ?2
             WHERE agent_id = ?1 AND revoked_at IS NULL",
            params![agent_id, now],
        )?;
        tx.execute(
            "UPDATE agent_keys SET retired_at = ?2
             WHERE agent_id = ?1 AND retired_at IS NULL",
            params![agent_id, now],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    // ── ledger ──────────────────────────────────────────────────────────────

    /// The position a new record must take: `(seq, prev_hash)`.
    ///
    /// `seq` is `max(anchor_head, last_row) + 1`, **not** `last_row + 1`. The
    /// difference is the whole point of keeping an anchor: if rows are deleted
    /// off the tail, the anchor still remembers how far the chain got, so the
    /// next append lands *above* the hole and leaves a permanent sequence gap
    /// for [`verify`](crate::identity::verify) to find. Chaining from
    /// `last_row + 1` would silently re-use the deleted sequence numbers and
    /// heal the evidence.
    ///
    /// `prev_hash` comes from the actual last row (`None` when the table is
    /// empty), so a wiped chain whose anchor survives produces a record with
    /// `seq > 1` and no predecessor — which `verify` reports as a missing
    /// genesis rather than accepting as a fresh chain.
    pub fn ledger_next_position(&self, agent_id: &str) -> SqliteResult<(i64, Option<Vec<u8>>)> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let anchor_seq: i64 = match conn.query_row(
            "SELECT head_seq FROM agent_identities WHERE agent_id = ?1",
            params![agent_id],
            |r| r.get(0),
        ) {
            Ok(seq) => seq,
            Err(rusqlite::Error::QueryReturnedNoRows) => 0,
            Err(e) => return Err(e),
        };
        let last: Option<(i64, Vec<u8>)> = match conn.query_row(
            "SELECT seq, hash FROM agent_ledger WHERE agent_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![agent_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(row) => Some(row),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e),
        };
        let (last_seq, prev_hash) = match last {
            Some((s, h)) => (s, Some(h)),
            None => (0, None),
        };
        Ok((anchor_seq.max(last_seq) + 1, prev_hash))
    }

    /// Append a fully-built record and advance the anchor, atomically.
    ///
    /// The `PRIMARY KEY (agent_id, seq)` is the correctness backstop: if a
    /// second writer ever appears, the racing append fails loudly here instead
    /// of forking the chain.
    pub fn ledger_insert(&self, rec: &LedgerRecord) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO agent_ledger
               (agent_id, seq, prev_hash, hash, signature, signer_fp, action, target, outcome,
                args_fp, detail, at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                rec.agent_id,
                rec.seq,
                rec.prev_hash,
                rec.hash,
                rec.signature,
                rec.signer_fp,
                rec.action.as_str(),
                rec.target,
                rec.outcome.as_str(),
                rec.args_fp,
                rec.detail,
                rec.at_ms,
            ],
        )?;
        // Anchor moves forward only. A record that somehow lands below the
        // recorded head must not drag the head back down with it.
        tx.execute(
            "UPDATE agent_identities SET head_seq = ?2, head_hash = ?3
             WHERE agent_id = ?1 AND head_seq < ?2",
            params![rec.agent_id, rec.seq, rec.hash],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Most recent records first — the operator/model read surface.
    pub fn ledger_recent(
        &self,
        agent_id: Option<&str>,
        limit: i64,
    ) -> SqliteResult<Vec<LedgerRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        match agent_id {
            Some(id) => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {LEDGER_COLUMNS} FROM agent_ledger WHERE agent_id = ?1
                     ORDER BY seq DESC LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![id, limit], row_to_record)?;
                rows.collect()
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {LEDGER_COLUMNS} FROM agent_ledger ORDER BY at_ms DESC LIMIT ?1"
                ))?;
                let rows = stmt.query_map(params![limit], row_to_record)?;
                rows.collect()
            }
        }
    }

    // ── health ──────────────────────────────────────────────────────────────

    /// Count one append this installation failed to write.
    ///
    /// Durable because a lost record leaves no trace a chain check can find,
    /// and the offline verifier — a different process, run when the daemon is
    /// not trusted — must be able to say "this trail is incomplete". A best
    /// effort by nature: the same disk failure that lost the record can lose
    /// the count of it, which is why the caller also keeps a process counter
    /// and reports the larger of the two.
    pub fn ledger_note_lost(&self) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO agent_ledger_health (id, lost_appends, last_lost_ms)
             VALUES (1, 1, ?1)
             ON CONFLICT(id) DO UPDATE SET
               lost_appends = lost_appends + 1,
               last_lost_ms = excluded.last_lost_ms",
            params![current_timestamp_ms()],
        )?;
        Ok(())
    }

    /// Appends this installation is known to have lost, across all time and
    /// every process that has written this database.
    pub fn ledger_lost_total(&self) -> SqliteResult<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT lost_appends FROM agent_ledger_health WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(0),
            other => Err(other),
        })
    }

    /// The whole chain in sequence order — the verification read.
    pub fn ledger_chain(&self, agent_id: &str) -> SqliteResult<Vec<LedgerRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {LEDGER_COLUMNS} FROM agent_ledger WHERE agent_id = ?1 ORDER BY seq ASC"
        ))?;
        let rows = stmt.query_map(params![agent_id], row_to_record)?;
        rows.collect()
    }
}
