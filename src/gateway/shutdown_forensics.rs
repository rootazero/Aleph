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

    /// Index of the `}` matching the `{` at `open`, by depth counting.
    ///
    /// Sound only on text that has already been through
    /// [`crate::utils::source_scan::code_text`]: raw braces written inside a string
    /// literal are gone from that text, so nothing but real block structure
    /// is counted.
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

    /// Every spelling that reaches the `BOOT_INSTANT` slot: the two public
    /// functions, plus the static itself — `install`/`decline` go through
    /// the static directly, so a census that watched only the two functions
    /// would not see a test that reached past them.
    ///
    /// This array is literal payload sitting inside the region that
    /// [`no_test_outside_the_one_designated_function_touches_boot_instant`]
    /// scans, so it doubles as that test's live positive control for
    /// payload removal: swapping [`crate::utils::source_scan::code_text`]
    /// there for plain comment-stripping makes this very line the named
    /// offender (falsified 2026-08-25, RED). A scanner's own strings are
    /// inside its corpus — here that is wired up as an alarm instead of
    /// being worked around with an exemption.
    ///
    /// It is NOT a control for
    /// [`the_only_files_naming_the_boot_slot_are_the_owner_the_boot_site_and_the_reader`],
    /// and assuming otherwise is the easy mistake: that test's verdict is a
    /// set equality over FILES, and this file is already in the set, so
    /// extra hits inside it change nothing (measured: that swap leaves it
    /// GREEN). The two failure directions are not symmetric either — losing
    /// payload removal can only ADD apparent hits, which is the loud
    /// direction. The dangerous direction is OVER-blanking, where a
    /// desynchronised scan reports a clean read of text it never saw; that
    /// is what `code_text`'s raw-string lexing prevents, and what
    /// `source_scan`'s own
    /// `code_text_survives_a_raw_string_with_an_embedded_quote` pins.
    const BOOT_SLOT_MARKERS: [&str; 3] = ["mark_boot()", "booted()", "BOOT_INSTANT"];

    /// The module that owns the slot.
    const OWNING_FILE: &str = "src/gateway/shutdown_forensics.rs";
    /// The one production caller of `mark_boot()`.
    const BOOT_SITE_FILE: &str = "src/bin/aleph-server/commands/start/mod.rs";
    /// The one production reader of `booted()`.
    const READER_FILE: &str = "src/diagnostics/checks/capability_wiring.rs";
    /// Every file in `src/` permitted to name a [`BOOT_SLOT_MARKERS`]
    /// spelling at all. Asserted as an EQUALITY, in both directions — see
    /// [`the_only_files_naming_the_boot_slot_are_the_owner_the_boot_site_and_the_reader`].
    const BOOT_SLOT_FILES: [&str; 3] = [OWNING_FILE, BOOT_SITE_FILE, READER_FILE];

    /// The one test function allowed to touch the slot.
    const ALLOWED_TEST: &str = "booted_is_false_before_mark_boot_and_true_after";

    /// The first [`BOOT_SLOT_MARKERS`] spelling `code` contains, if any.
    /// `code` must already be [`crate::utils::source_scan::code_text`] output.
    fn boot_slot_spelling(code: &str) -> Option<&'static str> {
        BOOT_SLOT_MARKERS.into_iter().find(|m| code.contains(m))
    }

    /// `code` with every `fn <name> … }` definition removed whole
    /// (signature through matching brace), paired with the removed spans.
    ///
    /// The caller checks the span count rather than trusting it: "removed
    /// nothing" and "removed the thing" produce the same clean verdict
    /// downstream, so the count is the only thing that tells them apart.
    fn without_fn(code: &str, name: &str) -> (String, Vec<String>) {
        let needle = format!("fn {name}");
        let bytes = code.as_bytes();
        let mut kept = String::new();
        let mut removed = Vec::new();
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(&needle) {
            let at = from + rel;
            let Some(open) = code[at..].find('{').map(|r| at + r) else {
                break;
            };
            let Some(close) = matching_brace(bytes, open) else {
                break;
            };
            kept.push_str(&code[from..at]);
            removed.push(code[at..=close].to_string());
            from = close + 1;
        }
        kept.push_str(&code[from..]);
        (kept, removed)
    }

    /// Every `.rs` file under `src/`, as `(repo-relative path, contents)`.
    fn src_tree() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        crate::utils::source_scan::rust_sources_under(&root)
    }

    /// **The assumption the invariant below rests on.**
    ///
    /// `booted_is_false_before_mark_boot_and_true_after` asserts
    /// `!booted()` before anything in the process has recorded boot. That
    /// is only sound while the set of things that CAN record boot is small
    /// and known — otherwise a test that calls some ordinary production
    /// function could set the slot transitively, and no text scan of test
    /// bodies would ever see it. Pinning the file set is what keeps that
    /// reasoning checkable: the whole `src/` tree names the slot in
    /// exactly three files, so the transitive surface is enumerable, and a
    /// fourth file appearing is a design event that has to be looked at.
    ///
    /// Asserted as set EQUALITY, deliberately, so it fires in both
    /// directions. A new file is the case above. A file dropping out means
    /// this pin has outlived its subject and must shrink — the force that
    /// an exemption list normally lacks, which is why this is a statement
    /// of the invariant rather than an allowlist for it.
    ///
    /// # What this cannot see
    ///
    /// A renaming import (`use …::mark_boot as mb; mb();`) and a
    /// macro-generated call are invisible to any text scan. Neither is
    /// hypothetically excluded — they are simply outside what this
    /// instrument measures, and saying so is cheaper than implying
    /// coverage it does not have.
    #[test]
    fn the_only_files_naming_the_boot_slot_are_the_owner_the_boot_site_and_the_reader() {
        let sources = src_tree();
        // Self-check. A lower bound on a quantity that only grows during
        // ordinary work costs nothing to set close to the truth: growth can
        // never trip it, and the tension a tight bound normally carries
        // belongs to ceilings, not floors. 2,447 files measured
        // 2026-08-25; 2,000 leaves 18% for a genuine consolidation while
        // still catching a walk that came back with a fraction of the tree.
        assert!(
            sources.len() > 2_000,
            "the source walk found only {} files under src/ — this census \
             scanned a fraction of the tree, which is not the same as \
             finding nothing wrong",
            sources.len()
        );

        let mut found: Vec<String> = sources
            .into_iter()
            .filter(|(_, text)| {
                boot_slot_spelling(&crate::utils::source_scan::code_text(text)).is_some()
            })
            .map(|(rel, _)| rel)
            .collect();
        found.sort();
        let mut expected: Vec<String> = BOOT_SLOT_FILES.iter().map(|f| (*f).to_string()).collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "the set of files naming the boot slot changed. A file that \
             APPEARED widens the transitive surface the one designated test \
             depends on — see this test's doc. A file that DISAPPEARED means \
             BOOT_SLOT_FILES now over-permits and must shrink to match."
        );
    }

    /// **The invariant.** The doc on
    /// `booted_is_false_before_mark_boot_and_true_after` declares that it is
    /// the only test in the lib binary that may touch the slot; that was
    /// enforced by nothing but the comment until this guard, and it became
    /// load-bearing for a second thing when `CapabilityWiringCheck`'s
    /// cold-process assertion was folded into the same function.
    ///
    /// # How the region is derived, and why not by parsing test functions
    ///
    /// The previous version of this census walked from each
    /// `#[test]`/`#[tokio::test]` attribute to the following `fn` and bailed
    /// if it met a `{` or `;` on the way. Nine real test functions in this
    /// repo carry a `;` between the two — inside `#[ignore = "…; use
    /// integration tests"]`, or a trailing `// TODO(windows): …;` comment on
    /// a code line, which whole-line comment stripping does not remove. All
    /// nine were skipped in silence. It then blanked string literals with an
    /// alternating-quote walk that desynchronised on
    /// `tokenize(r#"--role "unclosed role"#)` and blanked everything after
    /// it to the end of the scanned text.
    ///
    /// Both gaps are properties of hand-rolled parsing, so this version does
    /// none. Files outside [`BOOT_SLOT_FILES`] may not name the slot at all
    /// (checked by the test above), which covers every test in them without
    /// knowing where any function starts — including the 120 files under
    /// `src/` that carry test attributes but no `#[cfg(test)]` of their own
    /// (measured 2026-08-25; typically a whole test module that a parent
    /// declares with `#[cfg(test)] mod x;`), for which a region scan finds
    /// no test code at all and would report them clean without looking. Inside the three permitted files, the test region comes from
    /// [`crate::utils::source_scan::cfg_test_portion`] — the other half of the same walk
    /// that produces `production_prefix`, so the two cannot disagree about
    /// where test code begins — and only ONE function is located by name,
    /// with the count of hits asserted.
    ///
    /// # Scope
    ///
    /// The invariant is about the LIB test binary, which is the only process
    /// where the designated test's `!booted()` can be raced. `src/bin/…` and
    /// `tests/…` compile into their own binaries and cannot reach this
    /// process's `BOOT_INSTANT` at all. `src/bin/aleph-server/…/start/mod.rs`
    /// is still scanned here, deliberately: it is the boot site, so it is
    /// where a test would most plausibly reach for `mark_boot()`, and a RED
    /// there is the loud direction. A separate crate (`aleph-tui`,
    /// `aleph-cli`, …) could call the two public functions without this
    /// census seeing it, and equally without any effect on this binary.
    #[test]
    fn no_test_outside_the_one_designated_function_touches_boot_instant() {
        use crate::utils::source_scan::{cfg_test_portion, code_text, production_prefix};

        let sources = src_tree();
        let mut violations: Vec<String> = Vec::new();
        let mut files_checked = 0usize;

        for file in BOOT_SLOT_FILES {
            let (_, text) = sources
                .iter()
                .find(|(rel, _)| rel == file)
                .unwrap_or_else(|| panic!("{file} is pinned by this census but is not on disk"));

            // Self-check: this file really does still name the slot in
            // production. Ground truth beats a magnitude floor — if the walk
            // or the lexer breaks, this fires here, at a place where the
            // answer is known, instead of downstream as a clean verdict.
            let production = code_text(&production_prefix(text));
            assert!(
                boot_slot_spelling(&production).is_some(),
                "self-check: {file} no longer names the boot slot in its \
                 production half. Either the scan is broken, or this file \
                 does not belong in BOOT_SLOT_FILES any more"
            );

            // Self-check: the partition actually split this file. An empty
            // test portion and a clean test portion are the same verdict.
            let tests = cfg_test_portion(text);
            assert!(
                !tests.trim().is_empty(),
                "self-check: {file} yielded an empty #[cfg(test)] portion — \
                 the region scan is broken, not confirming a clean file"
            );

            let mut region = code_text(&tests);
            if file == OWNING_FILE {
                let (rest, removed) = without_fn(&region, ALLOWED_TEST);
                assert_eq!(
                    removed.len(),
                    1,
                    "self-check: expected exactly one definition of \
                     {ALLOWED_TEST} in {file}, found {}. Zero means the \
                     excision removed nothing and this census is about to \
                     approve a region it never looked at",
                    removed.len()
                );
                // The other half of what this guard is for: gutting the
                // designated function drops coverage in two places with no
                // compile error pointing at either.
                let body = &removed[0];
                for spelling in [BOOT_SLOT_MARKERS[0], BOOT_SLOT_MARKERS[1]] {
                    assert!(
                        body.contains(spelling),
                        "self-check: {ALLOWED_TEST} no longer contains \
                         `{spelling}`. It is the only test that may, and two \
                         separate assertions depend on it still doing so"
                    );
                }
                region = rest;
            }

            for line in region.lines() {
                if let Some(spelling) = boot_slot_spelling(line) {
                    violations.push(format!("{file}: `{spelling}` in `{}`", line.trim()));
                }
            }
            files_checked += 1;
        }

        assert_eq!(
            files_checked,
            BOOT_SLOT_FILES.len(),
            "self-check: not every pinned file was reached"
        );
        assert!(
            violations.is_empty(),
            "these test-side lines reach the boot slot outside the one \
             function that may (`gateway::shutdown_forensics::tests::\
             {ALLOWED_TEST}` — see its doc for why a second toucher is a \
             libtest-ordering race, not a correctness bug): {violations:#?}"
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
