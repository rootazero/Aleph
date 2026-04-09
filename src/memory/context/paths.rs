//! VFS preset paths and path computation utilities.

/// Preset directory paths for the aleph:// VFS
pub const PRESET_PATHS: &[(&str, &str)] = &[
    ("aleph://user/", "User domain root"),
    ("aleph://user/preferences/", "User preferences"),
    ("aleph://user/personal/", "Personal information"),
    ("aleph://user/plans/", "User plans and goals"),
    ("aleph://knowledge/", "Knowledge domain root"),
    ("aleph://knowledge/learning/", "Learning records"),
    ("aleph://knowledge/projects/", "Project knowledge"),
    ("aleph://agent/", "Agent domain root"),
    ("aleph://agent/tools/", "Tool usage experiences"),
    ("aleph://agent/experiences/", "Cortex experiences"),
    ("aleph://session/", "Session temporary data"),
];

/// Compute parent path from a VFS path
/// "aleph://user/preferences/coding/" -> "aleph://user/preferences/"
/// "aleph://user/preferences/" -> "aleph://user/"
/// "aleph://user/" -> "aleph://"
pub fn compute_parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(pos) => format!("{}/", &trimmed[..pos]),
        None => String::new(),
    }
}

/// Parse domain and topic from an `aleph://` VFS path.
///
/// Given `aleph://user/preferences/coding`, returns `("user", "preferences")`.
/// Returns `("", "")` for empty or non-conforming paths.
pub fn parse_domain_topic(path: &str) -> (&str, &str) {
    const PREFIX: &str = "aleph://";

    let rest = match path.strip_prefix(PREFIX) {
        Some(r) => r,
        None => return ("", ""),
    };

    let mut segments = rest.split('/').filter(|s| !s.is_empty());

    let domain = segments.next().unwrap_or("");
    let topic = segments.next().unwrap_or("");

    (domain, topic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_domain_topic_standard_path() {
        let (domain, topic) = parse_domain_topic("aleph://user/preferences/coding");
        assert_eq!(domain, "user");
        assert_eq!(topic, "preferences");
    }

    #[test]
    fn parse_domain_topic_with_trailing_slash() {
        let (domain, topic) = parse_domain_topic("aleph://knowledge/projects/");
        assert_eq!(domain, "knowledge");
        assert_eq!(topic, "projects");
    }

    #[test]
    fn parse_domain_topic_domain_only() {
        let (domain, topic) = parse_domain_topic("aleph://user/");
        assert_eq!(domain, "user");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_empty_path() {
        let (domain, topic) = parse_domain_topic("");
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_no_prefix() {
        let (domain, topic) = parse_domain_topic("random/path");
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_agent_tools() {
        let (domain, topic) = parse_domain_topic("aleph://agent/tools/shell");
        assert_eq!(domain, "agent");
        assert_eq!(topic, "tools");
    }
}
