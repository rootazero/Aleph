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

/// Handle `aleph-server pair [--ttl SECONDS]`.
///
/// Prints the ticket, its expiry, and every URL the core is reachable on with
/// the ticket already attached — paste one into a phone browser, or paste the
/// bare ticket into the Panel's authorize box.
pub fn handle_pair(
    config: Option<PathBuf>,
    ttl_seconds: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path =
        paths::get_security_db_path().map_err(|e| format!("resolve security DB path: {e}"))?;
    let store = Arc::new(
        SecurityStore::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?,
    );
    let mgr = DeviceTokenManager::new(store);

    // Same clamp as the RPC path so both entry points cannot disagree about
    // what "5 minutes" means.
    let ttl_ms = ttl_seconds.map(|s| s.clamp(60, 86_400) as i64 * 1000);
    // Headless/CLI pairing is admin-only and has no notion of "which user" —
    // the ticket is unbound, so the paired device defaults to the owner.
    let ticket = mgr
        .create_bootstrap_ticket(ttl_ms, None)
        .map_err(|e| format!("mint pairing ticket: {e}"))?;

    // Opportunistic hygiene, same as the RPC chokepoints.
    let _ = mgr.prune_now();

    let cfg = load_gateway_config(config);
    let hosts = reachable_hosts(&cfg.gateway.host, &discover_interface_ips());
    let urls = pairing_urls(&hosts, cfg.gateway.port, cfg.gateway.tls.enabled, &ticket);
    let minutes = ttl_ms.unwrap_or(5 * 60 * 1000) / 60_000;

    println!("Pairing ticket (single use, expires in {minutes} min):\n");
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
