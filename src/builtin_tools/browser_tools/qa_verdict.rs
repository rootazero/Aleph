//! Pure local logic behind `browser_qa` (C3/T2): counted baseline subtraction,
//! a data-table network-failure classifier, and tri-state verdict assembly.
//!
//! This module is **engine-free**: no CDP, no backend trait, no async. Every
//! function is a pure transform over strings so the semantics the spec pins
//! (spec §2②, §3) can be unit-tested to the teeth without a browser.
//!
//! **Honesty triangle (Global Constraints).** `passed == true` ⟺
//! `failed_checks` is empty **and** `unverified` is empty. A check the driver
//! cannot serve, or evidence the baseline made ambiguous, lands in
//! `unverified` with a reason — it never masquerades as a pass.
//!
//! **Counted baseline subtraction.** Attached-mode QA snapshots the
//! console/network ring tails at start (the baseline) and reads them again at
//! the end. A line whose normalized fingerprint occurs N times in the
//! baseline does not count as *new* for its first N occurrences in the final
//! read; occurrence N+1 onward does. This is a port of the reference
//! implementation's `subtractQaBaselineErrors` (job.ts) — counts, not set
//! membership. Lines consumed by the baseline allowance go to `matched`
//! (→ `unverified` at the tool layer): an identical new line inside the QA
//! window is indistinguishable from old residue, and the honesty triangle
//! forbids pretending otherwise.
//!
//! **The classifier is data, not logic (R8).** Substring matching against a
//! constant table — no regex. Every row carries its rationale in a comment
//! and at least one pinned test below.

use std::collections::HashMap;

/// How a [`BenignRule`] pattern matches against a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    /// The pattern appears anywhere in the (lowercased) URL.
    Contains,
    /// The URL path ends with the pattern (sourcemap suffix semantics; the
    /// reference implementation anchors its sourcemap/icon patterns to the
    /// path end, and a bare `.map` substring would false-positive on hosts
    /// like `maps.example.com`... which contain `.map` only across a dot
    /// boundary — suffix keeps the row honest).
    Suffix,
}

/// One row of the benign-failure table: a pattern, how to match it, and why
/// the failure is low-impact. The table is the policy; `classify` is a loop.
struct BenignRule {
    pattern: &'static str,
    kind: MatchKind,
    #[allow(dead_code)] // Documentation payload — the rationale is the row's reason to exist.
    rationale: &'static str,
}

/// Failures matching one of these rows are [`Impact::Benign`]; everything
/// else is [`Impact::Actionable`]. Ordered most-specific-first, though rows
/// are disjoint in practice. Rationale per row:
const BENIGN_RULES: &[BenignRule] = &[
    // Browser-chrome icon: the browser requests it on its own, the page never
    // depends on it, and a 404 has zero user-visible effect. This is the
    // reference implementation's canonical benign case
    // (`isBenignAssetFailure` in results/network.ts).
    BenignRule {
        pattern: "favicon.ico",
        kind: MatchKind::Suffix,
        rationale: "low-impact browser icon asset (reference: isBenignAssetFailure)",
    },
    // Same icon family as favicon: iOS home-screen touch icons, requested by
    // the platform, not the page (reference same regex group).
    BenignRule {
        pattern: "apple-touch-icon.png",
        kind: MatchKind::Suffix,
        rationale: "platform icon asset, page does not depend on it",
    },
    // Sourcemaps are consumed by devtools only; a missing or blocked .map
    // fetch never affects page runtime (spec §2② explicitly names them).
    BenignRule {
        pattern: ".map",
        kind: MatchKind::Suffix,
        rationale: "sourcemap: devtools-only fetch, no runtime impact",
    },
    // Third-party analytics beacon: a blocked/failed hit loses telemetry, not
    // page function (spec §2② names analytics/telemetry domains).
    BenignRule {
        pattern: "google-analytics.com",
        kind: MatchKind::Contains,
        rationale: "analytics beacon; failure loses telemetry, not function",
    },
    // Tag manager bootstraps analytics beacons; same rationale as above.
    BenignRule {
        pattern: "googletagmanager.com",
        kind: MatchKind::Contains,
        rationale: "tag manager feeding analytics; same class as the beacon",
    },
    // Ad/tracking pixel network; blocked by design on many clients, page
    // renders fine without it.
    BenignRule {
        pattern: "doubleclick.net",
        kind: MatchKind::Contains,
        rationale: "ad/tracking pixel; no functional impact",
    },
    // Error-reporting telemetry endpoint: failure means we lose crash
    // reports, the page itself is unaffected.
    BenignRule {
        pattern: "sentry.io",
        kind: MatchKind::Contains,
        rationale: "error-telemetry endpoint; loss is observability, not function",
    },
    // Generic telemetry path/host marker (spec §2② names telemetry). Kept
    // slash-anchored to reduce false positives on app routes.
    BenignRule {
        pattern: "/telemetry",
        kind: MatchKind::Contains,
        rationale: "telemetry endpoint; loss is observability, not function",
    },
];

/// The verdict impact of a failed network request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    /// Document, script, XHR/API, or any non-benign failure: the page very
    /// likely misbehaved because of it.
    Actionable,
    /// Low-impact failure (icon/sourcemap/analytics class): worth a warning,
    /// never a fail on its own.
    Benign,
}

/// Classify one failed network request by URL (and an optional resource-type
/// hint, e.g. "document"/"xhr"/"image"). Substring table lookup, R8-clean
/// (no regex). The hint is currently advisory — the URL decides — but is part
/// of the signature so T3 can thread the driver's resource type through
/// without a breaking change.
pub fn classify_network_failure(url: &str, resource_hint: &str) -> Impact {
    let haystack = url.to_lowercase();
    let hint = resource_hint.to_lowercase();
    for rule in BENIGN_RULES {
        let hit = match rule.kind {
            MatchKind::Contains => haystack.contains(rule.pattern) || hint.contains(rule.pattern),
            MatchKind::Suffix => haystack.ends_with(rule.pattern),
        };
        if hit {
            return Impact::Benign;
        }
    }
    Impact::Actionable
}

/// Normalize a console/network line to its fingerprint: whitespace runs
/// collapse to a single space, ends trimmed (reference job.ts `normalize`).
/// Identity for already-clean lines; deliberately does NOT lowercase — two
/// lines differing only in case are different evidence.
fn fingerprint(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The ring-tail snapshot taken at QA start: normalized fingerprint → count.
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    counts: HashMap<String, usize>,
}

impl Baseline {
    /// Build a baseline from the lines present in the buffers at QA start.
    pub fn from_lines(lines: &[String]) -> Self {
        let mut counts = HashMap::new();
        for line in lines {
            *counts.entry(fingerprint(line)).or_insert(0) += 1;
        }
        Self { counts }
    }
}

/// The result of [`subtract`]: which final-read lines are genuinely new, and
/// which were absorbed by the baseline allowance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subtraction {
    /// Occurrences beyond the baseline allowance — real new evidence.
    pub new: Vec<String>,
    /// Occurrences within the allowance — old residue OR identical new lines;
    /// indistinguishable, so the tool layer must surface them as unverified.
    pub matched: Vec<String>,
}

/// Count-aligned subtraction: a fingerprint seen N times in `baseline` lets
/// the first N occurrences of that fingerprint in `final_lines` pass into
/// `matched`; occurrence N+1 onward lands in `new`. Order of `final_lines` is
/// preserved in both output vectors. The baseline is not mutated (a fresh
/// allowance budget per call).
pub fn subtract(baseline: &Baseline, final_lines: &[String]) -> Subtraction {
    let mut allowance = baseline.counts.clone();
    let mut new = Vec::new();
    let mut matched = Vec::new();
    for line in final_lines {
        let fp = fingerprint(line);
        let budget = allowance.entry(fp).or_insert(0);
        if *budget > 0 {
            *budget -= 1;
            matched.push(line.clone());
        } else {
            new.push(line.clone());
        }
    }
    Subtraction { new, matched }
}

/// One check's outcome, as decided by the qa orchestrator (T3) or by
/// [`assemble`]'s callers. `Pass` carries no detail; the other three carry
/// the human/model-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Pass,
    Fail { detail: String },
    Warn { detail: String },
    Unverified { reason: String },
}

/// One entry of a verdict array: which check, and what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckEntry {
    pub check: String,
    pub detail: String,
}

/// The tri-state verdict (spec §3). `passed` is true **iff** both
/// `failed_checks` and `unverified` are empty — a skip or an ambiguous
/// baseline match never masquerades as a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub passed: bool,
    pub failed_checks: Vec<CheckEntry>,
    pub warnings: Vec<CheckEntry>,
    pub unverified: Vec<CheckEntry>,
    pub summary: String,
}

/// Assemble the verdict from named check outcomes. `checks` is
/// `(check_name, outcome)` pairs in pipeline order; entries are distributed
/// into the three arrays, `passed` is computed by the honesty criterion, and
/// `summary` is the one-sentence verdict. When `unverified` is non-empty the
/// summary MUST say how many checks passed and how many could not be
/// confirmed (「通过 X 项、Y 项无法证实」) — spec §3.
pub fn assemble(checks: &[(&str, CheckOutcome)]) -> Verdict {
    let mut failed_checks = Vec::new();
    let mut warnings = Vec::new();
    let mut unverified = Vec::new();
    let mut pass_count = 0usize;
    for (name, outcome) in checks {
        match outcome {
            CheckOutcome::Pass => pass_count += 1,
            CheckOutcome::Fail { detail } => failed_checks.push(CheckEntry {
                check: (*name).to_string(),
                detail: detail.clone(),
            }),
            CheckOutcome::Warn { detail } => warnings.push(CheckEntry {
                check: (*name).to_string(),
                detail: detail.clone(),
            }),
            CheckOutcome::Unverified { reason } => unverified.push(CheckEntry {
                check: (*name).to_string(),
                detail: reason.clone(),
            }),
        }
    }
    let passed = failed_checks.is_empty() && unverified.is_empty();
    let mut summary = if !failed_checks.is_empty() {
        format!(
            "未通过：{} 项失败、通过 {} 项",
            failed_checks.len(),
            pass_count
        )
    } else {
        format!("通过 {} 项", pass_count)
    };
    if !unverified.is_empty() {
        summary.push_str(&format!("、{} 项无法证实", unverified.len()));
    }
    if !warnings.is_empty() {
        summary.push_str(&format!("（{} 项低影响警告）", warnings.len()));
    }
    Verdict {
        passed,
        failed_checks,
        warnings,
        unverified,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // ---- baseline subtraction: counted alignment -------------------------

    /// Review Focus #3, boundary A: a fingerprint occurring N times in the
    /// baseline and N+1 times in the final read — the N+1-th is NEW.
    /// A boolean-dedup (set membership) implementation would report zero new
    /// here; that mutation must turn this test red.
    #[test]
    fn subtract_counts_occurrences_the_n_plus_1th_occurrence_is_new() {
        let baseline = Baseline::from_lines(&lines(&["boom", "boom"]));
        let out = subtract(&baseline, &lines(&["boom", "boom", "boom"]));
        assert_eq!(out.new, lines(&["boom"]));
        assert_eq!(out.matched, lines(&["boom", "boom"]));
    }

    /// Review Focus #3, boundary B: exactly N occurrences in the final read
    /// — every one is absorbed into `matched` (→ unverified at the tool
    /// layer), nothing is new.
    #[test]
    fn subtract_absorbs_exactly_n_occurrences_all_matched_nothing_new() {
        let baseline = Baseline::from_lines(&lines(&["boom", "boom"]));
        let out = subtract(&baseline, &lines(&["boom", "boom"]));
        assert!(out.new.is_empty());
        assert_eq!(out.matched, lines(&["boom", "boom"]));
    }

    #[test]
    fn subtract_treats_unseen_fingerprints_as_new() {
        let baseline = Baseline::from_lines(&lines(&["old residue"]));
        let out = subtract(&baseline, &lines(&["old residue", "fresh error"]));
        assert_eq!(out.new, lines(&["fresh error"]));
        assert_eq!(out.matched, lines(&["old residue"]));
    }

    #[test]
    fn subtract_with_empty_baseline_reports_everything_new() {
        let baseline = Baseline::from_lines(&[]);
        let out = subtract(&baseline, &lines(&["a", "a"]));
        assert_eq!(out.new, lines(&["a", "a"]));
        assert!(out.matched.is_empty());
    }

    #[test]
    fn subtract_normalizes_whitespace_before_fingerprinting() {
        // Same logical line, different whitespace in the final read — must
        // still match the baseline allowance.
        let baseline = Baseline::from_lines(&lines(&["[error]  something   failed"]));
        let out = subtract(&baseline, &lines(&["[error] something failed"]));
        assert!(out.new.is_empty());
        assert_eq!(out.matched.len(), 1);
    }

    #[test]
    fn subtract_preserves_final_read_order_within_each_bucket() {
        let baseline = Baseline::from_lines(&lines(&["a", "b"]));
        let out = subtract(&baseline, &lines(&["b", "x", "a", "y"]));
        assert_eq!(out.new, lines(&["x", "y"]));
        assert_eq!(out.matched, lines(&["b", "a"]));
    }

    #[test]
    fn subtract_does_not_mutate_the_baseline() {
        let baseline = Baseline::from_lines(&lines(&["a"]));
        let first = subtract(&baseline, &lines(&["a"]));
        let second = subtract(&baseline, &lines(&["a"]));
        assert_eq!(first, second);
        assert!(second.new.is_empty());
    }

    // ---- classifier: every table row pinned ------------------------------

    #[test]
    fn favicon_ico_is_benign() {
        assert_eq!(
            classify_network_failure("https://example.com/favicon.ico", "image"),
            Impact::Benign
        );
    }

    #[test]
    fn apple_touch_icon_is_benign() {
        assert_eq!(
            classify_network_failure("https://example.com/icons/apple-touch-icon.png", "image"),
            Impact::Benign
        );
    }

    #[test]
    fn sourcemap_suffix_is_benign() {
        assert_eq!(
            classify_network_failure("https://cdn.example.com/app.js.map", "script"),
            Impact::Benign
        );
    }

    #[test]
    fn google_analytics_is_benign() {
        assert_eq!(
            classify_network_failure("https://www.google-analytics.com/g/collect?v=2", "xhr"),
            Impact::Benign
        );
    }

    #[test]
    fn googletagmanager_is_benign() {
        assert_eq!(
            classify_network_failure("https://www.googletagmanager.com/gtm.js", "script"),
            Impact::Benign
        );
    }

    #[test]
    fn doubleclick_is_benign() {
        assert_eq!(
            classify_network_failure("https://ad.doubleclick.net/ddm/activity/", "image"),
            Impact::Benign
        );
    }

    #[test]
    fn sentry_is_benign() {
        assert_eq!(
            classify_network_failure("https://o123.ingest.sentry.io/api/456/store/", "fetch"),
            Impact::Benign
        );
    }

    #[test]
    fn telemetry_path_is_benign() {
        assert_eq!(
            classify_network_failure("https://app.example.com/telemetry/events", "xhr"),
            Impact::Benign
        );
    }

    // ---- classifier: actionable pins (the honest default) ----------------

    #[test]
    fn document_failure_is_actionable() {
        assert_eq!(
            classify_network_failure("https://example.com/dashboard", "document"),
            Impact::Actionable
        );
    }

    #[test]
    fn script_failure_is_actionable() {
        assert_eq!(
            classify_network_failure("https://example.com/app.js", "script"),
            Impact::Actionable
        );
    }

    #[test]
    fn xhr_api_failure_is_actionable() {
        assert_eq!(
            classify_network_failure("https://example.com/api/users", "xhr"),
            Impact::Actionable
        );
    }

    /// A non-icon .png is NOT benign — the benign icon rows are anchored,
    /// not a blanket image exemption (reference restricts by filename).
    #[test]
    fn arbitrary_image_failure_is_actionable() {
        assert_eq!(
            classify_network_failure("https://example.com/hero.png", "image"),
            Impact::Actionable
        );
    }

    /// `.map` is a suffix rule: a host that merely contains the substring
    /// across a path boundary must not be swept up.
    #[test]
    fn dot_map_substring_in_the_middle_is_actionable() {
        assert_eq!(
            classify_network_failure("https://example.com/.mapdata/config", "xhr"),
            Impact::Actionable
        );
    }

    /// Case-insensitivity: uppercase URL still hits the table.
    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            classify_network_failure("https://EXAMPLE.com/FAVICON.ICO", "image"),
            Impact::Benign
        );
    }

    // ---- assemble: passed iff failed_checks empty AND unverified empty ---

    /// Passed-criterion, forward direction: unverified non-empty forces
    /// passed=false AND the summary reports the unverified count.
    /// Cutting `unverified` out of the criterion must turn this red.
    #[test]
    fn unverified_checks_block_passing_and_are_reported_in_summary() {
        let verdict = assemble(&[
            ("expected_text", CheckOutcome::Pass),
            (
                "check_errors",
                CheckOutcome::Unverified {
                    reason: "2 rows matched the baseline; old residue and identical new errors are indistinguishable".into(),
                },
            ),
        ]);
        assert!(!verdict.passed);
        assert!(verdict.failed_checks.is_empty());
        assert_eq!(verdict.unverified.len(), 1);
        assert!(
            verdict.summary.contains("通过 1 项"),
            "summary: {}",
            verdict.summary
        );
        assert!(
            verdict.summary.contains("1 项无法证实"),
            "summary: {}",
            verdict.summary
        );
    }

    /// Passed-criterion, reverse direction: all-pass yields passed=true and
    /// the summary claims nothing unverified.
    #[test]
    fn all_passing_checks_yield_a_clean_pass() {
        let verdict = assemble(&[
            ("expected_text", CheckOutcome::Pass),
            ("check_console", CheckOutcome::Pass),
        ]);
        assert!(verdict.passed);
        assert!(verdict.failed_checks.is_empty());
        assert!(verdict.unverified.is_empty());
        assert!(
            !verdict.summary.contains("无法证实"),
            "summary: {}",
            verdict.summary
        );
        assert!(
            verdict.summary.contains("通过 2 项"),
            "summary: {}",
            verdict.summary
        );
    }

    /// A failed check blocks passing even with zero unverified.
    #[test]
    fn a_failed_check_blocks_passing_without_unverified() {
        let verdict = assemble(&[
            (
                "expected_text",
                CheckOutcome::Fail {
                    detail: "expected text not visible: 'Order confirmed'".into(),
                },
            ),
            ("check_console", CheckOutcome::Pass),
        ]);
        assert!(!verdict.passed);
        assert_eq!(verdict.failed_checks.len(), 1);
        assert_eq!(verdict.failed_checks[0].check, "expected_text");
        assert!(
            verdict.summary.contains("1 项失败"),
            "summary: {}",
            verdict.summary
        );
        assert!(
            !verdict.summary.contains("无法证实"),
            "summary: {}",
            verdict.summary
        );
    }

    /// Warnings never affect `passed`.
    #[test]
    fn warnings_do_not_affect_passed() {
        let verdict = assemble(&[
            ("expected_selector", CheckOutcome::Pass),
            (
                "check_network",
                CheckOutcome::Warn {
                    detail: "benign failure ignored: favicon.ico 404".into(),
                },
            ),
        ]);
        assert!(verdict.passed);
        assert_eq!(verdict.warnings.len(), 1);
        assert_eq!(verdict.warnings[0].check, "check_network");
    }

    /// Failure + unverified together: summary must carry both counts.
    #[test]
    fn failure_and_unverified_are_both_reported() {
        let verdict = assemble(&[
            (
                "gone_selector",
                CheckOutcome::Fail {
                    detail: "selector still present: .spinner".into(),
                },
            ),
            (
                "check_network",
                CheckOutcome::Unverified {
                    reason: "driver does not support network_log".into(),
                },
            ),
        ]);
        assert!(!verdict.passed);
        assert!(
            verdict.summary.contains("1 项失败"),
            "summary: {}",
            verdict.summary
        );
        assert!(
            verdict.summary.contains("1 项无法证实"),
            "summary: {}",
            verdict.summary
        );
    }

    /// Degenerate case: no checks at all passes trivially (both arrays empty).
    #[test]
    fn an_empty_check_list_passes_trivially() {
        let verdict = assemble(&[]);
        assert!(verdict.passed);
        assert!(
            verdict.summary.contains("通过 0 项"),
            "summary: {}",
            verdict.summary
        );
    }
}
