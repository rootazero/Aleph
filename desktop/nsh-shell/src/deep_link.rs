//! `aleph://` links the app claims (nsh.json `schemes`). nsh hands them to
//! the app through `next_url_scheme_open`, the one that launched it
//! included; this module only decides what a link asks for.

/// What an `aleph://` link asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeepLink {
    /// Bring the main window forward and open the chat.
    Open,
}

/// The action of `url`, or `None` for anything that is not a known
/// `aleph://` link.
pub(crate) fn deep_link_action(url: &str) -> Option<DeepLink> {
    let rest = url.strip_prefix("aleph://")?;
    match rest.strip_suffix('/').unwrap_or(rest) {
        "open" => Some(DeepLink::Open),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_with_or_without_a_trailing_slash() {
        assert_eq!(deep_link_action("aleph://open"), Some(DeepLink::Open));
        assert_eq!(deep_link_action("aleph://open/"), Some(DeepLink::Open));
    }

    #[test]
    fn other_links_are_ignored() {
        for url in ["aleph://evil", "https://open", "aleph:open", ""] {
            assert_eq!(deep_link_action(url), None, "{url}");
        }
    }
}
