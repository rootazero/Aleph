//! Recognising the server's "this needs operator privilege" refusal, so a
//! surface can explain it instead of guessing about it.
//!
//! # The failure this exists to stop
//!
//! Every one of these calls can come back refused for a member, and each place
//! that handles the refusal was inventing its own meaning for it:
//!
//! | Surface | What it did with the refusal | What the user read |
//! |---|---|---|
//! | Quick Setup checklist | `Err(_) => ready.set(Some(false))` | "PENDING — Configure a chat provider" for a provider that IS configured |
//! | Exec-tier pill | `Err(e) => console::warn` | a popover with one blank-labelled row |
//! | Session-mode pill | `Err(e) => console::warn` | no pill at all |
//! | ~20 settings pages | `format!("Failed to load X: {e}")` | a raw English protocol string |
//!
//! Three of those four are the same mistake in different clothes: **a refused
//! read was read as a VALUE.** "I am not allowed to know" became "the answer is
//! no / empty / zero". The first row is the expensive one, because a confident
//! false statement costs more than a blank.
//!
//! # Why this is not a permission check
//!
//! Nothing here decides anything. The Panel deliberately holds no client-side
//! role predicate — `DashboardState::is_operator()` was deleted on 2026-08-07
//! because a role captured at `connect` is wrong in both directions after
//! `handlers::users::restamp_live_connections` re-stamps a live connection, and
//! `cluster.rs` carries a source-level pin against its return. Authorization is
//! server-side (`method_admin.rs` for RPCs, `event_scope.rs` for topics); a
//! surface a member may not use learns so from the refusal it gets back, and
//! this module is only about saying that out loud.
//!
//! So: render, call, and report what the server said. Do not use
//! [`is_admin_refusal`] to pre-emptively hide a page — that is the same gate
//! under a new name.
//!
//! # Single source
//!
//! The gate refuses with `AUTH_REQUIRED` plus a fixed message, but the Panel's
//! RPC layer keeps only `error.message` (the code is dropped in `context.rs`'s
//! response arm), so the text is the only recognisable part. It is matched
//! through [`ADMIN_REQUIRED_MESSAGE`] — **the same `aleph_protocol` constant
//! the server emits** — so a reword moves the server and every consumer here in
//! one edit, instead of stranding members on the raw English string.
//!
//! # This is also where a server error becomes user copy
//!
//! Being the mandatory chokepoint for that (the guard at the bottom of this
//! file admits no exceptions) makes it the only place that sees *both* halves
//! of a message — the caller's framing and the server's — which is why the
//! stutter [`framed_once`] removes can only be removed here.

use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;
use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// Whether this error is the gateway's operator-privilege refusal.
///
/// Use it to tell "refused" apart from "the answer is empty". A checklist step,
/// a picker's option list and a page's contents all have a legitimate empty
/// state, and none of them mean the same thing as this.
#[must_use]
pub fn is_admin_refusal(err: &str) -> bool {
    err.contains(ADMIN_REQUIRED_MESSAGE)
}

/// Copy for a failed call: the operator-privilege refusal is replaced by
/// `explanation`, and **every other error passes through verbatim**.
///
/// The pass-through is the important half. A transport failure, a store error
/// and a malformed response are not permission verdicts, and inventing copy for
/// them would put a guess in front of the user instead of the cause — degraded
/// copy beats a wrong claim.
///
/// `explanation` is the caller's, because only the caller knows what was being
/// attempted; a refused fleet READ and a refused node ENROLMENT are the same
/// verdict about two different actions, and one sentence cannot honestly
/// describe both.
#[must_use]
pub fn labeled(err: &str, explanation: &str) -> String {
    if is_admin_refusal(err) {
        explanation.to_string()
    } else {
        err.to_string()
    }
}

/// Apply a call site's framing to a server error **without saying the verb
/// twice**.
///
/// # The failure this exists to stop
///
/// Both sides of the wire prepend a verb to the same failure, independently
/// and without knowing the other did. A gateway handler writes
/// `format!("Failed to create job: {e}")` into the JSON-RPC `message`, and the
/// page that made the call wraps whatever comes back in `|e| format!("Failed
/// to create job: {e}")`, so chaining a job to one that does not exist reads:
///
/// ```text
/// Failed to create job: Failed to create job: chain target not found: b
/// ```
///
/// Neither layer is wrong to want the verb. The server's message also reaches
/// the CLI and the logs, where nothing else names the operation; the caller's
/// framing is what makes a bare `"Not connected"` mean something. They are only
/// wrong *together* — and this module is the one place both are in scope, which
/// is what makes the fix a single edit rather than a sweep of call sites.
///
/// On 2026-08-10 thirteen verb prefixes were byte-identical across the two
/// trees — `Failed to create job`, `Failed to update scope`, `Failed to audit
/// permissions`, `Failed to toggle task`, … — of which ten reach a user
/// surface and three only `console::error_1`, which is a developer log and not
/// copy. The report named three lines in one file; fixing those by hand would
/// have left the other seven surfaces stuttering behind a green build.
///
/// # Why the collapse is exact, and why near misses are left alone
///
/// The frame's contribution is recovered by subtraction — whatever it put in
/// front of the error — and dropped only when the error already opens with
/// exactly those bytes. Nothing is guessed and nothing is matched loosely.
///
/// `"Failed to save job: Failed to update job: …"` is deliberately untouched.
/// Those two verbs are two different facts (what the user asked for, and what
/// the server was doing), and collapsing them means picking one to delete on a
/// similarity score. Leaving a stutter is a cosmetic loss; deleting the wrong
/// clause is a wrong claim, and this module's whole premise is that the second
/// is more expensive than the first.
///
/// A frame that does not end with the error — `{e:?}`, or wording that
/// continues past the interpolation — leaves no recoverable prefix and is
/// returned untouched.
fn framed_once(err: &str, frame: impl FnOnce(&str) -> String) -> String {
    let composed = frame(err);
    let Some(prefix) = composed.strip_suffix(err) else {
        return composed;
    };
    // An empty prefix means the frame added nothing, so there is nothing to
    // collapse — and `starts_with("")` is true for every string, which would
    // otherwise make this branch look like it fired.
    if !prefix.is_empty() && err.starts_with(prefix) {
        err.to_string()
    } else {
        composed
    }
}

/// What a settings page should display when its load failed.
///
/// A refusal becomes the localized explanation; **every other failure keeps the
/// page's own framing** — that is what `frame` is for, and why this takes a
/// closure instead of a pre-formatted string: `"Failed to load MCP servers: …"`
/// is useful context for a store error and an actively misleading preamble for
/// a permission verdict, which did not fail to load anything, it declined.
///
/// This is copy, not a gate. The page still renders, still calls, and still
/// reports what came back — see this module's doc for why it must not start
/// hiding itself instead.
pub fn settings_load_error(
    i18n: I18nContext<Locale>,
    err: &str,
    frame: impl FnOnce(&str) -> String,
) -> String {
    if is_admin_refusal(err) {
        t_string!(i18n, settings.admin_refusal.read_config).to_string()
    } else {
        framed_once(err, frame)
    }
}

/// What a settings page should display when its **write** failed.
///
/// The mirror of [`settings_load_error`], and it exists because the read half
/// alone left the defect standing on the other side of the same page. Members
/// are not kept off these pages — `settings::StepStatus::Restricted` only
/// colours the Quick Setup checklist — so a page whose load was explained and
/// whose Save button then answered with the raw protocol string was telling the
/// user two different stories about one permission verdict.
///
/// Refusals become the localized explanation; **every other failure keeps the
/// call site's own framing**, for the reason [`settings_load_error`] gives:
/// `"Failed to save: …"` is useful context for a store error and a misleading
/// preamble for a verdict that declined rather than failed.
///
/// One sentence covers every caller deliberately. The distinctions between
/// saving a provider, installing a skill and waking a heartbeat task are real,
/// but they are distinctions about the *action*, and the verdict is about the
/// *role* — a key per call site would be ~170 translations of one fact, and the
/// next call site would arrive without one.
///
/// Which of the two sentences a site gets is decided by the call it made, and
/// on the unframed idiom (`set(Some(e))`, no verb in the copy to read) that is
/// a judgement rather than a derivation — a refused load may say "this change".
/// Left imprecise on purpose: the sentence that matters is "you need operator
/// privilege", and the alternative was a heuristic that walks back through
/// `match` arms and picks up the *other* arm's API call, which is worse than
/// imprecise because it is confidently wrong.
pub fn settings_write_error(
    i18n: I18nContext<Locale>,
    err: &str,
    frame: impl FnOnce(&str) -> String,
) -> String {
    if is_admin_refusal(err) {
        t_string!(i18n, settings.admin_refusal.write_config).to_string()
    } else {
        framed_once(err, frame)
    }
}

/// Guard: no user-facing error signal is fed an unclassified server error.
///
/// # The failure this exists to stop
///
/// The read half of this module landed first and was applied page by page. The
/// write half was not — so on 2026-08-09 this scanner found **154** signal
/// writes that put the raw English protocol string in front of the user, on
/// surfaces whose RPC families are gated in `method_admin.rs`.
///
/// Two things about that number are the lesson. It was reported as **one**
/// site (`routing_rules.rs`), and the first pass at fixing it caught 48 — every
/// site written as `format!("Failed to …")`. That is a **lexical** subset, not
/// the defect: `set(Some(e))` hands over the same string with no framing at
/// all, and `format!("Delete failed: {e}")` is the same bug under a different
/// verb. The split was 91 framed to 76 unframed, so an idiom-shaped rule would
/// have certified nearly half the defect as fixed.
///
/// **And then the rule that replaced it made the same mistake one level up.**
/// It matched two idioms instead of one and filtered on the receiver's *name*,
/// which is still a proxy for "is this a server error" — three proxies, in
/// fact, and on 2026-08-10 each was found holding a different piece of the
/// class it claimed to cover: 14 more sites, 6 of them on `config_template.rs`
/// alone, all green under a guard that had a RED test for every shape it knew
/// about. Lexical rules do not fail loudly; they fail by not seeing. The rule
/// below asks about provenance instead — see [`unclassified_error_writes`].
///
/// # Why the rule is uniform and has no allowlist
///
/// Not every surface here can be refused — a member may use the chat composer
/// and the memory drawer. Wrapping those is still correct rather than noise,
/// because [`labeled`] and both `settings_*_error` helpers pass a non-refusal
/// through **verbatim**: on a surface that is never refused the wrapper is a
/// byte-for-byte no-op. So the uniform rule costs nothing and an allowlist
/// would cost a second source of truth about which families are gated — one
/// that rots the first time a carve-out moves (`heartbeat.list` is a carve-out
/// today). This is the same reasoning [`crate::disposed_reads`] gives for
/// admitting no exceptions.
///
/// # What counts
///
/// A `.set(Some(…)` expression that stores a value **bound by an enclosing
/// `Err(<ident>)` pattern**, on a receiver that can hold a `String`, without
/// the value passing through the classifier.
///
/// # Why provenance and not the idiom, or the receiver's name
///
/// The rule used to be "the receiver's name contains `err`, and the span looks
/// like `format!("… {e}")` or `Some(e)`". Every clause of that was a proxy,
/// and on 2026-08-10 each proxy was found holding a different piece of the
/// same class:
///
/// - **the name**: `set_test_result` and `status_msg` hold `(bool, String)`
///   where the `String` is the server's error on the failure half — five
///   provider "Test connection" panels and the channel binding picker, all
///   invisible because no receiver was called `*err*`;
/// - **the payload idiom**: `Some((false, e))` is neither `Some(e)` nor a
///   `format!`, so even a receiver named `error` slipped through
///   (`cluster.rs`);
/// - **the interpolation**: `format!("{}{}", localized_label, e)` interpolates
///   positionally, so the `{e}` literal never appears — six sites in
///   `config_template.rs`, on the admin-gated `channel.`/`config.` families.
///
/// Asking where the value *came from* answers all three at once, and it is
/// also the honest question: the defect is a **server error** reaching user
/// copy, not a formatting habit. It subsumes the old receiver filter too —
/// `on:click=move |e| last_click.set(Some(e))` binds an event, not an `Err`.
///
/// Three deliberate boundaries:
///
/// - **Console logging is not matched.** It is not a surface, and framing it
///   for a human would be the wrong fix.
/// - **The receiver must be able to hold a `String`** — see
///   [`receiver_holds_a_string`]. Two signals in this crate hold typed errors
///   (`ChatSendError`, `MicError`) that are local facts, never a gateway
///   verdict, and no wrapper can go there because the signal does not take a
///   `String` at all. This is a **type** exception, not an allowlist of pages:
///   it is re-derived from the source on every run and cannot rot into a
///   licence for a surface.
/// - **`Err(_)` is out of scope, and that is a boundary, not an oversight.**
///   A discarded error is a *different* defect — the refusal read as a VALUE,
///   row 1 of this module's own table — and it needs a different fix (a third
///   state in the signal), not a wrapper. It has its own census:
///   [`tests::a_discarded_refusal_is_a_separate_open_class`].
///
/// # What licenses a write
///
/// Either the span mentions `admin_refusal` directly, or it calls a function
/// **this file defines** whose body reaches the classifier — `cluster.rs`'s
/// `fleet_error_label` is one, because that page has three verbs and one
/// verdict. Derived from the source on every run, like the type exception, so
/// it cannot become a per-page licence.
///
/// The sanctioned form puts the frame in a closure, so the original wording
/// survives for non-refusals and the span gains the `admin_refusal` mention:
///
/// ```ignore
/// error.set(Some(admin_refusal::settings_write_error(
///     i18n, &e, |e| format!("Failed to save: {e}"),
/// )));
/// ```
///
/// # Why the opener is found two ways
///
/// The literal `.set(Some(` only matches when both calls land on one line.
/// Inside a `view!` this deeply nested, rustfmt breaks a chain before every
/// `.`, so `error.set(Some(fleet_error_label(…)))` becomes `error`, `.set(`
/// and `Some(` on three separate lines — and the single-line search never
/// sees it, whether the argument is classified or not. Measured:
/// `cluster.rs:339-343` (the deregister handler's `Err` arm) fed a raw
/// server error into the signal and this scanner stayed green, because it
/// was reading a line that no longer contained the shape it was looking for.
///
/// [`set_then_some_on_next_line`] adds the second opener: `.set(` alone at
/// the end of a line, `Some(` alone at the start of the next. It costs no
/// allowlist for the same reason the single-line rule does not need one —
/// `cluster.rs`'s actual call already routes through `fleet_error_label`
/// (`calls_a_local_classifier`, see below), so making the write visible
/// does not turn a correct site into a finding; it turns an unexamined site
/// into an examined one. When the receiver itself is also on its own line
/// (rustfmt's usual break point, one line above `.set(`), [`receiver_ident`]
/// on the `.set(` line's empty prefix returns nothing, so
/// [`receiver_on_line_above`] is the fallback rather than a second proxy.
#[cfg(test)]
fn unclassified_error_writes(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let (at, split) = if let Some(at) = line.find(".set(Some(") {
            (at, false)
        } else if set_then_some_on_next_line(&lines, i) {
            (line.trim_end().len() - ".set(".len(), true)
        } else {
            continue;
        };
        // The identifier itself, not the whole prefix: `Err(e) => { row` would
        // otherwise make the declaration lookup search for a name that is not
        // one and never find it. On the split shape the receiver is blank on
        // this line (it is on the line above), so fall back there.
        let same_line = receiver_ident(&line[..at]);
        let receiver = if same_line.is_empty() {
            receiver_on_line_above(&lines, i)
        } else {
            same_line
        };
        // Span of the write expression: from this line until the parens it
        // opened balance again. Over-reading swallows a later `admin_refusal`
        // and goes quiet; under-reading loses one and shouts. Both are possible
        // with a paren inside a string literal, and the loud direction is the
        // one this scanner is calibrated to take — see the RED tests below,
        // which pin the shapes that actually occur.
        let mut depth = 0i32;
        let mut end = i;
        for (j, inner) in lines.iter().enumerate().skip(i) {
            depth += paren_delta(inner);
            end = j;
            if depth <= 0 {
                break;
            }
        }
        let span = lines[i..=end].join("\n");
        if enclosing_error_binding(&lines, i, &span).is_none() {
            continue;
        }
        if span.contains("admin_refusal") || calls_a_local_classifier(src, &span) {
            continue;
        }
        if !receiver_holds_a_string(src, receiver) {
            continue;
        }
        let text = if split {
            format!(
                "{} {}",
                line.trim(),
                lines.get(i + 1).map_or("", |l| l.trim())
            )
        } else {
            (*line).trim().to_string()
        };
        out.push((i + 1, text));
    }
    out
}

/// Whether line `i` opens a `.set(Some(` write split across two lines:
/// `.set(` alone at the end of `lines[i]`, `Some(` alone at the start of
/// `lines[i + 1]`. This is the shape rustfmt produces for a chain this deep —
/// see [`unclassified_error_writes`]'s doc for the measured proof.
///
/// Only the immediately next line is accepted, not the next non-blank one:
/// every occurrence rustfmt actually produces has zero lines between `.set(`
/// and `Some(`, and reaching further would make this guess at a shape it has
/// no evidence for.
#[cfg(test)]
fn set_then_some_on_next_line(lines: &[&str], i: usize) -> bool {
    lines[i].trim_end().ends_with(".set(")
        && lines
            .get(i + 1)
            .is_some_and(|next| next.trim_start().starts_with("Some("))
}

/// The trailing identifier on the line above `i`, for the split shape where
/// rustfmt has put the receiver alone on its own line and `.set(` starts the
/// next one — so `line[..at]` on the `.set(` line is blank and
/// [`receiver_ident`] has nothing to read there.
#[cfg(test)]
fn receiver_on_line_above<'a>(lines: &[&'a str], i: usize) -> &'a str {
    match i.checked_sub(1) {
        Some(prev) => receiver_ident(lines[prev]),
        None => "",
    }
}

/// The `Err(<ident>)` binding whose value this write stores, if any.
///
/// Walks back from the write to the nearest `Err(` pattern that binds a name
/// the span actually mentions, so an arm's error and a later unrelated `Some(x)`
/// cannot be confused. The window is bounded because a binding thirty lines up
/// is not the one being stored — it is a different arm, and reaching for it
/// would make this shout at code it cannot see the shape of.
///
/// `Err(_)` binds nothing and returns `None` **by construction, not by
/// exception**: there is no server error in that expression to classify. That
/// defect is real and separately censused; see this function's caller.
#[cfg(test)]
fn enclosing_error_binding(lines: &[&str], at: usize, span: &str) -> Option<String> {
    const WINDOW: usize = 30;
    for line in lines[at.saturating_sub(WINDOW)..=at].iter().rev() {
        let mut rest = *line;
        while let Some(idx) = rest.find("Err(") {
            let tail = rest[idx + 4..]
                .trim_start()
                .trim_start_matches("ref ")
                .trim_start_matches("mut ")
                .trim_start();
            let name: String = tail
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // `_` and `_foo` are discards; a binding that starts with a digit
            // is not an identifier at all.
            let bound = !name.is_empty()
                && !name.starts_with('_')
                && !name.starts_with(|c: char| c.is_numeric())
                && tail[name.len()..].starts_with(')');
            if bound && mentions_token(span, &name) {
                return Some(name);
            }
            rest = tail;
        }
    }
    None
}

/// Whether the span hands the error to a function **defined in this file**
/// that reaches the classifier.
///
/// The one shape this exists for: a page with several verbs and one verdict
/// wraps [`labeled`] in a local helper so each call site can name what it was
/// attempting (`cluster.rs::fleet_error_label`). Recognising the wrapper keeps
/// the rule uniform without an allowlist — the alternative was either a false
/// alarm on correct code, or a per-page exemption, which is the second source
/// of truth this module refuses everywhere else.
#[cfg(test)]
fn calls_a_local_classifier(src: &str, span: &str) -> bool {
    let mut rest = span;
    while let Some(idx) = rest.find('(') {
        let before = &rest[..idx];
        let name = receiver_ident(before);
        rest = &rest[idx + 1..];
        if name.is_empty() {
            continue;
        }
        let Some(def) = src.find(&format!("fn {name}(")) else {
            continue;
        };
        // The definition's body, bounded so a later unrelated function's use
        // of the classifier cannot vouch for this one.
        let body_end = src[def..]
            .find("\nfn ")
            .or_else(|| src[def..].find("\npub fn "))
            .map_or(src.len(), |i| def + i);
        if src[def..body_end].contains("admin_refusal") {
            return true;
        }
    }
    false
}

/// Whether `receiver`'s declaration in this file leaves room for a `String`.
///
/// The stored value's type is the whole question: `send_error.set(Some(e))` is
/// this module's business when `e` is the server's `error.message` and is none
/// of its business when `e` is a `ChatSendError`, and the two lines are
/// textually identical.
///
/// **`String` anywhere in the declared type, not `Option<String>` exactly.**
/// The narrower test was itself a hole: `Option::<(bool, String)>` — the
/// provider test-connection panels' `(success, message)` pair — answered
/// "cannot hold a String" about a signal whose second field is precisely the
/// server's error string, so five pages were excused by the check meant to
/// protect `MicError`. `Option::<MicError>` still answers no, which is the
/// only thing this was ever for.
///
/// **Not finding a declaration returns `true`** — a receiver that comes in as a
/// prop or out of a context is reported rather than excused. That is the loud
/// direction, and the quiet one is how a scanner stops enforcing anything.
#[cfg(test)]
fn receiver_holds_a_string(src: &str, name: &str) -> bool {
    for line in src.lines() {
        // A declaration, not a use: it names a signal type.
        if !(line.contains("RwSignal") || line.contains("Signal<") || line.contains("signal(")) {
            continue;
        }
        if !mentions_token(line, name) {
            continue;
        }
        return line.contains("String");
    }
    true
}

/// The identifier a `.set(` was called on: the trailing `[A-Za-z0-9_]` run of
/// everything before it. `self.send_error` yields `send_error`, and
/// `Err(e) => { mic_error` yields `mic_error` rather than the whole match arm.
#[cfg(test)]
fn receiver_ident(prefix: &str) -> &str {
    let end = prefix.trim_end().len();
    let start = prefix[..end]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(end, |(i, _)| i);
    &prefix[start..end]
}

/// `name` appears in `line` as a whole identifier, so `error` does not match
/// inside `save_error` — which would otherwise hand one signal another's type.
#[cfg(test)]
fn mentions_token(line: &str, name: &str) -> bool {
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
    line.match_indices(name).any(|(idx, _)| {
        boundary(line[..idx].chars().next_back())
            && boundary(line[idx + name.len()..].chars().next())
    })
}

/// Paren delta of a line, ignoring whole-line comments.
#[cfg(test)]
fn paren_delta(line: &str) -> i32 {
    let code = if line.trim_start().starts_with("//") {
        ""
    } else {
        line
    };
    i32::try_from(code.matches('(').count()).unwrap_or(0)
        - i32::try_from(code.matches(')').count()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fed the SERVER's own refusal — `aleph_protocol`'s constant, which
    /// `gateway::server::handler` emits verbatim — not a local transcription of
    /// it. That is what makes this able to fail: if recognition ever drifts
    /// from the words the server actually sends, the refusal falls through.
    #[test]
    fn the_servers_own_refusal_is_recognised() {
        assert!(is_admin_refusal(ADMIN_REQUIRED_MESSAGE));
        // Handlers that wrap it in their own framing still match.
        assert!(is_admin_refusal(&format!(
            "Failed to load MCP servers: {ADMIN_REQUIRED_MESSAGE}"
        )));
    }

    #[test]
    fn every_other_failure_is_not_a_permission_verdict() {
        for raw in [
            "Invalid response: missing environments",
            "WebSocket disconnected",
            "Internal error: failed to read enrolled node devices",
            "Not connected",
        ] {
            assert!(!is_admin_refusal(raw), "{raw} must not read as a refusal");
            assert_eq!(labeled(raw, "explained"), raw, "{raw} must pass through");
        }
    }

    #[test]
    fn a_refusal_is_explained_in_the_callers_own_terms() {
        assert_eq!(
            labeled(ADMIN_REQUIRED_MESSAGE, "cannot enrol a node"),
            "cannot enrol a node"
        );
        assert_ne!(
            labeled(ADMIN_REQUIRED_MESSAGE, "cannot enrol a node"),
            ADMIN_REQUIRED_MESSAGE,
            "the refusal must be explained, not echoed as a bare protocol string"
        );
    }

    /// RED proof for [`framed_once`]: the exact sentence a user read after
    /// pointing a cron job's chain at a job that does not exist. `cron/real.rs`
    /// answers `"Failed to create job: chain target not found: b"`, and
    /// `job_editor.rs` frames it with the same words again.
    #[test]
    fn the_verb_is_not_said_twice() {
        let from_server = "Failed to create job: chain target not found: b";
        assert_eq!(
            framed_once(from_server, |e| format!("Failed to create job: {e}")),
            from_server,
            "the caller's frame and the server's are the same words; one of them has to go"
        );
    }

    /// …and the frame still lands where it is the only thing naming the action.
    /// A transport failure arrives with no verb at all, which is the reason the
    /// call sites frame in the first place.
    #[test]
    fn a_frame_still_reaches_an_error_that_carries_no_verb() {
        assert_eq!(
            framed_once("Not connected", |e| format!("Failed to create job: {e}")),
            "Failed to create job: Not connected"
        );
    }

    /// Two *different* verbs are two different facts — what the user asked for
    /// and what the server was doing. Collapsing them would mean choosing one
    /// to delete on a similarity score, which is a wrong claim rather than an
    /// ugly one.
    #[test]
    fn two_different_verbs_are_both_kept() {
        let composed = framed_once("Failed to update job: job not found: x", |e| {
            format!("Failed to save job: {e}")
        });
        assert_eq!(
            composed,
            "Failed to save job: Failed to update job: job not found: x"
        );
    }

    /// A frame whose wording continues past the interpolation leaves no
    /// recoverable prefix. Returning it untouched is the quiet direction here
    /// on purpose: the cost is a stutter, and the loud direction would be
    /// cutting bytes out of a sentence this function cannot parse.
    #[test]
    fn a_frame_that_does_not_end_with_the_error_is_left_alone() {
        assert_eq!(
            framed_once("boom", |e| format!("Failed to save ({e}) — retry?")),
            "Failed to save (boom) — retry?"
        );
        // `{e:?}` quotes the error, so the frame no longer ends with it either.
        assert_eq!(framed_once("boom", |e| format!("{e:?}")), "\"boom\"");
    }

    /// A frame that contributes nothing must not read as a repeat of itself:
    /// every string starts with `""`, so without the emptiness check this would
    /// take the collapsing branch and look like the feature working.
    #[test]
    fn a_frame_that_adds_nothing_is_not_mistaken_for_a_repeat() {
        assert_eq!(
            framed_once("Not connected", |e: &str| e.to_string()),
            "Not connected"
        );
    }

    /// Wiring pin: both copy helpers must compose through [`framed_once`].
    ///
    /// Source-level because the alternative — a behavioural test — needs an
    /// `I18nContext`, and thus a reactive runtime, to reach either function at
    /// all. `framed_once` keeps one caller if the other reverts to `frame(err)`,
    /// so nothing else would notice: no dead code, no failing test, just the
    /// stutter back on half the surfaces.
    ///
    /// `\r` is stripped before splitting because the repo's Windows checkout is
    /// CRLF, where a `\n`-anchored separator matches nothing and every window
    /// below silently becomes the whole file — at which point one wired
    /// function would vouch for both.
    #[test]
    fn both_copy_helpers_compose_through_the_collapse() {
        let src = include_str!("admin_refusal.rs").replace('\r', "");
        for name in ["settings_load_error", "settings_write_error"] {
            let start = src
                .find(&format!("pub fn {name}("))
                .unwrap_or_else(|| panic!("{name} is gone — this pin is now vouching for nothing"));
            let body = &src[start..];
            let end = body[1..]
                .find("\npub fn ")
                .or_else(|| body[1..].find("\n#[cfg(test)]"))
                .map_or(body.len(), |i| i + 1);
            assert!(
                body[..end].contains("framed_once"),
                "{name} frames the error itself instead of going through \
                 `framed_once`, so a server message that already opens with the \
                 caller's own verb is shown to the user twice"
            );
        }
    }

    /// RED proof: `routing_rules.rs`'s Save handler, exactly as it stood before
    /// this guard existed. This is the one that got reported; 166 others were
    /// the same defect.
    #[test]
    fn the_scan_rejects_a_bare_save_handler() {
        let before = r#"
            match result {
                Ok(()) => { error.set(None); }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {e}")));
                }
            }
        "#;
        let found = unclassified_error_writes(before);
        assert_eq!(found.len(), 1, "expected one finding, got {found:?}");
    }

    /// RED proof, second shape: the one-line `Err(e) => …` arm, which is how
    /// `moa/mod.rs` and `providers/detail_panel.rs` write theirs.
    #[test]
    fn the_scan_rejects_a_single_line_error_arm() {
        let before = r#"
            Err(e) => error.set(Some(format!("Failed to delete preset: {e}"))),
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// RED proof, third shape — **the one a `Failed to` scanner misses**, and
    /// the reason this guard matches the class instead of the idiom. There is
    /// no frame at all here: the protocol string goes to the user as-is. This
    /// is `runtimes.rs` and `settings/route.rs` as they stood.
    #[test]
    fn the_scan_rejects_an_unframed_error() {
        let before = r#"
            Err(e) => error_msg.set(Some(e)),
        "#;
        assert_eq!(
            unclassified_error_writes(before).len(),
            1,
            "an error with no frame is the same defect with less evidence"
        );
    }

    /// RED proof, fourth shape: a different verb. `Failed to` was never the
    /// rule — carrying the error is.
    #[test]
    fn the_scan_rejects_a_differently_worded_frame() {
        let before = r#"
            Err(e) => {
                save_error.set(Some(format!("Delete failed: {e}")));
            }
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// RED proof, fifth shape — **a tuple payload**. `set_test_result` holds
    /// `(success, message)`, so the server's error goes in as a field. Neither
    /// `Some(e)` nor a `format!`, and the receiver is not called `*err*`: this
    /// is the shape that was invisible on all three of the old rule's clauses
    /// at once, across five provider panels.
    #[test]
    fn the_scan_rejects_an_error_stored_as_a_tuple_field() {
        let before = r#"
            let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
            Err(e) => {
                set_test_result.set(Some((false, e)));
            }
        "#;
        assert_eq!(
            unclassified_error_writes(before).len(),
            1,
            "an error is no less the server's for being the second half of a pair"
        );
    }

    /// RED proof, sixth shape — **positional interpolation**. A localized
    /// label plus `{}`, so the `{e}` literal the old rule looked for never
    /// appears. Six sites in `config_template.rs`, on the admin-gated
    /// `channel.` / `config.` families.
    #[test]
    fn the_scan_rejects_a_positionally_interpolated_error() {
        let before = r#"
            let error = RwSignal::new(Option::<String>::None);
            Err(e) => {
                error.set(Some(format!(
                    "{}{}",
                    t_string!(i18n, channel_config.toast_save_failed),
                    e
                )));
            }
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// RED proof, seventh shape — **the receiver, `.set(` and `Some(` each on
    /// their own line**, the chain-break rustfmt produces inside a `view!`
    /// this deep. `cluster.rs:339-343` has exactly this shape, and until the
    /// scanner grew [`set_then_some_on_next_line`] as a second opener,
    /// feeding a raw error there left it green — the literal `.set(Some(`
    /// this scanner otherwise looks for never appears on any one line.
    #[test]
    fn the_scan_rejects_a_two_line_set_some_split() {
        let before = r#"
            Err(e) => {
                error
                    .set(
                        Some(e),
                    )
            }
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// …and goes quiet on the same split when the argument is the local
    /// classifier — the shape `cluster.rs::fleet_error_label` actually uses.
    /// A rule that only widened the opener without joining the span across
    /// lines would report this as unclassified, because the classifier call
    /// sits two lines below the line the opener was found on.
    #[test]
    fn the_scan_accepts_a_two_line_local_classifier_wrapper() {
        let after = r#"
            fn fleet_error_label(err: &str, action: &str) -> String {
                crate::components::admin_refusal::labeled(err, &format!("…{action}。"))
            }
            Err(e) => {
                error
                    .set(
                        Some(fleet_error_label(&e, ACTION_READ_FLEET)),
                    )
            }
        "#;
        assert!(
            unclassified_error_writes(after).is_empty(),
            "a split write that goes through a local wrapper is classified, \
             the same as the single-line form"
        );
    }

    /// …and goes quiet on the sanctioned form, whose `format!` is three lines
    /// below the `admin_refusal` mention that licenses it. A line-local check
    /// would report every fixed call site — which is how a guard gets deleted.
    #[test]
    fn the_scan_accepts_the_labeled_form() {
        let after = r#"
            Err(e) => error.set(Some(admin_refusal::settings_write_error(
                i18n,
                &e,
                |e| format!("Failed to save: {e}"),
            ))),
        "#;
        assert!(
            unclassified_error_writes(after).is_empty(),
            "the fixed shape must not be reported"
        );
    }

    /// A page with three verbs and one verdict wraps [`labeled`] locally so
    /// each site can name what it was attempting. That wrapper licenses the
    /// write — the alternative was a false alarm on correct code, or a
    /// per-page exemption, which is the second source of truth this module
    /// refuses everywhere else. This is `cluster.rs::fleet_error_label`.
    #[test]
    fn the_scan_accepts_a_local_classifier_wrapper() {
        let after = r#"
            fn fleet_error_label(err: &str, action: &str) -> String {
                crate::components::admin_refusal::labeled(err, &format!("…{action}。"))
            }
            let error = RwSignal::new(Option::<String>::None);
            Err(e) => error.set(Some(fleet_error_label(&e, ACTION_READ_FLEET))),
        "#;
        assert!(
            unclassified_error_writes(after).is_empty(),
            "a write that goes through a local wrapper is classified, just not on this line"
        );
    }

    /// …and a wrapper that does NOT reach the classifier licenses nothing.
    /// Without this the check above would be "any local function call", which
    /// is a licence anyone can mint by extracting a helper.
    #[test]
    fn a_local_helper_that_does_not_classify_licenses_nothing() {
        let before = r#"
            fn shout(err: &str) -> String {
                err.to_uppercase()
            }
            let error = RwSignal::new(Option::<String>::None);
            Err(e) => error.set(Some(shout(&e))),
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// An event handler binds `e` too, and stores it the same way. The old
    /// receiver-name filter existed to keep those out; provenance does it
    /// better, because it asks the question the name was standing in for.
    #[test]
    fn a_bound_event_is_not_a_bound_error() {
        let ok = r#"
            let last_click = RwSignal::new(Option::<String>::None);
            on:click=move |e| { last_click.set(Some(e)); }
        "#;
        assert!(unclassified_error_writes(ok).is_empty());
    }

    /// Console logging is not a surface. Framing it for a human would be the
    /// wrong fix, and reporting it would push callers into noise.
    #[test]
    fn the_scan_ignores_console_logging() {
        let ok = r#"
            web_sys::console::error_1(&format!("Failed to refresh: {e}").into());
        "#;
        assert!(unclassified_error_writes(ok).is_empty());
    }

    /// A write with no error in sight at all. A guard that cries about
    /// non-errors is a guard someone deletes.
    #[test]
    fn the_scan_ignores_a_non_error_signal() {
        let ok = r#"
            let selected_row = RwSignal::new(Option::<String>::None);
            selected_row.set(Some(row_id));
        "#;
        assert!(unclassified_error_writes(ok).is_empty());
    }

    /// A signal holding a **typed** error is out of scope, and provably so: no
    /// wrapper can go there, because it does not take a `String`. This is
    /// `voice/mod.rs`'s `MicError` — local hardware state, never a verdict.
    #[test]
    fn the_scan_ignores_a_typed_error_signal() {
        let ok = r#"
            let mic_error = RwSignal::new(Option::<MicError>::None);
            Err(e) => { mic_error.set(Some(e)); }
        "#;
        assert!(
            unclassified_error_writes(ok).is_empty(),
            "a typed error signal cannot hold the localized copy, so demanding it is a false alarm"
        );
    }

    /// …and the same shape over a `String` signal in the same file IS reported.
    /// Without this the exception above would be indistinguishable from the
    /// scanner simply not working.
    #[test]
    fn the_type_exception_does_not_disarm_the_string_case() {
        let before = r#"
            let mic_error = RwSignal::new(Option::<MicError>::None);
            let save_error = RwSignal::new(Option::<String>::None);
            Err(e) => { save_error.set(Some(e)); }
        "#;
        assert_eq!(unclassified_error_writes(before).len(), 1);
    }

    /// The declaration lookup matches whole identifiers. `error` reading
    /// `save_error`'s declaration would hand one signal the other's type — and
    /// in a file that has both, that is a coin flip on whether the rule applies.
    #[test]
    fn a_declaration_lookup_does_not_match_a_longer_name() {
        assert!(mentions_token("let error = RwSignal::new(x);", "error"));
        assert!(!mentions_token(
            "let save_error = RwSignal::new(x);",
            "error"
        ));
    }

    /// A receiver this file does not declare — a prop, a context — is reported,
    /// not excused. The quiet direction is how a scanner stops enforcing.
    #[test]
    fn an_undeclared_receiver_is_reported() {
        assert!(receiver_holds_a_string("no declaration here", "error"));
    }

    /// The receiver is the identifier, not the line before it. Taking the whole
    /// prefix made every declaration lookup miss (it searched for
    /// `Err(e) => { mic_error`), which silently turned the type exception off
    /// and made the guard report a signal it cannot apply to.
    #[test]
    fn the_receiver_is_the_identifier_not_the_line() {
        assert_eq!(
            receiver_ident("            Err(e) => { mic_error"),
            "mic_error"
        );
        assert_eq!(receiver_ident("        self.send_error"), "send_error");
        assert_eq!(receiver_ident("    error_msg"), "error_msg");
    }

    /// Sanity: the walk reaches real source. Without this the assertion below
    /// passes vacuously the day the crate layout moves or the manifest dir
    /// changes — a guard that scans nothing is green forever.
    #[test]
    fn the_scan_reaches_the_source_tree() {
        let files = scanned_sources();
        assert!(
            files.len() > 50,
            "found {} sources — the walk is broken, not the code",
            files.len()
        );
        let writes = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter(|src| src.contains(".set(Some("))
            .count();
        assert!(
            writes > 20,
            "only {writes} files contain a signal write — the walk is not \
             reaching the views, so the rule below is not being enforced"
        );
    }

    /// This crate's sources, minus this file. Its RED fixtures are by
    /// construction the exact shape the rule forbids, so scanning it would make
    /// the guard report itself and never go green.
    fn scanned_sources() -> Vec<std::path::PathBuf> {
        crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir())
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n != "admin_refusal.rs"))
            .collect()
    }

    /// Every `Err(_) => <signal>.set(Some(<literal>))` in the crate: a refused
    /// read collapsed into a **fabricated value**.
    ///
    /// # Why this is a census and not part of the rule above
    ///
    /// This is row 1 of this module's table — the expensive one, "a confident
    /// false statement costs more than a blank" — but it is a different
    /// defect with a different fix. There is no error string to route: the
    /// caller threw it away and substituted an answer. Fixing one means giving
    /// the signal a third state (`unknown`) and teaching its render site to
    /// show it, which is a UI decision per site, not a wrapper.
    ///
    /// Folding it into [`unclassified_error_writes`] would have meant either
    /// fixing four unrelated pages in the same change or excusing the shape,
    /// and excusing it is how the previous sweep certified 48 of 154 sites as
    /// done. So it is pinned instead: **this list may shrink, never grow.** A
    /// new site is a hard error with no way to add it except editing this
    /// list, which is the moment someone has to look at it.
    ///
    /// Ranked by what the fabricated value claims:
    ///
    /// - `providers/list.rs` — `Some(false)` on the admin-gated
    ///   `providers.oauth_status`: "not connected" about a provider that is.
    ///   The same sentence, on the same family, as the Quick Setup bug this
    ///   module was created for.
    /// - `providers/detail_panel.rs` — same call, same lie, one panel deeper.
    /// - `home.rs` ×2 — `activity.stats` and `memory.stats` are NOT
    ///   admin-gated, so these are error-swallowing rather than refusals
    ///   misread; `Some(0)` active tasks is still a claim the page cannot
    ///   support.
    #[test]
    fn a_discarded_refusal_is_a_separate_open_class() {
        const KNOWN: [(&str, &str); 4] = [
            ("settings/providers/list.rs", "status_signal"),
            ("settings/providers/detail_panel.rs", "oauth_status"),
            ("home.rs", "memory_stats"),
            ("home.rs", "active_tasks"),
        ];
        let mut found = Vec::new();
        for path in scanned_sources() {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = src.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let Some(at) = line.find(".set(Some(") else {
                    continue;
                };
                // The discard has to be on this line: `Err(_) =>` opening a
                // block whose body writes a signal is the same defect, but a
                // window would also pick up the arm above an unrelated write.
                if !line.contains("Err(_)") {
                    continue;
                }
                found.push((
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string(),
                    receiver_ident(&line[..at]).to_string(),
                    i + 1,
                ));
            }
        }
        let unknown: Vec<_> = found
            .iter()
            .filter(|(file, recv, _)| {
                !KNOWN
                    .iter()
                    .any(|(k, r)| k.ends_with(file.as_str()) && r == recv)
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "a NEW site turns a refused (or failed) read into a fabricated \
             value. Give the signal an `unknown` state instead of a value it \
             cannot support — see this test's doc for why it is not covered by \
             the wrapper rule:\n{unknown:?}"
        );
        assert!(
            found.len() <= KNOWN.len(),
            "expected at most {} known sites, found {}: {found:?}",
            KNOWN.len(),
            found.len()
        );
    }

    #[test]
    fn no_error_signal_is_fed_an_unclassified_error() {
        let mut offenders = Vec::new();
        for path in scanned_sources() {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line, text) in unclassified_error_writes(&src) {
                offenders.push(format!("{}:{line}: {text}", path.display()));
            }
        }
        assert!(
            offenders.is_empty(),
            "an error signal is handed a server error that nothing classified, so \
             on any surface whose RPC family is gated in `method_admin.rs` a \
             member reads the raw protocol string. Route it through \
             `admin_refusal::settings_write_error` (or `settings_load_error` for \
             a read) — on a surface that is never refused the wrapper is a no-op, \
             which is why this rule has no allowlist:\n{}",
            offenders.join("\n")
        );
    }
}
