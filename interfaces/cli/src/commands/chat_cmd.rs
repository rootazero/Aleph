//! Chat control commands (send, abort, history, clear)

use serde_json::Value;

use crate::output;
use crate::output::theme::{paint, Style};
use aleph_client::{AlephClient, CliConfig, CliResult};
use aleph_protocol::session_thread::HistoryWindow;

use super::run_follow::{self, FollowOptions};

/// Send a message via RPC (non-interactive)
pub async fn send(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    stream: bool,
    thinking: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, mut events) = AlephClient::connect(server_url, config).await?;

    let mut params = serde_json::json!({ "message": message });
    if let Some(s) = session {
        params["session_key"] = Value::String(s.to_string());
    }
    if stream {
        params["stream"] = Value::Bool(true);
    }
    if let Some(t) = thinking {
        params["thinking"] = Value::String(t.to_string());
    }

    let result: Value = client.call("chat.send", Some(params)).await?;

    if stream {
        // Follow the run live through the shared loop. Previously the flag
        // was forwarded to the server and the event stream dropped on the
        // floor, so `--stream` printed "Message sent." and exited. Pinned to
        // the dispatched run so concurrent runs on the broadcast bus don't
        // interleave.
        let verbose = std::env::var("ALEPH_VERBOSE").is_ok();
        let run_id = result
            .get("run_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let _outcome = run_follow::follow_run(
            &mut events,
            &FollowOptions {
                json,
                verbose,
                run_id,
            },
        )
        .await?;
    } else if json {
        output::print_json(&result);
    } else {
        let run_id = result.get("run_id").and_then(|v| v.as_str()).unwrap_or("-");
        let session_key = result
            .get("session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("Message sent.");
        println!("  Run ID:  {run_id}");
        println!("  Session: {session_key}");
    }

    client.close().await?;
    Ok(())
}

/// Abort a running chat
pub async fn abort(
    server_url: &str,
    run_id: &str,
    session_key: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // With a session key the abort also empties that session's wait lane.
    // Without one it stops a single run and leaves the backlog to fire the
    // moment the slot frees — see `chat.abort`'s `session_key` doc.
    let params = serde_json::json!({ "run_id": run_id, "session_key": session_key });
    let result: Value = client.call("chat.abort", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let aborted = result
            .get("aborted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if aborted {
            println!("Run '{run_id}' aborted.");
        } else {
            println!("Run '{run_id}' was not running or already completed.");
        }
        match result
            .get("dropped")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
        {
            0 => {}
            1 => println!("1 queued message dropped."),
            n => println!("{n} queued messages dropped."),
        }
    }

    client.close().await?;
    Ok(())
}

/// Show chat history for a session
pub async fn history(
    server_url: &str,
    session_key: &str,
    limit: Option<usize>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if let Some(l) = limit {
        params["limit"] = serde_json::json!(l);
    }

    let result: Value = client.call("chat.history", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        // Read through the shared contract instead of reaching for keys by
        // name: this footer used to take `count` — the length of the WINDOW —
        // and print it after the word "Total", so `--limit 20` on a 102-row
        // conversation reported "Total: 20 messages". A rename now fails here
        // rather than silently re-pointing the column.
        let window: Option<HistoryWindow> = serde_json::from_value(result.clone()).ok();
        println!(
            "{}",
            paint(Style::Header, &format!("Chat History · {session_key}"))
        );
        if let Some(messages) = result.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let ts = msg.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                println!();
                println!("{}", render_history_entry(role, ts, content));
            }
        }
        println!();
        println!("{}", paint(Style::Muted, &history_footer(window)));
    }

    client.close().await?;
    Ok(())
}

/// The one line under a transcript that says how much of the conversation the
/// reader is actually looking at.
///
/// Pure (no I/O) because that is the only seam this crate can test: it may not
/// depend on `alephcore`, so there is no way to exercise the RPC here, and a
/// test that asserts on a literal written beside it would be testing
/// `serde_json`. The cross-crate half is `alephcore`'s
/// `history_reports_the_whole_transcript_alongside_the_window_it_serves`.
///
/// Three cases, and the distinction between the last two is the point:
/// - the window IS the conversation → the old wording, now true;
/// - the window is a slice → say so, name what is missing, and say how to get
///   it, because `--limit` is the user's own doing and reversible;
/// - the server did not report a total → claim nothing. "Showing N" is honest
///   about what is on screen; "Total: N" would be a guess presented as fact,
///   which is what this function was written to remove.
fn history_footer(window: Option<HistoryWindow>) -> String {
    let Some(window) = window else {
        // The envelope did not parse — `count` is required, so this means the
        // wire shape moved. Say nothing about sizes rather than print a zero.
        return "(could not read this response's message counts)".to_string();
    };
    let count = window.count;
    match (window.above(), window.total) {
        (Some(0), _) => format!("Total: {count} {}", plural(count)),
        (Some(above), Some(total)) => format!(
            "Showing {count} of {total} {} · {above} earlier not shown \
             (raise --limit, or omit it for the whole conversation)",
            plural(total)
        ),
        // `above()` is `Some` exactly when `total` is, so this arm is the
        // no-answer case; matching on the pair rather than on `total` alone
        // keeps that from being re-derived.
        _ => format!("Showing {count} {}", plural(count)),
    }
}

/// "message" / "messages". Small, but "Total: 1 messages" is the kind of thing
/// that makes a reader distrust the number next to it.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        "message"
    } else {
        "messages"
    }
}

/// Render one history message for the human transcript view.
///
/// Pure (no I/O) so it unit-tests without a daemon. Role headers are
/// colour-coded (user = cyan, assistant = green, everything else muted);
/// assistant bodies go through the shared Markdown renderer — the same
/// pipeline `ask` uses — so a replayed answer looks like it did live. User
/// bodies print verbatim; system/tool bodies stay muted previews (they can
/// be multi-kilobyte prompts/blobs that would drown the transcript).
fn render_history_entry(role: &str, timestamp: &str, content: &str) -> String {
    let style = match role {
        "user" => Style::Info,
        "assistant" => Style::Success,
        _ => Style::Muted,
    };
    let bullet = if output::icon::use_unicode() {
        "●"
    } else {
        "*"
    };
    let mut head = paint(style, &format!("{bullet} {role}"));
    if !timestamp.is_empty() {
        head.push_str(&paint(Style::Muted, &format!(" · {timestamp}")));
    }

    let body = match role {
        "assistant" => output::markdown::render(content),
        "user" => content.to_string(),
        _ => {
            let flat = content.replace('\n', " ");
            let preview: String = flat.chars().take(200).collect();
            let suffix = if flat.chars().count() > 200 {
                "…"
            } else {
                ""
            };
            paint(Style::Muted, &format!("{preview}{suffix}"))
        }
    };

    format!("{head}\n{body}")
}

/// Clear chat history for a session
pub async fn clear(
    server_url: &str,
    session_key: &str,
    keep_system: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if keep_system {
        params["keep_system"] = Value::Bool(true);
    }

    let result: Value = client.call("chat.clear", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let cleared = result
            .get("cleared")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if cleared {
            println!("Chat history cleared for session '{session_key}'.");
        } else {
            println!("No history to clear for session '{session_key}'.");
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression: `--limit 20` against a 102-row conversation used to
    /// print "Total: 20 messages". A window is not a total.
    #[test]
    fn a_truncated_window_says_so_and_says_how_to_widen_it() {
        let footer = history_footer(Some(HistoryWindow {
            count: 20,
            total: Some(102),
        }));
        assert!(footer.contains("Showing 20 of 102 messages"), "{footer}");
        assert!(footer.contains("82 earlier not shown"), "{footer}");
        assert!(footer.contains("--limit"), "{footer}");
        assert!(
            !footer.contains("Total:"),
            "the window must never be labelled the total: {footer}"
        );
    }

    /// When the window IS the conversation the old wording is correct, so it
    /// stays — the fix is not to stop reporting a total, it is to stop
    /// reporting the window as one.
    #[test]
    fn a_complete_window_is_still_called_a_total() {
        let footer = history_footer(Some(HistoryWindow {
            count: 7,
            total: Some(7),
        }));
        assert_eq!(footer, "Total: 7 messages");
        assert_eq!(
            history_footer(Some(HistoryWindow { count: 1, total: Some(1) })),
            "Total: 1 message"
        );
    }

    /// A core that does not report `total` must leave the reader knowing only
    /// what is on screen. Printing "Total: 20" here is the original defect
    /// with a different cause.
    #[test]
    fn no_answer_from_the_server_claims_nothing() {
        let footer = history_footer(Some(HistoryWindow {
            count: 20,
            total: None,
        }));
        assert_eq!(footer, "Showing 20 messages");
        assert!(!footer.contains("Total"), "{footer}");
    }

    /// Two reads of a growing store can put the window ahead of the count; the
    /// footer must read that as complete, not as a negative remainder.
    #[test]
    fn a_window_ahead_of_the_count_reads_as_complete() {
        let footer = history_footer(Some(HistoryWindow {
            count: 5,
            total: Some(4),
        }));
        assert_eq!(footer, "Total: 5 messages");
    }

    /// An envelope that will not parse is a moved wire shape. Say that, rather
    /// than print a zero that reads as an empty conversation.
    #[test]
    fn an_unreadable_envelope_prints_no_number_at_all() {
        let footer = history_footer(None);
        assert!(!footer.contains('0'), "{footer}");
        assert!(footer.contains("could not read"), "{footer}");
    }

    #[test]
    fn history_entry_renders_role_header_and_body() {
        let entry = render_history_entry("user", "2026-06-10T12:00:00Z", "hello there");
        assert!(entry.contains("user"));
        assert!(entry.contains("2026-06-10T12:00:00Z"));
        assert!(entry.contains("hello there"));
    }

    #[test]
    fn history_entry_keeps_assistant_body_intact() {
        // Assistant content goes through the Markdown renderer — long bodies
        // must NOT be truncated (the old transcript cut everything at 200
        // chars, which made `chat-control history` useless as a replay).
        let long = "word ".repeat(100);
        let entry = render_history_entry("assistant", "", &long);
        assert!(entry.matches("word").count() >= 100);
    }

    #[test]
    fn history_entry_previews_system_messages() {
        let blob = "x".repeat(500);
        let entry = render_history_entry("system", "", &blob);
        // Muted preview keeps the transcript readable: capped + ellipsised.
        assert!(entry.chars().filter(|c| *c == 'x').count() <= 200);
        assert!(entry.contains('…'));
    }
}
