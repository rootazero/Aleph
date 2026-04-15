pub fn is_group_jid(jid: &str) -> bool {
    jid.ends_with("@g.us")
}

pub fn normalize_e164_or_jid(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('@') {
        return Some(trimmed.to_lowercase());
    }
    let digits: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();
    if digits.is_empty() {
        return None;
    }
    Some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_group_jid() {
        assert!(is_group_jid("123456789@g.us"));
        assert!(!is_group_jid("123456789@s.whatsapp.net"));
    }

    #[test]
    fn test_normalize_e164() {
        assert_eq!(
            normalize_e164_or_jid("+1 555 123 4567"),
            Some("+15551234567".into())
        );
        assert_eq!(
            normalize_e164_or_jid("GROUP@g.us"),
            Some("group@g.us".into())
        );
    }
}
