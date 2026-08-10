use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Child;

use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::live_tail::{LiveStream, LiveTail};

/// How long to wait for the stdout/stderr reader tasks to drain after we
/// kill a child that hit its wall-clock timeout. Matches codex's
/// `IO_DRAIN_TIMEOUT_MS = 2_000` — a grandchild process inheriting our
/// pipes can hold them open indefinitely, so we cap how long we'll wait
/// for "real" output before giving up and returning what we have.
const KILL_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub const LINUX_PLATFORM_DEFAULT_READ_ROOTS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];

#[must_use]
pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .is_ok_and(|content| content.to_lowercase().contains("microsoft"))
}

#[must_use]
pub fn wsl_version() -> Option<u32> {
    if !is_wsl() {
        return None;
    }

    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|content| if content.contains("WSL2") { 2 } else { 1 })
}

// NOTE (2026-08-10, entropy sweep): `normalize_path_for_sandbox`,
// `path_is_allowed` and `glob_to_regex` used to live here. All three were
// pre-`deny_globs` scaffolding with zero production consumers — only their
// own unit tests. They are deleted rather than wired, deliberately:
//
// * `glob_to_regex` was a semantically *weaker* twin of the live
//   [`crate::sandbox::deny_globs::glob_to_anchored_regex`]: it mapped `**`
//   to `.*` without whole-component consumption, escaped `[`/`]` instead of
//   preserving character classes, and had no bare-pattern-subtree rule.
//   Wiring it into a future landlock path (which `deny_globs.rs` invites)
//   would have produced a silently *weaker deny floor* that passed its own
//   tests — the worst shape a security predicate can have.
// * `path_is_allowed` was a bare uncanonicalized `starts_with`, i.e. exactly
//   the Windows `\\?\` / display-form hazard the root CLAUDE.md warns about,
//   shipped as public API where it invited being mistaken for the sandbox
//   path gate.
//
// The live answers live in two different places, and the split is the point:
//   * glob → regex, for the OS deny floor:
//     [`crate::sandbox::deny_globs::glob_to_anchored_regex`], consumed by
//     `deny_globs::resolve_deny_read_paths_under` → seatbelt / AppContainer.
//   * "is this path denied", for the model's own file tools:
//     [`crate::builtin_tools::file_ops::path_utils::path_is_denied`] and its
//     upward twin `contains_denied_descendant`.
// Do not re-add a translator or a containment predicate here.
// (`approval/config.rs::glob_to_regex_str` is a third, deliberate translator
// for the approval-rule domain and is out of scope of this note.)

/// Truncate a **contiguous** captured buffer so the retained content is at most
/// `max_bytes`, never cutting a UTF-8 codepoint in half (project rule P7).
/// Returns the (possibly rewritten) buffer and the number of bytes elided
/// from the original (0 when no truncation happened).
///
/// When the buffer overflows the budget we keep a **head + tail** slice with
/// an inline elision marker between them, rather than only the head. Shell
/// output buries the most important line — the final error, the test summary,
/// the failing assertion — at the *end*; head-only truncation silently dropped
/// exactly that. The split is 40% head / 60% tail (hermes parity), tail-weighted
/// because the recent output is usually what the model needs. Both cut points
/// are backed off / advanced past any UTF-8 continuation byte (`0b10xx_xxxx`)
/// so neither fragment splits a multi-byte char. The returned drop count is the
/// number of bytes elided from the middle (the marker text is not counted).
///
/// Tiny budgets (`< MIN_HEAD_TAIL_CAP`) and degenerate splits fall back to the
/// original head-only truncation — see [`truncate_head_only`].
///
/// **Contiguity is a precondition, not a detail.** This is only reachable for a
/// stream that fit entirely inside the drain's head buffer; a stream that
/// overflowed it already has a gap in the middle, and splicing a second gap
/// into it would make the one marker report a number that is not the whole
/// truth. That case is [`Drained::assemble`]'s, which owns both fragments and
/// emits exactly one marker over the exact total.
#[must_use]
pub fn truncate_output(buf: Vec<u8>, max_bytes: usize) -> (Vec<u8>, u64) {
    let orig_len = buf.len();
    if orig_len <= max_bytes {
        return (buf, 0);
    }
    // Tiny budgets keep the original head-only behaviour: a head+tail split
    // with an elision marker only carries signal once the cap is large enough
    // for both fragments to be meaningful.
    if max_bytes < MIN_HEAD_TAIL_CAP {
        return truncate_head_only(buf, max_bytes);
    }

    // Split the budget 40% head / 60% tail. The head preserves the command's
    // opening (banner, the first error a build prints); the larger tail keeps
    // the most recent output — the final error / test summary that head-only
    // truncation silently dropped. For a failing `cargo build` that emits
    // megabytes of progress before erroring, head-only would hand the model all
    // the progress and LOSE the actual error at the end.
    let (head_budget, tail_budget) = head_tail_budgets(max_bytes);
    let head_end = back_off_to_boundary(&buf, head_budget);
    let tail_start = advance_to_boundary(&buf, orig_len - tail_budget);

    // If the fragments meet or overlap (cap nearly equals len), nothing is
    // actually elided — fall back to head-only to avoid an empty/negative gap.
    if tail_start <= head_end {
        return truncate_head_only(buf, max_bytes);
    }

    let dropped = (tail_start - head_end) as u64;
    let marker = elision_marker(dropped);

    let mut out = Vec::with_capacity(head_end + marker.len() + (orig_len - tail_start));
    out.extend_from_slice(&buf[..head_end]);
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&buf[tail_start..]);
    (out, dropped)
}

/// Cap below which a head+tail split degrades to head-only. Below this the
/// elision marker plus two fragments would carry less signal than a single
/// contiguous head slice. Honoured by both [`truncate_output`] and
/// [`Drained::assemble`], because a budget too small to split is too small to
/// split no matter which of them is doing the cutting.
const MIN_HEAD_TAIL_CAP: usize = 256;

/// The 40/60 head/tail division of a retention budget. One function so the two
/// call sites cannot drift into two different splits.
const fn head_tail_budgets(max_bytes: usize) -> (usize, usize) {
    let head = max_bytes * 2 / 5;
    (head, max_bytes - head)
}

/// The marker spliced between a head fragment and a tail fragment. Its byte
/// count is deliberately NOT included in the reported drop count — the number
/// describes the output, not the annotation.
fn elision_marker(dropped: u64) -> String {
    format!("\n…[{dropped} bytes elided]…\n")
}

/// Largest index `<= limit` that does not sit inside a multi-byte sequence.
/// Used for a cut whose *left* side is kept. A `limit` at or past the end is
/// returned as the end: cutting there splits nothing, and indexing `buf[len]`
/// to discover that would panic.
fn back_off_to_boundary(buf: &[u8], limit: usize) -> usize {
    if limit >= buf.len() {
        return buf.len();
    }
    let mut end = limit;
    while end > 0 && (buf[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    end
}

/// Smallest index `>= from` that starts a codepoint. Used for a cut whose
/// *right* side is kept.
fn advance_to_boundary(buf: &[u8], from: usize) -> usize {
    let mut start = from.min(buf.len());
    while start < buf.len() && (buf[start] & 0xC0) == 0x80 {
        start += 1;
    }
    start
}

/// Head-only truncation: keep the first `max_bytes`, backing off any UTF-8
/// continuation byte so a multi-byte codepoint is never split. Used for tiny
/// budgets and the degenerate case where a head+tail split would not actually
/// elide anything. Returns the (possibly shortened) buffer and the number of
/// bytes dropped (0 when no truncation happened).
fn truncate_head_only(mut buf: Vec<u8>, max_bytes: usize) -> (Vec<u8>, u64) {
    let orig_len = buf.len();
    if orig_len <= max_bytes {
        return (buf, 0);
    }
    let end = back_off_to_boundary(&buf, max_bytes);
    buf.truncate(end);
    let dropped = (orig_len - end) as u64;
    (buf, dropped)
}

/// The Unix signal that terminated a child process, if it was killed by a
/// signal rather than exiting normally. `None` for a normal exit. Used to
/// populate `SandboxOutput.signal` so callers can distinguish a SIGSEGV /
/// rlimit-or-cgroup SIGKILL from a clean non-zero exit.
#[cfg(unix)]
#[must_use]
pub fn termination_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

/// Cross-platform variant of [`termination_signal`] — always `None` on
/// Windows where `ExitStatus` has no signal concept. Used inside helpers
/// that need to compile on all platforms.
#[cfg(unix)]
fn termination_signal_xplat(status: &std::process::ExitStatus) -> Option<i32> {
    termination_signal(status)
}

#[cfg(not(unix))]
fn termination_signal_xplat(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// What one drain loop retained from a stream.
///
/// Two disjoint fragments plus an exact byte total, never one head-shaped slab.
/// The gap between them is `total - head.len() - tail.len()`, which is the
/// number the model is owed and the reason this is a struct rather than a
/// `(Vec<u8>, u64)` pair: a caller handed only "what was kept" cannot tell a
/// contiguous buffer from one with a hole in it, and would splice a second
/// elision marker reporting the wrong count.
#[derive(Default)]
struct Drained {
    /// First `head_cap` bytes of the stream.
    head: Vec<u8>,
    /// The most recent bytes past the head, oldest first, at most `tail_cap`.
    /// Empty exactly when the whole stream fit in `head`.
    tail: Vec<u8>,
    /// Every byte the loop read, retained or not.
    total: u64,
}

impl Drained {
    /// Render the retained fragments into one `max_bytes` buffer, plus the
    /// number of bytes elided from the **original stream** (not merely from
    /// what survived the drain).
    ///
    /// Contiguous input takes [`truncate_output`] unchanged — that is the
    /// common case and its behaviour is byte-identical to before this struct
    /// existed. A drain-level gap takes the branch below, which cuts each
    /// fragment to the same 40/60 budget and emits exactly ONE marker whose
    /// count spans both holes at once.
    fn assemble(self, max_bytes: usize) -> (Vec<u8>, u64) {
        let Self { head, tail, total } = self;
        if total <= head.len() as u64 {
            // The head IS the stream: nothing was elided on the way in.
            return truncate_output(head, max_bytes);
        }
        // A budget too small to carry two fragments plus a marker keeps the
        // opening only — same rule `truncate_output` applies, same constant.
        if max_bytes < MIN_HEAD_TAIL_CAP {
            let (out, _) = truncate_head_only(head, max_bytes);
            let dropped = total.saturating_sub(out.len() as u64);
            return (out, dropped);
        }
        let (head_budget, tail_budget) = head_tail_budgets(max_bytes);
        let head_end = back_off_to_boundary(&head, head_budget);
        // The tail window starts at an arbitrary byte offset in the stream, so
        // its own front can already be mid-codepoint; advancing covers both
        // that and the budget cut in one walk.
        let tail_from = advance_to_boundary(&tail, tail.len().saturating_sub(tail_budget));
        let kept = head_end + (tail.len() - tail_from);
        let dropped = total.saturating_sub(kept as u64);
        let marker = elision_marker(dropped);
        let mut out = Vec::with_capacity(kept + marker.len());
        out.extend_from_slice(&head[..head_end]);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&tail[tail_from..]);
        (out, dropped)
    }
}

/// Read `pipe` to EOF, retaining the first `head_cap` bytes and a rolling
/// window of the most recent `tail_cap` bytes past them, and counting every
/// byte that flowed through.
///
/// **Head AND tail, because the end is where the answer is.** An earlier
/// version kept a single head-shaped buffer and discarded everything past it.
/// `truncate_output` then took its "tail" from the end of *that buffer* — so
/// for any stream over the ceiling (8 MiB at the default `max_output_bytes`)
/// the model was handed the tail of the first 8 MiB and the actual final error
/// was already gone before truncation ran. A `cargo build` that fails after
/// emitting 30 MiB of progress is precisely the shape this tool exists for, and
/// it was precisely the shape that lost its error message. The two windows sum
/// to the same ceiling as the old single buffer, so peak memory is unchanged.
///
/// When `tee` is present every chunk is *also* pushed into a rolling
/// [`LiveTail`] ring so a still-running background job can be polled for
/// partial output. The ring is not redundant with the tail window: this
/// function's fragments only materialise when the child exits, whereas the ring
/// is readable mid-run. `None` (the foreground path) skips the tee entirely.
///
/// Reading to EOF is the whole point: an earlier version wrapped the pipe in
/// `take(ceiling)`, which stops reading at the ceiling and then DROPS the read
/// end. The child's next write gets EPIPE/SIGPIPE and dies — so a command was
/// killed for the crime of being verbose, and the model was handed a signalled
/// exit instead of truncated output. A `cargo build -v`, a big `git log -p`, a
/// full test run all clear 8 MB of stdout routinely; killing them mid-flight
/// also abandons whatever side effects they were partway through.
///
/// The runaway guard is, and always was, the wall clock in
/// [`run_child_with_drain`] — not the pipe filling up. Draining costs one
/// read loop into a fixed buffer; the producer burns the same CPU it would
/// burn writing to a file.
async fn drain_bounded<R>(
    pipe: Option<R>,
    head_cap: usize,
    tail_cap: usize,
    tee: Option<(Arc<LiveTail>, LiveStream)>,
) -> Drained
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut pipe) = pipe else {
        return Drained::default();
    };
    let mut head: Vec<u8> = Vec::new();
    let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut total: u64 = 0;
    let mut chunk = [0u8; 16 * 1024];
    // A read error ends the loop just like EOF: the pipe is unusable either
    // way, and what was already read stays valid (matches the previous
    // `let _ = ...read_to_end(...)`).
    while let Ok(n) = pipe.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let bytes = &chunk[..n];
        if let Some((live, stream)) = &tee {
            live.push(*stream, bytes);
        }
        total = total.saturating_add(n as u64);
        // Fill the head first; only what will not fit there reaches the
        // rolling window, so the two never hold the same byte twice and
        // `total - head - tail` is the exact size of the hole between them.
        let take = head_cap.saturating_sub(head.len()).min(n);
        head.extend_from_slice(&bytes[..take]);
        let overflow = &bytes[take..];
        if !overflow.is_empty() && tail_cap > 0 {
            tail.extend(overflow.iter().copied());
            if tail.len() > tail_cap {
                tail.drain(..tail.len() - tail_cap);
            }
        }
    }
    Drained {
        head,
        tail: tail.into(),
        total,
    }
}

/// Spawn stdout/stderr reader tasks, optionally pipe `stdin_data`, then
/// race the child's `wait()` against `timeout`.
///
/// On natural exit: returns a `SandboxOutput` with both streams truncated
/// to `max_output_bytes` and the dropped byte counts surfaced.
///
/// On timeout: kills the child explicitly (so we don't rely on the
/// caller's `kill_on_drop` flag firing later), drains the reader tasks
/// for up to [`KILL_DRAIN_TIMEOUT`], and returns
/// `SandboxError::Timeout { elapsed_ms, partial_stdout, partial_stderr }`.
/// Partial buffers may be empty if a grandchild was holding the pipes
/// open longer than the drain budget.
///
/// While the child runs, both drain loops tee what they read into the
/// [`LIVE_TAIL`](crate::sandbox::context::LIVE_TAIL) task-local's ring when one
/// is in scope, so a backgrounded job can be polled for partial output instead
/// of staying opaque until it exits. Those bytes are PRE-scrub — see
/// [`LiveTail`] — and every reader owes them the same scrub-and-gate floor the
/// finished path runs below.
///
/// Used by every platform driver (seatbelt / bwrap / windows) so the
/// kill-and-drain logic only lives in one place.
pub async fn run_child_with_drain(
    mut child: Child,
    stdin_data: Option<&[u8]>,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<SandboxOutput, SandboxError> {
    // Always close the child's stdin so a non-interactive command that tries
    // to READ from stdin — a git credential prompt, `sudo -S`, an `apt`
    // confirmation, a bare `read`/`cat` — gets an immediate EOF instead of
    // blocking on a pipe nothing will ever write to until the wall-clock
    // timeout SIGKILLs it (turning a 1 ms command into a full-timeout stall).
    // The drivers all spawn with `Stdio::piped()`, so absent this the parent
    // holds the write end open for the child's whole life. When `stdin_data`
    // is present we feed it first; the trailing drop closes the pipe either
    // way, so the child always sees a clean EOF. (pi wires stdin from
    // /dev/null; kimi closes it explicitly — same guarantee, one place.)
    if let Some(mut child_stdin) = child.stdin.take() {
        if let Some(data) = stdin_data {
            use tokio::io::AsyncWriteExt;
            child_stdin
                .write_all(data)
                .await
                .map_err(|e| SandboxError::Io(format!("stdin write failed: {e}")))?;
        }
        // Drop closes stdin so the child sees EOF (an empty stdin when no data).
        drop(child_stdin);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Buffer PAST the keep-budget so the assembly step can both preserve a
    // head+tail slice AND report how many bytes were elided — the
    // `*_truncated_bytes` contract surfaced to the model (see `code_exec`).
    // Memory stays bounded: at most `DRAIN_BUFFER_FACTOR * max_output_bytes` is
    // ever held for a runaway child, split evenly between the opening bytes and
    // a rolling window on the most recent ones; everything between the two is
    // read and counted but not kept.
    const DRAIN_BUFFER_FACTOR: u64 = 8;
    let buffer_ceiling = (max_output_bytes as u64).saturating_mul(DRAIN_BUFFER_FACTOR);
    let head_cap = usize::try_from(buffer_ceiling / 2).unwrap_or(usize::MAX);
    let tail_cap = usize::try_from(buffer_ceiling - buffer_ceiling / 2).unwrap_or(usize::MAX);

    // Read the live-tail task-local HERE, on the caller's task: `tokio::spawn`
    // does not carry task-locals into the spawned future, so each drain task
    // has to be handed an owned clone instead of looking it up itself.
    // `None` (foreground) leaves both loops byte-identical to their pre-tee form.
    let live = crate::sandbox::context::current_live_tail();
    let stdout_task = tokio::spawn(drain_bounded(
        stdout,
        head_cap,
        tail_cap,
        live.clone().map(|t| (t, LiveStream::Stdout)),
    ));
    let stderr_task = tokio::spawn(drain_bounded(
        stderr,
        head_cap,
        tail_cap,
        live.map(|t| (t, LiveStream::Stderr)),
    ));

    let start = Instant::now();
    let wait_result = tokio::time::timeout(timeout, child.wait()).await;
    let elapsed_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    match wait_result {
        Ok(Ok(status)) => {
            // `assemble` reports against the stream total, so the count the
            // model reads is the whole overflow — the bytes elided between the
            // retained fragments AND the ones that never fit in either window.
            let (stdout, stdout_dropped) = stdout_task
                .await
                .unwrap_or_default()
                .assemble(max_output_bytes);
            let (stderr, stderr_dropped) = stderr_task
                .await
                .unwrap_or_default()
                .assemble(max_output_bytes);
            Ok(SandboxOutput {
                stdout,
                stderr,
                exit_code: status.code(),
                signal: termination_signal_xplat(&status),
                truncated: stdout_dropped > 0 || stderr_dropped > 0,
                stdout_truncated_bytes: stdout_dropped,
                stderr_truncated_bytes: stderr_dropped,
                duration_ms: elapsed_ms,
            })
        }
        Ok(Err(e)) => Err(SandboxError::ExecutionFailed(format!("wait error: {e}"))),
        Err(_) => {
            // Wall-clock fired. Force-kill the child immediately rather
            // than relying on `kill_on_drop` firing when we return.
            let _ = child.start_kill();
            let drain = tokio::time::timeout(KILL_DRAIN_TIMEOUT, async {
                let out = stdout_task.await.unwrap_or_default();
                let err = stderr_task.await.unwrap_or_default();
                (out, err)
            })
            .await;
            let (partial_stdout, partial_stderr) =
                drain.unwrap_or_else(|_| (Drained::default(), Drained::default()));
            // Cap partial output too — a runaway loop can fill the pipe with
            // megabytes before the kill takes effect. The tail matters most
            // here: a script killed at its wall clock was doing something at
            // the moment it died, and that is the last thing it printed.
            let (partial_stdout, _) = partial_stdout.assemble(max_output_bytes);
            let (partial_stderr, _) = partial_stderr.assemble(max_output_bytes);
            Err(SandboxError::Timeout {
                elapsed_ms,
                partial_stdout,
                partial_stderr,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_platform_defaults_not_empty() {
        assert!(!LINUX_PLATFORM_DEFAULT_READ_ROOTS.is_empty());
        assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/usr"));
        assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/bin"));
    }

    #[test]
    fn truncate_output_keeps_short_buffer_intact() {
        let (out, dropped) = truncate_output(b"hello".to_vec(), 1024);
        assert_eq!(out, b"hello");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn truncate_output_at_exact_len_does_not_truncate() {
        let (out, dropped) = truncate_output(b"hello".to_vec(), 5);
        assert_eq!(out, b"hello");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn truncate_output_cuts_ascii_at_boundary() {
        let (out, dropped) = truncate_output(b"hello world".to_vec(), 5);
        assert_eq!(out, b"hello");
        assert_eq!(dropped, 6, "' world' is 6 bytes dropped");
    }

    #[test]
    fn truncate_output_never_splits_a_multibyte_codepoint() {
        // "a€b" = 61 E2 82 AC 62 — the euro sign is a 3-byte sequence.
        let buf = "a€b".as_bytes().to_vec();
        // Cutting at 2 lands inside the euro sign → must back off to "a".
        let (out, dropped) = truncate_output(buf.clone(), 2);
        assert_eq!(out, b"a");
        // Originally 5 bytes ("a€b"), kept 1, so 4 dropped.
        assert_eq!(dropped, 4);
        assert!(
            std::str::from_utf8(&out).is_ok(),
            "result must stay valid UTF-8"
        );
        // Cutting at 4 lands exactly after the euro sign → "a€" kept whole.
        let (out, dropped) = truncate_output(buf, 4);
        assert_eq!(std::str::from_utf8(&out).unwrap(), "a€");
        // Originally 5 bytes, kept 4, so just the trailing "b" dropped.
        assert_eq!(dropped, 1);
    }

    #[test]
    fn truncate_output_zero_max_yields_empty() {
        let (out, dropped) = truncate_output(b"x".to_vec(), 0);
        assert!(out.is_empty());
        assert_eq!(dropped, 1);
    }

    #[test]
    fn truncate_output_keeps_head_and_tail_above_the_cap() {
        // Above MIN_HEAD_TAIL_CAP we keep the opening AND the ending — the
        // final lines (the real error / test summary) survive, which head-only
        // truncation dropped. Build a buffer where the start and end are
        // distinguishable so we can assert both are present.
        let cap = 1000usize;
        let head: String = "HEAD-".repeat(200); // 1000 bytes of head marker
        let tail: String = "-TAIL".repeat(200); // 1000 bytes of tail marker
        let mut buf = head.into_bytes();
        buf.extend_from_slice(&vec![b'x'; 5000]); // 5 KB of filler in the middle
        buf.extend_from_slice(tail.as_bytes());
        let orig_len = buf.len();

        let (out, dropped) = truncate_output(buf, cap);
        let text = std::str::from_utf8(&out).expect("stays valid UTF-8");

        // Head fragment (40% = 400 bytes) and tail fragment (60% = 600 bytes)
        // both present; the elided middle is gone.
        assert!(
            text.starts_with("HEAD-"),
            "head opening must survive: {text:.40}"
        );
        assert!(text.ends_with("-TAIL"), "tail ending must survive");
        assert!(
            text.contains("bytes elided"),
            "marker must announce the gap"
        );
        assert!(dropped > 0, "the middle must be reported as dropped");
        // Retained real content (excluding the marker) honours the budget.
        assert!(
            out.len() <= cap + 64,
            "retained content ~ cap + small marker"
        );
        assert_eq!(
            dropped as usize,
            orig_len - 400 - 600,
            "dropped == middle between the 400-byte head and 600-byte tail"
        );
    }

    #[test]
    fn truncate_output_head_tail_never_splits_multibyte() {
        // Fill head and tail regions with 3-byte euro signs so a naive byte
        // cut would land mid-codepoint; the result must stay valid UTF-8.
        let cap = 300usize; // just above MIN_HEAD_TAIL_CAP
        let buf = "€".repeat(1000).into_bytes(); // 3000 bytes, all multibyte
        let (out, _dropped) = truncate_output(buf, cap);
        assert!(
            std::str::from_utf8(&out).is_ok(),
            "head+tail split must not split a multibyte codepoint"
        );
    }

    // ---- Drained::assemble ---------------------------------------------------

    /// Contiguous input must behave exactly as it did before `Drained` existed:
    /// straight through `truncate_output`, one marker, same accounting.
    #[test]
    fn assemble_without_a_drain_gap_is_plain_truncation() {
        let buf = b"HEAD"
            .iter()
            .copied()
            .cycle()
            .take(4000)
            .collect::<Vec<_>>();
        let total = buf.len() as u64;
        let drained = Drained {
            head: buf.clone(),
            tail: Vec::new(),
            total,
        };
        let (out, dropped) = drained.assemble(1000);
        let (want, want_dropped) = truncate_output(buf, 1000);
        assert_eq!(out, want);
        assert_eq!(dropped, want_dropped);
    }

    /// The regression this whole change is about: the bytes the drain never
    /// buffered must still be counted, and the fragment shown as the "tail"
    /// must come from the END of the stream, not from the end of the head
    /// window. Exactly one marker, spanning both holes at once.
    #[test]
    fn assemble_reports_the_whole_hole_and_keeps_the_real_tail() {
        let drained = Drained {
            head: vec![b'H'; 4096],
            tail: vec![b'T'; 4096],
            // 4096 head + 4096 tail retained out of a 1,000,000-byte stream:
            // 991,808 bytes never touched either window.
            total: 1_000_000,
        };
        let (out, dropped) = drained.assemble(1000);
        let text = String::from_utf8(out).expect("ASCII in, ASCII out");

        assert!(text.starts_with("HHHH"), "the opening must survive");
        assert!(
            text.ends_with("TTTT"),
            "the tail fragment must come from the end of the STREAM: {:?}",
            &text[text.len().saturating_sub(40)..]
        );
        assert_eq!(
            dropped,
            1_000_000 - 1000,
            "everything not retained is reported, including what the drain \
             discarded between the two windows"
        );
        assert_eq!(
            text.matches("bytes elided").count(),
            1,
            "one hole, one marker — two would each report a partial truth"
        );
        assert!(
            text.contains(&format!("[{dropped} bytes elided]")),
            "the marker must carry the whole-stream count: {text:.200}"
        );
    }

    /// Both fragment cuts land on codepoint boundaries, and the tail window's
    /// own front (an arbitrary byte offset into the stream) is advanced past
    /// too — a ring cut and a budget cut in one walk.
    #[test]
    fn assemble_never_splits_a_multibyte_codepoint() {
        let euro = "€".as_bytes();
        // A tail window that itself starts mid-codepoint: drop the first byte.
        let mut tail: Vec<u8> = euro.iter().copied().cycle().take(3000).collect();
        tail.remove(0);
        let drained = Drained {
            head: euro.iter().copied().cycle().take(3000).collect(),
            tail,
            total: 500_000,
        };
        let (out, _) = drained.assemble(1000);
        let text = std::str::from_utf8(&out).expect("both cuts land on boundaries");
        assert!(text.starts_with('€'));
        assert!(text.ends_with('€'));
    }

    /// A budget too small for two fragments keeps the opening only — the same
    /// rule `truncate_output` applies, and the count still spans the stream.
    #[test]
    fn assemble_with_a_tiny_budget_keeps_the_head_and_still_counts_everything() {
        let drained = Drained {
            head: vec![b'H'; 4096],
            tail: vec![b'T'; 4096],
            total: 100_000,
        };
        let (out, dropped) = drained.assemble(64);
        assert_eq!(out, vec![b'H'; 64]);
        assert_eq!(dropped, 100_000 - 64);
    }

    #[cfg(unix)]
    #[test]
    fn termination_signal_reports_killed_child() {
        // A child killed by a signal must surface that signal, and have
        // no normal exit code — this is what BUG-9 wires into
        // SandboxOutput.signal.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        child.kill().expect("kill child");
        let status = child.wait().expect("wait for child");
        assert_eq!(termination_signal(&status), Some(9), "SIGKILL is signal 9");
        assert_eq!(status.code(), None, "a signalled process has no exit code");
    }

    #[cfg(unix)]
    #[test]
    fn termination_signal_is_none_for_clean_exit() {
        let status = std::process::Command::new("true")
            .status()
            .expect("run /usr/bin/true");
        assert_eq!(termination_signal(&status), None);
        assert_eq!(status.code(), Some(0));
    }

    // ---- run_child_with_drain ------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_natural_exit_captures_output() {
        use tokio::process::Command;
        let child = Command::new("bash")
            .arg("-c")
            .arg("echo hello; echo oops 1>&2; exit 0")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let out = run_child_with_drain(child, None, Duration::from_secs(5), 1024)
            .await
            .expect("natural exit");
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops\n");
        assert!(!out.truncated);
        assert_eq!(out.stdout_truncated_bytes, 0);
        assert_eq!(out.stderr_truncated_bytes, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_timeout_captures_partial_output() {
        use tokio::process::Command;
        // Emit *bulk* output, then hang so the wall-clock timeout trips and the
        // drain captures the buffered prefix. Two macOS-specific pitfalls were
        // hit getting here:
        //   1. `echo started; exec 1>&-` — macOS bash (3.2) block-buffers a
        //      builtin echo to a pipe and `exec 1>&-` closed the fd discarding
        //      the unflushed buffer, so partial_stdout came back empty.
        //   2. `yes started | head -n 1000; sleep 30` — `head` exits, but the
        //      trailing `sleep` is a *separate child* of bash that inherits the
        //      stdout pipe. start_kill() SIGKILLs bash; the orphaned `sleep`
        //      keeps the pipe write-end open, so the drain's `read_to_end`
        //      never sees EOF and KILL_DRAIN_TIMEOUT expires → empty again.
        // `exec sleep 30` makes the child process *itself* hold stdout, so the
        // SIGKILL closes the pipe immediately and the drain returns the
        // already-buffered ~1000 lines. `head` (a real binary) flushes via
        // libc so the data reliably lands in the pipe on every platform.
        let child = Command::new("bash")
            .arg("-c")
            .arg("yes started | head -n 1000; exec sleep 30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let err = run_child_with_drain(child, None, Duration::from_secs(3), 1024)
            .await
            .expect_err("should time out");
        match err {
            SandboxError::Timeout {
                elapsed_ms,
                partial_stdout,
                partial_stderr,
            } => {
                assert!(elapsed_ms >= 400, "elapsed_ms = {elapsed_ms}");
                // The drain captured the buffered stream prefix (truncated to
                // max_output_bytes); every line is "started".
                assert!(
                    !partial_stdout.is_empty(),
                    "partial stdout must be drained after kill"
                );
                assert!(
                    String::from_utf8_lossy(&partial_stdout).starts_with("started\n"),
                    "partial stdout = {:?}",
                    String::from_utf8_lossy(&partial_stdout)
                );
                assert!(partial_stderr.is_empty());
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_stdin_pipe_works() {
        use tokio::process::Command;
        let child = Command::new("bash")
            .arg("-s")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let script = b"echo from-stdin\n";
        let out = run_child_with_drain(child, Some(script), Duration::from_secs(5), 1024)
            .await
            .expect("natural exit");
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "from-stdin\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_closes_stdin_so_reads_get_eof() {
        use tokio::process::Command;
        // `cat` with no file args reads its stdin until EOF. Before the fix the
        // child's stdin pipe stayed open (nothing ever wrote to or closed it),
        // so this hung for the whole timeout window and came back as a
        // `SandboxError::Timeout`. Now stdin is closed immediately when there's
        // no `stdin_data`, so `cat` sees EOF and exits 0 in milliseconds. The
        // generous 20s timeout is only a backstop — a regression would trip it
        // (Err) and fail the `.expect`, never a false pass.
        let child = Command::new("bash")
            .arg("-c")
            .arg("cat")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let out = run_child_with_drain(child, None, Duration::from_secs(20), 1024)
            .await
            .expect("stdin-reading command must exit on EOF, not hang to timeout");
        assert_eq!(out.exit_code, Some(0), "cat exits 0 on a clean EOF");
        assert!(out.stdout.is_empty(), "empty stdin → no stdout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_records_truncation_bytes() {
        use tokio::process::Command;
        // Produce 200 bytes of stdout; cap at 50 → 150 bytes dropped.
        let child = Command::new("bash")
            .arg("-c")
            .arg("printf 'x%.0s' {1..200}")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let out = run_child_with_drain(child, None, Duration::from_secs(5), 50)
            .await
            .expect("natural exit");
        assert_eq!(out.stdout.len(), 50);
        assert_eq!(out.stdout_truncated_bytes, 150);
        assert!(out.truncated);
    }

    /// The tee must reach BOTH drain loops through the task-local, and it must
    /// see the whole stream — not the head-shaped slice `kept` retains. Without
    /// the `LIVE_TAIL` scope the same run must leave the tail untouched.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_tees_both_streams_into_the_live_tail() {
        use crate::sandbox::context::LIVE_TAIL;
        use tokio::process::Command;

        let spawn = || {
            Command::new("bash")
                .arg("-c")
                .arg("echo out-line; echo err-line 1>&2")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("spawn bash")
        };

        let tail = Arc::new(LiveTail::new());
        let scoped = tail.clone();
        LIVE_TAIL
            .scope(scoped, async {
                run_child_with_drain(spawn(), None, Duration::from_secs(20), 1024)
                    .await
                    .expect("natural exit");
            })
            .await;
        let snap = tail.snapshot();
        assert_eq!(String::from_utf8_lossy(&snap.stdout), "out-line\n");
        assert_eq!(String::from_utf8_lossy(&snap.stderr), "err-line\n");
        assert_eq!(snap.stdout_total, 9);
        assert_eq!(snap.stderr_total, 9);

        // No scope ⇒ no tee: the foreground path must not pay for this.
        let untouched = Arc::new(LiveTail::new());
        run_child_with_drain(spawn(), None, Duration::from_secs(20), 1024)
            .await
            .expect("natural exit");
        assert!(untouched.snapshot().is_empty());
    }

    /// End-to-end proof through a real child: a stream far past the drain
    /// ceiling must still hand back the bytes it printed LAST.
    ///
    /// Confirmed RED against the previous head-only drain, which discarded
    /// everything past the ceiling before `truncate_output` ran — so the
    /// "tail" the model got was the tail of the first ceiling bytes and the
    /// trailer below never appeared. That is the exact shape of a `cargo build`
    /// that fails after megabytes of progress.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_end_of_a_stream_past_the_drain_ceiling_still_reaches_the_caller() {
        use tokio::process::Command;
        // max_output_bytes = 1024 ⇒ ceiling 8192, split 4096 head / 4096 tail.
        // Emit 40 KB — five times the whole ceiling — then the trailer.
        let child = Command::new("bash")
            .arg("-c")
            .arg("head -c 40000 /dev/zero | tr '\\0' 'a'; printf 'FINAL-ERROR'")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let out = run_child_with_drain(child, None, Duration::from_secs(20), 1024)
            .await
            .expect("natural exit");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.ends_with("FINAL-ERROR"),
            "the last thing the command printed is the whole point: {:?}",
            &text[text.len().saturating_sub(60)..]
        );
        assert!(text.starts_with("aaaa"), "the opening survives too");
        assert!(text.contains("bytes elided"), "the gap must be announced");
        assert_eq!(
            out.stdout_truncated_bytes,
            40_011 - 1024,
            "the count spans the whole stream, not just the buffered part"
        );
    }

    /// The regression the live tail turns on: the drain's fragments only
    /// materialise when the child exits, so a mid-run view has to come from
    /// somewhere else. The ring must track the frontier while the job runs and
    /// the total must count every byte.
    #[cfg(unix)]
    #[tokio::test]
    async fn live_tail_keeps_tracking_past_the_drain_ceiling() {
        use crate::sandbox::context::LIVE_TAIL;
        use tokio::process::Command;

        // max_output_bytes = 64 ⇒ drain ceiling 512 bytes (8x). Emit ~4 KB of
        // 'a' then a distinctive trailer that lands far past the ceiling.
        let child = Command::new("bash")
            .arg("-c")
            .arg("head -c 4096 /dev/zero | tr '\\0' 'a'; printf 'THE-END'")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let tail = Arc::new(LiveTail::new());
        let scoped = tail.clone();
        let out = LIVE_TAIL
            .scope(scoped, async {
                run_child_with_drain(child, None, Duration::from_secs(20), 64)
                    .await
                    .expect("natural exit")
            })
            .await;

        assert!(out.truncated, "the retained slice hit its cap");
        let snap = tail.snapshot();
        assert_eq!(
            snap.stdout_total,
            4096 + 7,
            "the counter sees every byte the drain loop read, ceiling or not"
        );
        assert!(
            String::from_utf8_lossy(&snap.stdout).ends_with("THE-END"),
            "the live view tracks the frontier; a head-shaped tee would have \
             frozen 3.5 KB earlier"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_child_with_drain_does_not_kill_a_loud_child() {
        use tokio::process::Command;
        // A child whose stdout exceeds the *read* ceiling must still be
        // truncated — not executed. `exec` makes the writing process the very
        // child we wait on, so its death shows up in `signal` instead of being
        // absorbed by an intermediate shell (a real `cargo build` / `git log
        // -p` is the same shape one level down: the loud process dies and the
        // shell reports 128+13).
        let child = Command::new("bash")
            .arg("-c")
            .arg("exec head -c 100000 /dev/zero")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bash");

        let out = run_child_with_drain(child, None, Duration::from_secs(20), 1024)
            .await
            .expect("a loud command still exits naturally");
        assert_eq!(
            out.signal, None,
            "closing the read end early SIGPIPEs the child: a command is \
             killed for talking too much, and the model is told it crashed"
        );
        assert_eq!(out.exit_code, Some(0), "the command itself succeeded");
        assert!(out.truncated);
        assert_eq!(
            out.stdout_truncated_bytes,
            100_000 - 1024,
            "every overflowing byte is counted, not just the ones that fit \
             under the read ceiling"
        );
    }
}
