//! Ask command — send a single non-interactive message.
//!
//! Adds codex-parity affordances on top of the original "send and print":
//!
//! - `--last`: pick the session with the latest `last_active_at`
//! - `--json` (top-level): emit raw protocol events as JSONL to stdout
//! - `--output-last-message <FILE>` (`-o`): write the final agent text to FILE
//! - stdin piping: `echo "prompt" | aleph ask` (and `git diff | aleph ask "review"`),
//!   resolved by [`merge_prompt`] + [`read_piped_stdin`] before [`run`].
//!
//! Rendering (live tool activity, incrementally streamed Markdown body,
//! retry notices, summary footer) lives in [`super::run_follow`], shared
//! with `chat-control send --stream`.

use serde::Serialize;
use serde_json::Value;

use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

use super::run_follow::{self, FollowOptions};

/// Run the `ask` command.
pub async fn run(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    last: bool,
    json: bool,
    output_last_message: Option<&str>,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, mut events) = AlephClient::connect(server_url, config).await?;

    // The `connect` handshake happens inside `AlephClient::connect`.

    // Resolve session key. Precedence:
    //   1. explicit --session  (clap rejects --last + --session simultaneously)
    //   2. --last → newest by `last_active_at`
    //   3. config.default_session
    //   4. literal "default"
    let session_key = if let Some(s) = session {
        s.to_string()
    } else if last {
        resolve_last_session(&client).await?
    } else {
        config
            .default_session
            .clone()
            .unwrap_or_else(|| "default".to_string())
    };

    #[derive(Serialize)]
    struct RunParams {
        session_key: Option<String>,
        input: String,
    }

    let params = RunParams {
        session_key: Some(session_key.clone()),
        input: message.to_string(),
    };

    let accepted: Value = client.call("agent.run", Some(params)).await?;
    // Pin the follow loop to the accepted run: the gateway broadcasts every
    // stream frame to every connection, so a concurrent cron/channel run
    // would otherwise interleave into (or prematurely terminate) this one.
    let run_id = accepted
        .get("run_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Render the run live (tool activity, streamed Markdown body, retry
    // notices, summary footer) via the shared follow loop.
    let verbose = std::env::var("ALEPH_VERBOSE").is_ok();
    let outcome = run_follow::follow_run(
        &mut events,
        &FollowOptions {
            json,
            verbose,
            run_id,
        },
    )
    .await;

    if let Some(path) = output_last_message {
        if let Err(e) = tokio::fs::write(path, &outcome.final_text).await {
            eprintln!("warning: failed to write --output-last-message to {path}: {e}");
        }
    }

    client.close().await?;
    Ok(())
}

/// Pick the session with the maximum `last_active_at` (RFC3339 ISO strings
/// sort lexicographically). Errors when the server returns no sessions.
async fn resolve_last_session(client: &AlephClient) -> CliResult<String> {
    let listed: Value = client.call("sessions.list", None::<()>).await?;
    let sessions = listed
        .get("sessions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::Other("sessions.list returned no `sessions` array".to_string()))?;

    let mut best_key: Option<&str> = None;
    let mut best_ts: &str = "";
    for entry in sessions {
        let Some(k) = entry.get("key").and_then(|v| v.as_str()) else {
            continue;
        };
        let ts = entry
            .get("last_active_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Prefer strictly greater timestamps; ties keep the earlier candidate.
        if best_key.is_none() || ts > best_ts {
            best_key = Some(k);
            best_ts = ts;
        }
    }

    best_key
        .map(std::string::ToString::to_string)
        .ok_or_else(|| CliError::Other("--last: no sessions available to resume".to_string()))
}

/// Read piped stdin, returning `Some` only when stdin is NOT a TTY and the
/// piped text is non-empty. Interactive invocations (TTY stdin) return `None`
/// so the CLI never blocks waiting for keyboard input.
pub fn read_piped_stdin() -> Option<String> {
    use std::io::{IsTerminal, Read};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = String::new();
    match stdin.lock().read_to_string(&mut buf) {
        Ok(_) if !buf.trim().is_empty() => Some(buf),
        _ => None,
    }
}

/// Fold an explicit `message` argument together with piped stdin into the
/// effective prompt. Pure string transform (no I/O) so it is host-testable.
///
/// - both present → message, then the piped text appended as context
/// - exactly one present → that one (trimmed)
/// - neither → `None` (the caller surfaces a "no message" error)
pub fn merge_prompt(message: Option<&str>, piped: Option<&str>) -> Option<String> {
    let msg = message.map(str::trim).filter(|s| !s.is_empty());
    let pipe = piped.map(str::trim).filter(|s| !s.is_empty());
    match (msg, pipe) {
        (Some(m), Some(p)) => Some(format!("{m}\n\n{p}")),
        (Some(m), None) => Some(m.to_string()),
        (None, Some(p)) => Some(p.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::merge_prompt;

    #[test]
    fn message_only() {
        assert_eq!(merge_prompt(Some("hello"), None).as_deref(), Some("hello"));
    }

    #[test]
    fn stdin_only() {
        assert_eq!(
            merge_prompt(None, Some("piped text")).as_deref(),
            Some("piped text")
        );
    }

    #[test]
    fn message_appends_stdin_as_context() {
        assert_eq!(
            merge_prompt(Some("review this"), Some("diff --git a b")).as_deref(),
            Some("review this\n\ndiff --git a b")
        );
    }

    #[test]
    fn blank_inputs_are_ignored() {
        assert_eq!(merge_prompt(Some("   "), Some("\n\n")), None);
        assert_eq!(merge_prompt(Some(""), None), None);
        assert_eq!(merge_prompt(None, None), None);
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(merge_prompt(Some("  hi  "), None).as_deref(), Some("hi"));
    }
}
