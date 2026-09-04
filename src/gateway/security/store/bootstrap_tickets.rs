//! Bootstrap ticket storage for one-time remote Panel pairing.
//!
//! A bootstrap ticket is a short-lived, single-use code exchanged for a
//! per-device token during the WebSocket `connect` handshake. It keeps the
//! long-lived shared Gateway token out of URLs, QR codes, and server logs.

use rusqlite::{params, OptionalExtension, Result as SqliteResult};
use uuid::Uuid;

use super::{current_timestamp_ms, SecurityStore};

/// Errors that can occur when consuming a bootstrap ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapTicketError {
    /// The ticket is not redeemable, for any of three reasons: it does not
    /// exist, it has already been consumed, or it has been **revoked** (by a
    /// deactivation sweep or `gateway.ticket.revoke`). All three arrive here
    /// on purpose — see the `Display` impl below.
    Invalid,
    /// Ticket expired before consumption.
    Expired,
    /// Database error.
    Store(String),
}

impl std::fmt::Display for BootstrapTicketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Unknown, consumed and revoked deliberately render as one
            // message. The exchanger is unauthenticated, so telling it which
            // of the three it holds is an oracle over other people's tickets:
            // "already consumed" says the code was real and someone redeemed
            // it, "revoked" says the code was real and a principal was
            // deactivated. Do NOT "fix" this into three distinguishable
            // strings — the sameness is the security property.
            Self::Invalid => write!(f, "invalid or already consumed bootstrap ticket"),
            Self::Expired => write!(f, "bootstrap ticket expired"),
            Self::Store(e) => write!(f, "bootstrap ticket store error: {e}"),
        }
    }
}

impl std::error::Error for BootstrapTicketError {}

/// Mint a non-secret ticket handle.
///
/// Independent randomness, never a slice of the code: a fixed-length prefix
/// would be shorter to implement and would hand every listing a piece of the
/// credential. 64 bits is short enough to retype into `pair --revoke` and wide
/// enough that the UNIQUE index over the column is never the thing that fails.
fn new_ticket_id() -> String {
    let raw = Uuid::new_v4().simple().to_string();
    format!("bt-{}", raw.get(..16).unwrap_or(raw.as_str()))
}

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

/// A bootstrap ticket that is still redeemable: unconsumed, unrevoked and
/// unexpired. What `gateway.ticket.list` / `pair --list` show an operator.
///
/// There is deliberately **no `code` field**. `code` is both the table's
/// primary key and the credential itself, so a listing keyed on it would
/// either print the secret or return rows nothing can address; `ticket_id` is
/// the non-secret handle that closes that fork, and leaving the code out of
/// this type makes leaking it a compile error rather than a review question
/// (`security::audit` has the same rule: it never carries ticket codes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutstandingBootstrapTicket {
    /// Non-secret handle. Generated independently of the code — **not** a
    /// prefix of it, so nothing derived from the credential ever travels.
    pub ticket_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    /// Principal the ticket is bound to; `None` for an UNBOUND ticket, whose
    /// redeemer becomes the owner. The two grant very different authority and
    /// look identical everywhere else, so the listing has to say which.
    pub user_id: Option<String>,
}

impl SecurityStore {
    /// Insert a new bootstrap ticket, returning its non-secret `ticket_id`.
    ///
    /// # Arguments
    /// * `code` — opaque ticket string (caller should generate a high-entropy value)
    /// * `ttl_ms` — lifetime in milliseconds from now
    /// * `user_id` — user this ticket pairs a device to; `None` for an unbound
    ///   ticket (the exchange then defaults a brand-new device to the owner)
    ///
    /// The id is minted here rather than taken from the caller so that every
    /// producer of a ticket — the RPC, `aleph-server pair`, and any future
    /// third face — gets an addressable row without having to remember to.
    pub fn create_bootstrap_ticket(
        &self,
        code: &str,
        ttl_ms: i64,
        user_id: Option<&str>,
    ) -> SqliteResult<String> {
        let ticket_id = new_ticket_id();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        conn.execute(
            "INSERT INTO bootstrap_tickets (code, created_at, expires_at, user_id, ticket_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![code, now, now + ttl_ms, user_id, ticket_id],
        )?;
        Ok(ticket_id)
    }

    /// Every bootstrap ticket that could still be redeemed right now.
    ///
    /// Consumed, revoked and expired rows are excluded, each for the same
    /// reason: offering an operator a row they cannot cut (or that is already
    /// cut) turns an inventory of live credentials into a row dump. Ordered
    /// oldest-first so the output is stable across calls.
    ///
    /// A row with a NULL `ticket_id` fails the whole call rather than being
    /// filtered out. Every insert since v19 writes one and the migration
    /// backfilled the rest, so such a row means the migration did not finish —
    /// and quietly omitting it would hide a still-redeemable credential from
    /// the one surface that can cancel it, which is the wrong direction to
    /// fail in.
    pub fn list_bootstrap_tickets(&self) -> SqliteResult<Vec<OutstandingBootstrapTicket>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let mut stmt = conn.prepare(
            "SELECT ticket_id, created_at, expires_at, user_id FROM bootstrap_tickets
             WHERE consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?1
             ORDER BY created_at ASC, ticket_id ASC",
        )?;
        let rows = stmt.query_map(params![now], |row| {
            Ok(OutstandingBootstrapTicket {
                ticket_id: row.get(0)?,
                created_at: row.get(1)?,
                expires_at: row.get(2)?,
                user_id: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Burn one still-redeemable bootstrap ticket, addressed by its non-secret
    /// id. Returns whether a live credential was actually cut.
    ///
    /// `false` covers unknown id, already revoked, already redeemed and
    /// already expired — none of those cut anything, and reporting them as a
    /// revocation would be a success no-op the operator acts on.
    ///
    /// Reaches the UNBOUND tickets (`user_id IS NULL`) that
    /// [`Self::revoke_bootstrap_tickets_for_user`] deliberately cannot: they
    /// belong to no principal, so no deactivation can burn them, and until
    /// this verb existed nothing could.
    ///
    /// `revoked_at` is its own column on purpose — see the v18 migration.
    pub fn revoke_bootstrap_ticket(&self, ticket_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let updated = conn.execute(
            "UPDATE bootstrap_tickets
             SET revoked_at = ?1
             WHERE ticket_id = ?2 AND consumed_at IS NULL AND revoked_at IS NULL \
               AND expires_at > ?1",
            params![now, ticket_id],
        )?;
        Ok(updated > 0)
    }

    /// Atomically consume a bootstrap ticket if it exists and has not expired.
    ///
    /// Returns the consumed ticket metadata on success. The operation is
    /// idempotent from the caller's perspective: a previously consumed ticket
    /// is reported as `Invalid` — and so is a **revoked** one, the same answer
    /// an unknown code gets, deliberately (see [`BootstrapTicketError`]).
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

    /// The three ways a code can fail to be redeemable must arrive as ONE
    /// answer — same variant and same rendered message. The exchanger is
    /// unauthenticated, so splitting them is an oracle over other people's
    /// tickets: "already consumed" says the code was real and somebody
    /// redeemed it, "revoked" says the code was real and its principal was
    /// just deactivated. This is what goes red if a later reader mistakes the
    /// merged `Display` string for an oversight and gives revocation its own
    /// variant or its own wording.
    #[test]
    fn unknown_consumed_and_revoked_codes_are_one_indistinguishable_answer() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-consumed", 60_000, Some("u-alice"))
            .unwrap();
        store
            .create_bootstrap_ticket("bt-revoked", 60_000, Some("u-bob"))
            .unwrap();
        store
            .consume_bootstrap_ticket("bt-consumed", Some("dev-1"))
            .unwrap();
        assert_eq!(store.revoke_bootstrap_tickets_for_user("u-bob").unwrap(), 1);

        let answers: Vec<(BootstrapTicketError, String)> =
            ["bt-never-minted", "bt-consumed", "bt-revoked"]
                .into_iter()
                .map(|code| {
                    let err = store
                        .consume_bootstrap_ticket(code, Some("dev-2"))
                        .expect_err("none of these three may be redeemable");
                    let rendered = err.to_string();
                    (err, rendered)
                })
                .collect();

        assert_eq!(
            answers[0], answers[1],
            "a consumed code must answer exactly like a code that never existed"
        );
        assert_eq!(
            answers[0], answers[2],
            "a revoked code must answer exactly like a code that never existed"
        );
        assert_eq!(answers[0].0, BootstrapTicketError::Invalid);
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

    /// The whole point of the non-secret id: a listing has to be addressable
    /// without printing the credential. Minting must hand back an id that is
    /// not derived from the code, so no prefix of the code can leak through it.
    #[test]
    fn a_minted_ticket_gets_a_non_secret_id_that_is_no_part_of_the_code() {
        let store = store();
        let code = "aleph-bt-11112222-3333-4444-5555-666677778888";
        let ticket_id = store.create_bootstrap_ticket(code, 60_000, None).unwrap();

        assert!(!ticket_id.is_empty(), "every ticket must be addressable");
        assert!(
            !code.contains(&ticket_id),
            "ticket_id {ticket_id} is a substring of the code — that is a credential prefix, \
             not a non-secret id"
        );
        let listed = store.list_bootstrap_tickets().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].ticket_id, ticket_id);
        assert_eq!(listed[0].expires_at, listed[0].created_at + 60_000);
    }

    /// Revocation by id is what `gateway.ticket.revoke` / `pair --revoke` do.
    /// The effect asserted is redeemability, not the row: throw away the
    /// UPDATE and the consume below still succeeds.
    #[test]
    fn revoking_by_id_makes_the_ticket_unredeemable_and_drops_it_from_the_listing() {
        let store = store();
        let ticket_id = store
            .create_bootstrap_ticket("bt-unbound", 60_000, None)
            .unwrap();

        assert!(store.revoke_bootstrap_ticket(&ticket_id).unwrap());
        assert_eq!(
            store.consume_bootstrap_ticket("bt-unbound", Some("dev-1")),
            Err(BootstrapTicketError::Invalid)
        );
        assert!(store.list_bootstrap_tickets().unwrap().is_empty());
    }

    /// Burned is still not redeemed, on this face too: `revoked_at` is set and
    /// `consumed_at` stays NULL, so the ledger keeps "cut" and "used" apart.
    #[test]
    fn revoking_by_id_stamps_revoked_at_and_leaves_consumed_at_null() {
        let store = store();
        let ticket_id = store
            .create_bootstrap_ticket("bt-one", 60_000, None)
            .unwrap();
        store.revoke_bootstrap_ticket(&ticket_id).unwrap();

        let (consumed, revoked) = stamps(&store, "bt-one");
        assert!(revoked.is_some(), "the burned ticket must carry revoked_at");
        assert!(
            consumed.is_none(),
            "a burned ticket was never redeemed — consumed_at must stay NULL, got {consumed:?}"
        );
    }

    /// Only the transition counts. An unknown id, an already-burned ticket and
    /// an already-redeemed one all report `false`, so the CLI's count is
    /// "credentials cut", never "rows I looked at".
    #[test]
    fn revoking_by_id_reports_false_when_nothing_was_still_redeemable() {
        let store = store();
        let live = store
            .create_bootstrap_ticket("bt-live", 60_000, None)
            .unwrap();
        let used = store
            .create_bootstrap_ticket("bt-used", 60_000, None)
            .unwrap();
        store
            .consume_bootstrap_ticket("bt-used", Some("dev-a"))
            .unwrap();

        assert!(store.revoke_bootstrap_ticket(&live).unwrap());
        assert!(
            !store.revoke_bootstrap_ticket(&live).unwrap(),
            "a second burn of the same ticket cut no credential"
        );
        assert!(
            !store.revoke_bootstrap_ticket(&used).unwrap(),
            "a redeemed ticket is not a live credential"
        );
        assert!(
            !store.revoke_bootstrap_ticket("bt-nobody").unwrap(),
            "an unknown id cuts nothing"
        );
    }

    /// Exclusion 1 of 3, asserted on its own: a redeemed ticket is not
    /// outstanding.
    #[test]
    fn listing_omits_consumed_tickets() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-used", 60_000, None)
            .unwrap();
        store
            .consume_bootstrap_ticket("bt-used", Some("dev-a"))
            .unwrap();

        assert!(store.list_bootstrap_tickets().unwrap().is_empty());
    }

    /// Exclusion 2 of 3: a burned ticket is not outstanding. Covers both burn
    /// faces — the per-user deactivation sweep writes the same column.
    #[test]
    fn listing_omits_revoked_tickets() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-alice", 60_000, Some("u-alice"))
            .unwrap();
        store.revoke_bootstrap_tickets_for_user("u-alice").unwrap();

        assert!(store.list_bootstrap_tickets().unwrap().is_empty());
    }

    /// Exclusion 3 of 3: an expired ticket cannot be redeemed, so offering it
    /// for revocation would be an operator-facing lie.
    #[test]
    fn listing_omits_expired_tickets() {
        let store = store();
        store.create_bootstrap_ticket("bt-old", -1, None).unwrap();

        assert!(store.list_bootstrap_tickets().unwrap().is_empty());
    }

    /// The binding rides through to the listing: an UNBOUND ticket is the
    /// higher-authority half (its redeemer becomes the owner) and an operator
    /// deciding what to cut has to be able to see which one a row is.
    #[test]
    fn listing_carries_the_binding_so_an_unbound_ticket_is_visible_as_such() {
        let store = store();
        store
            .create_bootstrap_ticket("bt-unbound", 60_000, None)
            .unwrap();
        store
            .create_bootstrap_ticket("bt-alice", 60_000, Some("u-alice"))
            .unwrap();

        let listed = store.list_bootstrap_tickets().unwrap();
        let bindings: Vec<Option<&str>> = listed.iter().map(|t| t.user_id.as_deref()).collect();
        assert_eq!(bindings.len(), 2);
        assert!(
            bindings.contains(&None),
            "the unbound ticket must be listed"
        );
        assert!(bindings.contains(&Some("u-alice")));
    }
}
