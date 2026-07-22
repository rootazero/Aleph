# 静态代码审查报告 — shared-protocol

- 审查单元: `shared-protocol` | `shared/protocol` | 共享协议类型
- 审查日期: 2026-07-22(基于 worktree `/tmp/aleph-review-batch-5`,与 main 一致)
- 审查方式: 全量静态阅读(无 diff),四视角 checklist(安全/逻辑/架构/质量)

## 统计

- Rust 源文件: 25 个(含 1 个 bin target `export_desktop_bridge_schema.rs`)
- 总 LOC: 6388(含测试;非测试约 4200)
- 模块构成: `jsonrpc` / `events` / `trace_presentation` / `auth` / `invitation` / `discovery` / `thinking` / `canvas_format` / `subagent_tree` / `voice_text` / `trace_replay` / `ids`(私有)/ `desktop_bridge`(envelope + errors + 7 个 methods 子模块)
- 依赖: chrono, schemars, serde, serde_json, thiserror(未使用)

## 历史问题验证

| 历史问题(2026-07-20) | 现状 |
|---|---|
| jsonrpc.rs 使用 uuid(R3 重依赖) | **已修复**。`Cargo.toml` 无 uuid;新增私有模块 `ids.rs` 用 `AtomicU64` 进程内单调计数器生成 `"id-N"`,注释明确说明为避免 uuid→rand 依赖链(R3)。计数器仅用于请求/响应关联与审计标记,非安全用途,方案合理 |
| events.rs 980 行大文件 | **未修复**,仍为 980 行(见 L1) |
| trace_presentation.rs 933 行大文件 | **未修复**,现 965 行(见 L2) |

## 发现列表(按严重级排序)

**Critical: 0 | High: 0 | Medium: 0 | Low: 7**

### L1 — `src/events.rs`(全文件) — Low — 超大文件(980 行)
非测试代码约 733 行,超过 500 行阈值。内容几乎全是 `StreamEvent` / `AgentTraceEvent` / `RunSummary` 等纯类型定义 + serde 兼容测试,风险低,但 `AgentTraceEvent` 持续膨胀(已 20 个 variant)。
建议: 将 `AgentTraceEvent` 及其关联类型拆为 `events/agent_trace.rs`,`StreamEvent`/`RunSummary` 留在 `events/mod.rs`。

### L2 — `src/trace_presentation.rs`(全文件) — Low — 超大文件(965 行)
非测试代码约 657 行。`present_agent_trace_event` 单函数 340 行(217–555),随 `AgentTraceEvent` 每个新 variant 同步膨胀。
建议: 与 L1 联动拆分;或按 variant 域(core loop / worktree / MoA)拆子模块。

### L3 — `Cargo.toml:17` — Low — 死依赖 thiserror
`thiserror = "2.0"` 声明在 `[dependencies]`,但全 crate 无任何 `thiserror` 使用(grep 仅命中 Cargo.toml 自身)。crate 自述"intentionally minimal - pure types only",死依赖与此矛盾且拖入下游构建图。
建议: 删除该行。

### L4 — `src/desktop_bridge/methods/ax.rs:1-7` — Low — 模块文档与实际 API 漂移
模块 doc 称"Exposes three read-only AX operations",但同文件已定义 `METHOD_SET_VALUE` / `METHOD_PERFORM_ACTION` / `NOTIFY_MUTATION`(19–21 行)及 `SetValueParams`/`PerformActionParams` 等写操作类型。
建议: 更新模块 doc,列出 3 读 + 2 写 + 1 通知。

### L5 — `src/desktop_bridge/methods/pim.rs:399` — Low — DRY: MailGetResult 与 MailMessageDetail 完全相同
`MailMessageDetail`(367 行)与 `MailGetResult`(399 行)9 个字段逐一相同(id/subject/sender/recipients/cc/bcc/date/body/is_read/attachments),是纯复制。
建议: 删除 `MailGetResult`,`pim.mail.get` 直接返回 `MailMessageDetail`(wire 形状不变,向后兼容)。

### L6 — `src/subagent_tree.rs:130-137` — Low — 文档承诺与实现行为不一致
`build_tree` doc 声称 orphan-tolerance "never drop a node",但两个节点 parent_id 互指成环(且各自 parent 都存在)时,二者均不进 root 桶,被**静默丢弃**(测试 `cycle_does_not_infinite_loop` 明确断言森林为空)。当前运行时的 request_id 由 tracker 生成,成环实际不可达,故仅 Low。
建议: 修正 doc 注明环内节点会被丢弃;或将不可达节点兜底提升为 root(与 dangling parent 一致)。

### L7 — `src/jsonrpc.rs:26-46` 与 `src/desktop_bridge/errors.rs:9-14` — Low — 同 crate 两套错误码注册表数值重叠、语义不同
gateway 侧 `-32001` = `SESSION_NOT_FOUND`,desktop bridge 侧 `-32001` = `ERR_PERMISSION_DENIED`;`-32002` 亦分别表示 `RATE_LIMITED` / `ERR_NOT_IMPLEMENTED`。两套协议互不混线,数值冲突无功能影响,但同 crate 内容易误用。
建议: 在 `desktop_bridge/errors.rs` 顶部注释明确"与 jsonrpc.rs 的 gateway 错误码相互独立";或让 bridge 复用 `jsonrpc` 常量并为 bridge 专属码取不冲突值。

## 四视角结论

- **安全**: 无注入/泄漏/越权/SSRF/证书面——crate 无网络、无 I/O、无 unsafe。亮点: `AxElement.value`(ax.rs:150-155)带显式 SECURITY 注释,要求经 `safe_value` 脱敏后才进入 model 可见载荷;`IdentityContext` 权限快照冻结设计正确;`ids.rs` 明确计数器 id 非安全用途。
- **逻辑正确性**: 无竞态(`AtomicU64` 用法正确)、无锁、无 panic 路径(生产代码 0 个 `unwrap()`/`expect()`,全部位于 `#[cfg(test)]`);`truncate`(trace_presentation.rs:637)char-boundary 安全且处理了 limit≤3 边界;`compute_rollup`(subagent_tree.rs:215)防了除零;`auth.rs:220` 时间戳溢出用 `unwrap_or(i64::MAX)` 兜底。未发现 panic/边界/资源泄漏问题。
- **架构红线**: 全部合规。R3 历史违规(uuid)已修复;R1 合规(crate 不触平台 API,desktop_bridge 只是 IPC 类型契约);R4/R7 正向支持——trace_presentation 把渲染逻辑收敛为 CLI/TUI/Web 共享单一事实源;`AgentTraceEvent::SessionCompleted` 用 opaque `Value` 避免 protocol crate 依赖 alephcore 类型,方向正确。R8 无关(无正则)。
- **质量**: 测试覆盖良好(每个文件都有 serde round-trip 与 wire-compat 守卫测试,包括防 drift 的 golden-shape 测试);问题集中于两个超大文件、一个死依赖、若干文档漂移(见上)。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|---|---|---|
| R1 | ✅ | 纯类型 crate,无平台 API |
| R2 | ✅ | 无 UI 代码 |
| R3 | ✅(已修复) | uuid 已移除;但 thiserror 死依赖待清(L3) |
| R4 | ✅ | 无业务逻辑,纯 I/O 类型 + 展示格式化 |
| R7 | ✅ | presentation/tree 重建收敛在共享 crate,供多前端复用 |
| R8 | ✅ | 无正则 |
| R9 | N/A | 无可配置项 |
| R10 | N/A | 无 prompt 相关代码 |
