//! Bot-addressing detection for Microsoft Teams group/channel messages.
//!
//! Teams delivers every message in a group chat or team channel to the bot,
//! regardless of whether the bot was mentioned. To honour
//! [`MsTeamsConfig::require_mention`](super::config::MsTeamsConfig), the inbound
//! handler must decide whether a given activity actually addresses the bot.
//!
//! This module is pure: it inspects an already-parsed [`Activity`] and returns
//! booleans. All wiring (config lookup, dropping the message) lives in the
//! channel handler.

use super::types::Activity;

/// Returns `true` if the bot was explicitly addressed in this activity.
///
/// Two independent signals are honoured, in priority order:
///
/// 1. **Mention entity** — Teams attaches `entities[] = { type: "mention",
///    mentioned: { id, name } }` whenever a user `@`-mentions someone. A match
///    on the bot's `recipient.id` is the authoritative signal.
/// 2. **`<at>` tag fallback** — if entities are absent (some clients omit them),
///    a `<at>BotName</at>` tag in the raw text matching `bot_name`
///    (case-insensitive) is accepted.
///
/// `bot_id` is the bot's channel-account id (`activity.recipient.id`).
/// `bot_name` is the bot's display name (`activity.recipient.name`), used only
/// for the textual fallback.
#[must_use]
pub fn was_bot_addressed(activity: &Activity, bot_id: &str, bot_name: Option<&str>) -> bool {
    if mentioned_via_entities(activity, bot_id) {
        return true;
    }
    if let Some(name) = bot_name {
        if mentioned_via_at_tag(activity.text.as_deref().unwrap_or_default(), name) {
            return true;
        }
    }
    false
}

/// Scan `activity.entities` for a `mention` entity targeting `bot_id`.
fn mentioned_via_entities(activity: &Activity, bot_id: &str) -> bool {
    if bot_id.is_empty() {
        return false;
    }
    let Some(entities) = activity.entities.as_ref() else {
        return false;
    };
    entities.iter().any(|e| {
        e.get("type").and_then(|t| t.as_str()) == Some("mention")
            && e.get("mentioned")
                .and_then(|m| m.get("id"))
                .and_then(|id| id.as_str())
                == Some(bot_id)
    })
}

/// Detect a `<at>BotName</at>` tag matching `bot_name` (case-insensitive).
fn mentioned_via_at_tag(text: &str, bot_name: &str) -> bool {
    let bot_name = bot_name.trim();
    if bot_name.is_empty() {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    let needle = bot_name.to_ascii_lowercase();
    // Look for "<at>" ... bot_name ... "</at>" without crossing tag boundaries.
    let mut rest = lower.as_str();
    while let Some(open) = rest.find("<at>") {
        let after_open = &rest[open + 4..];
        let Some(close) = after_open.find("</at>") else {
            break;
        };
        let inner = after_open[..close].trim();
        if inner == needle {
            return true;
        }
        rest = &after_open[close + 5..];
    }
    false
}

/// Best-effort extraction of the team id for per-team config lookups.
///
/// Channel messages carry team identity under `channelData.team`. We prefer the
/// AAD group id (stable, documented) and fall back to the Teams team id. Group
/// chats (`groupChat`) have no team and yield `None`, which makes
/// [`effective_require_mention`](super::config::MsTeamsConfig::effective_require_mention)
/// fall back to the global setting.
#[must_use]
pub fn team_id_from_channel_data(activity: &Activity) -> Option<String> {
    let team = activity.channel_data.as_ref()?.get("team")?;
    team.get("aadGroupId")
        .or_else(|| team.get("id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg_with_entities(text: &str, entities: serde_json::Value) -> Activity {
        Activity {
            activity_type: "message".into(),
            text: Some(text.into()),
            entities: entities.as_array().cloned(),
            ..Default::default()
        }
    }

    #[test]
    fn mention_entity_matches_bot_id() {
        let a = msg_with_entities(
            "<at>Aleph</at> hello",
            json!([{ "type": "mention", "mentioned": { "id": "28:bot", "name": "Aleph" } }]),
        );
        assert!(was_bot_addressed(&a, "28:bot", Some("Aleph")));
    }

    #[test]
    fn mention_entity_for_other_user_is_not_bot() {
        let a = msg_with_entities(
            "<at>Bob</at> hello",
            json!([{ "type": "mention", "mentioned": { "id": "28:bob", "name": "Bob" } }]),
        );
        assert!(!was_bot_addressed(&a, "28:bot", Some("Aleph")));
    }

    #[test]
    fn at_tag_fallback_when_entities_absent() {
        let mut a = Activity {
            activity_type: "message".into(),
            text: Some("<at>Aleph</at> do the thing".into()),
            ..Default::default()
        };
        a.entities = None;
        assert!(was_bot_addressed(&a, "28:bot", Some("Aleph")));
        // case-insensitive
        a.text = Some("<at>aleph</at> hi".into());
        assert!(was_bot_addressed(&a, "28:bot", Some("Aleph")));
    }

    #[test]
    fn at_tag_for_other_name_does_not_match() {
        let a = Activity {
            activity_type: "message".into(),
            text: Some("<at>Someone Else</at> hi".into()),
            ..Default::default()
        };
        assert!(!was_bot_addressed(&a, "28:bot", Some("Aleph")));
    }

    #[test]
    fn no_mention_no_text_match() {
        let a = Activity {
            activity_type: "message".into(),
            text: Some("just a plain group message".into()),
            ..Default::default()
        };
        assert!(!was_bot_addressed(&a, "28:bot", Some("Aleph")));
    }

    #[test]
    fn empty_bot_id_falls_back_to_name() {
        // recipient.id missing → entity match impossible, name fallback still works
        let a = Activity {
            activity_type: "message".into(),
            text: Some("<at>Aleph</at> hi".into()),
            ..Default::default()
        };
        assert!(was_bot_addressed(&a, "", Some("Aleph")));
    }

    #[test]
    fn team_id_prefers_aad_group_id() {
        let a = Activity {
            activity_type: "message".into(),
            channel_data: Some(json!({ "team": { "id": "19:chan", "aadGroupId": "aad-grp-1" } })),
            ..Default::default()
        };
        assert_eq!(team_id_from_channel_data(&a).as_deref(), Some("aad-grp-1"));
    }

    #[test]
    fn team_id_falls_back_to_team_id_field() {
        let a = Activity {
            activity_type: "message".into(),
            channel_data: Some(json!({ "team": { "id": "19:chan" } })),
            ..Default::default()
        };
        assert_eq!(team_id_from_channel_data(&a).as_deref(), Some("19:chan"));
    }

    #[test]
    fn team_id_none_for_group_chat() {
        let a = Activity {
            activity_type: "message".into(),
            channel_data: Some(json!({ "tenant": { "id": "t1" } })),
            ..Default::default()
        };
        assert_eq!(team_id_from_channel_data(&a), None);
        let b = Activity::default();
        assert_eq!(team_id_from_channel_data(&b), None);
    }
}
