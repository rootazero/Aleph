//! Shutdown forensics — capture context when the gateway receives a signal.
//!
//! Ported from hermes-agent `gateway/shutdown_forensics.py`.
//!
//! The gateway's signal handlers run inside the async runtime. We can't
//! safely block them for long, but we DO want a durable record of who/what
//! triggered the shutdown so that "the gateway keeps dying" incidents can be
//! diagnosed after the fact.
//!
//! [`snapshot_shutdown_context`] is a fast (<10 ms), non-blocking probe that
//! returns a structured [`ShutdownContext`] the signal handler can log
//! immediately. Anything that needs to shell out (e.g. `ps -p <ppid>`) is
//! gated behind `with_parent_command = true` and uses a 250 ms timeout via
//! `Command::output` — the caller chooses whether they can tolerate that.

// `Command`/`Stdio` and `Duration` are only used by the Unix `ps`-shell-out
// path; on Windows that probe is compiled out, leaving them unused.
#[cfg_attr(windows, allow(unused_imports))]
use std::process::{Command, Stdio};
#[cfg_attr(windows, allow(unused_imports))]
use std::time::{Duration, Instant};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use serde::Serialize;

/// Process boot timestamp. Captured once via [`mark_boot`] so uptime can be
/// derived without threading the boot time through every caller.
///
/// `FailsClosed`: the single reader is
/// [`snapshot_shutdown_context`], which writes `uptime_secs: None` and — via
/// `skip_serializing_if` — omits the field entirely. Uptime is reported as
/// unknown rather than wrong, and no other behaviour changes.
///
/// This slot is also the wiring check's process sentinel — see [`booted`]. It
/// is the one member of the roster whose value nobody needs; what Task 12 reads
/// is whether it is there at all.
static BOOT_INSTANT: CapabilitySlot<Instant> =
    CapabilitySlot::new("gateway/boot-instant", MissingSemantics::FailsClosed);

/// Initialize the boot-time marker. Safe to call more than once; only the
/// first call wins. Call this from `aleph-server::main()` right after argv
/// parsing so the uptime number is meaningful.
pub fn mark_boot() {
    let _ = BOOT_INSTANT.install(Instant::now());
}

/// True iff this process ran `aleph-server start` far enough to reach
/// [`mark_boot`] (the first statement after argv parsing).
///
/// The `core/capability-wiring` check keys its three-state verdict on this: a
/// cold `aleph-server doctor` process installs nothing, so reporting its empty
/// roster as a problem — or as a pass — would both be fiction.
///
/// ⚠️ Deliberately `BOOT_INSTANT.get().is_some()` and NOT
/// `matches!(outcome(), Some(Outcome::Installed))`. The two agree today and
/// would diverge the day someone calls `decline` on this slot — and the honest
/// answer to "did this process boot" is about the VALUE, not the stamp.
#[must_use]
pub fn booted() -> bool {
    BOOT_INSTANT.get().is_some()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn boot_instant_slot() -> &'static dyn SlotStatus {
    &BOOT_INSTANT
}

/// Structured forensics snapshot. Serialized as JSON for grep-friendly
/// single-line logging.
#[derive(Debug, Clone, Serialize)]
pub struct ShutdownContext {
    /// Signal name (e.g. `SIGTERM`, `SIGINT`, `ctrl_c`).
    pub signal: String,
    /// Numeric signal value if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_num: Option<i32>,
    /// Our PID.
    pub pid: u32,
    /// Parent PID (best-effort; 0 if unavailable).
    pub ppid: u32,
    /// Parent command line (only filled when `with_parent_command` was set
    /// AND the `ps` invocation finished in <250 ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_command: Option<String>,
    /// Process uptime in seconds. `None` if [`mark_boot`] was never called.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Wall-clock timestamp (`SystemTime::now()` as Unix epoch seconds).
    pub at_unix_secs: u64,
}

impl ShutdownContext {
    /// Render as a single-line `key=value` block for grep-friendly logs.
    #[must_use]
    pub fn as_log_line(&self) -> String {
        let mut parts = vec![
            format!("signal={}", self.signal),
            format!("pid={}", self.pid),
            format!("ppid={}", self.ppid),
        ];
        if let Some(u) = self.uptime_secs {
            parts.push(format!("uptime_secs={u}"));
        }
        if let Some(s) = self.signal_num {
            parts.push(format!("signal_num={s}"));
        }
        if let Some(ref c) = self.parent_command {
            // Replace spaces with `_` so the line stays grep-tokenizable.
            // Truncate to 200 chars to keep the line bounded.
            let mut c = c.replace(' ', "_");
            if c.len() > 200 {
                // Truncate on a char boundary: `parent_command` is lossy-decoded
                // external (ps) output, so byte index 200 may split a multi-byte
                // char and panic.
                let end = (0..=200)
                    .rev()
                    .find(|&i| c.is_char_boundary(i))
                    .unwrap_or(0);
                c.truncate(end);
            }
            parts.push(format!("parent={c}"));
        }
        format!("[SHUTDOWN] {}", parts.join(" "))
    }
}

/// Capture forensic context for a shutdown signal. **Synchronous and fast.**
///
/// * `signal_label` — human label (e.g. `"SIGTERM"`, `"ctrl_c"`).
/// * `signal_num` — numeric signal value if known. May be `None` on Windows
///   where `ctrl_c` doesn't carry a value.
/// * `with_parent_command` — if `true`, runs `ps -p <ppid> -o args=` with
///   a 250 ms wall-clock budget. Skipped when `ppid == 0`. On any error or
///   timeout the field is silently dropped — forensics is best-effort.
#[must_use]
pub fn snapshot_shutdown_context(
    signal_label: impl Into<String>,
    signal_num: Option<i32>,
    with_parent_command: bool,
) -> ShutdownContext {
    let pid = std::process::id();
    let ppid = parent_pid();
    let parent_command = if with_parent_command && ppid != 0 {
        read_parent_command(ppid)
    } else {
        None
    };
    let uptime_secs = BOOT_INSTANT.get().map(|b| b.elapsed().as_secs());
    let at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    ShutdownContext {
        signal: signal_label.into(),
        signal_num,
        pid,
        ppid,
        parent_command,
        uptime_secs,
        at_unix_secs,
    }
}

#[cfg(unix)]
fn parent_pid() -> u32 {
    // SAFETY: getppid() has no preconditions and never fails.
    unsafe { libc::getppid() as u32 }
}

#[cfg(not(unix))]
fn parent_pid() -> u32 {
    0
}

#[cfg(unix)]
fn read_parent_command(ppid: u32) -> Option<String> {
    // Bounded subprocess — `ps` typically returns in <20 ms. We enforce a
    // 250 ms wall-clock budget by polling `try_wait` in a tight loop.
    let mut child = Command::new("ps")
        .arg("-p")
        .arg(ppid.to_string())
        .arg("-o")
        .arg("args=")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }

    let output = child.wait_with_output().ok()?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(not(unix))]
fn read_parent_command(_ppid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_pid_and_signal() {
        let ctx = snapshot_shutdown_context("ctrl_c", None, false);
        assert_eq!(ctx.signal, "ctrl_c");
        assert!(ctx.pid > 0);
        // ppid: macOS/Linux always > 0; Windows is hard-coded to 0.
        #[cfg(unix)]
        assert!(ctx.ppid > 0);
    }

    #[test]
    fn log_line_is_grep_friendly() {
        let mut ctx = snapshot_shutdown_context("SIGTERM", Some(15), false);
        ctx.uptime_secs = Some(42);
        let line = ctx.as_log_line();
        assert!(line.starts_with("[SHUTDOWN] "));
        assert!(line.contains("signal=SIGTERM"));
        assert!(line.contains("signal_num=15"));
        assert!(line.contains("uptime_secs=42"));
    }

    #[test]
    fn snapshot_finishes_quickly_without_parent_command() {
        let start = Instant::now();
        let _ = snapshot_shutdown_context("test", None, false);
        // The non-shelling path must be well under 10 ms.
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = boot_instant_slot();
        assert_eq!(slot.id(), "gateway/boot-instant");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    /// `booted()` is how the wiring check tells "this process never ran boot"
    /// from "boot ran and left holes". Without it a cold `aleph-server doctor`
    /// would report every slot missing on a perfectly healthy machine.
    ///
    /// ⚠️ THIS IS THE ONLY TEST IN THE LIB BINARY THAT MAY TOUCH `BOOT_INSTANT`,
    /// and that is why `mark_boot_is_idempotent` was folded into it rather than
    /// left beside it, and why (as of task 12's fix round) the cold-process
    /// half of `diagnostics::checks::capability_wiring::CapabilityWiringCheck`
    /// is asserted here too rather than in that module's own test file. The
    /// brief asked for a second test and told me to grep first; the grep says
    /// `mark_boot_is_idempotent` already called `mark_boot()`. A separate
    /// `assert!(!booted())` — or a separate call into `CapabilityWiringCheck`
    /// — would therefore only be correct when it won the libtest race against
    /// THIS test. What that race actually did, measured twice with different
    /// results (see `capability_wiring`'s own module doc for the fuller
    /// account and why the first number did not reproduce under the
    /// invocation it was attributed to): isolated to just the two relevant
    /// tests, the check's own cold-process test lost the race 8/8 times and
    /// silently skipped; under the full, unfiltered suite (the actual
    /// CI-shaped invocation) it lost 0/2 times (not padded further). So the
    /// defect removed here is not "it always loses" — it is
    /// invocation-dependent, which is the worse shape: a guard that runs for
    /// real under CI and silently skips under a narrower run teaches a
    /// reader that the assertion ran when it may not have. Its assertions
    /// are kept verbatim below (both the original ones and the check's), so
    /// nothing was traded away by folding them in — everything that needs
    /// "boot has not run yet" to be true now runs inside the one test
    /// function where that is guaranteed by program order, not by which
    /// invocation happens to be running.
    #[tokio::test]
    async fn booted_is_false_before_mark_boot_and_true_after() {
        // The negative half is the meaningful assertion and it is only sound
        // because of the invariant in the doc above: nothing else in this
        // binary reaches the marker, so no ordering can have set it already.
        assert!(!booted());

        // `CapabilityWiringCheck`'s cold-process branch. Must run before
        // `mark_boot()` below — see the doc above for why this is the only
        // place that assertion can be made deterministic.
        {
            use crate::diagnostics::check::{HealthCheck, Posture};
            use crate::diagnostics::checks::capability_wiring::TAG_WIRING_UNKNOWN;
            use crate::diagnostics::checks::CapabilityWiringCheck;
            use crate::diagnostics::finding::Severity;

            let findings = CapabilityWiringCheck::new().run(Posture::Inspect).await;
            assert_eq!(findings.len(), 1);
            // BOTH assertions are load-bearing, for two different reasons —
            // see `capability_wiring`'s module doc, "Why the cold row is
            // `Warning`, not `Info`" (fix round 2). Severity carries the
            // headline signal: `Info` renders identically to a genuine pass
            // in `report.ok()`, `--json`, the CLI exit code, AND — measured
            // against the real human render, not assumed — the `[ok]` tag
            // with `detail` suppressed (`render_human` only prints `detail`
            // when `is_problem()`, i.e. `severity > Info`, and never prints
            // `Finding::tags` at all). `media_codecs::TAG_CODECS_UNKNOWN`
            // stays `Info` because a missing probe tool is rare and isolated;
            // this branch is the entire, permanent behaviour of one of the
            // two doctor entry points, closer to `idle_extensions`' `Warning`
            // for an unenumerable category.
            assert_eq!(
                findings[0].severity,
                Severity::Warning,
                "the cold-process finding must not be Severity::Info — Info \
                 renders as [ok] with detail suppressed in render_human, and \
                 leaves report.ok()/--json/the CLI exit code reading a pass"
            );
            // The tag is still required because Warning alone does not say
            // WHICH problem this is: core/config-parse and
            // core/duplicate-instance are also Warning, for unrelated
            // reasons, and a consumer that wants to react specifically to
            // "the wiring question could not be answered" needs a signal
            // severity cannot carry.
            assert!(
                findings[0].has_tag(TAG_WIRING_UNKNOWN),
                "the cold-process finding must carry a tag distinguishing it \
                 from other Warning findings; got tags: {:?}",
                findings[0].tags
            );
            assert!(
                findings[0].detail.contains("did not"),
                "the cold-process finding must say this process did not boot, \
                 not that the wiring is broken; got: {}",
                findings[0].detail
            );
            assert!(findings[0]
                .fix_hint
                .as_deref()
                .is_some_and(|h| h.contains("aleph doctor")));
        }

        mark_boot();
        assert!(booted());

        // Absorbed from `mark_boot_is_idempotent`: the first call above may or
        // may not have been the first writer; if not, a subsequent set is a
        // no-op. Either way the elapsed value must be monotonic.
        let first = BOOT_INSTANT.get().expect("boot recorded").elapsed();
        mark_boot();
        let second = BOOT_INSTANT.get().expect("boot recorded").elapsed();
        assert!(second >= first);
    }

    /// Byte range of every `#[test]`/`#[tokio::test(...)]`-attributed
    /// function body in `text`, paired with the function's name.
    ///
    /// `text` must already be comment-stripped
    /// (`source_scan::strip_comment_lines`) — otherwise a doc comment that
    /// happens to contain the literal text `#[test]` could manufacture a
    /// span. This is a text scan, not a parser: it locates the attribute,
    /// then the next `fn` token, and bails if a `{` or `;` appears in
    /// between (a sign the attribute did not apply to the function it
    /// looked like it did — e.g. an intervening item). Good enough for the
    /// one census below, which only asks "does this specific body contain
    /// this specific literal call", not anything requiring full parsing.
    fn test_fn_bodies(text: &str) -> Vec<(String, std::ops::Range<usize>)> {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        for marker in ["#[test]", "#[tokio::test]", "#[tokio::test("] {
            let mut from = 0usize;
            while let Some(rel) = text[from..].find(marker) {
                let at = from + rel;
                from = at + marker.len();
                let Some(fn_at) = text[from..].find("fn ").map(|r| from + r) else {
                    continue;
                };
                if text[from..fn_at].contains(['{', ';']) {
                    continue; // the attribute did not reach a function
                }
                let name_start = fn_at + 3;
                let mut j = name_start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j == name_start {
                    continue;
                }
                let name = text[name_start..j].to_string();
                let Some(open) = text[j..].find('{').map(|r| j + r) else {
                    continue;
                };
                let Some(close) = matching_brace(bytes, open) else {
                    continue;
                };
                out.push((name, open..close + 1));
            }
        }
        out
    }

    /// Blank the payload of every `"..."`-delimited string literal in
    /// `text` (replaced with spaces, same length and byte offsets), leaving
    /// every other character untouched — so a panic message that quotes the
    /// literal text `mark_boot()` (this census's own violation message
    /// does exactly that) does not get misread as a call to it. Handles the
    /// common escaped-quote case (`"say \"hi\""`).
    ///
    /// Always UTF-8-safe: `"` and `\` are ASCII bytes, which by construction
    /// never occur inside a multi-byte UTF-8 sequence, so every blanked
    /// range starts and ends on a character boundary.
    ///
    /// Known gap, not reachable in this corpus as of writing this guard: a
    /// raw string (`r#"..."#`) containing an embedded, unescaped `"` closes
    /// early under this scan. The failure direction is under-blanking, not
    /// over-matching, so a genuine violation inside such a literal would
    /// still surface — only with a slightly wrong reported span.
    fn blank_string_literals(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = bytes.to_vec();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'"' {
                    break;
                }
                j += 1;
            }
            let end = j.min(bytes.len());
            for b in out.iter_mut().take(end).skip(start) {
                *b = b' ';
            }
            i = end + 1;
        }
        String::from_utf8(out).unwrap_or_else(|_| text.to_string())
    }

    /// Index of the `}` matching the `{` at `open`, by depth counting.
    fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
        let mut depth = 0i32;
        for (i, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The invariant declared on `booted_is_false_before_mark_boot_and_true_after`
    /// ("THIS IS THE ONLY TEST IN THE LIB BINARY THAT MAY TOUCH
    /// `BOOT_INSTANT`") was enforced by nothing but that comment until this
    /// guard. It became load-bearing for a second thing when task 12's fix
    /// round folded `CapabilityWiringCheck`'s cold-process assertion into
    /// the same test: gutting or deleting that one function now silently
    /// drops coverage from two places, with no compile error pointing at
    /// either. This census is the source-level enforcement the doc comment
    /// was making a claim about but not backing.
    ///
    /// Scoped to `#[test]`/`#[tokio::test]` function bodies specifically
    /// (via `test_fn_bodies`, comment-stripped first), so the two
    /// legitimate PRODUCTION callers — `commands::start::start_server`'s
    /// `mark_boot()` and `CapabilityWiringCheck::run`'s `booted()` — are
    /// never in scope; neither is a test function, so the scan never visits
    /// their bodies. Only test code answers to this rule.
    #[test]
    fn no_test_outside_the_one_designated_function_touches_boot_instant() {
        const ALLOWED: &str = "booted_is_false_before_mark_boot_and_true_after";

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = crate::utils::source_scan::rust_sources_under(&root);
        assert!(
            sources.len() > 100,
            "the source walk found only {} files under src/ — this census \
             scanned nothing, which is not the same as finding nothing wrong",
            sources.len()
        );

        let mut scanned_test_fns = 0usize;
        let mut violations: Vec<String> = Vec::new();
        for (rel, text) in sources {
            let stripped = crate::utils::source_scan::strip_comment_lines(&text);
            for (name, body) in test_fn_bodies(&stripped) {
                scanned_test_fns += 1;
                if name == ALLOWED {
                    continue;
                }
                let body_text = blank_string_literals(&stripped[body]);
                if body_text.contains("mark_boot()") || body_text.contains("booted()") {
                    violations.push(format!("{rel}::{name}"));
                }
            }
        }
        // Self-counting: a broken function-span scan that silently finds
        // zero test functions would report the same "no violations" verdict
        // as a genuinely clean repo. This repo has 12,000+ `#[test]` and
        // 4,700+ `#[tokio::test]` items; 1,000 is a floor with margin for
        // both, not a number tuned to today's count.
        assert!(
            scanned_test_fns > 1000,
            "only found {scanned_test_fns} test functions across src/ — the \
             census's function-span scan is broken, not confirming a clean \
             repo"
        );
        assert!(
            violations.is_empty(),
            "these tests call mark_boot()/booted() outside the one function \
             that may (`gateway::shutdown_forensics::tests::{ALLOWED}` — see \
             its doc for why a second caller is a libtest-ordering race, not \
             a correctness bug): {violations:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_command_lookup_is_bounded() {
        let ppid = parent_pid();
        let start = Instant::now();
        // We don't assert on content (could be the test runner, an IDE,
        // launchd, etc.) — only that the call respects its budget.
        let _ = read_parent_command(ppid);
        assert!(start.elapsed() < Duration::from_millis(300));
    }
}
