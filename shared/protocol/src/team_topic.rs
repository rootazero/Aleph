//! The `team.<id>.*` event-topic grammar — one parser, two consumers.
//!
//! Team events are published as a RAW `{topic, data}` string envelope
//! (`gateway::event_emitter::team_fanout::publish_team_event`, plus
//! `CoordTaskStore`'s `team.<id>.task.<verb>`), never as a typed
//! `GatewayEventFrame`. The topic string is therefore the ONLY place the team
//! identity lives, and two consumers read it with two different questions:
//!
//! - the **server** asks *whose team is this*, because the answer decides
//!   whether a member agent's chat body is delivered to a given connection.
//!   That question must be answered **structurally**: every
//!   `team.<id>.<anything>` resolves to `<id>`. A suffix whitelist on this side
//!   would mean the day a sixth suffix is published, its topic falls off the
//!   list and is broadcast to every connected user.
//! - the **Panel** asks *which renderer handles this*, which is a whitelist by
//!   nature — an unrecognized suffix must be ignored, not rendered.
//!
//! Different answers, but one shared fact: **where the team id ends**.
//! [`team_topic_id`] is that fact; [`parse_team_topic`] is built on top of it.
//! So the two sides cannot disagree about which team a topic addresses even
//! though they legitimately disagree about an unknown suffix. Both crates
//! depend on this one — neither keeps a second copy of the grammar.

/// Topic prefix every per-team topic (and the global `team.changed`) shares.
const PREFIX: &str = "team.";

/// Separator introducing the free-form `task.<verb>` family. The verb is
/// open-ended (`created` / `updated` / …), so this family anchors on the
/// separator rather than on a fixed suffix.
const TASK_SEP: &str = ".task.";

/// Which `team.<id>.<kind>` event arrived. `Task` folds the 4-segment
/// `team.<id>.task.<verb>` family — the verb is redundant with the payload's
/// `status`, which is what the Panel's upsert actually reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamTopicKind {
    /// A member's deliverable reply → an attributed bubble.
    Message,
    /// A broadcaster notice (guard explanation, member failure) → system chip.
    System,
    /// Member presence transition → roster status dot.
    Activity,
    /// Coordination-task create/update → task strip + drawer.
    Task,
    /// Fan-out tree lifecycle (`started` / `settled`) → run id + phase.
    Fanout,
}

/// The team a `team.<team_id>.<anything>` topic addresses, or `None` when the
/// topic is not per-team at all.
///
/// **Structural, not enumerated.** Any non-empty suffix counts, including one
/// that does not exist yet: this is the function a visibility decision hangs
/// off, and "a suffix I have not been taught" must resolve to a team (so the
/// frame is owner-scoped) rather than to `None` (so it is broadcast).
///
/// `None` is reserved for topics that genuinely address no team:
/// - `team.changed` — the global team-list invalidation the sidebar listens to;
/// - `team.` / `team..message` — an empty id;
/// - anything not under the `team.` prefix.
///
/// Team ids are opaque and may contain dots, so the id is everything before the
/// LAST separator, not the second segment.
#[must_use]
pub fn team_topic_id(topic: &str) -> Option<&str> {
    let rest = topic.strip_prefix(PREFIX)?;
    // `team.<id>.task.<verb>`: anchoring on `.task.` (not on the last dot)
    // is what keeps a dotted id addressable in this family too.
    if let Some(idx) = rest.rfind(TASK_SEP) {
        let id = &rest[..idx];
        let verb = &rest[idx + TASK_SEP.len()..];
        return (!id.is_empty() && !verb.is_empty()).then_some(id);
    }
    let idx = rest.rfind('.')?;
    let id = &rest[..idx];
    let suffix = &rest[idx + 1..];
    (!id.is_empty() && !suffix.is_empty()).then_some(id)
}

/// Split a per-team topic into its team id and the kind of thing it carries.
///
/// The render-routing face of [`team_topic_id`]: same id, but `None` for a
/// suffix no renderer knows. Do not use this to answer a visibility question —
/// its `None` covers both "not a team topic" and "a team topic I cannot
/// render", and those two must not lead to the same delivery decision.
#[must_use]
pub fn parse_team_topic(topic: &str) -> Option<(&str, TeamTopicKind)> {
    let team_id = team_topic_id(topic)?;
    // `team_topic_id` returned a slice of `topic`, so everything from the
    // prefix plus the id onward is exactly the `.`-led tail.
    let tail = &topic[PREFIX.len() + team_id.len()..];
    let kind = match tail {
        ".message" => TeamTopicKind::Message,
        ".system" => TeamTopicKind::System,
        ".activity" => TeamTopicKind::Activity,
        ".fanout" => TeamTopicKind::Fanout,
        _ if tail.starts_with(TASK_SEP) => TeamTopicKind::Task,
        _ => return None,
    };
    Some((team_id, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_per_team_topic_kind() {
        assert_eq!(
            parse_team_topic("team.t1.message"),
            Some(("t1", TeamTopicKind::Message))
        );
        assert_eq!(
            parse_team_topic("team.t1.system"),
            Some(("t1", TeamTopicKind::System))
        );
        assert_eq!(
            parse_team_topic("team.t1.activity"),
            Some(("t1", TeamTopicKind::Activity))
        );
        assert_eq!(
            parse_team_topic("team.t1.fanout"),
            Some(("t1", TeamTopicKind::Fanout))
        );
        assert_eq!(
            parse_team_topic("team.t1.task.created"),
            Some(("t1", TeamTopicKind::Task))
        );
    }

    #[test]
    fn ignores_the_global_team_changed_topic() {
        // The sidebar's team-list invalidation shares the `team.` prefix but is
        // not per-team — projecting it would push an empty bubble, and treating
        // it as a team would deny it to everyone.
        assert_eq!(parse_team_topic("team.changed"), None);
        assert_eq!(team_topic_id("team.changed"), None);
    }

    #[test]
    fn ignores_foreign_and_malformed_topics() {
        for topic in ["stream.response_chunk", "team.", "team..message", "teams.x"] {
            assert_eq!(parse_team_topic(topic), None, "{topic}");
            assert_eq!(team_topic_id(topic), None, "{topic}");
        }
    }

    #[test]
    fn team_id_may_contain_dots() {
        // Ids are opaque; suffix matching (not positional split) keeps a dotted
        // id addressable instead of silently misrouting its events.
        assert_eq!(
            parse_team_topic("team.acme.eu.west.message"),
            Some(("acme.eu.west", TeamTopicKind::Message))
        );
        assert_eq!(
            parse_team_topic("team.acme.eu.west.task.updated"),
            Some(("acme.eu.west", TeamTopicKind::Task))
        );
        assert_eq!(
            team_topic_id("team.acme.eu.west.task.updated"),
            Some("acme.eu.west")
        );
    }

    /// The whole reason the two faces are separate functions: a suffix nobody
    /// has taught the renderer still ADDRESSES a team. If this ever returns
    /// `None`, the next `publish_team_event("whisper", …)` is delivered to
    /// every connected user.
    #[test]
    fn an_unrendered_suffix_still_names_its_team() {
        assert_eq!(parse_team_topic("team.t1.unknown"), None);
        assert_eq!(team_topic_id("team.t1.unknown"), Some("t1"));
        assert_eq!(team_topic_id("team.t1.whisper"), Some("t1"));
        assert_eq!(team_topic_id("team.t1.task.some_new_verb"), Some("t1"));
    }

    /// A half-formed task topic addresses no team — the verb segment must
    /// exist, or `team.<id>.task.` would resolve to a team on the strength of
    /// a trailing dot.
    #[test]
    fn a_task_topic_without_a_verb_addresses_nothing() {
        assert_eq!(team_topic_id("team.t1.task."), None);
        assert_eq!(team_topic_id("team..task.created"), None);
    }
}
