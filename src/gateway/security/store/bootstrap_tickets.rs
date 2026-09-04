//! Bootstrap ticket storage for one-time remote Panel pairing.
//!
//! A bootstrap ticket is a short-lived, single-use code exchanged for a
//! per-device token during the WebSocket `connect` handshake. It keeps the
//! long-lived shared Gateway token out of URLs, QR codes, and server logs.

use rusqlite::{params, OptionalExtension, Result as SqliteResult};

use super::{current_timestamp_ms, SecurityStore};

/// Errors that can occur when consuming a bootstrap ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapTicketError {
    /// Ticket does not exist or has already been consumed.
    Invalid,
    /// Ticket expired before consumption.
    Expired,
    /// Database error.
    Store(String),
}

impl std::fmt::Display for BootstrapTicketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => write!(f, "invalid or already consumed bootstrap ticket"),
            Self::Expired => write!(f, "bootstrap ticket expired"),
            Self::Store(e) => write!(f, "bootstrap ticket store error: {e}"),
        }
    }
}

impl std::error::Error for BootstrapTicketError {}

/// Row returned after successfully consuming a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedBootstrapTicket {
    pub code: String,
    pub consumed_at: i64,
    /// User the ticket was minted for (`gateway.ticket.create`'s optional
    /// `user_id`). `None` for an unbound ticket — the exchange then defaults
    /// a brand-new device to the owner, see `DeviceTokenManager::exchange_bootstrap_ticket`.
    pub user_id: Option<String>,
}

impl SecurityStore {
    /// Insert a new bootstrap ticket.
    ///
    /// # Arguments
    /// * `code` — opaque ticket string (caller should generate a high-entropy value)
    /// * `ttl_ms` — lifetime in milliseconds from now
    /// * `user_id` — user this ticket pairs a device to; `None` for an unbound
    ///   ticket (the exchange then defaults a brand-new device to the owner)
    pub fn create_bootstrap_ticket(
        &self,
        code: &str,
        ttl_ms: i64,
        user_id: Option<&str>,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        conn.execute(
            "INSERT INTO bootstrap_tickets (code, created_at, expires_at, user_id) VALUES (?1, ?2, ?3, ?4)",
            params![code, now, now + ttl_ms, user_id],
        )?;
        Ok(())
    }

    /// Atomically consume a bootstrap ticket if it exists and has not expired.
    ///
    /// Returns the consumed ticket metadata on success. The operation is
    /// idempotent from the caller's perspective: a previously consumed ticket
    /// is reported as `Invalid`.
    pub fn consume_bootstrap_ticket(
        &self,
        code: &str,
        device_id: Option<&str>,
    ) -> Result<ConsumedBootstrapTicket, BootstrapTicketError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Fast path: reject unknown, consumed, revoked, or expired tickets
        // without locking a row. The ticket's user_id is read here too — it
        // cannot change between this check and the UPDATE below (only set
        // once, at creation).
        //
        // A revoked ticket is reported as `Invalid`, the same answer a
        // consumed one gets: an unauthenticated exchanger must not learn from
        // the error which of the two it was holding.
        let now = current_timestamp_ms();
        let row: Option<(i64, Option<i64>, Option<String>, Option<i64>)> = conn
            .query_row(
                "SELECT expires_at, consumed_at, user_id, revoked_at FROM bootstrap_tickets WHERE code = ?1",
                params![code],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| BootstrapTicketError::Store(e.to_string()))?;

        let user_id = match row {
            None => return Err(BootstrapTicketError::Invalid),
            Some((_, Some(_), _, _)) | Some((_, _, _, Some(_))) => {
                return Err(BootstrapTicketError::Invalid)
            }
            Some((expires_at, None, _, None)) if now >= expires_at => {
                return Err(BootstrapTicketError::Expired);
            }
            Some((_, None, user_id, None)) => user_id,
        };

        // Optimistic update; if another caller raced us, rows affected will be
        // 0. `revoked_at IS NULL` is repeated here and not only in the fast
        // path above, so this one statement stays the single chokepoint: a
        // deactivation that lands between the SELECT and this UPDATE still
        // wins.
        let consumed_at = current_timestamp_ms();
        let updated = conn
            .execute(
                "UPDATE bootstrap_tickets
                 SET consumed_at = ?1, consumed_by_device_id = ?2
                 WHERE code = ?3 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
                params![consumed_at, device_id, code],
            )
            .map_err(|e| BootstrapTicketError::Store(e.to_string()))?;

        if updated == 0 {
            return Err(BootstrapTicketError::Invalid);
        }

        Ok(ConsumedBootstrapTicket {
            code: code.to_string(),
            consumed_at,
            user_id,
        })
    }

    /// Burn every still-redeemable bootstrap ticket minted for `user_id`.
    /// Returns how many were actually burned.
    ///
    /// The fourth leg of the deactivation sweep. Without it, mint → deactivate
    /// → redeem is a two-step, wholly legal path to a **fresh, non-revoked**
    /// device row created *after* the sweep's other three legs have already
    /// run: `exchange_bootstrap_ticket` performs no user-status check (both
    /// status guards sit at mint time), so the device pairs, gets a 10-year
    /// token, and `connect` then walls every frame it sends to
    /// `(None, "guest")` — the exact "pairs successfully and then refuses
    /// everything" state the mint-time guards exist to prevent.
    ///
    /// Scope, stated as a limit rather than left to be discovered:
    /// - **Only tickets bound to this user.** An unbound ticket
    ///   (`user_id IS NULL`) is the higher-authority half — it defaults a
    ///   brand-new device to the owner — and belongs to no principal, so
    ///   deactivating one principal must not burn it.
    /// - **Only tickets that could still have been redeemed**: already
    ///   consumed, already revoked, and already expired rows are skipped, so
    ///   the returned count is the number of live credentials this call cut
    ///   and not a row tally.
    ///
    /// `revoked_at` is its own column on purpose — see the v18 migration.
    pub fn revoke_bootstrap_tickets_for_user(&self, user_id: &str) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        conn.execute(
            "UPDATE bootstrap_tickets
             SET revoked_at = ?1
             WHERE user_id = ?2 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
            params![now, user_id],
        )
    }

    /// Prune tickets that expired before `before_ms`. Returns number deleted.
    pub fn prune_expired_bootstrap_tickets(&self, before_ms: i64) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM bootstrap_tickets WHERE expires_at < ?1",
            params![before_ms],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SecurityStore {
        SecurityStore::in_memory().unwrap()
    }

    #[test]
    fn create_and_consume_ticket() {
        let store = store();
        store.create_bootstrap_ticket("bt-1", 60_000, None).unwrap();

        let result = store.consume_bootstrap_ticket("bt-1", Some("dev-1"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().code, "bt-1");

        // Already consumed.
        assert_eq!(
            store.consume_bootstrap_ticket("bt-1", Some("dev-1")),
            Err(BootstrapTicketError::Invalid)
        );
    }

    #[test]
    fn expired_ticket_rejected() {
        let store = store();
        // TTL of -1 makes it immediately expired.
        store
            .create_bootstrap_ticket("bt-expired", -1, None)
            .unwrap();

        assert_eq!(
            store.consume_bootstrap_ticket("bt-expired", None),
            Err(BootstrapTicketError::Expired)
        );
    }

    #[test]
    fn unknown_ticket_rejected() {
        let store = store();
        assert_eq!(
            store.consume_bootstrap_ticket("bt-missing", None),
            Err(BootstrapTicketError::Invalid)
        );
    }

    /// Read the two lifecycle stamps straight from the row. The whole point of
    /// `revoked_at` being a separate column is that these two can disagree, so
    /// the assertion has to look at both.
    fn stamps(store: &SecurityStore, code: &str) -> (Option<i64>, Option<i64>) {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT consumed_at, revoked_at FROM bootstrap_tickets WHERE code = ?1",
            params![code],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// The two-step at store level: a burned ticket is no longer redeemable.
    #[test]
    fn a_revoked_ticket_can_no_longer_be_consumed() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-alice", 60_000, Some("u-alice"))
            .unwrap();

        assert_eq!(
            store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
            1
        );
        assert_eq!(
            store.consume_bootstrap_ticket("bt-alice", Some("dev-1")),
            Err(BootstrapTicketError::Invalid)
        );
    }

    /// Burned is not redeemed. Folding revocation into `consumed_at` would
    /// make the two states byte-identical and no later reader could tell a
    /// credential that was cut from one a device actually used.
    #[test]
    fn a_burned_ticket_stays_distinguishable_from_a_redeemed_one() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-burned", 60_000, Some("u-alice"))
            .unwrap();
        store
            .create_bootstrap_ticket("bt-used", 60_000, Some("u-bob"))
            .unwrap();
        store
            .consume_bootstrap_ticket("bt-used", Some("dev-bob"))
            .unwrap();

        store.revoke_bootstrap_tickets_for_user("u-alice").unwrap();

        let (consumed, revoked) = stamps(&store, "bt-burned");
        assert!(revoked.is_some(), "the burned ticket must carry revoked_at");
        assert!(
            consumed.is_none(),
            "a burned ticket was never redeemed — consumed_at must stay NULL, got {consumed:?}"
        );

        let (consumed, revoked) = stamps(&store, "bt-used");
        assert!(
            consumed.is_some(),
            "the redeemed ticket must carry consumed_at"
        );
        assert!(revoked.is_none(), "a redeemed ticket was not burned");
    }

    /// Transition only: the second sweep finds nothing left to cut. The
    /// handler hangs its audit line on this count, so a non-zero answer here
    /// would make every retry write a fresh authority-change row.
    #[test]
    fn revoking_twice_burns_nothing_the_second_time() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-alice", 60_000, Some("u-alice"))
            .unwrap();

        assert_eq!(
            store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
            1
        );
        assert_eq!(
            store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
            0
        );
    }

    /// The limit, named: an UNBOUND ticket (`user_id IS NULL`) is the
    /// higher-authority half — it defaults a brand-new device to the owner and
    /// belongs to no principal. Deactivating one user must not reach it.
    /// Cancelling an unbound ticket is T16's job, not this leg's.
    #[test]
    fn deactivation_does_not_burn_unbound_tickets_only_t16_reaches_those() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-unbound", 60_000, None)
            .unwrap();
        store
            .create_bootstrap_ticket("bt-alice", 60_000, Some("u-alice"))
            .unwrap();

        assert_eq!(
            store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
            1,
            "only the bound ticket may be burned"
        );
        let (_, revoked) = stamps(&store, "bt-unbound");
        assert!(revoked.is_none(), "the unbound ticket must be untouched");
        assert!(
            store
                .consume_bootstrap_ticket("bt-unbound", Some("dev-1"))
                .is_ok(),
            "the unbound ticket must still redeem"
        );
    }

    /// Another principal's ticket is not this principal's to cut, and an
    /// already-consumed or already-expired one is not a live credential — the
    /// count means "credentials cut", not "rows touched".
    #[test]
    fn revocation_counts_only_this_users_still_redeemable_tickets() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-alice-live", 60_000, Some("u-alice"))
            .unwrap();
        store
            .create_bootstrap_ticket("bt-alice-expired", -1, Some("u-alice"))
            .unwrap();
        store
            .create_bootstrap_ticket("bt-alice-used", 60_000, Some("u-alice"))
            .unwrap();
        store
            .consume_bootstrap_ticket("bt-alice-used", Some("dev-a"))
            .unwrap();
        store
            .create_bootstrap_ticket("bt-bob", 60_000, Some("u-bob"))
            .unwrap();

        assert_eq!(
            store.revoke_bootstrap_tickets_for_user("u-alice").unwrap(),
            1
        );
        assert!(
            store
                .consume_bootstrap_ticket("bt-bob", Some("dev-b"))
                .is_ok(),
            "u-bob's ticket must survive u-alice's deactivation"
        );
    }

    #[test]
    fn prune_removes_expired_tickets() {
        let store = store();
        store.create_bootstrap_ticket("bt-old", -1, None).unwrap();
        store
            .create_bootstrap_ticket("bt-fresh", 60_000, None)
            .unwrap();

        let pruned = store
            .prune_expired_bootstrap_tickets(current_timestamp_ms())
            .unwrap();
        assert_eq!(pruned, 1);

        assert_eq!(
            store.consume_bootstrap_ticket("bt-old", None),
            Err(BootstrapTicketError::Invalid)
        );
        assert!(store.consume_bootstrap_ticket("bt-fresh", None).is_ok());
    }
}
