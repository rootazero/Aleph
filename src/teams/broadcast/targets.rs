//! 纯函数:从一条触发消息解析出本轮要唤醒的 agent 集合。
//!
//! 防风暴的"谁回复"闸 + 宽度闸 + 自环护栏全在这里,无 IO,host 可测。

use super::{MAX_FANOUT_WIDTH, RESERVED_USER_HANDLE};
use crate::teams::messages::{extract_mentions, MENTION_ALL};

/// 解析一条触发消息要唤醒哪些 agent。
///
/// - `content`: 触发消息正文(用户消息或 agent 回复)
/// - `sender`: 发送者 id(`"user"` 表示用户;否则是某 agent id)
/// - `leader_id`: 团队 leader,用于"没@人时兜底"
/// - `roster`: 团队全体成员 agent_id(含 leader)
/// - `user_triggered`: true=用户消息(没@时 leader 兜底);false=agent 回复(没@时不兜底,链停)
///
/// 规则(spec §7):`@all`/`@everyone` → 全员(除 sender);具体 `@` → 取名册内的;
/// 去掉自@和 `@user`;用户消息没@ → `[leader]`;agent 回复没@ → `[]`;宽度上限截断。
#[must_use]
pub fn resolve_targets(
    content: &str,
    sender: &str,
    leader_id: &str,
    roster: &[String],
    user_triggered: bool,
) -> Vec<String> {
    let mentions = extract_mentions(content);
    let is_all = mentions.iter().any(|m| m == MENTION_ALL);

    let mut targets: Vec<String> = if is_all {
        roster.to_vec()
    } else {
        // 只保留名册内的具体 mention,保持出现顺序
        mentions
            .into_iter()
            .filter(|m| m != MENTION_ALL && roster.iter().any(|r| r == m))
            .collect()
    };

    // 护栏:去掉发送者自己 + 保留 handle "user"
    targets.retain(|a| a != sender && a != RESERVED_USER_HANDLE);

    // 去重(保持首次出现顺序)
    let mut seen = std::collections::HashSet::new();
    targets.retain(|a| seen.insert(a.clone()));

    // 没@人:用户触发 → leader 兜底;agent 回复 → 链停(空)
    if targets.is_empty() && user_triggered {
        let leader = leader_id.to_string();
        if leader != sender {
            targets.push(leader);
        }
    }

    // 宽度上限
    targets.truncate(MAX_FANOUT_WIDTH);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<String> {
        ["leader", "alice", "bob", "carol", "dave", "erin", "frank"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn user_mention_specific_agents() {
        let t = resolve_targets("@alice @bob 看下", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn user_no_mention_falls_back_to_leader() {
        let t = resolve_targets("随便聊聊", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["leader".to_string()], "没@人时 leader 兜底(仅 user 触发)");
    }

    #[test]
    fn agent_reply_no_mention_stops_chain() {
        // agent 回复没@人 → 不兜底,返回空(链自然停)
        let t = resolve_targets("好的我做完了", "alice", "leader", &roster(), false);
        assert!(t.is_empty(), "agent 回复没@人不应触发 leader 兜底");
    }

    #[test]
    fn at_all_expands_to_everyone_except_sender_capped() {
        let t = resolve_targets("@all 报到", "user", "leader", &roster(), true);
        // roster 7 人,@all 排除 sender(user 不在 roster)→ 7 人,宽度上限 5
        assert_eq!(t.len(), MAX_FANOUT_WIDTH, "@all 受宽度上限截断");
        assert!(!t.contains(&"user".to_string()));
    }

    #[test]
    fn drops_self_mention_and_reserved_user() {
        // alice 回复里 @ 自己 + @user + @bob → 只剩 bob
        let t = resolve_targets("@alice @user @bob", "alice", "leader", &roster(), false);
        assert_eq!(t, vec!["bob".to_string()], "去掉自@和@user");
    }

    #[test]
    fn unknown_mention_ignored() {
        let t = resolve_targets("@nobody @alice", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["alice".to_string()], "不在名册的@被忽略");
    }
}
