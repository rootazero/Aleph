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
2)【锚点取证·真实执行，不信报表】取证默认派独立审计员：subagent(agent_type="loop-auditor", task="<探针清单：每个探针取哪个测量值，要求只返回测量值>")——它以全新上下文执行探针，防「与被审计者共读同一套记忆互证正确」；你只接收测量值并裁决。图很小（≤2 个锚点）时可自行执行。取证一律 in-core 只读工具，不再 shell sqlite——`~/.aleph/data` 在每会话工作区沙箱之外，bash 读不到（会被拒 "cwd outside workspace root"）。图外的三个常备锚点：① governance_metrics(window_days=7) → 一次拿到近7天用户真实纠正数（corrections）与 dreaming 近7天按 pipeline_type 的分布（每桶含 runs / synthesis_sum / consolidated_sum / woven_sum / archived_sum / feedback_distilled_sum；各计数怎么读见该工具自己的说明）；② cron_manage(action="list") → 每个 job 的 state.run_count 与 last_run_status（run_count=0=剧场循环，last_run_status 显示连续失败=点名；cron 运行计数以此为准）。**这一条只能你自己（本会话）取——loop-auditor 审计员没有 cron_manage（它含写动作，刻意不给），派它去取必然失败。** 顺带核对：名册里有没有跑着一个图里没有对应节点的治理 cron（`循环治理·审计环` 出现两次 = 有人 drop_node 之后重装过，两个审计环会互相 supersede 裁决——点名并请用户删掉一个）。图中每个 anchor 节点：按其 body 声明的 {probe, truth} 取值——优先用工具；仅当 probe 显式声明 bash 且目标落在会话工作区内时才 bash（只读，exit_code / numeric / line_count）。
3)【对账】各环的自我报告（status 渲染里的 live 状态、lessons、上一份 graph-audit 裁决）vs 锚点新鲜取证——把「报表对报表」变成「报表对现实」。特别核对：dreaming 的 feedback_distilled_sum（纠正→反馈规则的蒸馏产出）与用户纠正数 corrections 是否同时在涨（都在涨=记忆蒸馏可能在优化脱离用户真实需要的指标——Goodhart 偏航信号；反之 corrections 涨而 feedback_distilled_sum 恒 0=纠正根本没被蒸馏进反馈规则，看守环失职）；各环声明的 cadence 与其锚指标的信号周期是否匹配（快环挂慢信号只会学到噪声）。
4)【验尸探针与冻结节点】每个 watches 看守（heartbeat 探针/看守 cron）：最近是否成功运行、还在测原来那个对象吗（表改名/文件迁移=传感器漂移）？每个 frozen 节点：按其 body 里的执法点指针核验规则未被松动（如 git diff 查棘轮文件、读 config 查硬底线）。失败/从未运行/被松动=点名。
5)【点名】run 记录为空的环（剧场循环）、连续失败的环、lint 报告的裸奔优化环与悬空边、root 节点被机器路径改写的迹象（updated_at 异动而无人类操作记录）、loop_graph status 中 root 根参照节缺失或 body 被改写（根参照以图中 root 节点为准——它是模型实际引用的那份；其人供给源 ~/.aleph/soul.md 在沙箱外，不经文件取证）。
6)【裁决写 note】用 note_manage 写审计裁决：category=lesson，tags 含 graph-audit，正文开头 YAML 块携带机器可读证据（audited_node / anchor_id / evidence_cmd / evidence_result / evidence_ts / verdict∈{pass,drift,cheat,stale}，多对象多段）。声明 supersede 上一份 graph-audit 裁决（有界账本）。
7)【上报】存在 drift/cheat/stale → 一条简短消息通知用户（不打扰原则）；全部 pass → 静默结束，不发通知。

铁律：只读取证；不修改任何环的目标/计划/阈值；不改写 soul.md；不建/不删图节点与边（例外：可对确认已消失实体的悬空边执行 loop_graph(action="gc")）；成本自律（先廉价工具取证后分析，禁止网络搜索，bash 调用 ≤ 12 次——常备锚点走 governance_metrics / cron_manage，bash 只留给显式声明的工作区内探针）。"#;

/// Default schedule for the audit loop: Monday 10:00 (6-field cron expr).
pub const AUDIT_DEFAULT_CRON_EXPR: &str = "0 0 10 * * MON";

/// Body stamped on the node `enable_audit` installs — and the marker its
/// idempotency guard keys on.
///
/// It has to be a real marker rather than "does any `audits` edge exist":
/// `Audits` is a first-class verb any loop may use (`x -[audits]-> frozen:y`
/// is a documented, encouraged hand-wiring), and keying the installer on the
/// verb made one unrelated hand-wired edge refuse `enable_audit` **forever**
/// while telling the operator to `drop_node` a node that has nothing to do
/// with the audit ring. Single-sourced with the writer so the two cannot
/// drift.
pub const AUDIT_NODE_BODY: &str =
    "唯一职责：验证其他环的测量仍触到现实。模板见 loop_graph::templates::AUDIT_TEMPLATE";

/// Default schedule for a paired watcher: daily 09:30.
pub const WATCH_DEFAULT_CRON_EXPR: &str = "0 30 9 * * *";

/// Header prepended to every watcher cron installed by
/// `loop_graph(action="pair")`. The counter-metric itself — WHAT to watch —
/// is cognition and comes from the LLM/user prompt appended after this
/// header; the header only fixes the watcher's role and discipline.
pub const WATCH_TEMPLATE_HEADER: &str = r#"你是一个「看守环」——从反指标视角审查被看守优化环的表现：胜利是否用便宜方式取得（Goodhart）？先调用 loop_graph(action="status") 确认你看守的对象与其锚点，再按下面的看守指令真实取证（只读；不信自我报告）。治理常备信号——用户纠正数 / dreaming 分布——用 governance_metrics 工具取，cron 运行计数用 cron_manage(action="list")；`~/.aleph/data` 在工作区沙箱外，不要 shell sqlite（会被拒）。取证优先派独立审计员 subagent(agent_type="loop-auditor", task="<反指标探针+返回测量值>")——独立上下文测量，防与被看守环共读同套数据互证正确。

看守指令：
"#;

/// Footer appended after the watcher's counter-metric instructions.
pub const WATCH_TEMPLATE_FOOTER: &str = r#"

纪律：发现便宜赢法/指标偏航 → 裁决写 note（note_manage，category=lesson，tags 含 graph-audit，YAML 证据块含 evidence_cmd/evidence_result/evidence_ts/verdict）并简短通知用户；一切正常 → 静默结束。只读取证；不修改被看守环的目标/计划（认为参照本身错了→写提案 note，tag: reference-proposal）；bash 调用 ≤ 6 次。"#;

// Steward / arbitration prompts deliberately live in the `loop-governance`
// skill, not here: no tool action installs them (steward loops are created by
// hand via cron_manage + `owns_reference` edges, and arbitration is an EVENT,
// not a resident service — spec §4.3). The former STEWARD_TEMPLATE /
// ARBITRATION_TEMPLATE consts had zero consumers and were cut (R10 YAGNI);
// re-add one only together with the tool action that installs it.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_templates_carry_their_disciplines() {
        assert!(WATCH_TEMPLATE_HEADER.contains("反指标"));
        assert!(WATCH_TEMPLATE_FOOTER.contains("reference-proposal"));
        assert!(WATCH_TEMPLATE_HEADER.contains("loop-auditor"));
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
            "governance_metrics",
            "cron_manage(action=\"list\")",
            "loop-auditor",
            "agent_type",
        ] {
            assert!(
                AUDIT_TEMPLATE.contains(needle),
                "audit template missing: {needle}"
            );
        }
    }

    /// The audit/watch probes must NOT instruct shelling `sqlite3` against
    /// `~/.aleph/data` — that path is outside the per-session workspace sandbox,
    /// so a headless cron probe is denied ("cwd outside workspace root"). Reality
    /// signals go through in-core tools instead. Guards the fix from regressing
    /// back to the dead sqlite door.
    #[test]
    fn governance_probes_do_not_shell_into_walled_off_data_dir() {
        for tmpl in [AUDIT_TEMPLATE, WATCH_TEMPLATE_HEADER] {
            assert!(
                !tmpl.contains("sqlite3"),
                "governance probe must not shell sqlite3"
            );
            assert!(
                !tmpl.contains("/.aleph/data/"),
                "governance probe must not read the sandbox-walled ~/.aleph/data"
            );
        }
    }
}
