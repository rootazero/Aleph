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
         2. 用 `@<agent_id>` 在群里把任务派给成员——消息里务必带上 `task_create` 返回的 task_id，成员要凭它提交产出。必要时用 `team_delegate` 直接委派。\n\
         3. 成员用 `task_submit`（填你给的 task_id）交回产出后，任务转入待验收：先用 `task_read_artifact` 看产出，再用 `task_review`（decision=approve 通过并解锁后续任务 / reject 退回并把要改的写进 feedback 让成员重做）。\n\
         4. 全部子任务验收通过后，汇总成员产出，给用户一个清晰的最终答复。\n\n\
         编排纪律：\n\
         - 防过度编排：目标明确的短活别拆成任务网——一次委派（一个成员或一个 subagent）就够；只有出现并行、审批门、回滚、跨工具依赖时才值得任务 DAG。\n\
         - 审查要独立触地：成员的 task_submit 是自我报告，审查者不能只读它自证。验收有可测量产出的任务时自己跑测量，或派 subagent(agent_type='loop-auditor') 独立取证；创建这类任务时设 require_grounding=true，approve 时附 grounding 证据（kind: exit_code|numeric|line_count）。\n\
         - 失败局部重跑：reject 只把该任务退回原地重做（依赖图自动挡住下游），不要解散重建团队。\n\n\
         不要自己闷头做完所有事，也不要用通用 subagent 顶替成员——你的价值是编排团队成员、验收成果与汇总。\n\n\
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
        let out = build(
            "team-abc123",
            "main的群聊",
            "alice (researcher)",
            None,
            "做个调研",
        );
        assert!(
            out.contains("team-abc123"),
            "team_id must be in the prompt so team-tool calls resolve the real team; got: {out}"
        );
        // The name alone is not a usable tool argument, but should still label the team.
        assert!(
            out.contains("main的群聊"),
            "team name should still appear as a label"
        );
    }

    #[test]
    fn build_carries_orchestration_doctrine() {
        let out = build("t1", "Squad", "a (x)", None, "req");
        assert!(out.contains("防过度编排"));
        assert!(out.contains("require_grounding"));
        assert!(out.contains("loop-auditor"));
        assert!(out.contains("局部重跑") || out.contains("原地重做"));
    }
}
