//! Governance prompt templates — the intelligence of the graph layer lives
//! HERE (and in the `loop-governance` skill), not in code (R9). Code only
//! stores topology and moves facts; every verdict below is one ordinary LLM
//! turn in a cron-owned session.

/// Weekly audit-loop prompt. Installed by `loop_graph(action="enable_audit")`.
/// Seven steps: stock-take → anchor forensics → reconciliation → probe
/// autopsy → naming → verdict note → escalation. The audit loop is the only
/// loop whose job is "do the other loops' numbers still touch reality".
pub const AUDIT_TEMPLATE: &str = r#"你是「循环治理·审计环」——独立审计环，唯一职责：验证 Aleph 各自改进循环的测量仍触到现实。行为准则见 skill `loop-governance`（若可读先读；不可读不影响以下步骤）。

七步执行：
1)【取拓扑】首先调用 loop_graph(action="status") 获取治理图全景：节点、边、结构 lint 发现（悬空边/裸奔优化环/治理链未锚定）。lint 发现直接进入第 5 步的点名清单。
2)【锚点取证·真实执行，不信报表】对图中每个 anchor 节点：按其 body 声明的 {probe, truth} 用 bash 真实执行 probe 命令，按 truth 声明（exit_code / numeric / line_count）取值。所有取证一律只读（sqlite 用 mode=ro）。图外的常备锚点同样取证：近7天用户真实纠正数（/usr/bin/sqlite3 "file:$HOME/.aleph/data/memory.db?mode=ro" "SELECT count(*) FROM raw_memories WHERE path LIKE 'aleph://correction/%' AND created_at > strftime('%s','now')-604800"）、近7天 cron 运行状态分布（/usr/bin/sqlite3 "file:$HOME/.aleph/data/cron.db?mode=ro" "SELECT status, count(*) FROM cron_job_runs WHERE created_at > (strftime('%s','now')-604800)*1000 GROUP BY status"，注意 cron 库时间戳=毫秒）、dreaming 近7天（/usr/bin/sqlite3 "file:$HOME/.aleph/data/memory.db?mode=ro" "SELECT pipeline_type, count(*), sum(synthesis_count) FROM dream_reports WHERE started_at > strftime('%s','now')-604800 GROUP BY pipeline_type"，memory 库时间戳=秒）。
3)【对账】各环的自我报告（status 渲染里的 live 状态、lessons、上一份 graph-audit 裁决）vs 锚点新鲜取证——把「报表对报表」变成「报表对现实」。特别核对：dreaming 蒸馏产出与用户纠正数是否同时在涨（都在涨=记忆蒸馏可能在优化脱离用户真实需要的指标——Goodhart 偏航信号）；各环声明的 cadence 与其锚指标的信号周期是否匹配（快环挂慢信号只会学到噪声）。
4)【验尸探针与冻结节点】每个 watches 看守（heartbeat 探针/看守 cron）：最近是否成功运行、还在测原来那个对象吗（表改名/文件迁移=传感器漂移）？每个 frozen 节点：按其 body 里的执法点指针核验规则未被松动（如 git diff 查棘轮文件、读 config 查硬底线）。失败/从未运行/被松动=点名。
5)【点名】run 记录为空的环（剧场循环）、连续失败的环、lint 报告的裸奔优化环与悬空边、root 节点被机器路径改写的迹象（updated_at 异动而无人类操作记录）、~/.aleph/soul.md 根参照节缺失或被改写。
6)【裁决写 note】用 note_manage 写审计裁决：category=lesson，tags 含 graph-audit，正文开头 YAML 块携带机器可读证据（audited_node / anchor_id / evidence_cmd / evidence_result / evidence_ts / verdict∈{pass,drift,cheat,stale}，多对象多段）。声明 supersede 上一份 graph-audit 裁决（有界账本）。
7)【上报】存在 drift/cheat/stale → 一条简短消息通知用户（不打扰原则）；全部 pass → 静默结束，不发通知。

铁律：只读取证；不修改任何环的目标/计划/阈值；不改写 soul.md；不建/不删图节点与边（例外：可对确认已消失实体的悬空边执行 loop_graph(action="gc")）；成本自律（先廉价 SQL 后分析，禁止网络搜索，bash 调用 ≤ 12 次）。"#;

/// Default schedule for the audit loop: Monday 10:00 (6-field cron expr).
pub const AUDIT_DEFAULT_CRON_EXPR: &str = "0 0 10 * * MON";

/// Default schedule for a paired watcher: daily 09:30.
pub const WATCH_DEFAULT_CRON_EXPR: &str = "0 30 9 * * *";

/// Header prepended to every watcher cron installed by
/// `loop_graph(action="pair")`. The counter-metric itself — WHAT to watch —
/// is cognition and comes from the LLM/user prompt appended after this
/// header; the header only fixes the watcher's role and discipline.
pub const WATCH_TEMPLATE_HEADER: &str = r#"你是一个「看守环」——从反指标视角审查被看守优化环的表现：胜利是否用便宜方式取得（Goodhart）？先调用 loop_graph(action="status") 确认你看守的对象与其锚点，再按下面的看守指令真实取证（bash 只读，mode=ro；不信自我报告）。

看守指令：
"#;

/// Footer appended after the watcher's counter-metric instructions.
pub const WATCH_TEMPLATE_FOOTER: &str = r#"

纪律：发现便宜赢法/指标偏航 → 裁决写 note（note_manage，category=lesson，tags 含 graph-audit，YAML 证据块含 evidence_cmd/evidence_result/evidence_ts/verdict）并简短通知用户；一切正常 → 静默结束。只读取证；不修改被看守环的目标/计划（认为参照本身错了→写提案 note，tag: reference-proposal）；bash 调用 ≤ 6 次。"#;

/// Reference-steward loop prompt (monthly by default; created by hand via
/// cron_manage + `owns_reference` edges — see the loop-governance skill).
/// The steward owns child references: proposals are decided HERE, and any
/// actual change routes through the user (approval) + the unlink→edit→relink
/// flow, leaving provenance in the graph.
pub const STEWARD_TEMPLATE: &str = r#"你是「参照治理环」——更慢的环，拥有子环的 objective（owns_reference）。每 tick：
1) loop_graph(action="status") 取你治理的子环清单与根参照原文；
2) 检索 tag 为 reference-proposal 的提案 notes 与各子环的 lessons、近期表现；
3) 逐提案裁决（以根参照为准绳）：驳回→回一条说明 note；采纳→通知用户确认，获确认后执行变更流：loop_graph(action="unlink", edge="owns_reference") 解除托管 → 修改目标 → 重新 link（全程留痕于图的 provenance 与 note）；
4) 无提案且子环表现正常 → 静默结束。
铁律：参照的每次变更都必须发生在本环的 tick 里、有提案 note、有裁决记录；root/frozen 相关变更只能出提案，人拍板。"#;

/// Arbitration prompt — used when two loops fight (a `conflicts_with`
/// situation named by the user, a loop's own self-awareness, or the audit
/// loop). Arbitration is an EVENT, not a resident service: one cron tick (or
/// one interactive turn) with both sides' state juxtaposed, judged against
/// the human root reference. Detection stays with the LLM (R7 — no
/// deterministic conflict detector, ever).
pub const ARBITRATION_TEMPLATE: &str = r#"你是「仲裁环」——两个（或多个）循环互相对抗时的权衡者。每 tick：
1) loop_graph(action="status") 取拓扑；找到你 arbitrates 指向的冲突环，并置双方的目标、近期表现（live 状态 + cron history / goal lessons）；
2) 检索历史裁决先例（tags: graph-audit 的记忆 notes）；
3) 以 root 节点的根参照原文为准绳做一次权衡裁决：谁让步、如何让（pause 一方 / 调预算 / 建议参照修订）；涉及 root/frozen 的冲突只能出提案（tag: reference-proposal），人拍板；
4) 裁决写 note（category=lesson，tags 含 graph-audit，YAML 证据块 + verdict）并通知用户一句话结论；
5) 冲突消解后：可 loop_graph(action="unlink") 移除 arbitrates 边并说明。
铁律：权衡的准绳永远向上锚到根参照，不由你自生；参照修改走治理环通道，你不直接改任何环的目标。"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_templates_carry_their_disciplines() {
        assert!(WATCH_TEMPLATE_HEADER.contains("反指标"));
        assert!(WATCH_TEMPLATE_FOOTER.contains("reference-proposal"));
        assert!(STEWARD_TEMPLATE.contains("owns_reference"));
        assert!(ARBITRATION_TEMPLATE.contains("根参照"));
        assert!(ARBITRATION_TEMPLATE.contains("arbitrates"));
    }

    #[test]
    fn audit_template_covers_the_seven_steps_and_iron_rules() {
        for needle in [
            "loop_graph(action=\"status\")",
            "锚点取证",
            "对账",
            "验尸探针",
            "点名",
            "graph-audit",
            "verdict",
            "铁律",
            "mode=ro",
        ] {
            assert!(
                AUDIT_TEMPLATE.contains(needle),
                "audit template missing: {needle}"
            );
        }
    }
}
