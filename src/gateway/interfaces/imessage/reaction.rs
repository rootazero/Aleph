//! iMessage tapback reactions — the single source mapping Apple/BlueBubbles
//! reaction codes to their name and display emoji.
//!
//! Shared by the outbound sender (name → code) and both inbound transports
//! (Local chat.db + BlueBubbles: code / BlueBubbles string form → emoji), so the
//! numeric `associatedMessageType` codes live in exactly one place.

/// Canonical *add*-tapback kinds: `(associatedMessageType, name, emoji)`.
///
/// Only the six "add" reactions are listed. The "remove" variants (3000–3005)
/// and unknown types intentionally have no entry, so any emoji lookup against
/// them yields `None` and the reaction is dropped at the mapping layer — this is
/// how "only surface add tapbacks" is enforced without a second filter.
const TAPBACKS: &[(i64, &str, &str)] = &[
    (2000, "love", "❤️"),
    (2001, "like", "👍"),
    (2002, "dislike", "👎"),
    (2003, "laugh", "😂"),
    (2004, "emphasize", "‼️"),
    (2005, "question", "❓"),
];

/// Map a tapback name (BlueBubbles outbound form) to its `associatedMessageType`
/// integer. Returns `None` for unknown names.
#[must_use]
pub fn tapback_code(name: &str) -> Option<i64> {
    TAPBACKS
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(c, _, _)| *c)
}

/// Map an *add*-tapback code to its display emoji. Remove codes (3000–3005) and
/// unknown types return `None` — the caller drops them.
#[must_use]
pub fn tapback_emoji(code: i64) -> Option<&'static str> {
    TAPBACKS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, e)| *e)
}

/// Whether a raw `associatedMessageType` value denotes *any* tapback reaction
/// (add or remove), in either the integer or BlueBubbles string form. Used to
/// tell reaction records apart from normal messages before deciding whether to
/// surface them.
#[must_use]
pub fn is_reaction_type(value: &serde_json::Value) -> bool {
    if let Some(code) = value.as_i64() {
        return (2000..=3005).contains(&code);
    }
    value.as_str().is_some_and(|s| {
        // BlueBubbles removals prefix the name with '-' (e.g. "-love").
        let base = s.strip_prefix('-').unwrap_or(s);
        TAPBACKS.iter().any(|(_, n, _)| *n == base)
    })
}

/// Resolve a raw `associatedMessageType` JSON value to a display emoji, but only
/// for *add* reactions. BlueBubbles emits either an integer (`2000`) or a string
/// (`"love"`); a leading `-` (removal form, e.g. `"-love"`) or a remove code
/// yields `None`, matching "only surface add tapbacks".
#[must_use]
pub fn reaction_emoji(value: &serde_json::Value) -> Option<&'static str> {
    if let Some(code) = value.as_i64() {
        return tapback_emoji(code);
    }
    let name = value.as_str()?;
    if name.starts_with('-') {
        return None; // removal
    }
    TAPBACKS
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(_, _, e)| *e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn name_to_code_roundtrips() {
        assert_eq!(tapback_code("love"), Some(2000));
        assert_eq!(tapback_code("question"), Some(2005));
        assert_eq!(tapback_code("nonsense"), None);
    }

    #[test]
    fn add_code_maps_to_emoji_remove_does_not() {
        assert_eq!(tapback_emoji(2000), Some("❤️"));
        assert_eq!(tapback_emoji(2001), Some("👍"));
        // Remove codes and unknown types drop out.
        assert_eq!(tapback_emoji(3000), None);
        assert_eq!(tapback_emoji(2006), None);
        assert_eq!(tapback_emoji(0), None);
    }

    #[test]
    fn is_reaction_type_covers_int_and_string_forms() {
        assert!(is_reaction_type(&json!(2000))); // add
        assert!(is_reaction_type(&json!(3000))); // remove still counts as a reaction record
        assert!(is_reaction_type(&json!("love")));
        assert!(is_reaction_type(&json!("-love"))); // removal
        assert!(!is_reaction_type(&json!(0))); // normal message
        assert!(!is_reaction_type(&json!("chat"))); // not a tapback
        assert!(!is_reaction_type(&json!(null)));
    }

    #[test]
    fn reaction_emoji_only_surfaces_add() {
        // Integer add form.
        assert_eq!(reaction_emoji(&json!(2000)), Some("❤️"));
        // String add form (BlueBubbles).
        assert_eq!(reaction_emoji(&json!("laugh")), Some("😂"));
        // Removes drop out in both forms.
        assert_eq!(reaction_emoji(&json!(3000)), None);
        assert_eq!(reaction_emoji(&json!("-love")), None);
        // Non-reactions.
        assert_eq!(reaction_emoji(&json!(0)), None);
        assert_eq!(reaction_emoji(&json!("chat")), None);
    }
}
