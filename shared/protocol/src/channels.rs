//! The set of channel types a user can actually configure.
//!
//! This list exists because the same fact had two independent spellings and
//! nothing reconciled them:
//!
//! * the **server** answers it with `gateway::interfaces::register_channel_plugins`
//!   (the factory table `create_channel_from_config` reads) plus the one
//!   deliberate bypass, `imessage`, which `initialize_channels` constructs
//!   directly and `continue`s before ever consulting the table;
//! * the **Panel** answers it with `ALL_CHANNELS` in
//!   `platform/wide/views/settings/channels/definitions.rs`, the grid of cards
//!   the channels screen renders.
//!
//! When those two disagreed in the Panel's favour the user got a complete
//! settings form for a channel that could not exist: filling it in wrote
//! `[channels.<type>]`, boot logged one `Failed to create channel` line, and
//! nothing else ever happened. `msteams` and `feishu` both sat in that state —
//! msteams until its adapter was severed 2026-08-17, feishu from the day the
//! factory table landed until 2026-08-18.
//!
//! Neither side may spell the set itself any more. Both assert **set
//! equality** against this constant:
//!
//! * `alephcore`: the registered set equals this list (minus the bypass) — an
//!   adapter listed here but not registered is unconfigurable, and one
//!   registered but not listed means this list went stale.
//! * `aleph-panel`: the card set equals this list — a card outside it is the
//!   defect above, and a name here with no card is a channel only reachable by
//!   hand-editing `config.toml`.
//!
//! Equality on both sides is the point. A containment check can only see
//! "listed but missing"; it is structurally blind to a name absent from *both*
//! sides, which is exactly how `feishu` stayed unreachable for four months
//! with a green test watching it.
//!
//! The Panel direction was one-way until 2026-08-18 because `line`, `wechat`
//! and `qq` were configurable with no card, and pinning that would have meant
//! parking an exemption list with no force to shrink it. Writing the three
//! cards closed the set instead, so the exemption never had to exist.

/// Every channel type that a `[channels.<type>]` config block can actually
/// bring up, sorted so the two reconciliation tests can compare sets cheaply.
///
/// Adding an adapter is not enough to belong here — see the module docs.
pub const CONFIGURABLE_CHANNEL_TYPES: &[&str] = &[
    "discord",
    "email",
    "feishu",
    "imessage",
    "irc",
    "line",
    "matrix",
    "mattermost",
    "nostr",
    "qq",
    "signal",
    "slack",
    "telegram",
    "webhook",
    "wechat",
    "whatsapp",
    "xmpp",
];

/// The channel type that reaches config without passing through the factory
/// table, because `initialize_channels` constructs it inline (the local
/// transport needs a macOS-only `OffsetTracker` wired at construction).
///
/// Named rather than inlined so the server-side reconciliation can subtract
/// exactly one thing and say why.
pub const FACTORY_TABLE_BYPASS: &[&str] = &["imessage"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_sorted_and_free_of_duplicates() {
        let mut sorted = CONFIGURABLE_CHANNEL_TYPES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, CONFIGURABLE_CHANNEL_TYPES,
            "CONFIGURABLE_CHANNEL_TYPES must stay sorted and unique — both \
             reconciliation tests compare it as a sorted set",
        );
    }

    #[test]
    fn every_bypass_is_also_configurable() {
        for bypass in FACTORY_TABLE_BYPASS {
            assert!(
                CONFIGURABLE_CHANNEL_TYPES.contains(bypass),
                "`{bypass}` bypasses the factory table but is not listed as \
                 configurable, so the server-side reconciliation would subtract \
                 something that was never there",
            );
        }
    }
}
