---
date: 2026-04-04
topic: telegram-robustness-hardening
---

# Telegram Channel 鲁棒性加固 (Round 3)

## Problem Frame

Round 1（可靠性增强）和 Round 2（流式投递）为 Aleph 的 Telegram Channel 新增了大量功能：消息合并、offset 持久化、错误冷却、流式编辑。但通过与 OpenClaw 的对比分析和代码审计，发现底层基础设施仍有六处薄弱环节，任何一处在生产中都可能导致静默故障：

1. **附件获取无超时** — `extract_attachments()` 调用 `bot.get_file()` 无 timeout，Bot API 慢响应时 handler 线程无限阻塞
2. **配对用户重启即失** — `AccessController` 的运行时配对列表纯内存存储，进程重启后用户必须重新配对
3. **Watchdog 任务未监管** — `polling.rs` 中 watchdog 的 `JoinHandle` 被 `let _watchdog` 丢弃，panic 后 dispatcher 继续运行但无健康监控
4. **Typing 断路器永久熔断** — 连续 10 次 `sendChatAction` 失败后，断路器永远关闭直到一次成功（但已经不发送了，所以永远没有成功机会）
5. **错误冷却无自动清扫** — `sweep_expired()` 方法存在但无人调用，DashMap 可能无限增长
6. **无启动自检** — 常见配置错误（privacy mode 未关闭、群组不可达、bot 权限不足）静默失败，用户无从知晓

本次优化目标：**不增加新功能，只让现有功能在所有边缘场景下都正确工作**。

## Requirements

**附件获取超时与降级 (Attachment Fetch Resilience)**

- R1. `extract_attachments()` 中的 `bot.get_file()` 调用包裹 `tokio::time::timeout(Duration::from_secs(5), ...)`。超时后降级为无 URL 的附件（保留 file_id、mime_type、size），而非丢弃整个附件
- R2. 降级附件的 `url` 字段为 `None`。当前 `media_download` 阶段对 url/path/data 全为 None 的附件会返回错误并丢弃，需同步修改：遇到 `url: None` 且 `file_id` 存在时，跳过下载并将附件原样传递给 LLM 上下文（file_id + mime_type 作为引用）

**配对用户持久化 (Pairing Persistence)**

- R3. `AccessController` 的运行时配对用户列表持久化到 SQLite `StateDatabase`。表结构：`paired_users(channel_id TEXT, user_id INTEGER NOT NULL, paired_at TEXT, PRIMARY KEY(channel_id, user_id))`。注：Telegram user_id 为 i64，SQLite INTEGER 可存 8 字节，rusqlite 绑定参数必须使用 i64 以匹配 `AccessController` 内部的 `Vec<i64>`
- R4. 启动时从数据库加载已配对用户，合并到内存列表（与 config 中的静态 `allowed_users` 共存）
- R5. 配对成功时同步写入数据库。写入失败仅 warn 日志，不阻塞配对流程（内存已更新，下次重启可通过重新配对恢复）
- R6. `AccessController` 需要接收 `StateDatabase` 的引用。通过 `TelegramChannel::set_state_database()` 注入（复用 offset tracker 同样的注入模式）

**Watchdog 任务监管 (Watchdog Supervision)**

- R7. Watchdog 的 `JoinHandle` 纳入 `tokio::select!` 监控。若 watchdog task panic 或意外退出（非正常取消），视为健康检查失效，触发 dispatcher 重启（与 stall 相同的重启路径）
- R8. 将 `let _watchdog = tokio::spawn(...)` 改为 `let watchdog_handle = tokio::spawn(...)`，在 select! 中增加 `result = &mut watchdog_handle => { /* handle panic */ }` 分支

**Typing 断路器衰减 (Typing Breaker Decay)**

- R9. Typing 断路器从"永久熔断"改为"时间衰减"：熔断后 5 分钟自动半开（允许一次试探发送）。试探成功则完全恢复，失败则重新熔断 5 分钟
- R10. 实现方式：替换当前的纯计数器为能记录熔断时间戳的数据结构（具体同步原语由规划阶段决定）。`check_typing()` 语义：未熔断 → true；已熔断但 >5min → true（半开试探）；已熔断且 <5min → false
- R11. 半开试探成功调用 `record_typing_success()` 清除熔断状态；失败调用 `record_typing_failure()` 刷新 `tripped_at`

**错误冷却自动清扫 (Cooldown Auto-Sweep)**

- R12. 在 `run_polling_loop` 中启动一个后台 sweep 任务，每 30 分钟调用一次 `error_cooldown.sweep_expired()`。与 watchdog 共享 `CancellationToken`，polling loop 退出时一并取消
- R13. sweep 任务的 `JoinHandle` 无需纳入 select! 监控 — sweep 失败不影响核心功能，仅 warn 日志

**启动自检与诊断 (Boot Diagnostics)**

- R14. `TelegramChannel::start()` 在 `get_me()` 成功后执行一组非阻塞诊断检查，结果以结构化日志输出（warn 级别），不阻塞启动流程
- R15. 诊断项：(a) bot `can_read_all_group_messages` 是否为 true（privacy mode 关闭检测）；(b) 已配置的群组是否可达（`get_chat()` 探测）；(c) bot 在群组中的管理员权限（可选，仅在群组可达时检查）
- R16. 诊断结果汇总为一条日志，格式：`Telegram boot diagnostics: privacy_mode=disabled, groups_reachable=2/2, warnings=[]`。有问题时 warnings 数组包含可操作的修复建议（如 "Talk to @BotFather and disable privacy mode for group message access"）

## Success Criteria

- 模拟 Bot API 慢响应（>5s）时，附件获取超时降级而非 handler 阻塞（R1-R2）
- 服务重启后，之前通过配对码配对的用户无需重新配对（R3-R6）
- watchdog task panic 后，polling loop 在下一个 select! 周期检测到并重启（R7-R8）
- typing 断路器熔断后 5 分钟自动恢复试探（R9-R11）
- 长时间运行后 DashMap 中无过期 cooldown 条目堆积（R12-R13）
- 启动日志中可见 bot 配置状态和潜在问题警告（R14-R16）

## Scope Boundaries

- **不包含**: Reasoning Lane 双通道投递 — 留给 Round 4
- **不包含**: 消息去重/幂等性 — PostConnect 重试产生重复的概率极低（需要精确的"已发送但连接断开"窗口），ROI 不足
- **不包含**: 可观测性指标（Prometheus/metrics）— 当前用结构化日志足够，metrics 系统待整体规划
- **不包含**: Group migration 处理 — 低频边缘场景，观察实际需求再决定
- **不修改**: Channel trait 接口 — 所有变更对其他 channel 透明
- **不修改**: Coalescer 核心逻辑 — Round 1 实现已验证

## Key Decisions

- **附件降级而非丢弃**: 超时后保留 file_id 等元数据，LLM 至少知道"用户发了一个文件"。比完全丢弃（用户以为发送成功但 AI 无反应）体验好得多
- **配对存 SQLite 而非文件**: 复用 Aleph 现有的 `StateDatabase` 基础设施，事务安全，与 offset 持久化一致
- **Typing 断路器用时间衰减而非计数衰减**: 避免"死锁"——断路器关闭后不再发送 typing，自然也没有成功机会来重置计数器。时间衰减打破这个循环
- **诊断不阻塞启动**: 诊断失败（如网络问题）不应阻止 bot 启动。用 warn 日志提醒即可，用户可稍后修复
- **Sweep 频率 30 分钟**: 比 cooldown 最长持续时间（4 小时）短得多，确保过期条目及时清理；比每条消息都检查高效得多

## Dependencies / Assumptions

- `StateDatabase` 已支持幂等 migration 函数（`migrate_add_*` 模式），新增 `migrate_add_paired_users()` 即可
- `AccessController` 当前接收 `TelegramConfig`（clone）作为构造参数，新增 `StateDatabase` 引用需要调整构造方式
- teloxide 的 `get_me()` 返回的 `Me` struct 包含 `can_read_all_group_messages` 字段（需验证）
- `error_cooldown.rs` 中的 `sweep_expired()` 方法已存在且经测试，仅需从后台任务调用

## Outstanding Questions

### Resolve Before Planning

（无阻塞性问题 — 所有产品决策已确认）

### Deferred to Planning

- [Affects R1][Needs research] teloxide 的 `bot.get_file()` 是否原生支持 timeout 配置，还是必须用 `tokio::time::timeout` 包裹
- [Affects R3][Technical] `paired_users` 表的 migration 函数如何接入 `StateDatabase` 的初始化路径 — 参考 `migrate_add_channel_offsets` 的模式
- [Affects R6][Technical] `AccessController` 注入 `StateDatabase` 的最佳方式 — `Arc<StateDatabase>` 字段 vs `Option<Arc<StateDatabase>>` 延迟注入
- [Affects R7-R8][Needs research] `tokio::select!` 中 `JoinHandle` 的 `Result` 如何区分 panic（`Err(JoinError)`) vs 正常退出（`Ok(())`）
- [Affects R10][Technical] typing 断路器的 `tripped_at` 需要 `Mutex<Option<Instant>>` 还是可以用 `AtomicU64` 存储 timestamp — 考虑 `check_typing()` 的调用频率
- [Affects R15][Needs research] teloxide 的 `Me` struct 是否暴露 `can_read_all_group_messages` 字段，以及 `get_chat()` 对不可达群组的错误类型

## Next Steps

→ `/ce:plan` for structured implementation planning
