//! `aleph-server resume <session-key>` — re-trigger an interrupted run.
//!
//! Policy: `NoLock`. This command never writes to the data directory; it reads
//! the endpoint file and the bearer token and forwards to the running server.
//!
//! Deliberately **not** `LockOrIpc`. The local half of that policy exists for
//! commands that can do their job with the lock and no server. Resuming a run
//! cannot: it means re-entering the harness with the session's provider, tools
//! and workspace. A local fallback would either silently do nothing or stand up
//! a second runtime beside the singleton, so when no server is running the
//! honest answer is to say so — which is what `forward_to_server` already does
//! when the endpoint file is missing.

use std::error::Error;

use alephcore::cli::ipc_client::forward_to_server;
use alephcore::cli::policy::{run_no_lock, HttpMethod};
use alephcore::gateway::admin_api::resume::ResumeResponse;
use alephcore::utils::paths;

/// Handle `aleph-server resume`.
pub fn handle_resume_command(session_key: String, json: bool) -> Result<(), Box<dyn Error>> {
    let data_dir = paths::get_data_dir().map_err(|e| format!("data dir: {e}"))?;
    let body = serde_json::json!({ "session_key": &session_key });

    let response: ResumeResponse =
        run_no_lock(|| forward_to_server(&data_dir, HttpMethod::Post, "/v1/admin/resume", body))
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    // Each status gets its own sentence. "resumed" and "no_runs" are both
    // successful exits, and an operator who cannot tell them apart will sit
    // waiting for output from a session that never had a run to resume.
    match response.status.as_str() {
        "resumed" => println!(
            "Resumed the interrupted run in {}. Watch the session for output.",
            response.session_key
        ),
        "already_finished" => println!(
            "Nothing to resume: the newest run in {} already finished.",
            response.session_key
        ),
        "no_runs" => println!(
            "Nothing to resume: {} has no run history.",
            response.session_key
        ),
        "abandoned" => println!(
            "The interrupted run in {} was abandoned (too old, or it crashed on every \
             previous resume). It will not be retried.",
            response.session_key
        ),
        "not_resumed" => println!(
            "Found an interrupted run in {} but could not re-trigger it. \
             Check the server log for the reason.",
            response.session_key
        ),
        other => println!("{other} ({})", response.session_key),
    }
    Ok(())
}
