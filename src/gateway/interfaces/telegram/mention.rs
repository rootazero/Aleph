//! Pure group-mention gating for the Telegram channel.
//!
//! When privacy mode is disabled, a bot receives *every* message in a group it
//! belongs to. Forwarding all of them into the agent loop fires a turn on
//! ambient chatter that has nothing to do with the bot (an R5 "don't disturb
//! violation). When `require_mention` is enabled for an account, only messages
//! that actually address the bot are forwarded:
//!
//! - an `@username` mention anywhere in the text/caption,
//! - a `/command@username` command targeting the bot,
//! - a reply to one of the bot's own messages.
//!
//! All of this is a deterministic I/O filter (R4) — it decides whether the Core
//! ever *sees* the message, not how the LLM should respond to it.

/// Decide whether a group message addresses the bot and should be processed.
///
/// Pure and deterministic: the caller resolves all Telegram I/O into primitives.
///
/// - `text`: the message text or caption (whichever is present).
/// - `reply_to_bot`: `true` when the message replies to a message authored by us.
/// - `bot_username`: the bot's `@username` *without* the leading `@`.
///
/// Fail-open: when `bot_username` is unknown we cannot detect mentions, so we
/// treat the message as addressed rather than silently dropping everything.
#[must_use]
pub fn group_message_addresses_bot(
    text: Option<&str>,
    reply_to_bot: bool,
    bot_username: Option<&str>,
) -> bool {
    if reply_to_bot {
        return true;
    }
    let username = match bot_username {
        Some(u) if !u.is_empty() => u,
        // Unknown handle — cannot gate, so fail open.
        _ => return true,
    };
    match text {
        Some(t) => contains_username_mention(t, username),
        None => false,
    }
}

/// Scan `text` for an `@username` token matching `username` (case-insensitive),
/// bounded so `@foobar` does not match the bot username `foo`.
///
/// UTF-8 safe: candidate ranges are taken with [`str::get`] (never `&s[..n]`),
/// so multi-byte text around an `@` cannot panic (P7).
fn contains_username_mention(text: &str, username: &str) -> bool {
    let ulen = username.len();
    if ulen == 0 {
        return false;
    }
    for (at, _) in text.match_indices('@') {
        let start = at + 1;
        let end = start + ulen;
        let Some(candidate) = text.get(start..end) else {
            // `end` falls outside the string or on a non-char-boundary.
            continue;
        };
        if !candidate.eq_ignore_ascii_case(username) {
            continue;
        }
        // The next char must not extend the username (Telegram handles are
        // [A-Za-z0-9_]); `end` is a valid boundary because `get(start..end)`
        // returned `Some`.
        let boundary_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if boundary_ok {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_to_bot_always_passes() {
        // Even with no text and a known username, a reply to us is addressed.
        assert!(group_message_addresses_bot(None, true, Some("alephbot")));
    }

    #[test]
    fn unknown_username_fails_open() {
        // Cannot gate without knowing our handle — never drop everything.
        assert!(group_message_addresses_bot(
            Some("random chatter"),
            false,
            None
        ));
        assert!(group_message_addresses_bot(
            Some("random chatter"),
            false,
            Some("")
        ));
    }

    #[test]
    fn plain_mention_matches() {
        assert!(group_message_addresses_bot(
            Some("@alephbot hello there"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn mention_is_case_insensitive() {
        assert!(group_message_addresses_bot(
            Some("hey @AlephBot"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn command_targeting_bot_matches() {
        assert!(group_message_addresses_bot(
            Some("/start@alephbot"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn mention_followed_by_punctuation_matches() {
        assert!(group_message_addresses_bot(
            Some("@alephbot, can you help?"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn longer_username_does_not_match() {
        // `@alephbot` must not satisfy a bot named `aleph`.
        assert!(!group_message_addresses_bot(
            Some("@alephbot hi"),
            false,
            Some("aleph"),
        ));
    }

    #[test]
    fn unrelated_handle_does_not_match() {
        assert!(!group_message_addresses_bot(
            Some("@someone_else what's up"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn no_mention_no_reply_is_dropped() {
        assert!(!group_message_addresses_bot(
            Some("just talking among ourselves"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn missing_text_without_reply_is_dropped() {
        // A media-only message with no caption and no reply does not address us.
        assert!(!group_message_addresses_bot(None, false, Some("alephbot")));
    }

    #[test]
    fn email_like_at_does_not_match() {
        // "@host" — username is shorter than "alephbot", never matches.
        assert!(!group_message_addresses_bot(
            Some("mail me@host today"),
            false,
            Some("alephbot"),
        ));
    }

    #[test]
    fn mention_inside_multibyte_text_is_utf8_safe() {
        // Non-ASCII around the `@` must not panic and must still match.
        assert!(group_message_addresses_bot(
            Some("你好 @alephbot 请帮忙"),
            false,
            Some("alephbot"),
        ));
        // A bare `@` next to multi-byte text with a too-short window: no panic.
        assert!(!group_message_addresses_bot(
            Some("@日本語"),
            false,
            Some("alephbot"),
        ));
    }
}
