# Step 4 — Cache Token Observability Design Spec

**Date:** 2026-05-11
**Status:** Approved (user spec, all decisions locked)
**Scope:** Anthropic protocol cache token visibility + D4 NULL `input_tokens` migration noise

---

## Goal

让 e2e 验证不再依赖 probe — 每次 LLM 调用的 `cache_creation_tokens` / `cache_read_tokens` / `input_tokens` / `output_tokens` 直接进入 `tracing` 日志，且消除 boot 时 `Invalid column type Null at index: 11, name: input_tokens` 噪声。

## Non-Goals

- 不新增 `session_events.turn_completed` 持久化逻辑 — 现有 `LoopTraceEvent::ProviderUsage` → `GatewayTraceSink` → `TraceFlushHandle` 已具备完整链路，本步骤只补 stdout/stderr 可视化。
- 不改 `TokenUsage` struct 字段、不动 trait 签名。
- 不引入新数据库迁移 — D4 仅修读取侧 `Option<i64>` coercion。

## Background

### D3 — Cache Token Observability Gap

现状链路（已实现 → 缺口）：

```
sse.rs::dispatch_event              ✅ 已 extract cache_creation_input_tokens / cache_read_input_tokens
  ↓ ProviderDelta::Usage(TokenUsage)
DeltaCollector::push                 ✅ 累积到 self.usage
  ↓
DeltaCollector::finish               ✅ 输出到 ProviderResponse.usage
  ↓
MeteringProvider::process            ⚠️  emit LoopTraceEvent::ProviderUsage 到 TraceSink，但没有 tracing!
  ↓
GatewayTraceSink → TraceFlushHandle  ✅ 持久化路径（依赖 state_database 配置；NoopHandle 时丢弃）
```

**缺口：** 默认 stderr/stdout 日志看不到 cache 是否命中。e2e 验证必须依赖 TraceSink probe（subagent_spawner 测试套件那种模式）才能观察。生产环境如果 `TraceFlushHandle` 是 noop（很常见的部署形态），观察性归零。

**根因：** `MeteringProvider::process` 在 `src/providers/metering.rs:47-60` 只 emit 到 TraceSink，没有 `tracing::info!` 副本。

### D4 — `input_tokens` NULL Migration Noise

现状：

- 历史表 schema 通过 `migrate_legacy_sessions_columns` (`src/gateway/session_store/migration.rs:291`) 用 `ALTER TABLE ADD COLUMN input_tokens INTEGER`（无 `DEFAULT 0`、无 `NOT NULL`）补字段，已有行的 `input_tokens` 为 `NULL`。
- 读取侧 `map_session_metadata` (`src/gateway/session_store/sqlite_backend/mod.rs:61`) 用 `row.get(11)?` 期望非 NULL `i64` —— 历史行 panic 成 `Invalid column type Null at index: 11`。
- 同样问题在 `output_tokens` (line 62)、`message_count` (line 53)、`total_tokens` (line 54) 等列也可能潜伏，但目前只有 `input_tokens` 出现噪声。

**修法：** 读取侧 `Option<i64>::unwrap_or(0)` coerce NULL 为 0。无需 backfill，无需 ALTER COLUMN，无需新 migration。

## Architecture Compliance

| 红线 | 解读 |
|------|------|
| **R7 LLM 主权** | ✅ 纯结构化日志和列读取，不引入推理 |
| **R10 笨循环 / 薄 Harness** | ✅ MeteringProvider 是已有 decorator，不动 harness 核心；仅追加 tracing 副本 |
| **P1 低耦合** | ✅ tracing 是横切关注点，不增加跨模块依赖 |
| **P6 简洁性 (KISS)** | ✅ 两个 commit、~30 行净改动、复用现有 decorator 与 column 读取流程 |

**违反检查：**
- 不引入"工具循环检测"逻辑 ✅
- 不引入"错误恢复策略选择" ✅
- 不引入"完成度判断" ✅
- 不新增中间件 ✅

## Design

### Commit 1 — MeteringProvider tracing emit

**File:** `src/providers/metering.rs`

在 `process()` 既有 `sink.on_trace(...)` 调用旁追加 `tracing::info!`，字段命名沿用 `LoopTraceEvent::ProviderUsage` 的约定（保持搜索可定位）：

```rust
// 在 sink.on_trace 调用之前/之后追加
tracing::info!(
    target: "aleph::provider_usage",
    agent_id = %self.agent_id,
    provider = %self.inner.name(),
    input_tokens = usage.input_tokens,
    output_tokens = usage.output_tokens,
    cache_read_tokens = ?usage.cache_read_tokens,
    cache_creation_tokens = ?usage.cache_creation_tokens,
    thinking_tokens = ?usage.thinking_tokens,
    "LLM call completed"
);
```

**关键点：**
- `target: "aleph::provider_usage"` — 提供独立 filter target，运维可单独 `RUST_LOG=aleph::provider_usage=info` 查询。
- `provider = %self.inner.name()` — 不是 `MeteringProvider::name()`（那是 decorator 透传），是被包装的真实 provider。
- 仅在 `resp.usage.is_some()` 分支 emit（与 TraceSink emit 同条件），无 usage 不打 noise。

### Commit 2 — Sqlite NULL coercion fix

**File:** `src/gateway/session_store/sqlite_backend/mod.rs`

`map_session_metadata` 中所有可能 NULL 的 nullable 列改为 `Option<T>::unwrap_or(default)` 模式：

```rust
input_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
output_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
```

不动其他列。`message_count` / `total_tokens` 等如果将来出现同样症状再补；YAGNI。

### Tests

| Test | File | 验证 |
|------|------|------|
| `metering_emits_tracing_info_with_usage_fields` | `src/providers/metering.rs::tests` | tracing-test 或 `tracing-subscriber` 测试 subscriber 捕捉 INFO 事件 |
| `sqlite_backend_handles_legacy_null_input_tokens` | `src/gateway/session_store/sqlite_backend/tests.rs` 或 inline `#[cfg(test)]` | 插入 NULL 行后 `get_metadata` 返回 `input_tokens = 0` 不 panic |

如果 `tracing-test` crate 未引入，则用 `tracing-subscriber::fmt::layer()` 写入自定义 `MakeWriter` 收集器；或者降级为 "tracing event is emitted but not asserted on fields"，依赖 e2e 手测验证字段。**优先级：能加就加；不阻塞主修复。**

## E2E Verification

发布后回到 webchat（kimi-for-coding，default provider，short cache_retention），发一条短消息触发 cache write，再发一条触发 cache read。期望：

```
INFO aleph::provider_usage agent_id=root provider=kimi-for-coding
     input_tokens=5 output_tokens=24
     cache_read_tokens=Some(0) cache_creation_tokens=Some(1832)
     "LLM call completed"
[第二条]
INFO aleph::provider_usage agent_id=root provider=kimi-for-coding
     input_tokens=5 output_tokens=18
     cache_read_tokens=Some(1832) cache_creation_tokens=Some(0)
     "LLM call completed"
```

第二轮 `cache_read_tokens` 非零 = cache 命中。

同时 boot 日志不再有 `Invalid column type Null at index: 11`。

## Risk Assessment

| 风险 | 等级 | 缓解 |
|------|------|------|
| tracing 性能 (LLM 完成后 1 行 INFO) | 极低 | INFO 级别有 filter；ENABLE/DISABLE via `RUST_LOG` |
| Option coercion 改变行为 | 低 | NULL → 0 与 `i64::default()` 一致；调用方 `SessionMetadata.input_tokens` 已是 `i64` |
| 与 D3 后续 session_events 增强冲突 | 无 | 本步骤不动 session_events；后续 Step 可继续在 GatewayTraceSink 端补 |

## Out of Scope Followups

- D3-extended：session_events 持久化 ProviderUsage 事件（验证 GatewayTraceSink 在生产是否走的是 noop） — 留给后续 Step。
- D5：484 baseline test compile errors — Step 6 单独处理。
- D2：default provider hot-reload — Step 5 单独处理。
- D6：desktop screenshot 内容质量 — 独立桌面工具问题。

## Commit Plan

| # | Title | Files | LOC est. |
|---|-------|-------|---------|
| 1 | `providers/metering: emit tracing::info with full token usage fields` | `metering.rs` (+ optional test) | ~20 |
| 2 | `gateway/session_store: coerce legacy NULL token columns to zero` | `sqlite_backend/mod.rs` (+ optional test) | ~10 |

CHANGELOG 在每个 commit 的 `[Unreleased] ### Fixed` 段追加一行。

---

## Self-Review

- ✅ 覆盖 D3 + D4 两个观测性缺口
- ✅ 没有 placeholder（所有路径、行号、字段名已锁定）
- ✅ 类型一致（`Option<i64>::unwrap_or(0)` 返回 `i64`，与 `SessionMetadata.input_tokens` 字段类型一致）
- ✅ R7/R10 红线遵守
- ✅ KISS：2 commits、~30 行净改动
- ✅ E2E 验证脚本明确

Ready for implementation plan.
