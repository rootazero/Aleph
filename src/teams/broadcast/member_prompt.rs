//! 纯函数:组装一个被唤醒 agent 的 run 输入文本。
//!
//! 身份 + 名册 + 共享 transcript + 接话协议 + team_id;leader 多一段身份
//! (R7/R9:leader 的"领导力"在 prompt 身份,不是代码强制管控)。无 IO,host 可测。

/// 组装被唤醒 agent 的 run 输入。`is_leader` 时追加 leader 身份段。
#[must_use]
pub fn build_member_input(
    team_id: &str,
    agent_id: &str,
    role: &str,
    roster: &str,
    transcript: &str,
    is_leader: bool,
) -> String {
    let leader_block = if is_leader {
        "\n\n你还是这个群的 leader。当用户给团队的是一项需要完成的实质工作\
         (而非寒暄或简单问答)时,默认先用 `team_delegate` / `task_create` 把它\
         拆成可追踪任务派给成员、再汇总产出回复用户——这样进度会显示在团队看板上。\
         是否拆解、如何拆解由你判断;闲聊或你能直接答的,就直接答。"
    } else {
        ""
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
    fn member_prompt_has_identity_team_id_and_transcript() {
        let out = build_member_input(
            "team-xyz",
            "alice",
            "researcher",
            "bob (writer), leader (leader)",
            "[user]: @alice 查下 X",
            false,
        );
        assert!(out.contains("alice"), "含自己身份");
        assert!(out.contains("team-xyz"), "含 team_id(调团队工具必需)");
        assert!(out.contains("[user]: @alice 查下 X"), "含群 transcript");
        assert!(out.contains("bob (writer)"), "含名册");
        assert!(!out.contains("你还是这个群的 leader"), "成员无 leader 身份段");
    }

    #[test]
    fn leader_prompt_appends_leader_identity() {
        let out = build_member_input(
            "team-xyz",
            "leader",
            "leader",
            "alice (researcher)",
            "[user]: 这事谁跟进",
            true,
        );
        assert!(out.contains("你还是这个群的 leader"), "leader 身份段");
        assert!(out.contains("task_create"), "leader 段提到编排工具");
        assert!(out.contains("team_delegate"), "leader 段提到 team_delegate");
        assert!(
            out.contains("拆成可追踪任务") || out.contains("派给成员"),
            "leader 段指示把实质工作拆成可追踪任务"
        );
    }
}
