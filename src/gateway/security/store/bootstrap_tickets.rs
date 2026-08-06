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

        // Fast path: reject unknown, consumed, or expired tickets without locking a row.
        // The ticket's user_id is read here too — it cannot change between
        // this check and the UPDATE below (only set once, at creation).
        let now = current_timestamp_ms();
        let row: Option<(i64, Option<i64>, Option<String>)> = conn
            .query_row(
                "SELECT expires_at, consumed_at, user_id FROM bootstrap_tickets WHERE code = ?1",
                params![code],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| BootstrapTicketError::Store(e.to_string()))?;

        let user_id = match row {
            None => return Err(BootstrapTicketError::Invalid),
            Some((_, Some(_), _)) => return Err(BootstrapTicketError::Invalid),
            Some((expires_at, None, _)) if now >= expires_at => {
                return Err(BootstrapTicketError::Expired);
            }
            Some((_, None, user_id)) => user_id,
        };

        // Optimistic update; if another caller raced us, rows affected will be 0.
        let consumed_at = current_timestamp_ms();
        let updated = conn
            .execute(
                "UPDATE bootstrap_tickets
                 SET consumed_at = ?1, consumed_by_device_id = ?2
                 WHERE code = ?3 AND consumed_at IS NULL AND expires_at > ?1",
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
        store.create_bootstrap_ticket("bt-expired", -1, None).unwrap();

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

    #[test]
    fn prune_removes_expired_tickets() {
        let store = store();
        store.create_bootstrap_ticket("bt-old", -1, None).unwrap();
        store.create_bootstrap_ticket("bt-fresh", 60_000, None).unwrap();

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
