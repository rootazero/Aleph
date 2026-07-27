//! The runtime overlay the gateway applies on top of [`resolve_route`].
//!
//! [`resolve_route`](super::resolve::resolve_route) answers from **config**:
//! the `[[bindings]]` table, snapshotted at boot. Two runtime facts sit on top
//! of it before a message reaches an agent, and both can change the answer:
//!
//! * an explicit per-channel binding set at runtime by `agent_switch` or the
//!   Panel, which must beat a merely-default config answer (otherwise the
//!   namesake action is a silent no-op on any deployment that has bindings);
//! * whether the agent a *specific* binding names still exists — `agents.delete`
//!   cannot rewrite routing config, so a binding can outlive its target, and
//!   routing to the ghost would brick the channel forever.
//!
//! Both rules lived only inside the inbound router, so `gateway_route` — the
//! tool whose entire job is telling the model where a message would go —
//! answered from config alone and confidently reported the wrong agent. This
//! module is the shared decision: each caller supplies the runtime facts it can
//! see, and the *composition* is written once.
//!
//! Deliberately a pure function over already-gathered facts rather than a
//! store-aware helper: the router and the tool reach their stores through very
//! different handles, and the thing worth sharing is the precedence, not the
//! plumbing.

use super::resolve::MatchedBy;

/// Runtime facts that sit on top of a config-resolved route.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeOverlay<'a> {
    /// The channel's explicit `agent_switch` / Panel binding, already validated
    /// to still exist in the runtime registry. `None` when unset, unbound, or
    /// pointing at a deleted agent.
    pub channel_override: Option<&'a str>,
    /// Whether the agent named by a *specific* (non-default) binding still
    /// exists. `true` when there is no registry to check against, preserving
    /// the legacy/test behaviour of trusting config.
    pub bound_agent_exists: bool,
}

/// Where a route's final answer came from once the runtime overlay is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlaySource {
    /// The config binding stands; `MatchedBy` is unchanged.
    Binding(MatchedBy),
    /// An explicit per-channel `agent_switch` binding won.
    ChannelOverride,
    /// A specific binding named an agent that no longer exists; the route fell
    /// through to the channel override or the default agent.
    BindingAgentMissing,
}

impl OverlaySource {
    /// Wire-stable label for the `gateway_route` tool's `matched_by` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binding(m) => match m {
                MatchedBy::Peer => "peer",
                MatchedBy::Guild => "guild",
                MatchedBy::Team => "team",
                MatchedBy::Account => "account",
                MatchedBy::Channel => "channel",
                MatchedBy::Default => "default",
            },
            Self::ChannelOverride => "channel_override",
            Self::BindingAgentMissing => "binding_agent_missing",
        }
    }
}

/// The agent a message actually reaches, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlaidRoute {
    /// The agent id the gateway will dispatch to. `None` means "fall through to
    /// the default agent" — the caller owns that value.
    pub agent_id: Option<String>,
    pub source: OverlaySource,
}

/// Apply the runtime overlay to a config-resolved route.
///
/// Precedence, matching the inbound router exactly:
///
/// 1. a *specific* binding (peer / guild / team / account / channel) wins —
///    unless the agent it names no longer exists, in which case it is dropped
///    and the remaining rules decide;
/// 2. an explicit per-channel override beats a `Default` answer;
/// 3. otherwise the config answer stands.
#[must_use]
pub fn overlay_route(
    resolved_agent: &str,
    matched_by: MatchedBy,
    overlay: &RuntimeOverlay<'_>,
) -> OverlaidRoute {
    let specific = matched_by != MatchedBy::Default;
    if specific && overlay.bound_agent_exists {
        return OverlaidRoute {
            agent_id: Some(resolved_agent.to_string()),
            source: OverlaySource::Binding(matched_by),
        };
    }
    // Either the answer was merely the default, or a specific binding pointed at
    // a ghost. Both fall through to the same two remaining options.
    let source = if specific {
        OverlaySource::BindingAgentMissing
    } else {
        OverlaySource::Binding(MatchedBy::Default)
    };
    match overlay.channel_override {
        // An override naming the agent config already chose overrides nothing —
        // report it as the config answer so the caller keeps the session key
        // `resolve_route` computed instead of rebuilding it identically.
        Some(agent) if !specific && agent == resolved_agent => OverlaidRoute {
            agent_id: Some(resolved_agent.to_string()),
            source,
        },
        Some(agent) => OverlaidRoute {
            agent_id: Some(agent.to_string()),
            source: OverlaySource::ChannelOverride,
        },
        None if specific => OverlaidRoute {
            agent_id: None, // ghost binding → default agent
            source,
        },
        None => OverlaidRoute {
            agent_id: Some(resolved_agent.to_string()),
            source,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(channel_override: Option<&str>, exists: bool) -> RuntimeOverlay<'_> {
        RuntimeOverlay {
            channel_override,
            bound_agent_exists: exists,
        }
    }

    #[test]
    fn specific_binding_beats_channel_override() {
        // A carefully-scoped `[[bindings]]` entry is not overridden by a
        // channel-wide `agent_switch`.
        let out = overlay_route("vip", MatchedBy::Peer, &overlay(Some("researcher"), true));
        assert_eq!(out.agent_id.as_deref(), Some("vip"));
        assert_eq!(out.source, OverlaySource::Binding(MatchedBy::Peer));
    }

    #[test]
    fn channel_override_beats_default() {
        // The zero-config case the tool used to get wrong: no binding governs
        // this conversation, but the channel was switched at runtime.
        let out = overlay_route(
            "main",
            MatchedBy::Default,
            &overlay(Some("researcher"), true),
        );
        assert_eq!(out.agent_id.as_deref(), Some("researcher"));
        assert_eq!(out.source, OverlaySource::ChannelOverride);
        assert_eq!(out.source.as_str(), "channel_override");
    }

    #[test]
    fn override_agreeing_with_config_is_not_an_override() {
        let out = overlay_route("main", MatchedBy::Default, &overlay(Some("main"), true));
        assert_eq!(out.agent_id.as_deref(), Some("main"));
        assert_eq!(out.source.as_str(), "default");
    }

    #[test]
    fn default_stands_without_an_override() {
        let out = overlay_route("main", MatchedBy::Default, &overlay(None, true));
        assert_eq!(out.agent_id.as_deref(), Some("main"));
        assert_eq!(out.source.as_str(), "default");
    }

    #[test]
    fn ghost_binding_falls_through_to_the_override() {
        // `agents.delete` removed the bound agent; the binding is dropped and
        // the channel override picks the message up.
        let out = overlay_route(
            "deleted",
            MatchedBy::Channel,
            &overlay(Some("researcher"), false),
        );
        assert_eq!(out.agent_id.as_deref(), Some("researcher"));
        assert_eq!(out.source, OverlaySource::ChannelOverride);
    }

    #[test]
    fn ghost_binding_with_no_override_falls_to_the_default_agent() {
        // `None` means "the caller's default agent", not the ghost.
        let out = overlay_route("deleted", MatchedBy::Team, &overlay(None, false));
        assert_eq!(out.agent_id, None);
        assert_eq!(out.source.as_str(), "binding_agent_missing");
    }
}
