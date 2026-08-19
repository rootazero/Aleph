//! The single content gate a **pre-scrub** live snapshot must clear before
//! anything is allowed to read it.
//!
//! [`LiveTail`](crate::sandbox::live_tail::LiveTail) holds raw pipe bytes: the
//! drain loops tee them straight off the child, before
//! [`scrub_and_gate_output`](crate::sandbox::scrub::scrub_and_gate_output) has
//! seen them. Three surfaces want those bytes —
//!
//! * `bash`'s `poll` / `wait`, rendering a mid-run progress view;
//! * the registry's `kill` / shutdown paths, recording what a job had produced
//!   before Aleph aborted it;
//! * the periodic flusher, so a job the daemon died under still has an answer
//!   to "and what had it printed?";
//!
//! — and all three owe the same floor. It lives here rather than in whichever
//! of them was written first: the moment there are two copies of "may these
//! bytes be shown", the strict one and the lax one drift, and the lax one is
//! the one that ships.
//!
//! ## The invariant
//!
//! **Nothing durable, and nothing rendered, may contain bytes the live poll
//! path would have refused.** That is what makes persisting a live tail safe at
//! all: without it, "restart the daemon and read the recovered row" becomes a
//! way to read output the finished path fails the whole call over.
//!
//! ## The residual, stated rather than hidden
//!
//! The gate sees only the bytes read *so far*, so a secret straddling the
//! current read frontier can still leak its **prefix** — the pattern cannot
//! match a value whose second half has not arrived. The completed path catches
//! the whole value and fails the call; a snapshot taken mid-write does not.
//! Shrinking that window would mean holding back the tail of every snapshot,
//! which is the same freeze this whole side channel exists to remove.

use crate::sandbox::live_tail::LiveSnapshot;
use crate::tool_output::sanitize::sanitize_command_output;

/// What a live partial snapshot is allowed to show.
pub(crate) enum PartialView {
    /// Nothing captured yet — the child has not written a byte.
    Empty,
    /// Cleared the same content floor the finished path enforces.
    Text { stdout: String, stderr: String },
    /// Block-class secret material in the raw bytes. The finished path fails
    /// the whole call on this; every partial surface refuses to render it.
    Withheld,
}

/// Text substituted for a withheld snapshot. Silence would read as "it printed
/// nothing", which is the opposite of what happened.
pub(crate) const WITHHELD_NOTE: &str =
    "[output withheld: block-class secret material was detected in this job's output, so \
     Aleph refuses to show or store it — the same refusal the completed path applies]";

/// Run the completed path's content floor over a PRE-scrub live snapshot.
///
/// Runs exactly what `WorkspaceSandbox::execute` runs on a finished command —
/// [`scrub_and_gate_output`](crate::sandbox::scrub::scrub_and_gate_output)
/// (secret redaction + block-class gate + invisible/bidi neutralisation) via a
/// throwaway [`SandboxOutput`](crate::sandbox::SandboxOutput), then
/// [`sanitize_command_output`] for ANSI/control bytes. A block-class hit makes
/// the finished call fail closed, so the partial is withheld rather than shown
/// redacted.
pub(crate) fn gate(snapshot: &LiveSnapshot) -> PartialView {
    if snapshot.stdout.is_empty() && snapshot.stderr.is_empty() {
        return PartialView::Empty;
    }
    let mut probe = crate::sandbox::SandboxOutput {
        stdout: snapshot.stdout.clone(),
        stderr: snapshot.stderr.clone(),
        ..Default::default()
    };
    if !crate::sandbox::scrub::scrub_and_gate_output(&mut probe).is_empty() {
        return PartialView::Withheld;
    }
    // BT-D-R4-18 (partial fix): detect known secret prefixes at the tail
    // of either stream. The full pattern matcher needs the second half of
    // the secret to be in the snapshot before it can identify the
    // whole value; for live output the second half has not arrived yet
    // and the prefix would otherwise leak. Refuse the snapshot when a
    // recognised provider's key prefix is in the trailing bytes; the
    // finished path's scrub-and-gate still runs on the full output
    // and withholds the same way. The list of prefixes mirrors what
    // the scrubber already detects for the finished path; extending
    // either list updates both call sites.
    if ends_with_secret_prefix(&probe.stdout) || ends_with_secret_prefix(&probe.stderr) {
        return PartialView::Withheld;
    }
    PartialView::Text {
        stdout: sanitize_command_output(&String::from_utf8_lossy(&probe.stdout)).into_owned(),
        stderr: sanitize_command_output(&String::from_utf8_lossy(&probe.stderr)).into_owned(),
    }
}

/// BT-D-R4-18 (partial fix): known-secret-prefix check applied to the
/// tail of a live partial snapshot. The list is the same set of
/// provider prefixes the finished-path scrubber redacts. A prefix
/// match at the tail means a key is being written out one byte at a
/// time and the remainder has not yet arrived; the partial is
/// withheld so the model's context window never sees a half-key.
fn ends_with_secret_prefix(bytes: &[u8]) -> bool {
    const PREFIXES: &[&[u8]] = &[
        b"sk-ant-",      // Anthropic admin / project
        b"sk-ant-api",  // Anthropic API key
        b"sk-proj-",    // OpenAI project key
        b"sk-",         // OpenAI classic (less specific; used as fallback)
        b"AKIA",        // AWS access key id
        b"ASIA",        // AWS session token
        b"ghp_",        // GitHub personal access token
        b"gho_",        // GitHub OAuth token
        b"ghs_",        // GitHub server token
        b"glpat-",      // GitLab personal access token
        b"xoxb-",       // Slack bot token
        b"xoxp-",       // Slack user token
    ];
    // Search the trailing 80 bytes only — secrets are written out
    // sequentially, so the prefix is always near the tail. Bounding
    // the search keeps the cost trivial on a multi-KB snapshot.
    const TAIL: usize = 80;
    let tail = if bytes.len() > TAIL {
        &bytes[bytes.len() - TAIL..]
    } else {
        bytes
    };
    PREFIXES.iter().any(|p| tail.windows(p.len()).any(|w| w == *p))
}

/// The gated snapshot as one block of durable text, or `None` when there is
/// nothing worth writing.
///
/// `Withheld` deliberately returns [`WITHHELD_NOTE`] rather than `None`: the
/// durable file is rewritten in place, so returning `None` would leave an older
/// (safe, but now misleading) capture sitting there as if it were current,
/// while returning the note replaces it fail-closed and says why.
pub(crate) fn durable_text(snapshot: &LiveSnapshot) -> Option<String> {
    match gate(snapshot) {
        PartialView::Empty => None,
        PartialView::Withheld => Some(WITHHELD_NOTE.to_string()),
        PartialView::Text { stdout, stderr } => {
            let mut out = String::new();
            for (label, body) in [("stdout", &stdout), ("stderr", &stderr)] {
                if body.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[{label}]\n{body}"));
            }
            (!out.is_empty()).then_some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(stdout: &str, stderr: &str) -> LiveSnapshot {
        LiveSnapshot {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_total: stdout.len() as u64,
            stderr_total: stderr.len() as u64,
        }
    }

    #[test]
    fn an_untouched_snapshot_has_nothing_to_persist() {
        assert!(durable_text(&snap("", "")).is_none());
        assert!(matches!(gate(&snap("", "")), PartialView::Empty));
    }

    #[test]
    fn both_streams_are_labelled_in_the_durable_block() {
        let text = durable_text(&snap("compiling\n", "warning: unused\n")).expect("some output");
        assert!(text.contains("[stdout]\ncompiling"), "got: {text}");
        assert!(text.contains("[stderr]\nwarning: unused"), "got: {text}");
    }

    /// The whole reason this module exists: what gets persisted is what `poll`
    /// would already have shown, never more. A block-class hit is refused on
    /// BOTH surfaces, so a restart cannot become a way around the poll refusal.
    #[test]
    fn block_class_material_is_withheld_from_the_durable_text_too() {
        // A private key header is the canonical block-class trigger: the
        // finished path fails the entire call on it.
        let raw = snap(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n",
            "",
        );
        assert!(
            matches!(gate(&raw), PartialView::Withheld),
            "the render surface must refuse this"
        );
        let text = durable_text(&raw).expect("a refusal is still a statement");
        assert_eq!(text, WITHHELD_NOTE);
        assert!(
            !text.contains("MIIEowIBAAKCAQEA"),
            "the durable copy must not carry what the render surface refused"
        );
    }

    /// ANSI escapes and control bytes are stripped on the way out, so a stored
    /// capture cannot repaint a later terminal that renders it.
    #[test]
    fn control_bytes_are_neutralised_before_the_text_is_handed_over() {
        let text = durable_text(&snap("\u{1b}[31mred\u{1b}[0m\n", "")).expect("some output");
        assert!(!text.contains('\u{1b}'), "got: {text:?}");
        assert!(text.contains("red"));
    }
}
