// Browser qa tool — one-call diagnostic verdict (C3; spec:
// docs/superpowers/specs/2026-09-30-browser-qa-design.md §3, plan Task 3).
//
// The tool ORCHESTRATES existing trait verbs (`wait_for` / `console_messages`
// / `network_log` / `screenshot`) and defers every verdict to the pure logic
// in [`super::qa_verdict`] (counted baseline subtraction, data-table failure
// classifier, tri-state assembly). Zero new engine primitives: the backend
// trait is untouched.
//
// Honesty triangle (Global Constraints): `passed=true` ⟺ `failed_checks` and
// `unverified` are BOTH empty. A dimension the driver cannot serve lands in
// `unverified` with the driver named — a skip never masquerades as a pass.
// Assertion failure ≠ tool error: an expectation that does not arrive is a
// `Fail` check inside a SUCCESSFUL tool result; only engine/driver faults
// (TabGone, transport, SSRF refusal) leave through `backend_error_text`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::qa_verdict::{self, Baseline, CheckEntry, CheckOutcome, Impact, Subtraction};
use crate::browser::backend::BrowserBackend;
use crate::browser::error::BrowserError;
use crate::browser::manager::ProfileManager;
use crate::browser::tab_registry;
use crate::browser::types::{ScreenshotOpts, WaitCondition};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

const fn default_timeout_ms() -> u64 {
    5000
}

const fn default_true() -> bool {
    true
}

/// Clamp window for the per-expectation wait budget (spec §3: default 5000,
/// ceiling 30000). Narrower than `browser_wait_for`'s 120 s: QA runs the
/// budget once PER expectation, so a 120 s ceiling would let three
/// expectations pin the turn for six minutes.
pub(crate) const MIN_TIMEOUT_MS: u64 = 500;
pub(crate) const MAX_TIMEOUT_MS: u64 = 30_000;

/// The diagnostic settle between the expectation wait and the final buffer
/// read — the reference implementation's 150 ms (job.ts `qa.diagnosticSettle`),
/// letting asynchronous errors land in the rings before they are read.
const SETTLE_MS: u64 = 150;

/// Cap on `expected_text` array length. Each entry costs up to one
/// `timeout_ms` wait; an unbounded array is an unbounded turn.
const MAX_EXPECTED_TEXTS: usize = 16;

/// Per-buffer evidence tail budget (chars, head+tail split). The tails are
/// the NEW lines the verdict reasoned over — evidence, not a dump.
const EVIDENCE_TAIL_MAX: usize = 4_000;

/// Clamp the per-expectation wait budget into the safe window. Hand-rolled
/// rather than `Ord::clamp` so it stays `const` (same shape as
/// `wait_for::clamp_timeout`).
#[allow(clippy::manual_clamp)]
pub(crate) const fn clamp_timeout(ms: u64) -> u64 {
    if ms < MIN_TIMEOUT_MS {
        MIN_TIMEOUT_MS
    } else if ms > MAX_TIMEOUT_MS {
        MAX_TIMEOUT_MS
    } else {
        ms
    }
}

/// `expected_text` takes one string or an array of them (spec §3).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TextExpectation {
    One(String),
    Many(Vec<String>),
}

/// Arguments for the `browser_qa` tool (spec §3 — no `action` field; one
/// call is one verdict).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserQaArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Tab id (default: the active tab).
    #[serde(default)]
    pub tab_id: Option<String>,
    /// Visible text expected on the page — one string or an array (max 16
    /// entries, each waited up to timeout_ms).
    #[serde(default)]
    pub expected_text: Option<TextExpectation>,
    /// CSS selector expected to match at least one element.
    #[serde(default)]
    pub expected_selector: Option<String>,
    /// CSS selector expected to be ABSENT (e.g. a spinner that should be
    /// gone). Inverse polarity of expected_selector.
    #[serde(default)]
    pub gone_selector: Option<String>,
    /// Check the console ring for new warning-level lines (default: true).
    /// Level parsing is defined over the cdp backend's ring format; other
    /// drivers land this check in `unverified` with the driver named.
    #[serde(default = "default_true")]
    pub check_console: bool,
    /// Check for new error-level lines — console.error AND uncaught
    /// exceptions, which the cdp backend folds into the same [error] ring
    /// (default: true). Only the cdp backend subscribes
    /// Runtime.exceptionThrown; other drivers land this check in
    /// `unverified` with the driver named.
    #[serde(default = "default_true")]
    pub check_errors: bool,
    /// Check the network ring for new failed (status >= 400) responses,
    /// classified by impact — favicon/sourcemap/analytics failures warn,
    /// document/script/XHR failures fail (default: true). cdp ring format
    /// only; other drivers land this check in `unverified`.
    #[serde(default = "default_true")]
    pub check_network: bool,
    /// Attach an evidence screenshot when the run does not pass cleanly
    /// (default: false). Saved under ~/.aleph/qa/<profile>/ and named in
    /// `evidence.screenshot_path`.
    #[serde(default)]
    pub screenshot: bool,
    /// Per-expectation wait budget in ms (default: 5000; clamped to
    /// 500-30000).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

/// One validated call, with the model's raw optionality resolved away.
#[derive(Debug)]
struct Validated {
    texts: Vec<String>,
    selector: Option<String>,
    gone: Option<String>,
    check_console: bool,
    check_errors: bool,
    check_network: bool,
    screenshot: bool,
    timeout_ms: u64,
}

/// The evidence block of the wire verdict (spec §3): the NEW console/network
/// lines the verdict reasoned over (post-subtraction, redacted, bounded), and
/// the screenshot path when one was captured.
#[derive(Debug, Serialize)]
pub struct QaEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<String>,
    pub console_tail: String,
    pub network_tail: String,
}

/// The wire verdict (spec §3): T2's assembled tri-state plus the evidence.
#[derive(Debug, Serialize)]
pub struct QaVerdict {
    pub passed: bool,
    pub failed_checks: Vec<CheckEntry>,
    pub warnings: Vec<CheckEntry>,
    pub unverified: Vec<CheckEntry>,
    pub summary: String,
    pub evidence: QaEvidence,
}

/// Output from the `browser_qa` tool.
#[derive(Debug, Serialize)]
pub struct BrowserQaOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<QaVerdict>,
    pub message: Option<String>,
}

impl BrowserQaOutput {
    fn failed(message: String) -> Self {
        Self {
            success: false,
            verdict: None,
            message: Some(message),
        }
    }
}

/// Validate BEFORE any backend exists: a malformed call is a model mistake
/// and degrades to `success:false` with the contract spelled out, never a
/// hard Err (the family convention `browser_record` states).
fn validate(args: &BrowserQaArgs) -> std::result::Result<Validated, String> {
    let texts: Vec<String> = match &args.expected_text {
        None => Vec::new(),
        Some(TextExpectation::One(t)) => vec![t.trim().to_string()],
        Some(TextExpectation::Many(many)) => many.iter().map(|t| t.trim().to_string()).collect(),
    };
    if texts.iter().any(|t| t.is_empty()) {
        return Err("expected_text entries must be non-empty — drop empty strings".into());
    }
    if texts.len() > MAX_EXPECTED_TEXTS {
        return Err(format!(
            "expected_text takes at most {MAX_EXPECTED_TEXTS} entries (got {}) — each entry \
             costs up to one timeout_ms wait",
            texts.len()
        ));
    }
    let non_empty =
        |value: &Option<String>, key: &str| -> std::result::Result<Option<String>, String> {
            match value {
                None => Ok(None),
                Some(s) if s.trim().is_empty() => Err(format!(
                    "{key} is empty — drop the key instead of passing an empty string"
                )),
                Some(s) => Ok(Some(s.clone())),
            }
        };
    let selector = non_empty(&args.expected_selector, "expected_selector")?;
    let gone = non_empty(&args.gone_selector, "gone_selector")?;
    if texts.is_empty()
        && selector.is_none()
        && gone.is_none()
        && !args.check_console
        && !args.check_errors
        && !args.check_network
    {
        return Err(
            "nothing to check: set an expectation (expected_text / expected_selector / \
             gone_selector) or enable at least one of check_console / check_errors / \
             check_network — a run with zero checks would report a vacuous pass, and a \
             skip must not masquerade as one"
                .into(),
        );
    }
    Ok(Validated {
        texts,
        selector,
        gone,
        check_console: args.check_console,
        check_errors: args.check_errors,
        check_network: args.check_network,
        screenshot: args.screenshot,
        timeout_ms: clamp_timeout(args.timeout_ms),
    })
}

/// Which driver actually serves this run — answered by the BACKEND (a
/// downcast), never by the profile's configured driver: the configured value
/// is a wish, the concrete type is the fact (判据 §18). Test doubles resolve
/// to "unknown", which the unverified reasons then say plainly.
fn driver_label(backend: &dyn BrowserBackend) -> &'static str {
    let any = backend.as_any();
    if any.is::<crate::browser::cdp_backend::CdpBackend>() {
        "cdp"
    } else if any.is::<crate::browser::playwright_cli_backend::PlaywrightCliBackend>() {
        "managed (playwright-cli)"
    } else if any.is::<crate::browser::chrome_mcp_backend::ChromeMcpBackend>() {
        "existing_session (chrome-mcp)"
    } else {
        "unknown"
    }
}

/// Level of one console-ring line, in the cdp backend's `[{level}] {text}`
/// format (`events.rs::apply_event` writes the console API type verbatim).
/// `None` for every other line — informationals are not failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsoleLevel {
    Error,
    Warn,
}

fn console_level(line: &str) -> Option<ConsoleLevel> {
    if let Some(rest) = line.strip_prefix('[') {
        let (level, _) = rest.split_once(']')?;
        match level {
            "error" => Some(ConsoleLevel::Error),
            "warn" | "warning" => Some(ConsoleLevel::Warn),
            _ => None,
        }
    } else {
        None
    }
}

/// One failed response from a network-ring line: the cdp backend writes
/// `<- {status} {url}` for `Network.responseReceived`; a status >= 400 is a
/// failure. Request lines (`->`) carry no verdict.
fn failed_response(line: &str) -> Option<(u16, &str)> {
    let rest = line.strip_prefix("<- ")?;
    let (status, url) = rest.split_once(' ')?;
    let status: u16 = status.parse().ok()?;
    (status >= 400).then_some((status, url))
}

/// The three buffer-check flags, bundled so `evaluate_buffer_checks`'s
/// signature names them rather than positionally threading three bools.
struct CheckFlags {
    console: bool,
    errors: bool,
    network: bool,
}

/// Buffer-check outcomes, split from the async pipeline so the classification
/// semantics are unit-testable without a backend. `precise` is the driver
/// verdict: only the cdp backend's rings carry Aleph's own `[level]` /
/// `<- status` formats, so off the cdp backend every enabled buffer check is
/// `Unverified` with the driver named — nothing asserted, nothing ruled out
/// (Review Focus #1: a skip never masquerades as a pass).
///
/// Ownership between the two console flags: error-level lines belong to
/// `check_errors` while it is enabled and fall to `check_console` when it is
/// not (the cdp ring folds console.error and uncaught exceptions into the
/// same `[error]` lines — they are one bucket physically). Warning-level
/// lines are always `check_console`'s and become `warnings`, never failures.
///
/// Baseline-matched interesting lines (an error-level or failed-response
/// fingerprint absorbed by the baseline allowance) become ONE aggregated
/// `Unverified` per check: an identical line emitted inside the QA window is
/// indistinguishable from pre-existing residue, and the honesty triangle
/// forbids pretending otherwise (spec §2 honest point). Matched WARNING lines
/// are the deliberate asymmetry: a warn is advisory, so tracking its newness
/// would buy unverified noise at zero actionability — they are dropped.
fn evaluate_buffer_checks(
    checks: &mut Vec<(&'static str, CheckOutcome)>,
    precise: bool,
    driver: &str,
    flags: &CheckFlags,
    console_sub: Option<&Subtraction>,
    network_sub: Option<&Subtraction>,
) {
    if let Some(sub) = console_sub {
        if !precise {
            if flags.console {
                checks.push((
                    "check_console",
                    CheckOutcome::Unverified {
                        reason: format!(
                            "driver '{driver}' console output is not Aleph's [level]-prefixed \
                             ring, so levels are not parsed — nothing is asserted and nothing \
                             is ruled out (a cdp profile serves this check)"
                        ),
                    },
                ));
            }
            if flags.errors {
                checks.push((
                    "check_errors",
                    CheckOutcome::Unverified {
                        reason: format!(
                            "driver '{driver}' does not fold uncaught page exceptions into the \
                             console ring — only the cdp backend subscribes \
                             Runtime.exceptionThrown (a cdp profile serves this check)"
                        ),
                    },
                ));
            }
        } else {
            let mut new_errors: Vec<&str> = Vec::new();
            let mut new_warns: Vec<&str> = Vec::new();
            for line in &sub.new {
                match console_level(line) {
                    Some(ConsoleLevel::Error) => new_errors.push(line),
                    Some(ConsoleLevel::Warn) => new_warns.push(line),
                    None => {}
                }
            }
            let matched_errors = sub
                .matched
                .iter()
                .filter(|l| console_level(l) == Some(ConsoleLevel::Error))
                .count();
            if flags.errors {
                if !new_errors.is_empty() {
                    checks.push((
                        "check_errors",
                        CheckOutcome::Fail {
                            detail: format!(
                                "{} new error-level console line(s) (console.error and uncaught \
                                 exceptions share the cdp [error] ring): {}",
                                new_errors.len(),
                                sample_lines(&new_errors)
                            ),
                        },
                    ));
                } else {
                    checks.push(("check_errors", CheckOutcome::Pass));
                }
                if matched_errors > 0 {
                    checks.push((
                        "check_errors",
                        CheckOutcome::Unverified {
                            reason: format!(
                                "{matched_errors} error-level line(s) matched the baseline \
                                 allowance — an identical line emitted inside the QA window is \
                                 indistinguishable from pre-existing residue, so its newness \
                                 cannot be confirmed"
                            ),
                        },
                    ));
                }
            }
            if flags.console {
                // Error lines fall to check_console only when check_errors is
                // off — otherwise they are owned above, once (判据 §1).
                if !flags.errors && !new_errors.is_empty() {
                    checks.push((
                        "check_console",
                        CheckOutcome::Fail {
                            detail: format!(
                                "{} new error-level console line(s): {}",
                                new_errors.len(),
                                sample_lines(&new_errors)
                            ),
                        },
                    ));
                } else {
                    checks.push(("check_console", CheckOutcome::Pass));
                }
                if !new_warns.is_empty() {
                    checks.push((
                        "check_console",
                        CheckOutcome::Warn {
                            detail: format!(
                                "{} new warning-level console line(s): {}",
                                new_warns.len(),
                                sample_lines(&new_warns)
                            ),
                        },
                    ));
                }
                if !flags.errors && matched_errors > 0 {
                    checks.push((
                        "check_console",
                        CheckOutcome::Unverified {
                            reason: format!(
                                "{matched_errors} error-level line(s) matched the baseline \
                                 allowance — an identical line emitted inside the QA window is \
                                 indistinguishable from pre-existing residue"
                            ),
                        },
                    ));
                }
            }
        }
    }
    if let Some(sub) = network_sub {
        if !flags.network {
            return;
        }
        if !precise {
            checks.push((
                "check_network",
                CheckOutcome::Unverified {
                    reason: format!(
                        "driver '{driver}' network log is not Aleph's '<- status url' ring, so \
                         failed responses are not parsed — nothing is asserted and nothing is \
                         ruled out (a cdp profile serves this check)"
                    ),
                },
            ));
            return;
        }
        let mut actionable: Vec<String> = Vec::new();
        let mut benign: Vec<String> = Vec::new();
        for line in &sub.new {
            if let Some((status, url)) = failed_response(line) {
                // The resource hint is advisory (the ring line carries no
                // resource type); the URL decides (see qa_verdict).
                match qa_verdict::classify_network_failure(url, "") {
                    Impact::Actionable => actionable.push(format!("{status} {url}")),
                    Impact::Benign => benign.push(format!("{status} {url}")),
                }
            }
        }
        let matched_failures = sub
            .matched
            .iter()
            .filter(|l| failed_response(l).is_some())
            .count();
        if !actionable.is_empty() {
            checks.push((
                "check_network",
                CheckOutcome::Fail {
                    detail: format!(
                        "{} new failed response(s) (status >= 400): {}",
                        actionable.len(),
                        sample_lines(&actionable)
                    ),
                },
            ));
        } else {
            checks.push(("check_network", CheckOutcome::Pass));
        }
        if !benign.is_empty() {
            checks.push((
                "check_network",
                CheckOutcome::Warn {
                    detail: format!(
                        "{} new low-impact failed response(s) (icon/sourcemap/analytics \
                         class): {}",
                        benign.len(),
                        sample_lines(&benign)
                    ),
                },
            ));
        }
        if matched_failures > 0 {
            checks.push((
                "check_network",
                CheckOutcome::Unverified {
                    reason: format!(
                        "{matched_failures} failed-response line(s) matched the baseline \
                         allowance — an identical failure inside the QA window is \
                         indistinguishable from pre-existing residue"
                    ),
                },
            ));
        }
    }
}

/// Up to three example lines in a check detail, bounded — the detail is a
/// sentence for the model, not a log dump (the full new-lines list is in
/// `evidence`'s tails).
fn sample_lines(lines: &[impl AsRef<str>]) -> String {
    const MAX_SAMPLE: usize = 3;
    const MAX_LINE: usize = 160;
    let mut shown: Vec<String> = lines
        .iter()
        .take(MAX_SAMPLE)
        .map(|l| {
            let l = l.as_ref();
            if l.chars().count() > MAX_LINE {
                let cut: String = l.chars().take(MAX_LINE).collect();
                format!("{cut}…")
            } else {
                l.to_string()
            }
        })
        .collect();
    if lines.len() > MAX_SAMPLE {
        shown.push(format!("…and {} more", lines.len() - MAX_SAMPLE));
    }
    shown.join(" | ")
}

/// Engine/driver-level failure — the ONLY way a QA run fails as a tool.
/// Assertion failures are verdicts; a dead tab, a transport error or an SSRF
/// refusal leaves through the `backend_error_text` throat (spec §4).
fn engine_failure(manager: &ProfileManager, err: &BrowserError) -> BrowserQaOutput {
    BrowserQaOutput::failed(format!(
        "QA run aborted: {}",
        super::backend_error_text(manager, err)
    ))
}

/// The managed evidence path: `~/.aleph/qa/<profile>/<utc>-<tab_id>.png`,
/// same shape as the recording default (component sanitization is shared,
/// not copied).
fn evidence_path(profile: &str, tab_id: &str) -> std::path::PathBuf {
    let home = crate::utils::paths::get_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("aleph"));
    let utc = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    home.join("qa")
        .join(crate::browser::cdp_backend::recording::sanitize_component(
            profile,
        ))
        .join(format!(
            "{utc}-{}.png",
            crate::browser::cdp_backend::recording::sanitize_component(tab_id)
        ))
}

/// Persist the evidence PNG. Never overwrites an existing file — a second
/// run inside the same UTC second moves aside (`uniquify`).
async fn save_evidence_png(
    profile: &str,
    tab_id: &str,
    png: &[u8],
) -> std::result::Result<std::path::PathBuf, String> {
    let path = crate::browser::cdp_backend::recording::uniquify(evidence_path(profile, tab_id));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    tokio::fs::write(&path, png)
        .await
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Assert page health: expectations plus buffer checks, one verdict.
#[derive(Clone)]
pub struct BrowserQaTool {
    manager: Arc<ProfileManager>,
}

impl BrowserQaTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }

    /// The QA pipeline (spec §2①), split from profile/tab resolution so tests
    /// can drive it against a `FakeBackend` directly — the manager's backend
    /// routing is not test-injectable (`snapshot_via` precedent).
    ///
    /// Order: baseline snapshot → expectations via `wait_for` → 150 ms settle
    /// → final read → counted subtraction → classification → verdict →
    /// evidence. Every page-derived byte is redacted + sanitized BEFORE it
    /// reaches a fingerprint, a check detail or an evidence tail, so the
    /// verdict structures only ever carry scrubbed text.
    async fn qa_via(
        &self,
        backend: &dyn BrowserBackend,
        tab_id: &str,
        profile_key: &str,
        v: &Validated,
    ) -> BrowserQaOutput {
        let scrub = |text: &str| -> String {
            crate::security::content_sanitizer::sanitize_external_text(
                &self.manager.redact_content(text),
            )
        };
        let to_lines =
            |text: &str| -> Vec<String> { scrub(text).lines().map(str::to_string).collect() };
        let want_console = v.check_console || v.check_errors;

        // ① Baseline — taken BEFORE the expectation wait, so the QA window
        // covers everything the wait lets the page do.
        let console_baseline = if want_console {
            match backend.console_messages(tab_id).await {
                Ok(text) => text,
                Err(e) => return engine_failure(&self.manager, &e),
            }
        } else {
            String::new()
        };
        let network_baseline = if v.check_network {
            match backend.network_log(tab_id).await {
                Ok(text) => text,
                Err(e) => return engine_failure(&self.manager, &e),
            }
        } else {
            String::new()
        };

        // ② Expectations — delegated to the `wait_for` polling machinery.
        // `Ok(false)` is the assertion failure (a verdict, NOT an error);
        // `Err` is an engine fault and aborts the run through the throat.
        let mut checks: Vec<(&'static str, CheckOutcome)> = Vec::new();
        for text in &v.texts {
            match backend
                .wait_for(tab_id, &WaitCondition::Text(text.clone()), v.timeout_ms)
                .await
            {
                Ok(true) => checks.push(("expected_text", CheckOutcome::Pass)),
                Ok(false) => checks.push((
                    "expected_text",
                    CheckOutcome::Fail {
                        detail: format!(
                            "expected text not visible within {} ms: {}",
                            v.timeout_ms,
                            sample_lines(&[text])
                        ),
                    },
                )),
                Err(e) => return engine_failure(&self.manager, &e),
            }
        }
        if let Some(selector) = &v.selector {
            match backend
                .wait_for(
                    tab_id,
                    &WaitCondition::Selector(selector.clone()),
                    v.timeout_ms,
                )
                .await
            {
                Ok(true) => checks.push(("expected_selector", CheckOutcome::Pass)),
                Ok(false) => checks.push((
                    "expected_selector",
                    CheckOutcome::Fail {
                        detail: format!(
                            "selector matched no element within {} ms: {}",
                            v.timeout_ms,
                            sample_lines(&[selector])
                        ),
                    },
                )),
                Err(e) => return engine_failure(&self.manager, &e),
            }
        }
        if let Some(gone) = &v.gone {
            match backend
                .wait_for(
                    tab_id,
                    &WaitCondition::SelectorGone(gone.clone()),
                    v.timeout_ms,
                )
                .await
            {
                Ok(true) => checks.push(("gone_selector", CheckOutcome::Pass)),
                Ok(false) => checks.push((
                    "gone_selector",
                    CheckOutcome::Fail {
                        detail: format!(
                            "selector still present after {} ms: {}",
                            v.timeout_ms,
                            sample_lines(&[gone])
                        ),
                    },
                )),
                Err(e) => return engine_failure(&self.manager, &e),
            }
        }

        // ③ Diagnostic settle — only when a buffer read follows.
        if want_console || v.check_network {
            tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        }

        // ④ Final read.
        let console_final = if want_console {
            match backend.console_messages(tab_id).await {
                Ok(text) => text,
                Err(e) => return engine_failure(&self.manager, &e),
            }
        } else {
            String::new()
        };
        let network_final = if v.check_network {
            match backend.network_log(tab_id).await {
                Ok(text) => text,
                Err(e) => return engine_failure(&self.manager, &e),
            }
        } else {
            String::new()
        };

        // ⑤⑥ Counted subtraction + classification (both pure — T2's
        // subtract/classify and this file's evaluate_buffer_checks).
        let console_sub = want_console.then(|| {
            qa_verdict::subtract(
                &Baseline::from_lines(&to_lines(&console_baseline)),
                &to_lines(&console_final),
            )
        });
        let network_sub = v.check_network.then(|| {
            qa_verdict::subtract(
                &Baseline::from_lines(&to_lines(&network_baseline)),
                &to_lines(&network_final),
            )
        });
        let precise = backend
            .as_any()
            .is::<crate::browser::cdp_backend::CdpBackend>();
        evaluate_buffer_checks(
            &mut checks,
            precise,
            driver_label(backend),
            &CheckFlags {
                console: v.check_console,
                errors: v.check_errors,
                network: v.check_network,
            },
            console_sub.as_ref(),
            network_sub.as_ref(),
        );

        // ⑦ Verdict.
        let verdict = qa_verdict::assemble(&checks);

        // ⑧ Evidence: the NEW lines the verdict reasoned over, bounded. The
        // screenshot rides the existing verb — QA adds no second gate on top
        // of it (spec §3.3) — and is attached when the run did not pass
        // cleanly (a failure OR an unconfirmed check: both are the cases a
        // model needs eyes for).
        let console_tail = console_sub
            .as_ref()
            .map(|s| super::bound_content_head_tail(&s.new.join("\n"), EVIDENCE_TAIL_MAX).0)
            .unwrap_or_default();
        let network_tail = network_sub
            .as_ref()
            .map(|s| super::bound_content_head_tail(&s.new.join("\n"), EVIDENCE_TAIL_MAX).0)
            .unwrap_or_default();
        let mut screenshot_path = None;
        let mut notes: Vec<String> = Vec::new();
        if v.screenshot && !verdict.passed {
            match backend.screenshot(tab_id, ScreenshotOpts::default()).await {
                Ok(shot) => match save_evidence_png(profile_key, tab_id, &shot.png_bytes).await {
                    Ok(path) => screenshot_path = Some(path.display().to_string()),
                    Err(msg) => notes.push(msg),
                },
                Err(e) => notes.push(format!(
                    "evidence screenshot failed: {}",
                    super::backend_error_text(&self.manager, &e)
                )),
            }
        }

        let message = match notes.is_empty() {
            true => verdict.summary.clone(),
            false => format!("{} ({})", verdict.summary, notes.join("; ")),
        };
        BrowserQaOutput {
            success: true,
            verdict: Some(QaVerdict {
                passed: verdict.passed,
                failed_checks: verdict.failed_checks,
                warnings: verdict.warnings,
                unverified: verdict.unverified,
                summary: verdict.summary,
                evidence: QaEvidence {
                    screenshot_path,
                    console_tail,
                    network_tail,
                },
            }),
            message: Some(message),
        }
    }
}

#[async_trait]
impl AlephTool for BrowserQaTool {
    const NAME: &'static str = "browser_qa";
    // R9 byte discipline: ≤80 bytes (pinned by a test below). Field-level
    // detail lives in the JsonSchema doc comments, not here.
    const DESCRIPTION: &'static str =
        "Assert page health: text/selector expectations plus console/network/error checks";
    type Args = BrowserQaArgs;
    type Output = BrowserQaOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let validated = match validate(&args) {
            Ok(v) => v,
            Err(message) => return Ok(BrowserQaOutput::failed(message)),
        };
        // The caller's key FIRST (the principal boundary), then the backend
        // for that key — never the raw `args.profile` at a live-state method.
        let key = match super::resolve_caller_profile(&self.manager, &args.profile) {
            Ok(key) => key,
            Err(e) => return Ok(engine_failure(&self.manager, &e)),
        };
        let backend = match super::backend_for_key(&self.manager, &key) {
            Ok(backend) => backend,
            Err(e) => return Ok(engine_failure(&self.manager, &e)),
        };
        // ONE listing, like `make_backend_and_tab_guarded`: the tab QA vets
        // and the tab QA reads come from one snapshot. An explicit `tab_id`
        // skips the active-tab question but not the SSRF re-check — QA reads
        // page content (console/network rings, evaluate probes), so it owes
        // the read-time guard every content read performs.
        let tabs = match backend.list_tabs().await {
            Ok(tabs) => tabs,
            Err(e) => return Ok(engine_failure(&self.manager, &e)),
        };
        let tab_id = match &args.tab_id {
            Some(id) => id.clone(),
            None => match tab_registry::active_tab_id(&tabs) {
                Some(id) => id,
                None => {
                    return Ok(BrowserQaOutput::failed(
                        "No tabs open. Use browser_open first.".into(),
                    ));
                }
            },
        };
        if let Some(violation) = super::current_page_block(&self.manager, &tabs, &tab_id).await {
            return Ok(engine_failure(
                &self.manager,
                &BrowserError::NavigationFailed(format!(
                    "current page blocked by SSRF policy ({violation}); \
                     navigate to an allowed URL before reading page content"
                )),
            ));
        }
        self.manager.touch_tab(&key, &tab_id);
        Ok(self
            .qa_via(backend.as_ref(), &tab_id, &key, &validated)
            .await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::testkit::FakeBackend;

    fn manager() -> Arc<ProfileManager> {
        Arc::new(ProfileManager::new(BrowserSystemConfig::default()))
    }

    /// All checks off and no expectations — validation refuses this; tests
    /// enable exactly what they exercise.
    fn bare_args() -> BrowserQaArgs {
        BrowserQaArgs {
            profile: "default".into(),
            tab_id: None,
            expected_text: None,
            expected_selector: None,
            gone_selector: None,
            check_console: false,
            check_errors: false,
            check_network: false,
            screenshot: false,
            timeout_ms: 1000,
        }
    }

    fn validated(args: &BrowserQaArgs) -> Validated {
        validate(args).expect("these args must validate")
    }

    // ---- the four plan-mandated pins -------------------------------------

    /// Review Focus #2: an expectation that never arrives is a FAILED CHECK
    /// inside a SUCCESSFUL tool result — the three-array structure is the
    /// answer the model needs; a bare tool error would drop it.
    #[tokio::test]
    async fn failed_expectations_return_a_successful_tool_result_with_passed_false() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None).with_wait_found(false);
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("Order confirmed".into()));
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        assert!(
            out.success,
            "an assertion failure is a verdict, not a tool error"
        );
        let verdict = out.verdict.expect("verdict present");
        assert!(!verdict.passed);
        assert_eq!(verdict.failed_checks.len(), 1);
        assert_eq!(verdict.failed_checks[0].check, "expected_text");
        assert!(
            verdict.failed_checks[0].detail.contains("Order confirmed"),
            "the detail names the missing text: {:?}",
            verdict.failed_checks[0].detail
        );
        assert!(
            backend.calls().iter().any(|c| c.starts_with("wait:Text")),
            "the wait was delegated to the wait_for machinery: {:?}",
            backend.calls()
        );
    }

    /// Review Focus #1: a dimension the driver cannot serve lands in
    /// `unverified` — with the driver and the door named — and `passed` stays
    /// false. A skip must never masquerade as a pass.
    ///
    /// `FakeBackend` is the honest stand-in for the two legacy drivers: it is
    /// NOT the cdp backend, so the pipeline cannot parse its buffers and must
    /// say so rather than assert a vacuous "no errors".
    #[tokio::test]
    async fn a_dimension_the_driver_cannot_serve_lands_in_unverified_not_pass() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None);
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("hello".into()));
        args.check_console = true;
        args.check_errors = true;
        args.check_network = true;
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        assert!(out.success);
        let verdict = out.verdict.expect("verdict present");
        assert!(
            !verdict.passed,
            "unverified dimensions must block the pass: {verdict:?}"
        );
        assert!(
            verdict.failed_checks.is_empty(),
            "nothing was measured to fail: {:?}",
            verdict.failed_checks
        );
        assert_eq!(
            verdict.unverified.len(),
            3,
            "all three buffer checks are unservable off the cdp backend: {:?}",
            verdict.unverified
        );
        for entry in &verdict.unverified {
            assert!(
                entry.detail.contains("unknown") && entry.detail.contains("cdp"),
                "the reason names the driver AND the door that opens: {entry:?}"
            );
        }
        assert!(
            verdict.summary.contains("通过 1 项"),
            "summary counts the pass: {}",
            verdict.summary
        );
        assert!(
            verdict.summary.contains("3 项无法证实"),
            "summary counts the unverified: {}",
            verdict.summary
        );
    }

    /// A dead tab is an engine-level fact, not a verdict: T1's TabGone
    /// surfaces through the `backend_error_text` throat — the tool fails,
    /// the message carries the tab-gone prose, and the recovery trailer
    /// names the door (browser_tabs list).
    #[tokio::test]
    async fn qa_on_a_dead_tab_answers_tab_gone_immediately() {
        let tool = BrowserQaTool::new(manager());
        // The first backend call in the pipeline (the console baseline read)
        // answers TabGone.
        let backend = FakeBackend::new(Some(1)).with_failure_error(BrowserError::TabGone {
            tab_id: "1".into(),
            last_url: Some("https://example.com".into()),
        });
        let mut args = bare_args();
        args.check_console = true;
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        assert!(
            !out.success,
            "TabGone is throat business, not a verdict: {out:?}"
        );
        assert!(out.verdict.is_none());
        let message = out.message.expect("message present");
        assert!(message.contains("is gone"), "{message}");
        assert!(
            message.contains("browser_tabs"),
            "names the door: {message}"
        );
        assert!(
            message.contains("tab_gone"),
            "the recovery trailer classifies it: {message}"
        );
        // And it answered on the FIRST backend call — no budget was spent
        // discovering what the death event already knew.
        assert_eq!(
            backend.calls().len(),
            1,
            "the run aborted at the first verb: {:?}",
            backend.calls()
        );
    }

    /// R9: the DESCRIPTION ships on every tool listing. Spec-measured at
    /// exactly 80 bytes — pinned both ways so an edit in either direction is
    /// a deliberate act.
    #[test]
    fn description_stays_within_the_80_byte_discipline() {
        assert!(
            BrowserQaTool::DESCRIPTION.len() <= 80,
            "DESCRIPTION is {} bytes (max 80): {:?}",
            BrowserQaTool::DESCRIPTION.len(),
            BrowserQaTool::DESCRIPTION
        );
        assert_eq!(
            BrowserQaTool::DESCRIPTION,
            "Assert page health: text/selector expectations plus console/network/error checks",
            "the spec's measured 80-byte text changed — re-measure and update \
             the catalog ceiling ledger"
        );
    }

    // ---- validation ------------------------------------------------------

    #[test]
    fn a_run_with_nothing_to_check_is_refused() {
        let err = validate(&bare_args()).expect_err("zero checks must not validate");
        assert!(err.contains("nothing to check"), "{err}");
        // screenshot alone is evidence, not a check.
        let mut args = bare_args();
        args.screenshot = true;
        let err = validate(&args).expect_err("screenshot is not a check");
        assert!(err.contains("nothing to check"), "{err}");
    }

    #[test]
    fn empty_expectations_are_refused() {
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("   ".into()));
        assert!(validate(&args)
            .expect_err("empty text")
            .contains("non-empty"));
        let mut args = bare_args();
        args.expected_selector = Some(String::new());
        assert!(validate(&args)
            .expect_err("empty selector")
            .contains("empty"));
        let mut args = bare_args();
        args.gone_selector = Some("  ".into());
        assert!(validate(&args).expect_err("empty gone").contains("empty"));
    }

    #[test]
    fn expected_text_arrays_are_capped() {
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::Many(
            (0..=MAX_EXPECTED_TEXTS).map(|i| format!("t{i}")).collect(),
        ));
        let err = validate(&args).expect_err("over the cap");
        assert!(err.contains("at most"), "{err}");
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::Many(
            (0..MAX_EXPECTED_TEXTS).map(|i| format!("t{i}")).collect(),
        ));
        assert!(validate(&args).is_ok());
    }

    #[test]
    fn timeout_is_clamped_to_the_qa_window() {
        let mut args = bare_args();
        args.check_console = true;
        args.timeout_ms = 0;
        assert_eq!(validated(&args).timeout_ms, MIN_TIMEOUT_MS);
        args.timeout_ms = u64::MAX;
        assert_eq!(validated(&args).timeout_ms, MAX_TIMEOUT_MS);
    }

    // ---- the pipeline ----------------------------------------------------

    /// A passing expectation with every buffer check off is a clean pass —
    /// the inverse pin of the two Review Focus tests (passed=true is
    /// reachable only when nothing failed AND nothing is unverified).
    #[tokio::test]
    async fn a_met_expectation_with_all_checks_off_passes_cleanly() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None);
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::Many(vec![
            "Order confirmed".into(),
            "Total: $9".into(),
        ]));
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        assert!(out.success);
        let verdict = out.verdict.expect("verdict present");
        assert!(verdict.passed, "{verdict:?}");
        assert!(verdict.unverified.is_empty());
        assert!(verdict.summary.contains("通过 2 项"), "{}", verdict.summary);
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|c| c.starts_with("wait:Text"))
                .count(),
            2,
            "each array entry got its own bounded wait: {:?}",
            backend.calls()
        );
    }

    /// gone_selector drives the SelectorGone arm (T1's variant), with inverse
    /// polarity: a selector that will not go away fails the check.
    #[tokio::test]
    async fn a_selector_that_stays_fails_gone_selector() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None).with_wait_found(false);
        let mut args = bare_args();
        args.gone_selector = Some(".spinner".into());
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        let verdict = out.verdict.expect("verdict present");
        assert!(!verdict.passed);
        assert_eq!(verdict.failed_checks[0].check, "gone_selector");
        assert!(verdict.failed_checks[0].detail.contains(".spinner"));
        assert!(
            backend
                .calls()
                .iter()
                .any(|c| c.starts_with("wait:SelectorGone")),
            "the wait used the SelectorGone variant, not Selector: {:?}",
            backend.calls()
        );
    }

    /// On the cdp backend the pipeline reads both rings twice (baseline +
    /// final) and the evidence tails carry the NEW lines — the FakeBackend
    /// answers both reads with the same text, so everything is baseline-
    /// matched and the tails are empty.
    #[tokio::test]
    async fn evidence_tails_carry_only_the_new_lines() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None)
            .with_console_text("[log] page booted\n[warn] old warning")
            .with_network_text("<- 200 https://example.com/");
        let mut args = bare_args();
        args.check_console = true;
        args.check_errors = true;
        args.check_network = true;
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        let verdict = out.verdict.expect("verdict present");
        assert!(
            verdict.evidence.console_tail.is_empty(),
            "identical final read subtracts to nothing new: {:?}",
            verdict.evidence.console_tail
        );
        assert!(verdict.evidence.network_tail.is_empty());
        let reads = backend
            .calls()
            .iter()
            .filter(|c| c.as_str() == "console_messages")
            .count();
        assert_eq!(reads, 2, "baseline + final: {:?}", backend.calls());
    }

    /// Evidence screenshot: requested, and the run did not pass → the capture
    /// lands under the managed qa dir and the verdict names the path.
    #[tokio::test]
    async fn a_failing_run_with_screenshot_requested_attaches_the_evidence() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None)
            .with_wait_found(false)
            .with_screenshot_png(b"\x89PNG-fake");
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("never appears".into()));
        args.screenshot = true;
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        let verdict = out.verdict.expect("verdict present");
        assert!(!verdict.passed);
        let path = verdict.evidence.screenshot_path.expect("evidence attached");
        assert!(path.contains("qa"), "{path}");
        let bytes = std::fs::read(&path).expect("the file exists");
        assert_eq!(bytes, b"\x89PNG-fake");
    }

    /// A passing run takes no evidence screenshot even when asked — the
    /// capture exists to explain a non-pass, not to tax a pass.
    #[tokio::test]
    async fn a_passing_run_takes_no_evidence_screenshot() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None);
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("hello".into()));
        args.screenshot = true;
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        let verdict = out.verdict.expect("verdict present");
        assert!(verdict.passed);
        assert!(verdict.evidence.screenshot_path.is_none());
        assert!(
            !backend.calls().iter().any(|c| c.as_str() == "screenshot"),
            "no capture on a pass: {:?}",
            backend.calls()
        );
    }

    // ---- buffer classification (the cdp-precise path, pure) --------------

    fn sub(new: &[&str], matched: &[&str]) -> Subtraction {
        Subtraction {
            new: new.iter().map(|s| s.to_string()).collect(),
            matched: matched.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn flags(console: bool, errors: bool, network: bool) -> CheckFlags {
        CheckFlags {
            console,
            errors,
            network,
        }
    }

    #[test]
    fn new_error_lines_fail_check_errors() {
        let mut checks = Vec::new();
        let console = sub(&["[error] boom", "[log] noise"], &[]);
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(false, true, false),
            Some(&console),
            None,
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_errors"
                    && matches!(o, CheckOutcome::Fail { detail } if detail.contains("boom"))
            }),
            "{checks:?}"
        );
        // The [log] line is informational — not a failure, not a warning.
        assert!(!checks
            .iter()
            .any(|(_, o)| matches!(o, CheckOutcome::Warn { .. })));
    }

    /// Review Focus #3 at the tool layer: a fingerprint the baseline absorbs
    /// is NOT a pass — it is unverified. The mutation "treat matched as
    /// clean" must turn this red.
    #[test]
    fn matched_error_lines_are_unverified_not_pass() {
        let mut checks = Vec::new();
        let console = sub(&[], &["[error] boom"]);
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(true, true, false),
            Some(&console),
            None,
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_errors" && matches!(o, CheckOutcome::Unverified { .. })
            }),
            "{checks:?}"
        );
        // …and no Fail: nothing was MEASURED new.
        assert!(!checks
            .iter()
            .any(|(_, o)| matches!(o, CheckOutcome::Fail { .. })));
    }

    /// New + matched together: the new occurrence fails AND the matched
    /// allowance is still reported unconfirmed.
    #[test]
    fn the_n_plus_1th_error_fails_and_the_allowance_stays_unconfirmed() {
        let mut checks = Vec::new();
        let console = sub(&["[error] boom"], &["[error] boom"]);
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(false, true, false),
            Some(&console),
            None,
        );
        assert!(checks
            .iter()
            .any(|(_, o)| matches!(o, CheckOutcome::Fail { .. })));
        assert!(checks
            .iter()
            .any(|(_, o)| matches!(o, CheckOutcome::Unverified { .. })));
    }

    #[test]
    fn actionable_network_failures_fail_and_benign_ones_warn() {
        let mut checks = Vec::new();
        let network = sub(
            &[
                "<- 500 https://app.example.com/api/users",
                "<- 404 https://cdn.example.com/app.js.map",
                "<- 200 https://example.com/ok",
                "-> GET https://example.com/pending",
            ],
            &[],
        );
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(false, false, true),
            None,
            Some(&network),
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_network"
                    && matches!(o, CheckOutcome::Fail { detail } if detail.contains("api/users"))
            }),
            "{checks:?}"
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_network"
                    && matches!(o, CheckOutcome::Warn { detail } if detail.contains("app.js.map"))
            }),
            "{checks:?}"
        );
    }

    #[test]
    fn new_warn_lines_warn_without_failing() {
        let mut checks = Vec::new();
        let console = sub(&["[warn] deprecated api"], &[]);
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(true, true, false),
            Some(&console),
            None,
        );
        assert!(checks.iter().any(
            |(_, o)| matches!(o, CheckOutcome::Warn { detail } if detail.contains("deprecated"))
        ));
        assert!(!checks
            .iter()
            .any(|(_, o)| matches!(o, CheckOutcome::Fail { .. })));
    }

    /// Error lines have exactly ONE owner: check_errors while it is enabled,
    /// check_console when it is not. Both halves pinned so the fallback never
    /// double-counts nor drops the bucket.
    #[test]
    fn error_lines_have_exactly_one_owner() {
        // check_errors off → check_console owns the error lines.
        let mut checks = Vec::new();
        let console = sub(&["[error] boom"], &[]);
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(true, false, false),
            Some(&console),
            None,
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_console" && matches!(o, CheckOutcome::Fail { .. })
            }),
            "{checks:?}"
        );
        assert!(!checks.iter().any(|(name, _)| *name == "check_errors"));

        // Both on → check_errors owns them; check_console still passes.
        let mut checks = Vec::new();
        evaluate_buffer_checks(
            &mut checks,
            true,
            "cdp",
            &flags(true, true, false),
            Some(&console),
            None,
        );
        assert!(
            checks.iter().any(|(name, o)| {
                *name == "check_errors" && matches!(o, CheckOutcome::Fail { .. })
            }),
            "{checks:?}"
        );
        assert!(
            !checks.iter().any(|(name, o)| {
                *name == "check_console" && matches!(o, CheckOutcome::Fail { .. })
            }),
            "{checks:?}"
        );
    }

    #[test]
    fn off_cdp_every_enabled_buffer_check_is_unverified() {
        let mut checks = Vec::new();
        let console = sub(&["[error] boom"], &[]);
        let network = sub(&["<- 500 https://x/api"], &[]);
        evaluate_buffer_checks(
            &mut checks,
            false,
            "managed (playwright-cli)",
            &flags(true, true, true),
            Some(&console),
            Some(&network),
        );
        assert_eq!(checks.len(), 3, "{checks:?}");
        for (name, outcome) in &checks {
            match outcome {
                CheckOutcome::Unverified { reason } => {
                    assert!(reason.contains("managed (playwright-cli)"), "{reason}");
                    assert!(reason.contains("cdp"), "names the door: {reason}");
                }
                other => panic!("{name} must be unverified off cdp, got {other:?}"),
            }
        }
    }

    // ---- the wire shape ---------------------------------------------------

    /// The verdict serializes in the spec §3 shape: three `{check, detail}`
    /// arrays, `passed`, `summary`, and an `evidence` object inside the
    /// verdict.
    #[tokio::test]
    async fn the_wire_shape_matches_the_spec() {
        let tool = BrowserQaTool::new(manager());
        let backend = FakeBackend::new(None).with_wait_found(false);
        let mut args = bare_args();
        args.expected_text = Some(TextExpectation::One("nope".into()));
        let out = tool
            .qa_via(&backend, "1", "default", &validated(&args))
            .await;
        let json = serde_json::to_value(&out).expect("serializes");
        assert_eq!(json["success"], true);
        let verdict = &json["verdict"];
        assert_eq!(verdict["passed"], false);
        assert_eq!(verdict["failed_checks"][0]["check"], "expected_text");
        assert!(verdict["failed_checks"][0]["detail"].is_string());
        assert!(verdict["warnings"].is_array());
        assert!(verdict["unverified"].is_array());
        assert!(verdict["summary"].is_string());
        assert!(verdict["evidence"]["console_tail"].is_string());
        assert!(verdict["evidence"]["network_tail"].is_string());
        // No screenshot requested → the key is absent, not null.
        assert!(verdict["evidence"].get("screenshot_path").is_none());
    }

    /// `expected_text` accepts both the string and the array spellings on the
    /// wire (spec §3: `string | string[]`).
    #[test]
    fn expected_text_parses_both_spellings() {
        let one: BrowserQaArgs =
            serde_json::from_str(r#"{"expected_text":"hello"}"#).expect("string parses");
        assert!(matches!(one.expected_text, Some(TextExpectation::One(_))));
        let many: BrowserQaArgs =
            serde_json::from_str(r#"{"expected_text":["a","b"]}"#).expect("array parses");
        assert!(matches!(many.expected_text, Some(TextExpectation::Many(_))));
        // The check flags default to true on the wire.
        assert!(one.check_console && one.check_errors && one.check_network);
        assert!(!one.screenshot);
        assert_eq!(one.timeout_ms, 5000);
    }

    /// Every failure mode degrades gracefully with no browser running.
    #[tokio::test]
    async fn call_degrades_without_a_browser() {
        let tool = BrowserQaTool::new(manager());
        let mut args = bare_args();
        args.check_console = true;
        let out = tool.call(args).await.unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }
}
