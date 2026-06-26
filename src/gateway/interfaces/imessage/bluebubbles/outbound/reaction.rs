//! Tapback reaction name → BlueBubbles associatedMessageType code.

/// Map a tapback name to its BlueBubbles `associatedMessageType` integer.
/// Returns `None` for unknown reaction names.
#[must_use]
pub fn tapback_code(name: &str) -> Option<i64> {
    match name {
        "love" => Some(2000),
        "like" => Some(2001),
        "dislike" => Some(2002),
        "laugh" => Some(2003),
        "emphasize" => Some(2004),
        "question" => Some(2005),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::tapback_code;

    #[test]
    fn maps_known_tapbacks() {
        assert_eq!(tapback_code("love"), Some(2000));
        assert_eq!(tapback_code("like"), Some(2001));
        assert_eq!(tapback_code("nonsense"), None);
    }
}
