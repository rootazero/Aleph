//! The `config.get_tool_permissions` wire contract — specifically, which keys
//! each role receives.
//!
//! This method has two response shapes, and the difference is a role decision:
//! a member gets the session DIALS and the ids each dial can take; an operator
//! also gets the two server-global policy axes Settings → Policies edits. The
//! server narrows by REMOVAL — so a new field joins the member response unless
//! it is named in [`OPERATOR_ONLY_KEYS`], which is the right default for a
//! response that is otherwise entirely dial vocabulary — but it means the
//! member shape is defined by what is *absent*, and absence is exactly what a
//! hand-written client DTO fails to decode.
//!
//! It did. The narrowing shipped with a server-side test pinning `default` as
//! absent, and a Panel DTO whose `default` field had no `#[serde(default)]`.
//! Every member's fetch failed with "missing field `default`", the carve-out
//! that was supposed to hand members the dial ids was 100% inert, and both test
//! suites stayed green because each side only ever read its own literal.
//!
//! The two halves live in different crates (`alephcore` and `aleph-panel`) and
//! neither may depend on the other, so the contract lives here, in the crate
//! they both depend on, and each side reconciles against it:
//!
//! - server: `handlers::config`'s tests assert the values it builds carry
//!   exactly these keys.
//! - client: `api::tool_permissions`'s tests assert its DTO decodes an object
//!   built from [`MEMBER_VISIBLE_KEYS`] alone.
//!
//! Change a key on one side and the other side's test fails by name.

/// Keys every caller receives, whatever their role: each dial's position and
/// the id enumeration behind it.
///
/// A member's entire response is exactly this set. Anything a client needs in
/// order to render a control a MEMBER may use belongs here — and all four dials
/// qualify, because a member already writes every one of them for their own
/// session (`sessions.patch`, and `chat.send`'s per-request `exec_tier` /
/// `mode` / `thinking` / `memory`).
///
/// `think_levels` has no position key beside it on purpose: reasoning depth
/// resolves request > session > **no directive**, so there is no global for a
/// client to report. Every other dial names where its global sits.
pub const MEMBER_VISIBLE_KEYS: &[&str] = &[
    "exec_tier",
    "tiers",
    // The tiers a single CONVERSATION may take — `tiers` plus `plan`, the
    // read-only planning posture that ends when a human approves a plan.
    // Member-visible for the same reason `tiers` is: a member already writes
    // this dial for their own session, and withholding the enumeration locks a
    // menu the server would still honour.
    "session_tiers",
    "mode",
    "modes",
    "think_levels",
    "memory",
    "memory_modes",
];

/// Keys withheld from a member: the server-global policy axes.
///
/// A client must tolerate their absence — decoding must not fail, and the
/// surface that renders them is admin-gated anyway (`update_tool_permissions`
/// is not carved out of `config.`).
pub const OPERATOR_ONLY_KEYS: &[&str] = &["default", "overrides"];

/// Every key the full (operator) response carries.
#[must_use]
pub fn all_keys() -> Vec<&'static str> {
    MEMBER_VISIBLE_KEYS
        .iter()
        .chain(OPERATOR_ONLY_KEYS.iter())
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two sets are what "narrowing by removal" means; an overlap would
    /// make the phrase meaningless and silently widen the member shape.
    #[test]
    fn the_two_key_sets_are_disjoint() {
        for key in MEMBER_VISIBLE_KEYS {
            assert!(
                !OPERATOR_ONLY_KEYS.contains(key),
                "{key} cannot be both member-visible and operator-only"
            );
        }
        assert_eq!(
            all_keys().len(),
            MEMBER_VISIBLE_KEYS.len() + OPERATOR_ONLY_KEYS.len()
        );
    }
}
