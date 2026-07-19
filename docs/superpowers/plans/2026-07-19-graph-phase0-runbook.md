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

## 待办：daemon 重启后创建反指标探针

运行中 daemon 的 heartbeat 服务因旧配置 `enabled=false` 未装配（工具报 "heartbeat service not configured"）。配置已翻为 true；**下次重启/重新部署后**执行：

```bash
/Applications/Aleph.app/Contents/MacOS/aleph-server gateway call tools.invoke -p '{
  "tool_name": "heartbeat_create",
  "arguments": {
    "name": "Dreaming×用户纠正 反指标看守：出现新用户纠正时，检查记忆蒸馏(dreaming)是否在优化脱离用户真实需要的指标，结论写 graph-audit note",
    "probe_tool_name": "bash",
    "probe_tool_params": {"cmd": "c=$(/usr/bin/sqlite3 \"file:$HOME/.aleph/data/memory.db?mode=ro\" \"SELECT count(*) FROM raw_memories WHERE path LIKE '"'"'aleph://correction/%'"'"' AND created_at > strftime('"'"'%s'"'"','"'"'now'"'"')-90000\"); if [ \"${c:-0}\" -gt 0 ]; then echo \"ALERT corrections_25h=$c\"; else echo \"ok corrections_25h=0\"; fi"},
    "interval_ms": 86400000,
    "probe_trigger_condition": {"contains": {"text": "ALERT"}}
  }
}'
```

要点（来自实测）：
- **触发条件只能用哨兵词 + `contains`**：bash 探针输出（`CodeExecOutput`）含 `duration_ms` 等每次变化字段，`changed` 等于 `always`；`greater_than` 对整个 JSON 对象 `as_f64()` 恒 None 永不触发。命令自算条件、输出 `ALERT`/`ok` 哨兵。
- 时间戳单位：`memory.db`（raw_memories/dream_reports）= 秒；`cron.db`（cron_job_runs）= 毫秒。
- L2 语义由任务 **name** 携带（heartbeat 无自定义 L2 prompt 字段，L2 收到 name + 探针原始输出 + 通用指令）。
- 在探针创建之前，周审计环第 4 步会点名「看守缺席」——这是机制在正常工作，不是故障。

## 验证清单（Phase 0 完成标准）

- [ ] 周一 10:00 审计 cron 首跑，产出第一份含结构化 YAML 证据块的 graph-audit note（tags 含 `graph-audit`）
- [ ] 若有 drift/cheat/stale 裁决 → 用户收到一条简短通知；全 pass → 静默
- [ ] daemon 重启后探针创建成功，`heartbeat_list` 可见
- [ ] 反指标探针至少一次触发或一次静默周期确认
