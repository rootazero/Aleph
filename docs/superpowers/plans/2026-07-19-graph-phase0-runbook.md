# Graph 层 Phase 0 运行手册（零代码宪章 · 落地记录与待办）

> 对应 spec：[2026-07-19-graph-engineering-loop-graph-layer-design.md](../specs/2026-07-19-graph-engineering-loop-graph-layer-design.md) §9 Phase 0
> 落地时间：2026-07-19（运行中 daemon = Aleph.app 26.7.18）

## 已落地

| 制品 | 位置 | 状态 |
|---|---|---|
| `loop-governance` skill | `Aleph-skills/loop-governance/SKILL.md` + 已复制 `~/.aleph/skills/loop-governance/` | ✅ |
| 根参照（人供给） | `~/.aleph/soul.md` 追加「根参照」节 | ✅ |
| 周审计环 cron | 运行中 daemon，job_id `fa2a34ad-475e-4e3f-bf67-fa212297aa9c`，`0 0 10 * * MON` Asia/Shanghai | ✅ 下周一首跑 |
| heartbeat 服务开关 | `~/.aleph/config.toml` `[heartbeat] enabled = true` | ✅ 已翻，**下次 daemon 重启生效** |
| Dreaming×用户纠正 反指标探针 | — | ⏳ 待 daemon 重启后创建（见下） |

## 待办：创建 Dreaming×用户纠正 反指标看守（改为 watcher cron，非 heartbeat）

> **更新（2026-07-20）**：原计划用 heartbeat + `bash → sqlite ~/.aleph/data/memory.db` 做 L1 探针。这条路已废——`~/.aleph/data` 在每会话工作区沙箱之外，headless 探针的 bash 会被拒 `cwd outside workspace root`（周审计环首跑的 `cheat` 裁决点名的正是这个）。in-core 只读工具 `governance_metrics` 取代了 sqlite 探针。
>
> heartbeat **L1** 也不适合承载它：`governance_metrics` 返回 JSON 对象，`greater_than` 的 `value.as_f64()` 对对象恒 None、永不触发（与旧 JSON 探针同病）。故这个看守改由 **watcher cron（L2 LLM）** 承载——它每 tick 直接调 `governance_metrics` 并以反指标视角裁决，正是 `WATCH_TEMPLATE` 描述的形态。

**下次重启/重新部署后**，用 `loop_graph(action="pair")` 把一个 watcher cron 配到 dreaming 优化环上（watcher 模板已内置「常备信号走 governance_metrics」）：

```bash
/Applications/Aleph.app/Contents/MacOS/aleph-server gateway call tools.invoke -p '{
  "tool_name": "loop_graph",
  "arguments": {
    "action": "pair",
    "to_id": "daemon:dreaming",
    "label": "Dreaming×用户纠正 反指标看守",
    "prompt": "每 tick 调 governance_metrics(window_days=7)：若 dreaming 各 pipeline 的 synthesis_sum 与 corrections 同时在涨（记忆蒸馏可能在优化脱离用户真实需要的指标——Goodhart 偏航），裁决写 graph-audit note 并简短通知用户；否则静默。"
  }
}'
```

要点：
- **常备信号一律 `governance_metrics`**（corrections + dreaming 分布）/ `cron_manage(action="list")`（cron run_count），**不再 shell sqlite**。
- 时间戳单位（仅供理解语义）：`memory.db`（raw_memories/dream_reports）= 秒；`cron.db`（cron_job_runs）= 毫秒——工具已内部处理，调用方不需再关心。
- 在看守创建之前，周审计环第 4 步会点名「看守缺席」——这是机制在正常工作，不是故障。

## 验证清单（Phase 0 完成标准）

- [ ] 周一 10:00 审计 cron 首跑，产出第一份含结构化 YAML 证据块的 graph-audit note（tags 含 `graph-audit`）
- [ ] 若有 drift/cheat/stale 裁决 → 用户收到一条简短通知；全 pass → 静默
- [ ] daemon 重启后探针创建成功，`heartbeat_list` 可见
- [ ] 反指标探针至少一次触发或一次静默周期确认
