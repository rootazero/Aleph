//! The one place a finished background `bash` job becomes a bus event.
//!
//! Two producers announce a completion and they must not describe it
//! differently: [`bash_exec`](super::bash_exec)'s detached task, the instant the
//! job settles, and [`process_journal`](super::process_journal)'s boot handback,
//! for a completion whose notice died with the previous daemon. Building the
//! payload in each of them is how one face grows a masking gate the other does
//! not have.
//!
//! ## What this gate is for
//!
//! The consumer of [`AlephEvent::ProcessCompleted`] drives a **fresh model
//! turn** on the owning session, whose reply fans out to whatever channel that
//! session is bound to. So the reader is a later run in a context this producer
//! cannot see, and redaction cannot be deferred to it (§5.1: the same reasoning
//! that made the sub-agent sidecar mask unconditionally rather than by the
//! writing run's attendedness).
//!
//! The bytes handed in are the sandbox's finished-path output, which has already
//! been through `scrub_and_gate_output`; the [`SecretMasker`] pass here is the
//! belt, and it is the pass that picks up the operator's own
//! `[[security.mask_patterns]]`.
//!
//! ## Why a tail, and why it says so
//!
//! The notice is prompt text on a turn nobody asked for, so it is bounded — the
//! full output stays exactly one `{"process_action":"poll"}` away, and the event
//! carries the id to poll with. A tail rendered as if it were the whole output
//! is how a model concludes a build printed nothing before its last line, hence
//! [`ProcessCompletionEvent::output_truncated`].

use crate::event::{AlephEvent, GlobalBus, ProcessCompletionEvent};
use crate::exec::masker::SecretMasker;
use crate::routing::session_key::SessionKey;

/// Characters of finished output the notice carries. Roughly the journal's
/// per-line cap, and about half the live tail's 8 KiB budget: enough for a
/// build's closing summary, small enough that a turn opened by the announce is
/// mostly the model's own reasoning rather than replayed logs.
const ANNOUNCE_TAIL_CHARS: usize = 4_000;

/// Longest command preview the notice carries. Matches the registry's own
/// preview budget so `list` and the announce name a job identically.
const COMMAND_PREVIEW_CHARS: usize = 120;

/// Build the completion payload for job `id`.
///
/// `stdout` / `stderr` are the finished-path streams; they are joined with the
/// same `[stdout]` / `[stderr]` labels the durable capture uses, so a job whose
/// only output went to stderr does not read as a silent one.
pub(crate) fn completion_event(
    id: u64,
    command: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> ProcessCompletionEvent {
    let masker = SecretMasker::new();
    let mut body = String::new();
    for (label, stream) in [("stdout", stdout), ("stderr", stderr)] {
        if stream.trim().is_empty() {
            continue;
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!("[{label}]\n{stream}"));
    }
    let (output_tail, output_truncated) = keep_tail(&masker.mask(&body), ANNOUNCE_TAIL_CHARS);
    ProcessCompletionEvent {
        process_id: id,
        command: keep_head(&masker.mask(command), COMMAND_PREVIEW_CHARS),
        exit_code,
        success: exit_code == 0,
        output_tail,
        output_truncated,
    }
}

/// Same payload from an already-masked, already-bounded durable trail — the boot
/// handback's shape, where the streams were merged and labelled on their way to
/// disk and there is nothing left to split apart.
pub(crate) fn recovered_completion_event(
    id: u64,
    command: &str,
    exit_code: i32,
    recorded_output: &str,
) -> ProcessCompletionEvent {
    let masker = SecretMasker::new();
    let (output_tail, output_truncated) =
        keep_tail(&masker.mask(recorded_output), ANNOUNCE_TAIL_CHARS);
    ProcessCompletionEvent {
        process_id: id,
        command: keep_head(&masker.mask(command), COMMAND_PREVIEW_CHARS),
        exit_code,
        success: exit_code == 0,
        output_tail,
        output_truncated,
    }
}

/// Put a completion on the global bus, scoped to the session that owns the job.
///
/// The scope is what makes the announce addressable: `gateway::process_announce`
/// resolves the parent from `GlobalEvent::source_session_id`, exactly the way
/// `subagent_tool::spawn` scopes `SubAgentCompleted`. A job with no session has
/// nobody to announce to, so callers hold an `Option<SessionKey>` and simply do
/// not call this.
pub(crate) async fn broadcast(session: &SessionKey, event: ProcessCompletionEvent) {
    GlobalBus::global()
        .broadcast(
            session.agent_id(),
            &session.to_key_string(),
            AlephEvent::ProcessCompleted(event),
        )
        .await;
}

/// Last `max` chars, plus whether anything was dropped. Char-boundary safe (P7).
fn keep_tail(text: &str, max: usize) -> (String, bool) {
    let total = text.chars().count();
    if total <= max {
        return (text.to_string(), false);
    }
    (text.chars().skip(total - max).collect(), true)
}

/// First `max` chars, first line only — the registry's preview rule.
fn keep_head(text: &str, max: usize) -> String {
    let first_line = text.lines().next().unwrap_or_default();
    if first_line.chars().count() <= max {
        return first_line.to_string();
    }
    let mut head: String = first_line.chars().take(max).collect();
    head.push('…');
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_streams_are_labelled_so_a_stderr_only_job_is_not_silent() {
        let ev = completion_event(7, "cargo build", 0, "", "warning: unused\n");
        assert!(
            ev.output_tail.contains("[stderr]"),
            "got: {}",
            ev.output_tail
        );
        assert!(!ev.output_tail.contains("[stdout]"));
        assert!(ev.success);
    }

    #[test]
    fn a_long_output_keeps_the_tail_and_says_it_did() {
        let long = "x".repeat(ANNOUNCE_TAIL_CHARS * 2);
        let ev = completion_event(1, "true", 0, &format!("{long}TAIL-MARKER"), "");
        assert!(ev.output_truncated, "a cut must be declared");
        assert!(
            ev.output_tail.ends_with("TAIL-MARKER"),
            "the actionable part of a build's output is what it printed last"
        );
        assert!(ev.output_tail.chars().count() <= ANNOUNCE_TAIL_CHARS);
    }

    #[test]
    fn a_short_output_is_not_marked_truncated() {
        let ev = completion_event(1, "echo hi", 0, "hi\n", "");
        assert!(!ev.output_truncated);
        assert!(ev.output_tail.contains("hi"));
    }

    /// The command line is model-authored and routinely carries a credential.
    /// It rides into a prompt whose reply may fan out to a chat channel, so the
    /// masker runs on it too — not only on the output.
    #[test]
    fn the_command_and_the_output_both_go_through_the_masker() {
        let ev = completion_event(
            2,
            "curl -H 'Authorization: Bearer sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'",
            0,
            "token=sk-ant-api03-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB\n",
            "",
        );
        assert!(
            !ev.command.contains("sk-ant-api03-AAAA"),
            "command still carries the key: {}",
            ev.command
        );
        assert!(
            !ev.output_tail.contains("sk-ant-api03-BBBB"),
            "output still carries the key: {}",
            ev.output_tail
        );
    }

    #[test]
    fn a_failing_exit_code_is_not_reported_as_success() {
        let ev = completion_event(3, "make", 2, "", "error: no rule\n");
        assert!(!ev.success);
        assert_eq!(ev.exit_code, 2);
    }

    #[test]
    fn a_multiline_command_is_previewed_by_its_first_line() {
        let ev = completion_event(4, "set -e\nmake all\nmake test", 0, "", "");
        assert_eq!(ev.command, "set -e");
        assert!(ev.output_tail.is_empty(), "a silent job says nothing here");
    }
}
