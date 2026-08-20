//! One predicate for "this string is about to become a directory name".
//!
//! Marketplace names and plugin names both arrive from outside — a config
//! file, a marketplace manifest, an RPC parameter — and both are `join`ed onto
//! a managed root before anything is created or deleted there. A value like
//! `../../etc` therefore has to be refused at every such site.
//!
//! It was refused at five: `sync_github_marketplace`, `removal_refusal`,
//! `resolve_cache_dir`, `install_plugin_from_cache` and
//! `update_plugin_from_cache`, each with its own copy of the same four
//! conditions and two of them with a shorter message that did not say what was
//! wrong. Five copies of a security predicate is five chances for the sixth
//! site to be written without one — and a sixth site is exactly what
//! [`super::source_spec::classify`] is.

/// Refuse `value` if it could name anything other than a direct child of the
/// directory it is about to be joined onto.
///
/// `kind` names the thing for the message (`"marketplace name"`,
/// `"plugin name"`), so one predicate can serve callers that guard different
/// vocabularies.
///
/// # Errors
/// A human-readable refusal when `value` is empty, contains `/` or `\`, or
/// contains `..`.
pub fn reject_unsafe_segment(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('/') || value.contains('\\') || value.contains("..") {
        return Err(format!(
            "Invalid {kind} '{value}': must not be empty or contain path separators or '..'."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_segment_is_accepted() {
        assert!(reject_unsafe_segment("marketplace name", "aleph-official").is_ok());
        assert!(reject_unsafe_segment("plugin name", "my_plugin.v2").is_ok());
    }

    #[test]
    fn every_escape_shape_is_refused_and_says_which_value() {
        for bad in ["", "a/b", "a\\b", "..", "../../etc", "x..y"] {
            let err = reject_unsafe_segment("marketplace name", bad)
                .expect_err(&format!("{bad:?} must be refused"));
            assert!(
                err.contains("marketplace name") && err.contains(&format!("'{bad}'")),
                "refusal must name the kind and the offending value, got {err}"
            );
        }
    }

    /// The refusal is read by a human deciding what to type instead, so it has
    /// to say what is wrong — not merely that something is. Two of the five
    /// sites this replaced said only "Invalid marketplace name 'x'".
    #[test]
    fn the_refusal_explains_the_rule() {
        let err = reject_unsafe_segment("plugin name", "../x").unwrap_err();
        assert!(err.contains("path separators"), "got {err}");
    }
}
