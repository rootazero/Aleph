//! `pair` subcommand — mint a one-time pairing ticket for a remote Panel.
//!
//! This is the headless counterpart of Settings → Security → "Pair new device".
//! Before it existed, the only way to authorize a remote Panel without already
//! having an authorized Panel was `bootstrap-token`, which prints the **shared
//! Gateway token** — a credential that never expires, that the Panel then stores
//! in `localStorage` forever, and that doubles as the secret vault's master key
//! (`store/tokens.rs`). Handing that to a phone to read a chat log is the wrong
//! trade. A bootstrap ticket is single-use, expires in minutes, and is exchanged
//! at `connect` for a device-scoped token that can be revoked on its own.
//!
//! Same threat model and mechanics as `bootstrap-token` / `secret`: opens
//! `~/.aleph/data/security.db` directly (mode 0600, WAL + `busy_timeout`), so it
//! works whether or not the daemon is running and never contends with it.

use alephcore::gateway::handlers::gateway_ticket::{pairing_urls, reachable_hosts};
use alephcore::gateway::security::{
    store::{OutstandingBootstrapTicket, SecurityStore, UserStatus},
    DeviceTokenManager,
};
use alephcore::gateway::tls::discover_interface_ips;
use alephcore::gateway::GatewayConfig;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

/// Handle `aleph-server pair [--ttl SECONDS] [--user u-…] | --list | --revoke <TICKET_ID>`.
///
/// Prints the ticket, its expiry, and every URL the core is reachable on with
/// the ticket already attached — paste one into a phone browser, or paste the
/// bare ticket into the Panel's authorize box.
///
/// `--user` is the second half of the invite flow whose first half is
/// `aleph users create`. Without it the ticket is **unbound**: the device that
/// redeems it resolves through `resolve_connection_identity`'s
/// unbound-device arm to `(OWNER_USER_ID, "operator")`, which is the correct
/// zero-config behaviour for pairing your own phone and the wrong one for
/// handing a colleague access. With it, `set_device_user_if_unbound` binds the
/// redeeming device to that principal, and every P0/P1/P2 predicate downstream
/// — session visibility, memory partition, roster membership, event delivery —
/// finally has a second subject to distinguish.
///
/// The parameter has been sitting in `create_bootstrap_ticket`'s signature
/// since P0 with `None` hard-coded at this one call site, which is why the
/// multi-user arc had no reachable way to admit a second person.
///
/// `--list` and `--revoke <TICKET_ID>` are the client half of
/// `gateway.ticket.list` / `gateway.ticket.revoke`. They read and write the
/// same `security.db` through the same two store methods the RPC calls, so the
/// two faces of "cancel a pairing ticket" cannot disagree about what
/// outstanding means. A server capability with no client is not delivered, and
/// this is the cheapest client for an admin-gated family.
pub fn handle_pair(
    config: Option<PathBuf>,
    ttl_seconds: Option<u64>,
    user_id: Option<String>,
    list: bool,
    revoke: Option<String>,
) -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path =
        paths::get_security_db_path().map_err(|e| format!("resolve security DB path: {e}"))?;
    let store = Arc::new(
        SecurityStore::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?,
    );
    let mgr = DeviceTokenManager::new(Arc::clone(&store));

    if let Some(ticket_id) = revoke.as_deref() {
        let (_, line) = revoke_outstanding(&store, ticket_id)?;
        print!("{line}");
        return Ok(());
    }
    if list {
        // Prune first so an expired row cannot be reported as outstanding by a
        // stale table — same opportunistic hygiene as the RPC chokepoints.
        let _ = mgr.prune_now();
        let tickets = store
            .list_bootstrap_tickets()
            .map_err(|e| format!("list pairing tickets: {e}"))?;
        print!("{}", render_outstanding(&tickets, current_timestamp_ms()));
        return Ok(());
    }

    // Same clamp as the RPC path so both entry points cannot disagree about
    // what "5 minutes" means.
    let ttl_ms = ttl_seconds.map(|s| s.clamp(60, 86_400) as i64 * 1000);

    // Refuse a `--user` that names nobody, here rather than at redemption.
    // A ticket bound to a dangling id is not a harmless typo: the device that
    // redeems it resolves through the dangling-user arm to `("guest")` and hits
    // the login wall on every frame — an invitation that looks minted, prints a
    // URL, and silently cannot work. Fail where the operator is still looking.
    //
    // A DEACTIVATED id produces byte-identical symptoms: `connect` walls it to
    // `(None, "guest")` on the same arm the dangling one takes. So the guard
    // asks the whole question — existence *and* status — the way the sibling
    // id-binding producer `channel.pairing.approve` does.
    if let Some(ref uid) = user_id {
        match store.get_user(uid) {
            Ok(Some(u)) if u.status == UserStatus::Active => {}
            Ok(Some(_)) => {
                return Err(format!(
                    "user {uid} is deactivated\n\nA ticket bound to a walled principal mints, \
                     prints a URL, and refuses every frame after pairing. Run \
                     `aleph users update {uid} --status active` first."
                )
                .into())
            }
            Ok(None) => {
                return Err(format!(
                    "no such user: {uid}\n\nRun `aleph users list` to see who exists, \
                     or `aleph users create <name>` to add someone."
                )
                .into())
            }
            Err(e) => return Err(format!("look up {uid}: {e}").into()),
        }
    }

    // An unbound ticket (`--user` omitted) is the zero-config path: the paired
    // device defaults to the owner, which is what pairing your own phone means.
    let ticket = mgr
        .create_bootstrap_ticket(ttl_ms, user_id.as_deref())
        .map_err(|e| format!("mint pairing ticket: {e}"))?;

    // Opportunistic hygiene, same as the RPC chokepoints.
    let _ = mgr.prune_now();

    let cfg = load_gateway_config(config);
    let hosts = reachable_hosts(&cfg.gateway.host, &discover_interface_ips());
    let urls = pairing_urls(&hosts, cfg.gateway.port, cfg.gateway.tls.enabled, &ticket);
    let minutes = ttl_ms.unwrap_or(5 * 60 * 1000) / 60_000;

    // Say who this ticket is for. An unbound ticket and a bound one look
    // identical on the wire and grant very different authority — printing the
    // binding is the only place the operator can see which one they just made.
    match user_id.as_deref() {
        Some(uid) => println!("Pairing ticket for {uid} (single use, expires in {minutes} min):\n"),
        None => println!(
            "Pairing ticket, UNBOUND — whoever redeems it becomes the owner \
             (single use, expires in {minutes} min):\n"
        ),
    }
    println!("  {ticket}\n");
    if urls.is_empty() {
        println!(
            "This core is bound to {} — reachable only from this machine, so there is\n\
             no LAN URL to hand out. Set `[gateway] host = \"0.0.0.0\"` to open it to\n\
             the local network, or paste the ticket above into the Panel's authorize box.",
            cfg.gateway.host
        );
    } else {
        println!("Open one of these on the device you want to authorize:\n");
        for url in &urls {
            println!("  {url}");
        }
        println!("\nOr paste the ticket itself into the Panel's authorize box.");
    }
    Ok(())
}

/// Load the gateway config for host/port/scheme, degrading to defaults with a
/// warning. A wrong URL is worth printing with a warning; refusing to mint the
/// ticket because a config file moved is not.
fn load_gateway_config(config: Option<PathBuf>) -> GatewayConfig {
    let loaded = match config {
        Some(path) => GatewayConfig::load(&path),
        None => GatewayConfig::load_default(),
    };
    loaded.unwrap_or_else(|e| {
        eprintln!("aleph-server: {e}; assuming default host/port for the pairing URL");
        GatewayConfig::default()
    })
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Render the outstanding-ticket table for `pair --list`.
///
/// Never prints a ticket **code**: the code is the credential, and `ticket_id`
/// is the non-secret handle `--revoke` (and `gateway.ticket.revoke`) act on.
/// An unbound row is labelled as such — it is the higher-authority half
/// (whoever redeems it becomes the owner), and an operator deciding what to cut
/// has to be able to see which kind a row is.
fn render_outstanding(tickets: &[OutstandingBootstrapTicket], now_ms: i64) -> String {
    if tickets.is_empty() {
        return "No outstanding pairing tickets.\n".to_string();
    }
    let mut out = format!(
        "{} outstanding pairing ticket(s) — revoke one with \
         `aleph-server pair --revoke <ID>`:\n\n",
        tickets.len()
    );
    for t in tickets {
        let minutes = (t.expires_at - now_ms).max(0) / 60_000;
        let binding = match t.user_id.as_deref() {
            Some(uid) => format!("for {uid}"),
            None => "UNBOUND (redeemer becomes the owner)".to_string(),
        };
        out.push_str(&format!(
            "  {}  expires in {minutes} min  {binding}\n",
            t.ticket_id
        ));
    }
    out
}

/// Burn one outstanding ticket by id and render the operator-facing line.
///
/// Returns the **count of credentials actually cut** (0 or 1), not a row tally:
/// an unknown id, an already-burned ticket and an already-redeemed one all cut
/// nothing and must say so rather than reporting a success no-op.
fn revoke_outstanding(
    store: &SecurityStore,
    ticket_id: &str,
) -> Result<(usize, String), Box<dyn Error>> {
    let cut = usize::from(
        store
            .revoke_bootstrap_ticket(ticket_id)
            .map_err(|e| format!("revoke {ticket_id}: {e}"))?,
    );
    let line = if cut == 0 {
        format!(
            "Revoked {cut} pairing ticket(s): {ticket_id} was not outstanding \
             (unknown id, already revoked, already redeemed, or expired).\n"
        )
    } else {
        format!("Revoked {cut} pairing ticket(s): {ticket_id} can no longer be redeemed.\n")
    };
    Ok((cut, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SecurityStore {
        SecurityStore::in_memory().unwrap()
    }

    /// The client half, asserted on what the operator actually reads: the rows
    /// reach stdout, they are addressable, and no ticket code is in the output.
    #[test]
    fn list_prints_the_outstanding_rows_and_never_a_ticket_code() {
        let store = store();
        let code = "aleph-bt-11112222-3333-4444-5555-666677778888";
        let id = store.create_bootstrap_ticket(code, 600_000, None).unwrap();
        let bound = store
            .create_bootstrap_ticket("aleph-bt-second", 600_000, Some("u-alice"))
            .unwrap();

        let rows = store.list_bootstrap_tickets().unwrap();
        let now = rows[0].created_at;
        let out = render_outstanding(&rows, now);

        assert!(
            out.contains(&id),
            "the unbound row must be addressable: {out}"
        );
        assert!(
            out.contains(&bound),
            "the bound row must be addressable: {out}"
        );
        assert!(
            out.contains("UNBOUND"),
            "an unbound ticket must read as such: {out}"
        );
        assert!(
            out.contains("u-alice"),
            "a bound ticket must name its principal: {out}"
        );
        assert!(
            !out.contains(code) && !out.contains("11112222"),
            "the listing leaked credential material: {out}"
        );
    }

    #[test]
    fn list_says_so_when_there_is_nothing_outstanding() {
        let out = render_outstanding(&[], 0);
        assert!(out.contains("No outstanding"), "{out}");
    }

    /// The revoke client returns a COUNT of credentials cut, and the count is
    /// the effect: after it reports 1 the ticket is gone from the listing.
    #[test]
    fn revoke_reports_the_count_of_credentials_it_actually_cut() {
        let store = store();
        let id = store
            .create_bootstrap_ticket("aleph-bt-live", 600_000, None)
            .unwrap();

        let (cut, line) = revoke_outstanding(&store, &id).unwrap();
        assert_eq!(cut, 1);
        assert!(line.contains('1') && line.contains(&id), "{line}");
        assert!(store.list_bootstrap_tickets().unwrap().is_empty());

        let (cut, line) = revoke_outstanding(&store, &id).unwrap();
        assert_eq!(cut, 0, "a second burn cut no credential");
        assert!(line.contains("not outstanding"), "{line}");
    }

    #[test]
    fn revoking_an_unknown_id_reports_zero_rather_than_a_success_no_op() {
        let store = store();
        let (cut, line) = revoke_outstanding(&store, "bt-nobody").unwrap();
        assert_eq!(cut, 0);
        assert!(line.contains("not outstanding"), "{line}");
    }
}
