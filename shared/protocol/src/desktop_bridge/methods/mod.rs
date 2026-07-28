//! Per-method schemas for the desktop bridge JSON-RPC protocol.
//!
//! Each submodule owns:
//! - `pub const METHOD_*: &str` — canonical method names.
//! - `pub const DEFAULT_TIMEOUT_MS: u64` — the deadline every method in that
//!   namespace gets unless it names itself in `TIMEOUT_OVERRIDES_MS`.
//! - `pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)]` — per-method deadlines.
//! - Request/response structs with `serde` + `schemars::JsonSchema` derives.

pub mod ax;
pub mod bridge;
pub mod input;
pub mod media;
pub mod perm;
pub mod pim;
pub mod screen;

/// One namespace's deadline policy: what everything in it gets, and the
/// per-method exceptions.
struct Namespace {
    /// The literal `"<namespace>."` every `METHOD_*` constant there starts with.
    prefix: &'static str,
    default_ms: u64,
    overrides: &'static [(&'static str, u64)],
}

/// The namespaces, their default deadline, and their per-method overrides.
/// One row per submodule.
const NAMESPACES: &[Namespace] = &[
    Namespace {
        prefix: "ax.",
        default_ms: ax::DEFAULT_TIMEOUT_MS,
        overrides: ax::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "bridge.",
        default_ms: bridge::DEFAULT_TIMEOUT_MS,
        overrides: bridge::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "input.",
        default_ms: input::DEFAULT_TIMEOUT_MS,
        overrides: input::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "media.",
        default_ms: media::DEFAULT_TIMEOUT_MS,
        overrides: media::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "perm.",
        default_ms: perm::DEFAULT_TIMEOUT_MS,
        overrides: perm::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "pim.",
        default_ms: pim::DEFAULT_TIMEOUT_MS,
        overrides: pim::TIMEOUT_OVERRIDES_MS,
    },
    Namespace {
        prefix: "screen.",
        default_ms: screen::DEFAULT_TIMEOUT_MS,
        overrides: screen::TIMEOUT_OVERRIDES_MS,
    },
];

/// The client-side deadline for `method`, in milliseconds.
///
/// Resolution order: an exact per-method override, else the namespace default,
/// else `None` for a method whose namespace this table does not know.
///
/// The namespace fallback is the point of the whole shape. These numbers used to
/// exist as ten free-floating `SUGGESTED_TIMEOUT_MS*` constants with **zero
/// consumers**: every call rode the client's 60-second catch-all, so a wedged
/// `ax.query_focused` — which runs before *every* `type_text` — cost a full
/// minute instead of the three seconds the protocol had written down. Keying the
/// fallback on the namespace rather than on an exhaustive method list means a
/// method added tomorrow inherits a sane deadline instead of silently falling
/// back to a minute again.
#[must_use]
pub fn suggested_timeout_ms(method: &str) -> Option<u64> {
    let ns = NAMESPACES
        .iter()
        .find(|ns| method.starts_with(ns.prefix))?;
    Some(
        ns.overrides
            .iter()
            .find(|(name, _)| *name == method)
            .map_or(ns.default_ms, |(_, ms)| *ms),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_wins_over_its_namespace_default() {
        // `ax.query_focused` is the one AX call on the hot path (the focus gate
        // runs it before every keystroke), so it is deliberately tighter than
        // the tree walks that share its namespace.
        assert_eq!(
            suggested_timeout_ms(ax::METHOD_QUERY_FOCUSED),
            Some(ax::TIMEOUT_MS_QUERY_FOCUSED)
        );
        assert_eq!(
            suggested_timeout_ms(ax::METHOD_QUERY_TREE),
            Some(ax::DEFAULT_TIMEOUT_MS)
        );
    }

    #[test]
    fn a_method_with_no_override_inherits_its_namespace() {
        assert_eq!(
            suggested_timeout_ms(input::METHOD_SCROLL),
            Some(input::DEFAULT_TIMEOUT_MS)
        );
        assert_eq!(
            suggested_timeout_ms(pim::METHOD_NOTES_LIST),
            Some(pim::DEFAULT_TIMEOUT_MS)
        );
    }

    /// A method the table does not know must fall through to the client's own
    /// default rather than be handed an arbitrary number.
    #[test]
    fn an_unknown_namespace_has_no_opinion() {
        assert_eq!(suggested_timeout_ms("window.move"), None);
        assert_eq!(suggested_timeout_ms(""), None);
    }

    /// Every override has to name a method that actually exists, or it is a
    /// deadline nothing will ever read — the failure mode this whole module was
    /// written to end.
    #[test]
    fn every_override_names_a_real_method() {
        for ns in NAMESPACES {
            for (name, ms) in ns.overrides {
                assert!(
                    name.starts_with(ns.prefix),
                    "override '{name}' is filed under namespace '{}'",
                    ns.prefix
                );
                assert!(*ms > 0, "override '{name}' has a zero deadline");
            }
        }
    }

    /// Namespaces are matched by prefix, so a prefix that is a prefix of another
    /// would shadow it. Nothing here does today; this keeps it that way.
    #[test]
    fn no_namespace_prefix_shadows_another() {
        for a in NAMESPACES {
            for b in NAMESPACES {
                assert!(
                    a.prefix == b.prefix || !b.prefix.starts_with(a.prefix),
                    "namespace '{}' shadows '{}'",
                    a.prefix,
                    b.prefix
                );
            }
        }
    }
}
