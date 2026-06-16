//! Leader orchestration preamble injected at the head of a team-chat leader run.
//! Per R7/R9/R10 the orchestration intelligence lives here in the prompt, not in
//! gateway/dispatcher code.

/// Build the leader orchestration instruction. `roster` is a comma-joined list
/// like "alice (researcher), bob (writer)".
///
/// `team_id` MUST be surfaced verbatim: every team tool (`task_create`,
/// `team_delegate`, `message_send`, `team_status`, `task_submit`) takes
/// `team_id` as a required argument. If the leader is only told the team
/// *name*, it passes the name as the id and every call fails with
/// "Team '<name>' not found" — the team is never actually orchestrated and
/// members are never reached.
#[must_use]
pub fn build(
    team_id: &str,
    team_name: &str,
    roster: &str,
    protocol: Option<&str>,
    user_request: &str,
) -> String {
    let protocol_block = protocol
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("\n\n# 团队协议\n{p}"))
        .unwrap_or_default();
    format!(
        "你是团队「{team_name}」的 leader（team_id: `{team_id}`）。成员名册：{roster}。{protocol_block}\n\n\
         ⚠️ 调用任何团队工具（task_create、team_delegate、message_send、team_status、task_submit 等）\
         时，team_id 参数必须填 `{team_id}`，否则工具会因找不到团队而失败。指代成员一律用名册里的 agent_id。\n\n\
         作为 leader，你要：\n\
         1. 把用户需求拆解成可分配的子任务，用 `task_create` 建任务并把 owner 设为合适成员的 agent_id。\n\
         2. 必要时用 `message_send` 与成员沟通、用 `team_delegate` 直接委派给成员。\n\
         3. 成员通过 dispatcher 异步执行，产出经 `task_submit` 落为 artifact。\n\
         4. 汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事，也不要用通用 subagent 顶替成员——你的价值是编排团队成员与汇总。\n\n\
         # 用户需求\n{user_request}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_roster_protocol_and_request() {
        let out = build(
            "team-42",
            "Squad",
            "alice (researcher), bob (writer)",
            Some("Be concise"),
            "Write a report",
        );
        assert!(out.contains("Squad"));
        assert!(out.contains("alice (researcher), bob (writer)"));
        assert!(out.contains("Be concise"));
        assert!(out.contains("Write a report"));
    }

    #[test]
    fn build_omits_protocol_block_when_none_or_empty() {
        let out = build("team-1", "Squad", "alice (researcher)", None, "Do X");
        assert!(!out.contains("团队协议"));
        let out2 = build("team-1", "Squad", "alice (researcher)", Some("   "), "Do X");
        assert!(!out2.contains("团队协议"));
    }

    #[test]
    fn build_surfaces_team_id_so_leader_can_address_its_team() {
        // Regression: the prompt used to pass only the team NAME, so the leader
        // guessed the name as team_id and every team tool (task_create,
        // team_delegate, team_status, message_send) failed with
        // "Team '<name>' not found" — members were never reached and the leader
        // flailed with generic subagents. The exact team_id must appear verbatim
        // so the leader can fill it into every team-tool call.
        let out = build("team-abc123", "main的群聊", "alice (researcher)", None, "做个调研");
        assert!(
            out.contains("team-abc123"),
            "team_id must be in the prompt so team-tool calls resolve the real team; got: {out}"
        );
        // The name alone is not a usable tool argument, but should still label the team.
        assert!(out.contains("main的群聊"), "team name should still appear as a label");
    }
}
