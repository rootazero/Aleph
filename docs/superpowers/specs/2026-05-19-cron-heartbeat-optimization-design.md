# Cron & Heartbeat 子系统优化设计 (2026-05-19)

> 参考 hermes-agent (`/Volumes/TBU4/Github/hermes-agent`) 的定时任务 / 心跳实现，
> 对 Aleph `src/tasks/` 下的 cron 与 heartbeat 子系统做缺陷修复、可靠性加固、
> 功能补全与死代码清理。**非破坏性重构** —— 复用现有模块，只补缺线、修错误。

## 背景 (Context)

Aleph 已有完整的 `src/tasks/cron/` 与 `src/tasks/heartbeat/` 子系统，二者均在
`aleph-server` 启动时接线。但文件级审计（对照 hermes 设计逐项核对）发现：基础设施
齐备，**关键连线缺失或错误**，导致核心能力实际不工作。

hermes 参考要点：
- cron：执行**前** `advance_next_run` 推进 schedule → at-most-once；漏跑按周期
  缩放的宽限期 fast-forward；失败 backoff；inactivity timeout。
- heartbeat：hermes 的 "heartbeat" 是父 agent 给子 agent 的存活探测，**不是**
  调度特性。Aleph 的 heartbeat 是更先进的 L1 探针 / L2 agent 两层主动监控 ——
  这是 Aleph 的超越点，但投递链路断了。

## 缺陷清单 (Findings)

### Cron

| ID | 级别 | 位置 | 缺陷 |
|----|------|------|------|
| C1 | CRITICAL | `service/concurrency.rs:54` | 执行前不推进 `next_run_at_ms`；崩溃后重复执行 |
| C2 | HIGH | `service/catchup.rs:70` | 漏跑无周期宽限期；补发任意陈旧的 run |
| C3 | HIGH | `config.rs:130` / `concurrency.rs` | `delete_after_run` 默认开启却从不执行 |
| C4 | HIGH | `shared/schedule.rs:121` | `compute_backoff_ms` 已实现已测试但从不调用 |
| C5 | HIGH | `config.rs:394` | `timeout_ms()` 硬编码 600s；`job_timeout_secs` 配置死字段；自带测试断言 300s 实际失败 |
| C6 | MEDIUM | `stagger.rs:43` | stagger 推进窗口最坏前移 ~2 窗口 |
| C7 | MEDIUM | `cron_manage.rs` | `Every { every_ms }` 接受亚秒间隔；缺 `Update` action |
| C8 | LOW | `store.rs` / `execution/lightweight.rs` / `template.rs` | 死结构 `CronStoreFile`、假桩 `execute_lightweight`、未接线的 `template.rs` |

### Heartbeat

| ID | 级别 | 位置 | 缺陷 |
|----|------|------|------|
| H1 | CRITICAL | `heartbeat/executor.rs:152` | `DefaultHeartbeatAdapter` 永远返回 `Silent`；`heartbeat_report` 输出被丢弃 |
| H2 | CRITICAL | `start/mod.rs:1568` | `DeliveryEngine::new()` 零注册目标；所有投递静默失败 |
| H3 | CRITICAL | `heartbeat/service/timer.rs:145` | `running_at_ms` 从不写入；任务级互斥守卫惰性死代码 |
| H4 | HIGH | `heartbeat/service/timer.rs:86` | 全局 `running` CAS 锁住所有任务直到最慢 L2 完成 |
| H5 | HIGH | `heartbeat_manage.rs:437` | `heartbeat_report` 无 run 绑定，任何 agent 可调 |
| H6 | HIGH | `heartbeat/config.rs:23` | `max_concurrent=0` / `tick_interval_secs=0` 无校验 → 饿死 / 忙循环 |
| H7 | MEDIUM | `heartbeat/probe.rs:99` | L1 探针调用无超时 |
| H8 | MEDIUM | `heartbeat/service/timer.rs:245` | 投递失败仍 `dedup.record()` → 后续重试被去重静默 |
| H9 | MEDIUM | `heartbeat/dedup.rs:107` | 每投递周期 `embed()` 调用两次 |
| H10 | MEDIUM | (缺失) | 无 heartbeat 启动 catchup（H3 修复后会导致崩溃后任务永久拉黑）|

### 死代码 (R10 YAGNI 撤回)

- `src/tools/adapters/daemon_adapter.rs` (275 行) —— `DaemonBackend` 零生产实现，
  `DaemonQueryTool`/`DaemonSubscribeTool` 从不注册；`daemon_subscribe` 接受
  `cron:` 模式是 cron 子系统的半成品重复。**删除**。

## 实施方案 (Plan) —— 4 阶段

### Phase 1 — Cron 正确性

1. **C1 at-most-once**：`phase1_mark_due_jobs` 在 `running_at_ms = now` 之后，对
   每个 due job 调用 `recompute_next_run_full`（递归类推进到下一次，`At` 类推进为
   `None`）。`phase3_writeback` 不再无条件 `next_run_at_ms = None` —— 改为只对
   `At` 类清零；递归类保留 phase1 已推进的值。崩溃发生在 phase1↔phase3 之间时，
   schedule 已推进 → 不重复执行。
2. **C2 宽限期 fast-forward**：新增 `compute_grace_ms(schedule)`（递归类 = 半周期
   clamp [120s, 2h]）。`run_startup_catchup` Phase 2 收集 missed job 时，递归类若
   `now - next_run > grace` 则就地 `recompute_next_run_full` 快进、不补发；`At`
   类始终补发一次。
3. **C4 backoff 接线**：`phase3_writeback` 结果回写后，对失败的递归类 job
   `next_run = max(next_run, now + compute_backoff_ms(consecutive_errors))`。
4. **C5 timeout 接线**：`CronJob::timeout_ms()` 改回 `300_000`（与配置默认 + 自带
   测试一致）；`phase1_*` 新增 `default_timeout_ms` 参数，由 `on_timer_tick` 从
   `config.job_timeout_secs` 注入 snapshot。
5. **C3 delete_after_run**：`phase3_writeback` 写完历史后，对
   `ScheduleKind::At { delete_after_run: true }` 的 job `remove_job`。
6. **C6 stagger**：`compute_staggered_next` 的 else 分支改为模运算落到 `now` 之后
   首个窗口（实际不可达但属正确性加固）。

### Phase 2 — Heartbeat 投递接线（核心"缺线"）

1. **H1 heartbeat_report 回流**：`HeartbeatReportTool` 持有一个按 `run_id` keyed 的
   共享 slot（`Arc<Mutex<HashMap<run_id, HeartbeatReportOutput>>>`）。L2 executor
   构造工具时注入当次 `run_id`；`call()` 写入 slot。`DefaultHeartbeatAdapter`
   在 `adapter.execute()` 返回后从 slot 读回 → 据此返回 `NeedsDelivery` 或 `Silent`。
2. **H5 run 绑定**：`HeartbeatReportTool` 的 `call()` 仅当其 `run_id` 在活跃集合
   中才生效；非 heartbeat agent 调用为 no-op。
3. **H2 投递目标注册**：在 `start/mod.rs` 给 `DeliveryEngine` 注册 `WebhookTarget`
   （已存在，含 SSRF 防护）、新增 `GatewayDeliveryTarget`（经 `ChannelRegistry`
   投递，复用 cron executor 的 `deliver_to_channel` 路径）、`MemoryDeliveryTarget`
   （写入 memory note）。
4. **H3 running_at_ms 写入**：`collect_due_tasks` 取锁后写 `running_at_ms = now`，
   镜像 cron `concurrency.rs:54`。

### Phase 3 — 可靠性加固

1. **H4 解耦 running CAS**：tick 锁只覆盖"收集 + spawn"，不覆盖 `join_all`；
   spawn 后立即释放，并发上限交由 `Semaphore` + 任务级 `running_at_ms` 守卫。
2. **H6 config 校验**：`HeartbeatConfig::validate()` 拒绝 `tick_interval_secs==0`
   / `max_concurrent==0`，启动时调用。
3. **H10 heartbeat catchup**：启动时扫描清除陈旧 `running_at_ms`（镜像 cron）。
4. **H7 探针超时**：`probe.rs` 的 `executor.execute()` 包 `tokio::time::timeout`。
5. **H8 去重仅成功后记录**：`dedup.record()` 仅在投递成功时调用。
6. **H9 embed 复用**：`is_duplicate` 返回算得的 embedding 给 `record` 复用。
7. **C7 cron_manage**：`Every` 校验 `every_ms >= 1000`；新增 `Update` action 暴露
   已实现的 `update_job`。

### Phase 4 — 死代码清理 + template 接线

1. 删除 `src/tools/adapters/daemon_adapter.rs` 及 `mod.rs` 的 `pub use`。
2. 删除 `store.rs` 的 `CronStoreFile`、`execution/lightweight.rs` 的假桩
   `execute_lightweight`（确认无消费者后）。
3. **接线 `template.rs`**（用户确认的功能增强）：
   - `JobStateV2` 新增 `#[serde(default)] last_output: Option<String>` 与
     `run_count: u64`（向后兼容）。
   - `render_template` 签名改为接收 `last_output: Option<&str>` + `run_count` +
     `context_vars: Option<&str>`（解耦 `JobRun`，并支持自定义 `context_vars`）。
   - `phase1` 构造 snapshot 时：`job.prompt_template` 为 `Some` 则渲染，否则用
     `job.prompt`。
   - `phase3` 写回时 `run_count += 1`、`last_output = result.output`（截断 ~2KB）。
   - 这是 Aleph 对标 hermes `context_from` 链式上下文的等价能力（同一 job 的
     上一次输出可经 `{{last_output}}` 注入下一次 prompt）。

## 验证 (Verification)

- 每个修复配套单元测试（TDD：先写失败测试再修）。
- `cargo test -p alephcore --lib tasks::` 全绿（修复后含新测试）。
- `cargo build -p alephcore` 通过；`just clippy` 对改动文件无新增 warning。
- 注意：main 上已有基线测试失败（见 memory `project_baseline_test_failures`），
  不因无关基线失败阻塞；但 `cron_job_new_sets_defaults` 属本设计 C5 范畴，须转绿。

## 非目标 (Non-Goals)

- 不重写三阶段并发模型、不改 SQLite schema 结构（仅 `JobStateV2` 增字段）。
- 不为 cron 增加 per-job model/provider/workdir/toolset 覆盖（hermes 有，但属
  独立大功能，超出本轮范围）。
- 不实现桌面通知 / Email 投递目标（Gateway/Memory/Webhook 三目标已够闭环）。
