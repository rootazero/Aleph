//! Pure function: assemble the run input text for a woken agent.
//!
//! Identity + roster + shared transcript + reply protocol + team_id; leader gets an
//! extra identity block (R7/R9: the leader's "leadership" lives in the prompt identity,
//! not in code-enforced control). Zero IO, host-testable.

/// Build a woken agent's run input. Leader uses the strong orchestration contract
/// (`leader_prompt::build`), regular members use the obey contract (accept tasks /
/// complete / submit back to leader, not just chat). R7/R9: leadership and convergence
/// pressure both live in the prompt identity, not in code enforcement. Zero IO,
/// host-testable.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_member_input(
    team_id: &str,
    agent_id: &str,
    role: &str,
    roster: &str,
    human_roster: &str,
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
        "\n\n团队纪律:你在 leader 的统筹下工作。当 leader 通过 @ 把任务派给你时,他会带上一个 task_id;\
         你接下后尽力完成,用 `task_submit`(填那个 task_id)把产出交回,leader 会用 task_review 验收\
         ——被 reject 就按反馈重做再交。你仍可自由 @ 其他成员协作,但讨论要服务于把任务做完,而不是只在群里闲聊。"
            .to_string()
    };
    // Empty `human_roster` (no human has spoken yet, or the identity store is
    // unavailable — `speaker::resolve_labels`'s degradation) omits this clause
    // entirely, so a single-human/no-human thread's prompt is byte-identical to
    // before this line existed.
    let human_roster_line = if human_roster.is_empty() {
        String::new()
    } else {
        format!("真人参与者:{human_roster}。")
    };
    format!(
        "你是团队群聊里的成员 `{agent_id}`({role}),team_id: `{team_id}`。\n\
         群成员名册:{roster}。{human_roster_line}{leader_block}\n\n\
         下面是群聊记录(每行 `[发言人]: 内容`):\n{transcript}\n\n\
         请以你的身份在群里回应。约定:\n\
         - 要不要发言、说什么由你判断;与你无关可以简短跳过。\n\
         - 想让某成员接话,在回复里 `@<agent_id>`(用名册里的 id);`@all` 叫全员。\n\
         - 调任何团队工具(task_create / task_submit / team_status 等,以你实际可用的工具为准)时,team_id 必须填 `{team_id}`。\n\
         - 不要 @ 自己,也不要 @ user 或任何真人参与者。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_prompt_has_identity_and_obey_contract() {
        let out = build_member_input(
            "team-xyz",
            "alice",
            "researcher",
            "bob (writer), leader (leader)",
            "",
            "[user]: @alice 查下 X",
            false,
            "Squad",
            None,
            "查下 X",
        );
        assert!(out.contains("alice"));
        assert!(out.contains("team-xyz"));
        assert!(out.contains("[user]: @alice 查下 X"));
        assert!(out.contains("团队纪律"), "member gets the obey contract");
        assert!(
            !out.contains("你是团队「Squad」的 leader"),
            "member has no leader contract"
        );
        assert!(
            out.contains("task_submit"),
            "member told to submit via task_submit"
        );
        assert!(
            out.contains("不要 @ 自己,也不要 @ user 或任何真人参与者。"),
            "member is told never to @ any human participant"
        );
    }

    /// Empty `human_roster` (no human has spoken yet, or the identity store was
    /// unavailable) must omit the "真人参与者:" clause entirely — not print it
    /// with an empty value — so a single-human/no-human thread's prompt stays
    /// byte-identical to before this parameter existed.
    #[test]
    fn empty_human_roster_omits_the_clause() {
        let out = build_member_input(
            "team-xyz",
            "alice",
            "researcher",
            "bob (writer), leader (leader)",
            "",
            "[user]: @alice 查下 X",
            false,
            "Squad",
            None,
            "查下 X",
        );
        assert!(
            !out.contains("真人参与者:"),
            "empty human_roster must not render the clause at all: {out}"
        );
        // The unconditional closing-bullet addition ("或任何真人参与者") still
        // applies regardless of whether human_roster is empty this turn.
        assert!(
            out.contains("不要 @ 自己,也不要 @ user 或任何真人参与者。"),
            "closing bullet must still warn against @-ing any human participant: {out}"
        );
    }

    /// A non-empty `human_roster` renders as its own clause right after the
    /// agent roster, telling the member which humans are in the room.
    #[test]
    fn non_empty_human_roster_renders_its_own_clause() {
        let out = build_member_input(
            "team-xyz",
            "alice",
            "researcher",
            "bob (writer), leader (leader)",
            "Alice(human), u-bob(human)",
            "[Alice]: @alice 查下 X",
            false,
            "Squad",
            None,
            "查下 X",
        );
        assert!(
            out.contains("真人参与者:Alice(human), u-bob(human)。"),
            "human roster clause missing or malformed: {out}"
        );
    }

    /// The shared convention block reaches workers too, so it must not name a
    /// verb only the leader has. `team_delegate` is the one every declared
    /// template deliberately withholds from members — the invariant
    /// `templates::materialize::only_the_leader_declares_team_delegate`
    /// enforces on the declaration side. Naming it here told a worker to call
    /// a tool it cannot see. (`task_*` and `team_status` are NOT leader-only:
    /// the declared worker roles do carry them.)
    #[test]
    fn member_prompt_names_no_leader_only_verb() {
        let out = build_member_input(
            "team-xyz",
            "alice",
            "researcher",
            "bob (writer), leader (leader)",
            "",
            "[user]: @alice 查下 X",
            false,
            "Squad",
            None,
            "查下 X",
        );
        assert!(
            !out.contains("team_delegate"),
            "worker prompt names leader-only `team_delegate`: {out}"
        );
    }

    #[test]
    fn leader_prompt_uses_strong_orchestration_contract() {
        let out = build_member_input(
            "team-xyz",
            "leader",
            "leader",
            "alice (researcher)",
            "",
            "[user]: 这事谁跟进",
            true,
            "Squad",
            Some("Be concise"),
            "做个调研",
        );
        assert!(
            out.contains("你是团队「Squad」的 leader"),
            "leader contract present"
        );
        assert!(
            out.contains("task_create"),
            "leader told to decompose with task_create"
        );
        assert!(out.contains("不要自己闷头做完"), "anti-pattern present");
        assert!(out.contains("做个调研"), "user request surfaced to leader");
        assert!(
            out.contains("task_review"),
            "leader told to accept/reject via task_review"
        );
        assert!(
            out.contains("task_id"),
            "leader told to name the task_id when assigning"
        );
    }
}
