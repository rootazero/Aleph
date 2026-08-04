//! Network proxy configuration — placeholder CLI surface.
//!
//! Aleph has an internal *sandbox* proxy (see `src/sandbox/proxy/`)
//! that mediates network egress for sandboxed tool invocations on
//! macOS — but that is per-sandbox-spawn, not a user-facing config
//! concept. A general "configure a system-wide proxy" surface (HTTPS
//! upstream, `NO_PROXY` rules, auth, env injection into providers) has
//! never been designed.
//!
//! This subcommand surfaces the gap explicitly so users discover the
//! design doc instead of staring at a `clap` error.

use crate::output;
use aleph_client::CliResult;

const GAPS_DOC: &str = "docs/reference/CLI_BACKEND_GAPS.md";

pub async fn show(json: bool) -> CliResult<()> {
    let payload = serde_json::json!({
        "status": "not_implemented",
        "note": "No user-facing proxy config layer exists yet.",
        "see": GAPS_DOC,
        "today": "Set HTTPS_PROXY / NO_PROXY env vars before launching aleph-server."
    });
    if json {
        output::print_json(&payload);
    } else {
        eprintln!(
            "aleph proxy: no user-facing proxy config layer exists yet.\n\
             Today: set HTTPS_PROXY / HTTP_PROXY / NO_PROXY in the env that\n\
             launches `aleph-server`. The sandbox subsystem uses its own\n\
             internal proxy for managed egress on macOS — unrelated.\n\
             See {GAPS_DOC} for the proposed surface."
        );
    }
    Ok(())
}