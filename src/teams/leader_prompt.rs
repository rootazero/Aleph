//! Leader orchestration preamble injected at the head of a team-chat leader run.
//! Per R7/R9/R10 the orchestration intelligence lives here in the prompt, not in
//! gateway/dispatcher code.

/// Build the leader orchestration instruction. `roster` is a comma-joined list
/// like "alice (researcher), bob (writer)".
#[must_use]
pub fn build(team_name: &str, roster: &str, protocol: Option<&str>, user_request: &str) -> String {
    let protocol_block = protocol
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("\n\n# 团队协议\n{p}"))
        .unwrap_or_default();
    format!(
        "你是团队「{team_name}」的 leader。成员名册：{roster}。{protocol_block}\n\n\
         作为 leader，你要：\n\
         1. 把用户需求拆解成可分配的子任务，用 `task_create` 建任务并指定 owner 为合适的成员。\n\
         2. 必要时用 `message_send` 与成员沟通、用 `team_delegate` 直接委派。\n\
         3. 成员通过 dispatcher 异步执行，产出经 `task_submit` 落为 artifact。\n\
         4. 汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事——你的价值是编排与汇总。\n\n\
         # 用户需求\n{user_request}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_roster_protocol_and_request() {
        let out = build(
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
        let out = build("Squad", "alice (researcher)", None, "Do X");
        assert!(!out.contains("团队协议"));
        let out2 = build("Squad", "alice (researcher)", Some("   "), "Do X");
        assert!(!out2.contains("团队协议"));
    }
}
