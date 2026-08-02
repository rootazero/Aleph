use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Child;

use crate::sandbox::command::{SandboxError, SandboxOutput};

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

#[must_use]
pub fn normalize_path_for_sandbox(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(cwd.join(path))
    }
}

#[must_use]
pub fn path_is_allowed(path: &Path, allowed: &[PathBuf]) -> bool {
    allowed.iter().any(|prefix| path.starts_with(prefix))
}

#[must_use]
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn glob_to_regex(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }

    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    regex.push_str(".*");
                } else {
                    regex.push_str("[^/]*");
                }
            }
            '?' => regex.push_str("[^/]"),
            '.' => regex.push_str("\\."),
            '+' => regex.push_str("\\+"),
            '(' => regex.push_str("\\("),
            ')' => regex.push_str("\\)"),
            '[' => regex.push_str("\\["),
            ']' => regex.push_str("\\]"),
            '{' => regex.push_str("\\{"),
            '}' => regex.push_str("\\}"),
            '^' => regex.push_str("\\^"),
            '$' => regex.push_str("\\$"),
            '|' => regex.push_str("\\|"),
            '\\' => regex.push_str("\\\\"),
            '/' => regex.push('/'),
            c => regex.push(c),
        }
    }

    regex.push('$');
    Some(regex)
}

/// Truncate captured process output so the retained content is at most
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
    let head_budget = max_bytes * 2 / 5;
    let tail_budget = max_bytes - head_budget;

    // Head cut: back off any UTF-8 continuation byte so we never split a char.
    let mut head_end = head_budget;
    while head_end > 0 && (buf[head_end] & 0xC0) == 0x80 {
        head_end -= 1;
    }

    // Tail cut: walk forward from (orig_len - tail_budget) to the next char
    // boundary so the tail fragment also starts on a clean codepoint.
    let mut tail_start = orig_len - tail_budget;
    while tail_start < orig_len && (buf[tail_start] & 0xC0) == 0x80 {
        tail_start += 1;
    }

    // If the fragments meet or overlap (cap nearly equals len), nothing is
    // actually elided — fall back to head-only to avoid an empty/negative gap.
    if tail_start <= head_end {
        return truncate_head_only(buf, max_bytes);
    }

    let dropped = (tail_start - head_end) as u64;
    let marker = format!("\n…[{dropped} bytes elided]…\n");

    let mut out = Vec::with_capacity(head_end + marker.len() + (orig_len - tail_start));
    out.extend_from_slice(&buf[..head_end]);
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&buf[tail_start..]);
    (out, dropped)
}

/// Cap below which [`truncate_output`] keeps the original head-only behaviour
/// instead of a head+tail split. Below this the elision marker plus two
/// fragments would carry less signal than a single contiguous head slice.
const MIN_HEAD_TAIL_CAP: usize = 256;

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
    let mut end = max_bytes;
    while end > 0 && (buf[end] & 0xC0) == 0x80 {
        end -= 1;
    }
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

/// Read `pipe` to EOF, keeping at most `buffer_ceiling` bytes and counting
/// (but discarding) the rest. Returns `(kept, discarded)`.
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
async fn drain_bounded<R>(pipe: Option<R>, buffer_ceiling: u64) -> (Vec<u8>, u64)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut pipe) = pipe else {
        return (Vec::new(), 0);
    };
    let mut kept: Vec<u8> = Vec::new();
    let mut discarded: u64 = 0;
    let mut chunk = [0u8; 16 * 1024];
    // A read error ends the loop just like EOF: the pipe is unusable either
    // way, and what was already read stays valid (matches the previous
    // `let _ = ...read_to_end(...)`).
    while let Ok(n) = pipe.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let room = usize::try_from(buffer_ceiling)
            .unwrap_or(usize::MAX)
            .saturating_sub(kept.len());
        let take = room.min(n);
        kept.extend_from_slice(&chunk[..take]);
        discarded = discarded.saturating_add((n - take) as u64);
    }
    (kept, discarded)
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

    // Buffer PAST the keep-budget so `truncate_output` can both preserve a
    // head+tail slice AND report how many bytes were elided — the
    // `*_truncated_bytes` contract surfaced to the model (see `code_exec`).
    // Buffering exactly `max_output_bytes` would silently drop the overflow
    // before it could be counted (drop count stuck at 0, `truncated` stuck
    // false). Memory stays bounded: at most `DRAIN_BUFFER_FACTOR *
    // max_output_bytes` is ever held for a runaway child; everything past that
    // is read and discarded, counted but not kept.
    const DRAIN_BUFFER_FACTOR: u64 = 8;
    let buffer_ceiling = (max_output_bytes as u64).saturating_mul(DRAIN_BUFFER_FACTOR);

    let stdout_task = tokio::spawn(drain_bounded(stdout, buffer_ceiling));
    let stderr_task = tokio::spawn(drain_bounded(stderr, buffer_ceiling));

    let start = Instant::now();
    let wait_result = tokio::time::timeout(timeout, child.wait()).await;
    let elapsed_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    match wait_result {
        Ok(Ok(status)) => {
            let (stdout_buf, stdout_overflow) = stdout_task.await.unwrap_or_default();
            let (stderr_buf, stderr_overflow) = stderr_task.await.unwrap_or_default();
            // Bytes elided from the buffered slice PLUS the bytes that never
            // fit in the buffer at all — the count the model reads is the
            // whole overflow, not just the visible part of it.
            let (stdout, stdout_kept_drop) = truncate_output(stdout_buf, max_output_bytes);
            let (stderr, stderr_kept_drop) = truncate_output(stderr_buf, max_output_bytes);
            let stdout_dropped = stdout_kept_drop.saturating_add(stdout_overflow);
            let stderr_dropped = stderr_kept_drop.saturating_add(stderr_overflow);
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
                let (stdout_buf, _) = stdout_task.await.unwrap_or_default();
                let (stderr_buf, _) = stderr_task.await.unwrap_or_default();
                (stdout_buf, stderr_buf)
            })
            .await;
            let (partial_stdout, partial_stderr) =
                drain.unwrap_or_else(|_| (Vec::new(), Vec::new()));
            // Cap partial output too — a runaway loop can fill the pipe
            // with megabytes before the kill takes effect.
            let (partial_stdout, _) = truncate_output(partial_stdout, max_output_bytes);
            let (partial_stderr, _) = truncate_output(partial_stderr, max_output_bytes);
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
    fn normalize_absolute_path() {
        let path = Path::new("/usr/bin/python");
        let cwd = Path::new("/home/user");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/usr/bin/python"))
        );
    }

    #[test]
    fn normalize_relative_path() {
        let path = Path::new("src/main.rs");
        let cwd = Path::new("/home/user/project");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/home/user/project/src/main.rs"))
        );
    }

    #[test]
    fn normalize_empty_path() {
        let path = Path::new("");
        let cwd = Path::new("/home/user");
        assert_eq!(normalize_path_for_sandbox(path, cwd), None);
    }

    #[test]
    fn normalize_dot_path() {
        let path = Path::new(".");
        let cwd = Path::new("/home/user");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/home/user/."))
        );
    }

    #[test]
    fn path_allowed_exact_match() {
        let path = Path::new("/home/user/project/src/main.rs");
        let allowed = vec![PathBuf::from("/home/user/project")];
        assert!(path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_allowed_multiple_prefixes() {
        let path = Path::new("/tmp/test.txt");
        let allowed = vec![
            PathBuf::from("/home/user"),
            PathBuf::from("/tmp"),
            PathBuf::from("/var"),
        ];
        assert!(path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_not_allowed() {
        let path = Path::new("/etc/passwd");
        let allowed = vec![PathBuf::from("/home/user"), PathBuf::from("/tmp")];
        assert!(!path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_allowed_empty_list() {
        let path = Path::new("/home/user/file.txt");
        let allowed: Vec<PathBuf> = vec![];
        assert!(!path_is_allowed(path, &allowed));
    }

    #[test]
    fn glob_star_matches_single_segment() {
        let regex = glob_to_regex("*.rs").unwrap();
        assert_eq!(regex, "^[^/]*\\.rs$");
    }

    #[test]
    fn glob_double_star_matches_any() {
        let regex = glob_to_regex("src/**/*.rs").unwrap();
        assert_eq!(regex, "^src/.*/[^/]*\\.rs$");
    }

    #[test]
    fn glob_question_mark() {
        let regex = glob_to_regex("file?.txt").unwrap();
        assert_eq!(regex, "^file[^/]\\.txt$");
    }

    #[test]
    fn glob_literal_match() {
        let regex = glob_to_regex("hello.txt").unwrap();
        assert_eq!(regex, "^hello\\.txt$");
    }

    #[test]
    fn glob_empty_pattern() {
        assert_eq!(glob_to_regex(""), None);
    }

    #[test]
    fn glob_special_chars_escaped() {
        let regex = glob_to_regex("file(name)+[1].txt").unwrap();
        assert_eq!(regex, "^file\\(name\\)\\+\\[1\\]\\.txt$");
    }

    #[test]
    fn glob_mixed_pattern() {
        let regex = glob_to_regex("src/**/test_*.rs").unwrap();
        assert_eq!(regex, "^src/.*/test_[^/]*\\.rs$");
    }

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
