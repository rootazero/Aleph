//! Pure functions: parse the set of agents to awaken this round from a trigger message.
//!
//! The storm-prevention "who-replies" gate + width gate + self-loop guard all live here,
//! zero IO, host-testable.

use super::RESERVED_USER_HANDLE;
use crate::teams::messages::{extract_mentions, MENTION_ALL};

/// Parse which agents a trigger message should awaken.
///
/// - `content`: the trigger message body (user message or agent reply)
/// - `sender`: sender id (`"user"` = user; otherwise an agent id)
/// - `leader_id`: team leader, used for "fallback when no @"
/// - `roster`: all team member agent_ids (including leader)
/// - `user_triggered`: true = user message (leader fallback when no @);
///   false = agent reply (no fallback when no @, chain stops)
/// - `leader_first`: true = leader-priority hard gate (strategy round 2; when active, leader
///   decomposes tasks first, then dispatches)
///
/// Rules (spec §7): `@all`/`@everyone` → everyone (except sender); explicit `@` → keep only
/// those in the roster; drop self-@ and `@user`; user message with no @ → `[leader]`;
/// agent reply with no @ → `[]`; width cap truncates.
///
/// `max_fanout_width` from [`BroadcastConfig::max_fanout_width`](super::BroadcastConfig),
/// truncates this round's awakenings (prevents `@all` from exploding a large group).
#[must_use]
pub fn resolve_targets(
    content: &str,
    sender: &str,
    leader_id: &str,
    roster: &[String],
    user_triggered: bool,
    leader_first: bool,
    max_fanout_width: usize,
) -> Vec<String> {
    // Hard gate (strategy round 2): on the user's first message to a team while
    // the leader has just minted a plan, route ONLY to the leader so it
    // decomposes + assigns first — even if the user @-named a member. Purely
    // structural (a boolean), zero content inspection (R7). Once a plan exists
    // `leader_first` is false and the equal-broadcast below resumes.
    if user_triggered && leader_first {
        let leader = leader_id.to_string();
        return if leader != sender {
            vec![leader]
        } else {
            Vec::new()
        };
    }

    let mentions = extract_mentions(content);
    let is_all = mentions.iter().any(|m| m == MENTION_ALL);

    let mut targets: Vec<String> = if is_all {
        roster.to_vec()
    } else {
        // Keep only explicit mentions present in the roster, preserving appearance order
        mentions
            .into_iter()
            .filter(|m| m != MENTION_ALL && roster.iter().any(|r| r == m))
            .collect()
    };

    // Guard rails: remove the sender + the reserved handle "user"
    targets.retain(|a| a != sender && a != RESERVED_USER_HANDLE);

    // Dedup (preserve first-occurrence order)
    let mut seen = std::collections::HashSet::new();
    targets.retain(|a| seen.insert(a.clone()));

    // No @: user-triggered → leader fallback; agent reply → chain stops (empty)
    if targets.is_empty() && user_triggered {
        let leader = leader_id.to_string();
        if leader != sender {
            targets.push(leader);
        }
    }

    // Width cap
    targets.truncate(max_fanout_width);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::broadcast::MAX_FANOUT_WIDTH;

    fn roster() -> Vec<String> {
        ["leader", "alice", "bob", "carol", "dave", "erin", "frank"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn user_mention_specific_agents() {
        let t = resolve_targets(
            "@alice @bob 看下",
            "user",
            "leader",
            &roster(),
            true,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(t, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn user_no_mention_falls_back_to_leader() {
        let t = resolve_targets(
            "随便聊聊",
            "user",
            "leader",
            &roster(),
            true,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(
            t,
            vec!["leader".to_string()],
            "没@人时 leader 兜底(仅 user 触发)"
        );
    }

    #[test]
    fn agent_reply_no_mention_stops_chain() {
        // agent reply with no @ → no fallback, returns empty (chain naturally stops)
        let t = resolve_targets(
            "好的我做完了",
            "alice",
            "leader",
            &roster(),
            false,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert!(t.is_empty(), "agent 回复没@人不应触发 leader 兜底");
    }

    #[test]
    fn at_all_expands_to_everyone_except_sender_capped() {
        let t = resolve_targets(
            "@all 报到",
            "user",
            "leader",
            &roster(),
            true,
            false,
            MAX_FANOUT_WIDTH,
        );
        // roster of 7, @all excludes sender (user not in roster) → 7 people, width cap 5
        assert_eq!(t.len(), MAX_FANOUT_WIDTH, "@all 受宽度上限截断");
        assert!(!t.contains(&"user".to_string()));
    }

    #[test]
    fn fanout_width_is_configurable() {
        // Set the width gate to 2 → @all in a group of 7 only awakens 2 (verifies the gate
        // value actually applies, not reusing the old hardcoded MAX_FANOUT_WIDTH).
        let t = resolve_targets("@all 报到", "user", "leader", &roster(), true, false, 2);
        assert_eq!(t.len(), 2, "自定义宽度闸截断");
    }

    #[test]
    fn drops_self_mention_and_reserved_user() {
        // alice's reply @-ing herself + @user + @bob → only bob remains
        let t = resolve_targets(
            "@alice @user @bob",
            "alice",
            "leader",
            &roster(),
            false,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(t, vec!["bob".to_string()], "去掉自@和@user");
    }

    #[test]
    fn unknown_mention_ignored() {
        let t = resolve_targets(
            "@nobody @alice",
            "user",
            "leader",
            &roster(),
            true,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(t, vec!["alice".to_string()], "不在名册的@被忽略");
    }

    #[test]
    fn leader_first_overrides_explicit_mention() {
        // hard gate ON + user message that @-named alice → still routes to leader only
        let t = resolve_targets(
            "@alice 看下",
            "user",
            "leader",
            &roster(),
            true,
            true,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(
            t,
            vec!["leader".to_string()],
            "leader_first ignores the user @"
        );
    }

    #[test]
    fn leader_first_inactive_keeps_normal_routing() {
        // hard gate OFF → existing behavior (alice gets it)
        let t = resolve_targets(
            "@alice 看下",
            "user",
            "leader",
            &roster(),
            true,
            false,
            MAX_FANOUT_WIDTH,
        );
        assert_eq!(t, vec!["alice".to_string()]);
    }
}
