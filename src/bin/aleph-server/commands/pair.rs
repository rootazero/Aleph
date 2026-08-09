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
use alephcore::gateway::security::{store::SecurityStore, DeviceTokenManager};
use alephcore::gateway::tls::discover_interface_ips;
use alephcore::gateway::GatewayConfig;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

/// Handle `aleph-server pair [--ttl SECONDS] [--user u-…]`.
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
pub fn handle_pair(
    config: Option<PathBuf>,
    ttl_seconds: Option<u64>,
    user_id: Option<String>,
) -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path =
        paths::get_security_db_path().map_err(|e| format!("resolve security DB path: {e}"))?;
    let store = Arc::new(
        SecurityStore::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?,
    );
    let mgr = DeviceTokenManager::new(Arc::clone(&store));

    // Same clamp as the RPC path so both entry points cannot disagree about
    // what "5 minutes" means.
    let ttl_ms = ttl_seconds.map(|s| s.clamp(60, 86_400) as i64 * 1000);

    // Refuse a `--user` that names nobody, here rather than at redemption.
    // A ticket bound to a dangling id is not a harmless typo: the device that
    // redeems it resolves through the dangling-user arm to `("guest")` and hits
    // the login wall on every frame — an invitation that looks minted, prints a
    // URL, and silently cannot work. Fail where the operator is still looking.
    if let Some(ref uid) = user_id {
        match store.get_user(uid) {
            Ok(Some(_)) => {}
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
