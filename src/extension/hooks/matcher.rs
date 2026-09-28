//! A hook's `matcher`: compiled in one place, judged in one place.
//!
//! What a matcher is tested against is the event's
//! [`MatchSubject`](crate::extension::types::MatchSubject). Everything that
//! answers "can this hook fire?" reads [`matcher_verdict`]: the executor's
//! inventory (`hooks list`), `hooks_manage add`'s warning, and the load-time
//! notice every reader of a matcher logs ([`warn_on_matcher`]) —
//! `~/.aleph/hooks.json`, a plugin's `hooks.json` and an `aleph.plugin.toml`
//! `[[hooks]] filter`. They are twins: a notice only one of them gives is a
//! hook another loads in silence.

use crate::extension::types::{HookEvent, MatchSubject, SESSION_SOURCES_FIRED};

/// A matcher, compiled once ([`compile_matcher`]).
#[derive(Debug, Clone)]
pub(crate) enum CompiledMatcher {
    /// `"*"` or `""`: every occurrence, as with no matcher. Claude Code's
    /// wildcard; `*` alone is not a valid regex.
    All,
    /// A regex, searched for in the subject (`Edit|Write`, `mcp__.*`).
    Regex(regex::Regex),
    /// Not a valid regex (or over the size bound): it can never match.
    Invalid(String),
}

impl CompiledMatcher {
    /// Whether this matcher selects `subject`.
    pub(crate) fn is_match(&self, subject: &str) -> bool {
        match self {
            Self::All => true,
            Self::Regex(re) => re.is_match(subject),
            Self::Invalid(_) => false,
        }
    }
}

/// Compile a matcher — the one place, for every hook the executor holds,
/// whichever reader loaded it.
#[must_use]
pub(crate) fn compile_matcher(pattern: &str) -> CompiledMatcher {
    if pattern.is_empty() || pattern == "*" {
        return CompiledMatcher::All;
    }
    match crate::security::safe_regex::bounded_builder(pattern).build() {
        Ok(re) => CompiledMatcher::Regex(re),
        Err(e) => CompiledMatcher::Invalid(e.to_string()),
    }
}

/// Whether a hook with this matcher can fire on `event`, and what is worth
/// saying about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatcherVerdict {
    /// It fires when its subject matches; nothing to say.
    Fires,
    /// It fires, but not the way the matcher reads: the notice says how.
    Caveat(String),
    /// It can never fire; the notice says why.
    Never(String),
}

/// The verdict on `matcher` for `event` — what `hooks list` shows and what a
/// reader logs when it loads the hook.
#[must_use]
pub(crate) fn matcher_verdict(event: HookEvent, matcher: Option<&str>) -> MatcherVerdict {
    let Some(pattern) = matcher else {
        return MatcherVerdict::Fires;
    };
    let subject = event.match_subject();
    match compile_matcher(pattern) {
        CompiledMatcher::All => MatcherVerdict::Fires,
        _ if subject == MatchSubject::Ignored => MatcherVerdict::Caveat(format!(
            "`matcher` `{pattern}` is ignored: Aleph has nothing to test it against on this \
             event, so the hook fires on every occurrence"
        )),
        CompiledMatcher::Invalid(why) => MatcherVerdict::Never(format!(
            "`matcher` `{pattern}` is not a valid regex ({why}), so this hook never fires; \
             use `*` (or no matcher) to match everything"
        )),
        compiled
            if subject == MatchSubject::SessionSource
                && !SESSION_SOURCES_FIRED.iter().any(|s| compiled.is_match(s)) =>
        {
            MatcherVerdict::Never(format!(
                "`matcher` `{pattern}` is tested against the session source, and Aleph fires \
                 this event only as {SESSION_SOURCES_FIRED:?} (a reset session fires as \
                 `startup` too; Aleph never sends `resume` / `clear` / `compact`), so this \
                 hook never fires"
            ))
        }
        // DEVIATION A3: Claude Code's documentation (outside this round's
        // evidence) matches a Notification against its notification type;
        // Aleph has only the tool the permission card is about.
        CompiledMatcher::Regex(_) if event == HookEvent::Notification => {
            MatcherVerdict::Caveat(format!(
                "`matcher` `{pattern}` is tested against the tool name the notification is \
                 about, not a notification type (`permission_prompt`, `idle_prompt`, … never \
                 match here)"
            ))
        }
        CompiledMatcher::Regex(_) => MatcherVerdict::Fires,
    }
}

/// [`matcher_verdict`]'s notice, when it has one: what both writing faces
/// return (`hooks_manage add`, the `hooks.add` RPC) and every reader logs
/// ([`warn_on_matcher`]).
#[must_use]
pub(crate) fn matcher_notice(event: HookEvent, matcher: Option<&str>) -> Option<String> {
    match matcher_verdict(event, matcher) {
        MatcherVerdict::Fires => None,
        MatcherVerdict::Caveat(notice) | MatcherVerdict::Never(notice) => Some(notice),
    }
}

/// Log [`matcher_notice`] for a hook a file reader just loaded — `source` is
/// the file's path or the plugin's id, `event` as written.
pub(crate) fn warn_on_matcher(
    source: &str,
    declared_event: &str,
    event: HookEvent,
    matcher: Option<&str>,
) {
    if let Some(notice) = matcher_notice(event, matcher) {
        tracing::warn!(source, event = declared_event, "hook matcher: {notice}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Code's wildcard and an empty matcher both mean "every
    /// occurrence"; a bare `*` is not a valid regex, so it needs saying.
    #[test]
    fn a_wildcard_and_an_empty_matcher_match_everything() {
        for pattern in ["*", ""] {
            assert!(
                matches!(compile_matcher(pattern), CompiledMatcher::All),
                "{pattern:?}"
            );
        }
        assert!(matches!(compile_matcher("(("), CompiledMatcher::Invalid(_)));
        assert!(compile_matcher("Edit|Write").is_match("Write"));
    }

    /// The census, one row per subject.
    #[test]
    fn the_verdict_follows_the_events_subject() {
        use MatcherVerdict::{Caveat, Fires, Never};
        let v = |event, m| matcher_verdict(event, Some(m));
        assert_eq!(v(HookEvent::BeforeToolCall, "bash"), Fires);
        assert!(matches!(v(HookEvent::BeforeToolCall, "(("), Never(_)));
        assert_eq!(v(HookEvent::SessionStart, "startup|clear|compact"), Fires);
        assert!(matches!(v(HookEvent::SessionStart, "resume"), Never(_)));
        assert!(matches!(
            v(HookEvent::UserPromptSubmit, "anything"),
            Caveat(_)
        ));
        // Ignored means ignored, a broken regex included: the hook fires.
        assert!(matches!(v(HookEvent::Stop, "(("), Caveat(_)));
        assert_eq!(v(HookEvent::Stop, "*"), Fires);
        assert_eq!(matcher_verdict(HookEvent::SessionStart, None), Fires);
        // DEVIATION A3, said rather than listed as a plain match.
        assert!(matches!(
            v(HookEvent::Notification, "permission_prompt"),
            Caveat(_)
        ));
        assert_eq!(v(HookEvent::Notification, "*"), Fires);
    }
}
