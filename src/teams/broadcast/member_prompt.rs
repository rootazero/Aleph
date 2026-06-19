//! 纯函数:组装一个被唤醒 agent 的 run 输入文本。
//!
//! 身份 + 名册 + 共享 transcript + 接话协议 + team_id;leader 多一段身份
//! (R7/R9:leader 的"领导力"在 prompt 身份,不是代码强制管控)。无 IO,host 可测。

/// 组装被唤醒 agent 的 run 输入。leader 用强编排契约(`leader_prompt::build`),
/// 普通成员用服从契约(接单/完成/交回 leader,而非只闲聊)。R7/R9:领导力与
/// 收敛压力都在 prompt 身份里,不靠代码强制。无 IO,host 可测。
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_member_input(
    team_id: &str,
    agent_id: &str,
    role: &str,
    roster: &str,
    transcript: &str,
    is_leader: bool,
    team_name: &str,
    protocol: Option<&str>,
    user_request: &str,
) -> String {
    let leader_block = if is_leader {
        format!(
            "\n\n{}",
            crate::teams::leader_prompt::build(team_id, team_name, roster, protocol, user_request)
        )
    } else {
        "\n\n团队纪律:你在 leader 的统筹下工作。当 leader 通过 @ 或任务把活派给你时,\
         优先接下并尽力完成,把产出交回 leader,而不是只在群里闲聊。你仍可自由 @ 其他\
         成员协作,但讨论要服务于把任务做完。"
            .to_string()
    };
    format!(
        "你是团队群聊里的成员 `{agent_id}`({role}),team_id: `{team_id}`。\n\
         群成员名册:{roster}。{leader_block}\n\n\
         下面是群聊记录(每行 `[发言人]: 内容`):\n{transcript}\n\n\
         请以你的身份在群里回应。约定:\n\
         - 要不要发言、说什么由你判断;与你无关可以简短跳过。\n\
         - 想让某成员接话,在回复里 `@<agent_id>`(用名册里的 id);`@all` 叫全员。\n\
         - 调任何团队工具(task_create / team_delegate / team_status 等)时,team_id 必须填 `{team_id}`。\n\
         - 不要 @ 自己,也不要 @ user。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_prompt_has_identity_and_obey_contract() {
        let out = build_member_input(
            "team-xyz", "alice", "researcher",
            "bob (writer), leader (leader)",
            "[user]: @alice 查下 X",
            false,
            "Squad", None, "查下 X",
        );
        assert!(out.contains("alice"));
        assert!(out.contains("team-xyz"));
        assert!(out.contains("[user]: @alice 查下 X"));
        assert!(out.contains("团队纪律"), "member gets the obey contract");
        assert!(!out.contains("你是团队「Squad」的 leader"), "member has no leader contract");
    }

    #[test]
    fn leader_prompt_uses_strong_orchestration_contract() {
        let out = build_member_input(
            "team-xyz", "leader", "leader",
            "alice (researcher)",
            "[user]: 这事谁跟进",
            true,
            "Squad", Some("Be concise"), "做个调研",
        );
        assert!(out.contains("你是团队「Squad」的 leader"), "leader contract present");
        assert!(out.contains("task_create"), "leader told to decompose with task_create");
        assert!(out.contains("不要自己闷头做完"), "anti-pattern present");
        assert!(out.contains("做个调研"), "user request surfaced to leader");
    }
}
