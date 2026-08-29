//! What to call a run's terminate reason — one table, every terminal client.
//!
//! `RunSummary::terminate_reason` carries a stable token
//! (`TerminateReason::as_static_str` on the core side) and
//! `RunSummary::terminate_detail` carries the granular cap hiding under an
//! umbrella token. Turning that pair into a line a person reads is a question
//! **five** surfaces answer, and before this module each of them answered it
//! privately:
//!
//! `TerminateReason` has fifteen tokens; fourteen of them are halts (all but
//! `completed`, which every badge surface suppresses). Counted against those
//! fourteen:
//!
//! | surface | halt tokens labelled | language | read `terminate_detail` |
//! |---|---|---|---|
//! | `aleph exec` receipt | 10, + a neutral fall-back | Chinese | no |
//! | `aleph watch` feed | 0 — printed the raw token | — | no |
//! | TUI run footer | 4 | English | yes |
//! | Panel halt badge | 13 | localised | yes |
//! | `reply_emitter::cap_notice_for` (channels) | 7 | English | no |
//!
//! Five coverage counts, two languages and two precedence rules for one field —
//! and their union is 13, not 14: `diminishing_returns` had a label on none of
//! the five.
//!
//! The cost was not evenly spread. `aleph exec` rendered
//! `budget_exhausted_partial_result`'s umbrella while the TUI beside it named
//! the actual cap, and `aleph watch` printed `hit_max_iterations` at a human.
//! Worse, `aleph exec`'s fall-through was the only one that *lost* information
//! — an unrecognised token became the neutral "已结束", so a core newer than
//! the binary reported every new halt as "ended". A missing label reads as
//! "not done yet"; a wrong one reads as a fact, and only the second is
//! invisible to the person reading it.
//!
//! **The fifth row is not fixed here, and saying which is the point.** The
//! channel notice is server-side, and the server has a locale source these
//! clients do not (`[general] language`, via `gateway::i18n`). Wiring it into
//! this table would change the bytes every `zh` deployment's channels emit,
//! which is a product decision and not a refactor. It keeps its own seven
//! labels; core's census names it as out of scope rather than implying
//! coverage.
//!
//! # Why this crate
//!
//! Same reason [`crate::trace_presentation`] is here: it is presentation over a
//! wire field, the wire field is defined next door in [`crate::events`], and
//! the clients that render it (`aleph-cli`, `aleph-tui`) are forbidden from
//! depending on `alephcore` and so cannot reach the enum itself. The
//! **vocabulary** is core's; the **words** are the client's; this is where they
//! meet.
//!
//! # The Panel deliberately does not call this
//!
//! `interfaces/webchat` depends on this crate and still keeps its own copy of
//! the strings in `locales/{en,zh}.json`, resolved through `td_string!`. That
//! is not an oversight and it is not drift-by-neglect:
//!
//! * the panel's locale is a **browser-side** setting, not `LC_MESSAGES` — a
//!   reader who set the UI to English on a machine with `LANG=zh_CN.UTF-8` must
//!   get English, so the two resolvers genuinely answer different questions;
//! * `td_string!` resolves keys at **compile time**, so a missing key is a
//!   build error. Reading a `&'static str` out of a table here would trade that
//!   away.
//!
//! What keeps the two copies honest is not prose but
//! `RunHalt::label`'s guard in the panel crate, which walks
//! [`labelled_tokens`] and asserts the panel renders this table's exact bytes
//! in both locales. Two implementations, one measurement — the shape
//! `production_lines` already records for the `#[cfg(test)]` cut. It skips
//! exactly one row, [`CLEAN_TOKEN`], because the panel deliberately has no key
//! for a word it never prints and a key with no reader is the other half of
//! the defect that guard exists to stop.

use std::sync::OnceLock;

/// The one token that means "the run ended the way it was supposed to".
///
/// [`effective_token`] returns `None` for it, because the four *badge*
/// surfaces suppress it entirely — a marker on every finished run is noise.
/// It still has a row in [`LABELS`]: the two *receipt* surfaces
/// (`aleph exec`'s footer, `aleph watch`'s feed) print a status word on every
/// line including the clean one, and giving them the word here is what stops
/// "完成" from being hard-coded twice in a crate with no i18n of its own.
pub const CLEAN_TOKEN: &str = "completed";

/// UI language for a **terminal** client.
///
/// Deliberately not `alephcore`'s `gateway::i18n::Locale`: that one resolves
/// from `[general] language` in the server's `config.toml`, which a CLI cannot
/// read (it loads `~/.aleph/cli.toml`, a different file) and must not read (it
/// may be pointed at a server on another machine). POSIX environment variables
/// are the signal a terminal program actually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiLocale {
    /// Also the answer for `C`, `POSIX`, and an environment that says nothing.
    #[default]
    En,
    Zh,
}

impl UiLocale {
    /// Resolve from the three POSIX variables, in POSIX precedence order.
    ///
    /// Pure, and that is the point: `std::env` is process-global while libtest
    /// runs in parallel, so a resolver that reads it directly can only be
    /// tested behind a mutex — and the sibling tests that forget the mutex go
    /// green for the wrong reason. `interfaces/cli/src/output/icon.rs` pays
    /// exactly that tax for `detect_unicode`; this one does not.
    ///
    /// `LC_MESSAGES`, not `LC_CTYPE`: the charset chain and the language chain
    /// share their first and last link and differ in the middle, and a terminal
    /// with `LC_CTYPE=zh_CN.UTF-8` (set for glyph width) and
    /// `LC_MESSAGES=en_US.UTF-8` is asking for English words in a font that can
    /// draw Chinese. An empty value is "unset" per POSIX, not "the C locale".
    #[must_use]
    pub fn from_locale_vars(
        lc_all: Option<&str>,
        lc_messages: Option<&str>,
        lang: Option<&str>,
    ) -> Self {
        let chosen = [lc_all, lc_messages, lang]
            .into_iter()
            .flatten()
            .find(|v| !v.trim().is_empty());
        match chosen {
            // `zh`, `zh_CN`, `zh_TW.UTF-8`, `zh-Hans` — every Chinese tag in
            // use starts with the ISO-639 code, and matching the code rather
            // than a list of regions is what keeps `zh_HK` from falling to
            // English on the day someone uses it.
            Some(v) if v.to_ascii_lowercase().starts_with("zh") => Self::Zh,
            _ => Self::En,
        }
    }

    /// [`Self::from_locale_vars`] against the live environment, resolved once.
    ///
    /// Cached because a receipt renders many labels per run and the answer
    /// cannot change inside a process — the same `OnceLock` discipline
    /// `icon::use_unicode` applies to the charset half of the same question.
    #[must_use]
    pub fn from_env() -> Self {
        static CACHE: OnceLock<UiLocale> = OnceLock::new();
        *CACHE.get_or_init(|| {
            let read = |k: &str| std::env::var(k).ok();
            Self::from_locale_vars(
                read("LC_ALL").as_deref(),
                read("LC_MESSAGES").as_deref(),
                read("LANG").as_deref(),
            )
        })
    }
}

/// How loudly a surface should render a halt.
///
/// Three states rather than the `completed` / not-`completed` split every
/// surface used to apply, because that split cannot distinguish a run that hit
/// a ceiling from one that died: `aleph watch` painted a crashed run with the
/// same warning glyph as a capped one, and the only thing separating them on
/// screen was a raw token the reader had to know how to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateSeverity {
    /// [`CLEAN_TOKEN`] — say nothing at all.
    Clean,
    /// The run reached a ceiling, a timeout, a veto, or was stopped. Its work
    /// up to that point stands.
    Capped,
    /// The run died. Distinct because the advice differs: a cap invites raising
    /// a budget, a crash does not.
    Failed,
}

/// Classify a token. Unknown tokens are [`TerminateSeverity::Capped`] — the
/// conservative answer, since a core newer than this binary is far more likely
/// to have added a ceiling than a new way to crash, and over-reporting a crash
/// sends a reader hunting for a stack trace that does not exist.
#[must_use]
pub fn severity(token: &str) -> TerminateSeverity {
    match token {
        CLEAN_TOKEN => TerminateSeverity::Clean,
        "failed" => TerminateSeverity::Failed,
        _ => TerminateSeverity::Capped,
    }
}

/// The token a surface should actually render, or `None` when there is nothing
/// to say.
///
/// `detail` beats `reason`: `terminate_reason` collapses every escalated budget
/// exit into the umbrella `budget_exhausted_partial_result`, and
/// `terminate_detail` carries which budget it actually was. Empty strings on
/// either field read as absent — core omits the field rather than sending `""`,
/// but a `""` that did arrive would render as a blank badge, which is the one
/// output worse than no badge.
///
/// One author for a rule three surfaces used to spell separately, and two of
/// them spelled it as "ignore `detail`".
#[must_use]
pub fn effective_token<'a>(reason: Option<&'a str>, detail: Option<&'a str>) -> Option<&'a str> {
    let clean = |v: Option<&'a str>| v.filter(|s| !s.trim().is_empty());
    let reason = clean(reason)?;
    if reason == CLEAN_TOKEN {
        return None;
    }
    Some(clean(detail).unwrap_or(reason))
}

/// `(token, English, Chinese)`.
///
/// The strings are byte-identical to the panel's `chat.halt_*` keys, and a
/// guard in `interfaces/webchat` asserts that they stay so. Wording came from
/// there rather than from `aleph exec`'s older table because it already existed
/// in two languages and had already been reviewed as a pair; where the two
/// disagreed (`verifier_veto` was "目标未达成（已暂停等待指示）" here and
/// "被验证器否决" there) the panel's shorter form won, on the grounds that a
/// badge naming the *cause* leaves the remedy to the prose beneath it.
///
/// [`CLEAN_TOKEN`] has a row here and is skipped by the panel's guard rather
/// than by this table — see its doc for which surfaces need the word.
const LABELS: &[(&str, &str, &str)] = &[
    (CLEAN_TOKEN, "completed", "完成"),
    ("hit_max_iterations", "hit max iterations", "已达迭代上限"),
    (
        "context_budget_exhausted",
        "context budget exhausted",
        "上下文预算耗尽",
    ),
    (
        "max_output_tokens_exhausted",
        "max output tokens reached",
        "输出 token 上限耗尽",
    ),
    (
        "budget_exhausted_partial_result",
        "budget exhausted (partial result)",
        "预算耗尽（保留了部分结果）",
    ),
    (
        "consecutive_failure_cap",
        "repeated tool failures",
        "连续工具调用失败",
    ),
    (
        "empty_response_exhausted",
        "empty model responses",
        "模型连续返回空响应",
    ),
    (
        "reactive_compact_exhausted",
        "context overflow, compaction failed",
        "上下文超窗，自动压缩未能恢复",
    ),
    ("stall_timeout", "stalled", "长时间无进展，已熔断"),
    ("turn_timeout", "turn timed out", "单轮超时"),
    ("verifier_veto", "verifier blocked", "被验证器否决"),
    ("stop_hook_halt", "halted by stop hook", "被 Stop hook 拦截"),
    ("cancelled", "cancelled", "已取消"),
    ("failed", "failed", "运行失败"),
    // Retired on the core side (`TerminateReason::DiminishingReturns` has no
    // producer) and kept for summary back-compat. It gets a row anyway: a
    // persisted summary from an older core can still carry it, and "a token
    // that cannot be produced today" is not the same claim as "a token no
    // reader will ever see".
    ("diminishing_returns", "no further progress", "进展递减"),
];

/// The words on screen for `token`.
///
/// An unrecognised token falls through **verbatim**: a core newer than this
/// binary must still say something true, and the raw token is truer than either
/// a guess or a neutral placeholder. That fall-through is what `aleph exec`
/// used to get wrong in the one direction a reader cannot detect — it answered
/// "已结束" for anything it did not know, so a brand-new halt reason and a
/// clean-ish finish read identically.
#[must_use]
pub fn label(token: &str, locale: UiLocale) -> &str {
    LABELS
        .iter()
        .find(|(t, _, _)| *t == token)
        .map_or(token, |(_, en, zh)| match locale {
            UiLocale::En => *en,
            UiLocale::Zh => *zh,
        })
}

/// Every token this module has words for.
///
/// Exists for the two guards that keep the table honest — core's census
/// (`every terminate token this enum can produce is labelled here`) and the
/// panel's (`the other copy of these strings still matches`). Both walk it
/// rather than restating it, so a fourteenth row is picked up by both on the
/// commit that adds it.
pub fn labelled_tokens() -> impl Iterator<Item = &'static str> {
    LABELS.iter().map(|(t, _, _)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_beats_reason_and_a_clean_run_says_nothing() {
        assert_eq!(
            effective_token(
                Some("budget_exhausted_partial_result"),
                Some("hit_max_iterations")
            ),
            Some("hit_max_iterations"),
            "the umbrella token hid which budget was actually hit",
        );
        assert_eq!(
            effective_token(Some("hit_max_iterations"), None),
            Some("hit_max_iterations"),
        );
        assert_eq!(effective_token(Some("completed"), None), None);
        assert_eq!(effective_token(None, Some("hit_max_iterations")), None);
    }

    /// `""` on either field is absent, not a label.
    #[test]
    fn an_empty_string_is_not_a_reason() {
        assert_eq!(effective_token(Some(""), None), None);
        assert_eq!(
            effective_token(Some("hit_max_iterations"), Some("")),
            Some("hit_max_iterations"),
            "an empty detail fell back to the reason, not to a blank badge",
        );
    }

    #[test]
    fn an_unknown_token_survives_verbatim_in_both_languages() {
        for locale in [UiLocale::En, UiLocale::Zh] {
            assert_eq!(label("quota_exceeded_v9", locale), "quota_exceeded_v9");
        }
    }

    /// Deliberately NOT "the label differs from the token": `cancelled`'s
    /// English label *is* the token, which is a coincidence of vocabulary and
    /// not a missing row. Asserting otherwise would force a worse English word
    /// to satisfy the guard.
    #[test]
    fn every_labelled_token_says_something_in_both_languages() {
        for token in labelled_tokens() {
            let en = label(token, UiLocale::En);
            let zh = label(token, UiLocale::Zh);
            assert!(!en.trim().is_empty(), "{token} has no English words");
            assert!(!zh.trim().is_empty(), "{token} has no Chinese words");
            assert_ne!(en, zh, "{token} renders the same in both locales");
        }
    }

    /// The table is a linear scan keyed by string; a duplicate row would be
    /// unreachable and its wording would silently never render.
    #[test]
    fn no_token_is_listed_twice() {
        let mut seen: Vec<&str> = labelled_tokens().collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate row in LABELS");
        assert!(
            seen.contains(&CLEAN_TOKEN),
            "the receipt surfaces read their clean status word out of this table",
        );
    }

    #[test]
    fn posix_precedence_and_the_c_locale() {
        let z = "zh_CN.UTF-8";
        let e = "en_US.UTF-8";
        assert_eq!(UiLocale::from_locale_vars(Some(z), Some(e), Some(e)), UiLocale::Zh);
        assert_eq!(UiLocale::from_locale_vars(None, Some(z), Some(e)), UiLocale::Zh);
        assert_eq!(UiLocale::from_locale_vars(None, None, Some(z)), UiLocale::Zh);
        // LC_CTYPE is deliberately not consulted: see `from_locale_vars`.
        assert_eq!(UiLocale::from_locale_vars(None, Some(e), Some(z)), UiLocale::En);
        // Nothing set, and the two locales that mean "no locale".
        assert_eq!(UiLocale::from_locale_vars(None, None, None), UiLocale::En);
        for none_at_all in ["C", "POSIX", "c"] {
            assert_eq!(
                UiLocale::from_locale_vars(Some(none_at_all), None, Some(z)),
                UiLocale::En,
                "{none_at_all} is not a Chinese locale",
            );
        }
        // An empty value is unset, so the next link in the chain decides.
        assert_eq!(UiLocale::from_locale_vars(Some(""), None, Some(z)), UiLocale::Zh);
        assert_eq!(UiLocale::from_locale_vars(Some("  "), Some(""), Some(z)), UiLocale::Zh);
        // Region variants and the modern script tag.
        for tag in ["zh", "zh_TW", "zh_HK.Big5", "zh-Hans", "ZH_CN.UTF-8"] {
            assert_eq!(
                UiLocale::from_locale_vars(Some(tag), None, None),
                UiLocale::Zh,
                "{tag}",
            );
        }
    }

    #[test]
    fn a_crash_is_not_a_cap() {
        assert_eq!(severity("failed"), TerminateSeverity::Failed);
        assert_eq!(severity(CLEAN_TOKEN), TerminateSeverity::Clean);
        assert_eq!(severity("hit_max_iterations"), TerminateSeverity::Capped);
        assert_eq!(
            severity("quota_exceeded_v9"),
            TerminateSeverity::Capped,
            "an unknown token must not be reported as a crash",
        );
    }
}
