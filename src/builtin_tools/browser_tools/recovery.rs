//! The structured failure contract for the browser tool face (B1).
//!
//! Every backend error a browser tool surfaces already funnels through
//! [`super::backend_error_text`]. That chokepoint turns a [`BrowserError`]
//! into bounded, redacted *prose*; this module adds the second half of the
//! contract — a machine-readable classification plus a short list of next
//! actions — so the model's recovery is data it can reason about, not prose
//! it has to parse.
//!
//! **R7: nextActions are data, not routing.** Nothing here executes, reorders
//! by preference, or hides the raw error. The trailer is appended *after* the
//! error text, and the error text is never elided in its favour.
//!
//! **Wire shape.** The pinned spec shape is a sibling `"recovery"` JSON key.
//! The browser tools' outputs are 26 independent typed structs with ~50
//! failure literals between them, and this task's file boundary is
//! `recovery.rs` + `mod.rs`, so the sibling key is not reachable without
//! either a hundred-site mechanical diff or a task-local smuggled into the
//! generic serializer (a hidden channel). The recovery is therefore attached
//! as a trailing, line-anchored JSON record inside the `message` the
//! chokepoint already produces:
//!
//! ```text
//! <bounded, redacted error text>
//!
//! recovery: {"category":"stale_ref","next_actions":[{"tool":"browser_snapshot", …}]}
//! ```
//!
//! `RECOVERY_LINE_PREFIX` is the anchor; a consumer splits on the last line
//! that starts with it and parses the remainder. The `{success, message}`
//! shape and the error prose itself are untouched — additive only.
//!
//! `classify` is an **exhaustive match with no wildcard arm**: a new
//! `BrowserError` variant that nobody classified is a compile error here.
//! That is the exhaustiveness guard — it is meant to hurt.

use serde::Serialize;

use crate::browser::error::BrowserError;

/// The line anchor for the machine-readable trailer [`attach`] appends.
/// Everything after the prefix on that line is one compact JSON `Recovery`.
pub(crate) const RECOVERY_LINE_PREFIX: &str = "recovery: ";

/// Why a browser call failed, in fifteen buckets.
///
/// Deliberately coarser than [`BrowserError`]: the model's *next move* has
/// fewer shapes than our errors do. The mapping lives in [`classify`] and
/// nowhere else.
///
/// Three categories — `SecretInInput`, `ApprovalRequired`, `BudgetExhausted`,
/// and today also `DialogPending` / `UnsupportedByDriver` — have no
/// *producing* `BrowserError` variant yet: the input-secret gate, the
/// approval gate and the exec budget speak plain refusal strings, not typed
/// errors. They are kept because the category list is the two-line contract
/// (spec §3) that the exec DSL and future structured gates classify INTO;
/// `classify` never returns them until a variant exists that means them. The
/// `allow(dead_code)` covers exactly those contract-pinned variants; any
/// variant `classify` can return is constructed by it, so the lint still
/// fires for a variant nobody classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // contract-pinned variants with no producing BrowserError yet
pub enum BrowserFailureCategory {
    /// SSRF / post-navigation refusal.
    NavigateBlocked,
    /// The deterministic input-side secret scan refused the text.
    SecretInInput,
    /// A snapshot ref's generation ended, or its page drifted.
    StaleRef,
    /// A recorded targetId is absent from the live enumeration.
    TabGone,
    /// The engine took the command but has not answered inside the budget.
    EngineBusy,
    /// The capability table refuses this verb on this engine.
    UnsupportedByEngine,
    /// A driver-single-side capability (e.g. save_state).
    UnsupportedByDriver,
    /// wait_for / poll_wait_for elapsed.
    WaitTimeout,
    /// The addressed element/tab is not there to act on.
    SelectorMiss,
    /// An unanswered JS dialog is blocking the page.
    DialogPending,
    /// The CDP/MCP/CLI channel itself failed, or never came up.
    Transport,
    /// The approval gate denied or parked the call.
    ApprovalRequired,
    /// A step / wall-clock / character budget ran out.
    BudgetExhausted,
    /// Dispatch was accepted, but the effect probe saw no event arrive.
    EffectNotDelivered,
    /// "I do not know" (判据 §8) — never a near-enough bucket.
    Unknown,
}

/// Whether repeating / following the suggestion is page-safe.
///
/// `NeedsUserDecision` has no registry entry today — the user-decision
/// categories (`NavigateBlocked`, `SecretInInput`, `ApprovalRequired`,
/// `BudgetExhausted`) deliberately suggest NO tool, and the variant exists
/// for the entry a future category may need (spec §3 pins the three-state
/// axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // `NeedsUserDecision` is contract-pinned, entry-less today
pub enum NextActionSafety {
    /// A read, or a retry of the same call — no page state at stake.
    SafeToRetry,
    /// Only the user can unblock this (policy, approval, install).
    NeedsUserDecision,
    /// Following it changes page or session state (switching engines drops
    /// cookies, tabs and every minted ref).
    ChangesPage,
}

/// One suggested follow-up call. `tool` must name a real registered tool —
/// the census in `super::tests` (`recovery_registry_entries_name_real_tools`)
/// walks [`REGISTRY`] against the builtin definitions table, because a
/// suggestion that names a tool nobody registered sends the model into a
/// `ToolError::NotFound` it cannot recover from (the `fallback_registry`
/// ghost-tool lesson, FEATURE_LOCATOR §3.12).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextAction {
    pub tool: &'static str,
    /// One sentence for the model. Static text only — never interpolated
    /// from the error, so the trailer carries zero page-influenced bytes.
    pub reason: String,
    /// A static template of the argument shape, not a filled call.
    pub params_hint: serde_json::Value,
    pub safety: NextActionSafety,
}

/// The trailer payload: the bucket, plus what to try next (possibly empty —
/// some failures only the user can resolve, and inventing a tool suggestion
/// for those would be routing, not data).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recovery {
    pub category: BrowserFailureCategory,
    pub next_actions: Vec<NextAction>,
}

/// The const half of a registry entry. `params_hint` is a builder rather than
/// a value because `serde_json::json!` is not const; a fn pointer keeps the
/// table itself `const` without a second, driftable spelling of the same
/// hint (判据 §1).
pub(crate) struct RegistryEntry {
    pub category: BrowserFailureCategory,
    pub tool: &'static str,
    pub reason: &'static str,
    pub params_hint: fn() -> serde_json::Value,
    pub safety: NextActionSafety,
}

/// One entry per category that has a *sane* recovery. Categories with no
/// entry produce `next_actions: []` — `NavigateBlocked`, `SecretInInput`,
/// `ApprovalRequired` and `BudgetExhausted` are user-decision failures, and
/// `Unknown` means we do not know, so suggesting a tool would be a guess
/// dressed as guidance (判据 §8).
pub(crate) const REGISTRY: &[RegistryEntry] = &[
    RegistryEntry {
        category: BrowserFailureCategory::StaleRef,
        tool: "browser_snapshot",
        reason: "the ref's generation ended — re-snapshot and act on a ref from the new listing",
        params_hint: || serde_json::json!({}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::TabGone,
        tool: "browser_tabs",
        reason: "the tab is gone from the browser — list what is open now; every id captured \
                 before this error may be stale",
        params_hint: || serde_json::json!({"action": "list"}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::EngineBusy,
        tool: "browser_session",
        reason: "the engine has not answered — check what it can do before deciding whether to \
                 retry or switch",
        params_hint: || serde_json::json!({"action": "capabilities"}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::UnsupportedByEngine,
        tool: "browser_session",
        reason: "this engine cannot do the verb — when the error names one that can, switch \
                 this profile to it",
        params_hint: || serde_json::json!({"action": "switch_engine"}),
        safety: NextActionSafety::ChangesPage,
    },
    RegistryEntry {
        category: BrowserFailureCategory::WaitTimeout,
        tool: "browser_snapshot",
        reason: "the wait elapsed — look at the page as it is now before deciding whether the \
                 condition can ever hold",
        params_hint: || serde_json::json!({}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::SelectorMiss,
        tool: "browser_snapshot",
        reason: "the addressed element was not found — a fresh listing shows what is actually \
                 there",
        params_hint: || serde_json::json!({}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::DialogPending,
        tool: "browser_dialog",
        reason: "a JS dialog is blocking the page — answer it (dismiss is the non-committal \
                 choice) before retrying the blocked call",
        params_hint: || serde_json::json!({"action": "dismiss"}),
        safety: NextActionSafety::ChangesPage,
    },
    RegistryEntry {
        category: BrowserFailureCategory::Transport,
        tool: "browser_tabs",
        reason: "the channel to the browser failed — a cheap liveness re-read shows what, if \
                 anything, is still there",
        params_hint: || serde_json::json!({"action": "list"}),
        safety: NextActionSafety::SafeToRetry,
    },
    RegistryEntry {
        category: BrowserFailureCategory::EffectNotDelivered,
        tool: "browser_snapshot",
        reason: "the engine accepted the dispatch but the page saw no effect — the element \
                 most often moved or was replaced mid-dispatch, so re-snapshot before any \
                 blind retry",
        params_hint: || serde_json::json!({}),
        safety: NextActionSafety::SafeToRetry,
    },
];

/// Which bucket `err` belongs in.
///
/// Exhaustive on purpose, and there is no wildcard arm to add: the day
/// `BrowserError` grows a variant, this function stops compiling until
/// somebody decides what the new failure *means* for recovery. That decision
/// is the whole contract — it may not be defaulted.
///
/// `Unknown` is the answer for genuinely heterogeneous string-typed failures
/// (`ActionFailed`, the MCP/CLI server-verdict buckets) and for setup-state
/// errors the category list has no honest bucket for (not-installed,
/// no-session, profile/ownership states). Mapping those to a near-enough
/// bucket would tell the model a fact we did not observe (判据 §8).
pub fn classify(err: &BrowserError) -> BrowserFailureCategory {
    use BrowserFailureCategory as C;
    match err {
        // A launch that never reached a usable browser is the channel never
        // coming up — the transport bucket's pre-flight half.
        BrowserError::LaunchFailed { .. } => C::Transport,
        // "I never heard of that tab id" is an addressing miss, not a
        // vanished tab (that is TabGone — the two license different
        // recoveries, which is why they are different variants).
        BrowserError::TabNotFound(_) => C::SelectorMiss,
        BrowserError::TabGone { .. } => C::TabGone,
        // `NavigationFailed` is also what the read-time SSRF re-check
        // (`make_backend_and_tab_guarded`) returns — blocked and failed-share
        // one bucket because the model's move is the same: get to an allowed
        // page first.
        BrowserError::NavigationFailed(_) => C::NavigateBlocked,
        BrowserError::ActionFailed(_) => C::Unknown,
        BrowserError::Timeout(_) => C::WaitTimeout,
        BrowserError::ChromiumNotFound => C::Unknown,
        BrowserError::EngineUnavailable { .. } => C::Unknown,
        BrowserError::ScreenshotFailed(_) => C::Unknown,
        BrowserError::AttachFailed(_) => C::Transport,
        BrowserError::ChromeMcpError(_) => C::Unknown,
        BrowserError::ChromeMcpTransport(_) => C::Transport,
        BrowserError::PlaywrightCliError(_) => C::Unknown,
        BrowserError::PlaywrightCliNotInstalled => C::Unknown,
        BrowserError::NoSession(_) => C::Unknown,
        BrowserError::AlreadyOnEngine { .. } => C::Unknown,
        BrowserError::Io(_) => C::Transport,
        BrowserError::ProfileNotFound(_) => C::Unknown,
        BrowserError::EngineBusy { .. } => C::EngineBusy,
        BrowserError::UnsupportedByEngine { .. } => C::UnsupportedByEngine,
        BrowserError::EngineFailure { .. } => C::Transport,
        BrowserError::EngineMismatch { .. } => C::Unknown,
        BrowserError::StaleRef { .. } => C::StaleRef,
        BrowserError::EffectNotDelivered { .. } => C::EffectNotDelivered,
        BrowserError::Cdp { .. } => C::Transport,
    }
}

/// The full recovery payload for `err`: its category, plus the registry's
/// entries for that category with hints built.
pub fn recovery_for(err: &BrowserError) -> Recovery {
    let category = classify(err);
    let next_actions = REGISTRY
        .iter()
        .filter(|e| e.category == category)
        .map(|e| NextAction {
            tool: e.tool,
            reason: e.reason.to_string(),
            params_hint: (e.params_hint)(),
            safety: e.safety,
        })
        .collect();
    Recovery {
        category,
        next_actions,
    }
}

/// Append the machine-readable recovery trailer to an already-produced error
/// `message`, returning the composite. Consumed at exactly one place —
/// [`super::backend_error_text`] — so every browser tool failure inherits it
/// and none can forget it.
///
/// The trailer is OUR data (a category from a closed enum, static reasons,
/// static param templates): no byte of it is derived from the error's text,
/// so it adds nothing the sanitizer needed to catch, and it is appended after
/// the sanitize/truncate pipeline so it always survives intact.
pub(crate) fn attach(message: String, err: &BrowserError) -> String {
    let recovery = recovery_for(err);
    // Serializing this shape cannot fail (closed enums, string keys, static
    // text); if it ever did, the error prose is the primary channel and must
    // still reach the model, so the message degrades rather than the call.
    match serde_json::to_string(&recovery) {
        Ok(json) => format!("{message}\n\n{RECOVERY_LINE_PREFIX}{json}"),
        Err(_) => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::engine::Engine;
    use crate::browser::error::StaleReason;

    fn engine() -> Engine {
        Engine::Chromium
    }

    /// One instance per `BrowserError` variant, classified. The pins that
    /// matter are named explicitly; the rest assert only that classify
    /// answered at all (the exhaustive match is what guarantees a decision
    /// was made — a wildcard arm would compile this test green while
    /// defaulting the decision, which is why there isn't one).
    #[test]
    fn every_browser_error_variant_has_a_category() {
        let cases: Vec<BrowserError> = vec![
            BrowserError::LaunchFailed {
                stage: "spawn",
                detail: "x".into(),
            },
            BrowserError::TabNotFound("t".into()),
            BrowserError::TabGone {
                tab_id: "t".into(),
                last_url: None,
            },
            BrowserError::NavigationFailed("x".into()),
            BrowserError::ActionFailed("x".into()),
            BrowserError::Timeout(1),
            BrowserError::ChromiumNotFound,
            BrowserError::EngineUnavailable {
                engine: engine(),
                tried: "x".into(),
                install_hint: "x".into(),
            },
            BrowserError::ScreenshotFailed("x".into()),
            BrowserError::AttachFailed("x".into()),
            BrowserError::ChromeMcpError("x".into()),
            BrowserError::ChromeMcpTransport("x".into()),
            BrowserError::PlaywrightCliError("x".into()),
            BrowserError::PlaywrightCliNotInstalled,
            BrowserError::NoSession("p".into()),
            BrowserError::AlreadyOnEngine {
                profile: "p".into(),
                engine: engine(),
            },
            BrowserError::Io(std::io::Error::other("x")),
            BrowserError::ProfileNotFound("p".into()),
            BrowserError::EngineBusy {
                engine: engine(),
                method: "m".into(),
                waited_secs: 1,
            },
            BrowserError::UnsupportedByEngine {
                engine: engine(),
                verb: "v",
                supported_by: None,
            },
            BrowserError::EngineFailure {
                engine: engine(),
                reason: "x".into(),
            },
            BrowserError::EngineMismatch {
                profile: "p".into(),
                running: engine(),
                requested: engine(),
            },
            BrowserError::StaleRef {
                ref_id: "e1".into(),
                reason: StaleReason::Navigated,
            },
            BrowserError::EffectNotDelivered {
                verb: "browser_click",
                detail: "x".into(),
            },
            BrowserError::Cdp {
                engine: engine(),
                method: "m".into(),
                code: -1,
                message: "x".into(),
            },
        ];
        for err in &cases {
            let _ = classify(err); // must not panic
        }
        // The pins from the plan:
        assert_eq!(
            classify(&BrowserError::TabGone {
                tab_id: "t".into(),
                last_url: None
            }),
            BrowserFailureCategory::TabGone
        );
        assert_eq!(
            classify(&BrowserError::StaleRef {
                ref_id: "e1".into(),
                reason: StaleReason::Navigated
            }),
            BrowserFailureCategory::StaleRef
        );
        assert_eq!(
            classify(&BrowserError::EngineBusy {
                engine: engine(),
                method: "m".into(),
                waited_secs: 1
            }),
            BrowserFailureCategory::EngineBusy
        );
        assert_eq!(
            classify(&BrowserError::EffectNotDelivered {
                verb: "browser_click",
                detail: "x".into()
            }),
            BrowserFailureCategory::EffectNotDelivered
        );
        assert_eq!(
            classify(&BrowserError::Timeout(5)),
            BrowserFailureCategory::WaitTimeout
        );
        assert_eq!(
            classify(&BrowserError::UnsupportedByEngine {
                engine: engine(),
                verb: "v",
                supported_by: None
            }),
            BrowserFailureCategory::UnsupportedByEngine
        );
    }

    /// `Unknown` is a deliberate answer, not the arm nobody wrote. A
    /// navigation refusal is `NavigateBlocked` even though its variant name
    /// says "failed".
    #[test]
    fn unknown_is_not_a_dumping_ground() {
        assert_eq!(
            classify(&BrowserError::NavigationFailed("x".into())),
            BrowserFailureCategory::NavigateBlocked
        );
        // …and the genuinely unclassifiable generic bucket IS Unknown.
        assert_eq!(
            classify(&BrowserError::ActionFailed("x".into())),
            BrowserFailureCategory::Unknown
        );
    }

    /// The trailer: one line, anchored, parseable, snake_case category — and
    /// the error prose rides in front of it, never replaced by it.
    #[test]
    fn attach_appends_a_parseable_trailer_without_hiding_the_error() {
        let err = BrowserError::StaleRef {
            ref_id: "e7".into(),
            reason: StaleReason::Navigated,
        };
        let out = attach("ref e7 is stale".to_string(), &err);
        assert!(out.starts_with("ref e7 is stale"), "error prose first: {out}");
        let line = out
            .lines()
            .find(|l| l.starts_with(RECOVERY_LINE_PREFIX))
            .expect("a recovery line");
        let v: serde_json::Value =
            serde_json::from_str(&line[RECOVERY_LINE_PREFIX.len()..]).expect("trailer is JSON");
        assert_eq!(v["category"], "stale_ref");
        let tools: Vec<&str> = v["next_actions"]
            .as_array()
            .expect("next_actions is a list")
            .iter()
            .filter_map(|a| a["tool"].as_str())
            .collect();
        assert_eq!(tools, ["browser_snapshot"]);
        assert_eq!(v["next_actions"][0]["safety"], "safe_to_retry");
    }

    /// A category with no sane recovery names no tool — the empty list is the
    /// honest answer (判据 §8), and `Unknown` must not invent one.
    #[test]
    fn user_decision_and_unknown_categories_suggest_nothing() {
        for err in [
            BrowserError::ActionFailed("x".into()),
            BrowserError::ChromiumNotFound,
        ] {
            let r = recovery_for(&err);
            assert_eq!(r.category, BrowserFailureCategory::Unknown);
            assert!(r.next_actions.is_empty(), "Unknown suggested a tool");
        }
    }

    /// Every registry tool name is static and every reason is static — the
    /// trailer must carry zero error-derived (potentially page-influenced)
    /// bytes. Pinned by construction: `RegistryEntry.reason` is `&'static str`
    /// and `params_hint` takes no input, so there is nothing to interpolate
    /// from. This test exists so a signature change that opens the channel
    /// (e.g. `reason: String` built from the error) is a deliberate act.
    #[test]
    fn registry_hints_are_static_templates() {
        for entry in REGISTRY {
            let hint = (entry.params_hint)();
            assert!(hint.is_object(), "{:?}'s hint is not an object", entry.tool);
        }
    }
}
