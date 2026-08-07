//! Webhook subscription management — placeholder CLI surface.
//!
//! Aleph already ships a webhook *receiver* infrastructure in
//! `src/gateway/webhooks/` (HMAC verification, GitHub/Stripe/Generic
//! signature formats, session-key templating). What it does NOT yet have
//! is a JSON-RPC management surface — endpoints are declared once in
//! TOML and the system isn't currently mounted in production startup.
//!
//! This thin-client surface exists so the gap is *visible* in the CLI
//! catalogue and the help text points operators at the design doc.
//! When the backend lands the dispatch swaps to real RPC calls without
//! the user-facing shape changing.

use crate::output;
use aleph_client::CliResult;

const GAPS_DOC: &str = "docs/reference/CLI_BACKEND_GAPS.md";

pub async fn list(json: bool) -> CliResult<()> {
    let payload = serde_json::json!({
        "status": "not_implemented",
        "note": "Webhook receiver exists in gateway/webhooks/ but has no management RPC.",
        "see": GAPS_DOC,
        "configured_via": "TOML — `[webhooks]` + `[[webhooks.endpoints]]` blocks"
    });
    if json {
        output::print_json(&payload);
    } else {
        eprintln!(
            "aleph webhook list is not yet wired to a backend RPC.\n\
             Webhook endpoints are still configured via TOML (`[[webhooks.endpoints]]`).\n\
             Management surface tracked in {GAPS_DOC}."
        );
    }
    Ok(())
}
