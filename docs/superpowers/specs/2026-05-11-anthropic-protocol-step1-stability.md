---
title: Anthropic Protocol Step 1 — Stability Hardening
date: 2026-05-11
status: draft
spec_owner: Aleph
brainstorming_source: superpowers:brainstorming session 2026-05-11
follow_up:
  - step-2-cache-control-breadth (planned, separate spec after step 1 ships)
  - step-3-feature-parity (planned, separate spec after step 1/2 ship)
---

# Anthropic Protocol Step 1 — Stability Hardening

## 1. Background

本次 brainstorming 参照 `/Volumes/TBU4/Github/openclaw` 对 Aleph 当前 Anthropic 协议适配器 (`src/providers/protocols/anthropic/`) 做了双向探索，识别出 2 个稳定性缺口：

1. **流式空闲超时缺失** — `src/providers/protocols/anthropic/adapter.rs:170-177` 的 `stream_deltas()` 没有 per-event idle timeout。长时间 stall（如 2026-05-10 观察到的 kimi-for-coding 3.5 分钟无响应事件，参见 MEMORY.md S2033）会让请求挂死直到 `ProviderConfig.timeout_seconds`（默认 300s）才超时——而 300s 之内的 idle 流没有任何信号会被识别为故障
2. **工具参数 JSON 解析失败回退不安全** — `src/providers/delta.rs::DeltaCollector::finish()` 第 134 行附近，partial_json 累积失败时回退为 `Value::String(raw_args)`，违反下游 dispatcher 期望的 "arguments 永远是 object" 类型不变量，导致 schema 校验路径行为不可预测

**已核实在本 spec 范围外的非缺口**：

- `IndexIdTracker` 已经 per-call 创建（`adapter.rs:225` `State.block_ids: IndexIdTracker::new()` 在每次 `stream_deltas()` 调用入口处），早期探索报告中"跨请求复用"的判断**错误**——本 spec 不动它

Step 1 用 2 次小型外科手术式修复消除这 2 个真实缺口，**不引入新的抽象、不改 `ProtocolAdapter` trait 签名、不动 harness 层、不改 `ProviderDelta` 枚举、不动其他 protocol 的 adapter**。

## 2. Roadmap Context（3 步走总览）

本设计是面向 Anthropic 协议的 **3 步走优化** 第 1 步。后两步推迟到 Step 1 ship 后独立 brainstorm。

本设计是 3 步走方案的第 1 步：

| Step | 主题 | 状态 | 改动量 | 价值 |
|---|---|---|---|---|
| **1. 稳定性兜底**（本文档） | idle 超时 + parse 失败硬化 | 待 ship | 2 文件 ~60 行 | 消除当下 stall + 静默失败 |
| 2. 缓存广度 | `CacheControl` 扩 TTL + 末条 user msg 缓存 + system prompt 边界切分 | Step 1 ship 后单独 brainstorm | 预估 4 文件 ~150 行 | 多轮会话 input token 降 40–80% |
| 3. 特性追平 | adaptive thinking + beta headers + 流式 partial JSON | Step 1/2 ship 后视情况 brainstorm | 预估 5–6 文件 ~250 行 | Opus 4.7 高效推理 + 实时工具参数 UX |

每步独立 brainstorm → spec → plan → 实施 → revert-able commit。

## 3. Design Decision: Unified Failure-Mode Strategy

复用 Aleph 已经 ship 的下列基础设施作为 Step 1 所有失败场景的统一兜底机制：

- **`DefaultPromptBuilder` 孤儿过滤器** (`src/harness/prompt.rs:62-106`) — 检测无配对 `tool_result` 的孤儿 `tool_use` 块并剥除
- **`providers/message.rs:316-343`** 合成 ToolResult 兜底（兜底的兜底）

**两个 Step 1 修复点遵循同一失败哲学**：

```
失败发生
  ↓
干净抛错（AlephError::provider）
  ↓
harness 现有错误冒泡处理结束本轮
  ↓
session log 持久化：assistant 消息含未配对的 tool_use
  ↓
下一轮 DefaultPromptBuilder::build() → 孤儿过滤器自动剥除
  ↓
模型看到上下文里少了那次工具调用，自主决定如何继续
```

**零新代码路径**——这是 R10 笨循环哲学的延伸：harness 不做错误恢复策略选择，让 LLM 在下一轮自己决定。

## 4. Step 1 改动详述

### 4.1 改动 1：工具参数 JSON 解析失败硬化（Commit 1）

**文件**：`src/providers/delta.rs`，函数 `DeltaCollector::finish`

**当前代码**（`delta.rs` 内 `DeltaCollector::finish` 的 partial_json 解析分支，行号约 134 区段，实施时以 grep `"falling back to raw string value"` 定位为准）：

```rust
Err(e) => {
    warn!(
        tool_id = %id,
        tool_name = %name,
        error = %e,
        raw_args = %raw_args,
        "Malformed tool arguments — falling back to raw string value"
    );
    Value::String(raw_args)
}
```

**注意**：现有 `warn!` 宏已经具备 `tool_id`/`tool_name`/`error`/`raw_args` 四个结构化字段——所以**唯一真实代码改动是 1 行返回值 + 1 行日志消息文案**。

**目标代码**：

```rust
Err(e) => {
    warn!(
        tool_id = %id,
        tool_name = %name,
        error = %e,
        raw_args = %raw_args,
        "Malformed tool arguments — defaulting to empty object (was: raw string fallback)"
    );
    Value::Object(serde_json::Map::new())
}
```

**设计理由**：

- 维持 `arguments: Value` 类型为 Object 的不变量（dispatcher 假设条件）
- Dispatcher 走现有 schema 校验路径，自动报 "missing required field X"
- ToolError 通过既有路径写入 session log
- 模型在下一轮看到错误反馈自我纠正
- `raw_args` 保留在结构化日志里供遥测/调试，不丢失信号

**风险评估**：极低。`Value::Object({})` 是 `Value::String("...")` 的安全弱化——dispatcher 对 String 本来就会失败，改为 Object 也会失败，但失败信息更标准（"missing field" vs "type mismatch"）。**不会让任何当前正常工作的代码停止工作。**

### 4.2 改动 2：流式空闲超时（Commit 2）

**文件**：

- `src/providers/protocols/anthropic.rs`（`AnthropicProtocol` struct 定义在 line 47）
- `src/providers/protocols/anthropic/adapter.rs::stream_deltas`（`adapter.rs:170` 起的方法体）
- `src/config/types/provider.rs::ProviderConfig`（line 28，已存在 `timeout_seconds: u64`，本改动新增同级字段）

**配置传递路径设计（关键设计决策）**

实施面临的问题：`ProtocolAdapter::stream_deltas(&self, response)` trait 签名**没有 `config` 参数**（`src/providers/adapter.rs:177`），而 idle timeout 阈值在 `ProviderConfig` 里。直接给 trait 加 config 参数会破坏所有其他 protocol（OpenAI/Gemini/Ollama）——违反 Step 1 "不动其他 protocol" 的非目标。

**采用方案 β'**：**复用 `AnthropicProtocol` 已有的 `last_model: Arc<RwLock<Option<String>>>` 模式**（`anthropic.rs:50` 附近），新增同级 `stream_idle_timeout_secs` 字段：

```rust
pub struct AnthropicProtocol {
    client: Client,
    name_map: ToolNameMap,
    last_model: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    /// Per-event idle timeout (seconds). Written by build_request from
    /// ProviderConfig, read by stream_deltas. 0 = disabled. Default 60.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
```

**用 `AtomicU64` 而非 `RwLock<u64>` 的理由**：
- 单 u64 值，原子操作 lock-free 即可，无需 RwLock 开销
- 哨兵语义：`0` 表示禁用（与用户面 `stream_idle_timeout_secs = 0` 一致），不需要 `Option`
- 与现有 `last_model: RwLock<Option<String>>` 不冲突——String 需要 RwLock，u64 不需要

**数据流**：

```
ProviderConfig.stream_idle_timeout_secs (config.toml 用户配置)
  ↓ 用户未配 → unwrap_or(60)
build_request(&self, payload, &config) 内：
  self.stream_idle_timeout_secs.store(secs, Relaxed)
  ↓
stream_deltas(&self, response) 内：
  let secs = self.stream_idle_timeout_secs.load(Relaxed);
  // wrap byte_stream with idle-timeout watchdog
```

**新增配置字段**（`ProviderConfig` in `src/config/types/provider.rs`）：

```rust
/// Per-event idle timeout for streaming responses, in seconds.
/// 0 or unset = 60 seconds (default).
/// Set explicitly to disable: requires future explicit "disabled" semantic
/// (currently 0 = use default; if user wants to disable they should set a
/// very large value like 86400). NB: 0-means-default chosen to match
/// timeout_seconds field's existing semantics.
#[serde(default)]
pub stream_idle_timeout_secs: Option<u64>,
```

**`stream_deltas` 改动伪代码**：

```rust
async fn stream_deltas(&self, response: reqwest::Response)
    -> Result<BoxStream<'static, Result<ProviderDelta>>>
{
    // …existing status/error handling unchanged…

    let idle_secs = self.stream_idle_timeout_secs
        .load(std::sync::atomic::Ordering::Relaxed);

    let byte_stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
        .boxed();

    let byte_stream: BoxStream<'static, Result<Bytes>> = if idle_secs == 0 {
        byte_stream
    } else {
        // tokio_stream::StreamExt::timeout wraps each .next() with deadline;
        // Err(Elapsed) → AlephError::Timeout (existing variant, already
        // classified as Transient in error.rs:635)
        use tokio_stream::StreamExt as _;
        byte_stream
            .timeout(std::time::Duration::from_secs(idle_secs))
            .map(move |res| match res {
                Ok(inner) => inner,
                Err(_elapsed) => Err(AlephError::Timeout {
                    suggestion: Some(format!(
                        "Anthropic stream stalled (no SSE event for {idle_secs}s). \
                         Connection appears dead; aborting turn."
                    )),
                }),
            })
            .boxed()
    };

    // …existing State { bytes: byte_stream, ... } unfold loop unchanged…
}
```

**错误变体选择**：用 `AlephError::Timeout { suggestion }`（`error.rs:405`，已分类为 `ErrorClass::Transient` at `error.rs:635`）而非 `AlephError::provider(...)`——语义更贴，复用 retry 策略表的 `retry_on_timeout` 决策（`retry.rs:42`）。

**配置面**：

`config.toml` 文档示例（非强制，不配则默认 60s）：

```toml
# [providers.anthropic]
# stream_idle_timeout_secs = 60  # 每个 SSE 事件最大空闲秒数；建议 ≥30
```

**设计理由**：

- 60s 是 Anthropic 流式响应正常事件间隔（通常 1–2s ping + content_block_delta）的 30+ 倍——保留充足缓冲应对偶发慢事件
- per-provider 配置允许 kimi-for-coding（实测可能更慢）单独调宽到 120s
- 用 `Arc<AtomicU64>` 而非新 trait 参数 → 不动 `ProtocolAdapter` 接口 → 不影响 OpenAI/Gemini/Ollama 适配器
- 复用 `AlephError::Timeout` 而非新建错误变体 → 已有 retry 分类逻辑生效

**风险评估**：中。引入新失败模式（"stream idle timeout"），但路径完全复用既有 `AlephError::Timeout` → 既有错误冒泡 → 既有孤儿过滤器接管。需要 CHANGELOG 标注用户可能见到的新错误信息。

## 5. 复用现有基础设施清单

本 Step 1 完全建立在已有基础设施之上：

| 复用项 | 路径 | 用途 |
|---|---|---|
| `AlephError::Timeout { suggestion }` | `src/error.rs:405` | idle timeout 错误变体，已分类为 `ErrorClass::Transient` (`error.rs:635`)，已在 retry 策略表生效 (`retry.rs:42`)，零新增 |
| `DefaultPromptBuilder` 孤儿过滤器 | `src/harness/prompt.rs:62-106` | 失败兜底，零新增 |
| 合成 ToolResult 兜底 | `src/providers/message.rs:316-343` | 兜底的兜底，零新增 |
| `ProviderConfig` struct | `src/config/types/provider.rs:28` | 已有 per-provider 配置容器（已有 `timeout_seconds: u64` 等字段），只加 1 个 `Option<u64>` 同级字段 |
| `AnthropicProtocol::last_model` Arc 模式 | `src/providers/protocols/anthropic.rs:50` 附近 | 已有 build_request→stream_deltas 共享状态模式，新字段 `stream_idle_timeout_secs: Arc<AtomicU64>` 镜像该模式 |
| `tracing::warn!` 宏 | 已全局使用 | 结构化日志通道，仅改文案 |

**不引入**：

- 不新增 trait 或 module
- 不改 `ProviderDelta` 枚举
- 不动 `src/harness/` 任何文件
- 不动其他 protocol（OpenAI / Gemini）
- 不动 webchat / CLI / Panel 任何客户端

## 6. 测试矩阵

### 6.1 新增单元测试

| 文件 | 测试名 | 验证内容 |
|---|---|---|
| `delta.rs` 测试模块 | `malformed_tool_args_becomes_empty_object` | `partial_json = "{\"file_path\":\"/foo"`（截断）→ `NativeToolCall.arguments == Value::Object({})` |
| `delta.rs` 测试模块 | `malformed_tool_args_logs_raw` | 同上 → 日志含 `raw_args` 字段为完整截断字符串 |
| `adapter.rs` 测试模块 | `stream_idle_timeout_fires_after_threshold` | mock 60s+ 不发数据的 SSE 流 → 返回 `AlephError::Timeout` 且 suggestion 含 "stalled" |
| `adapter.rs` 测试模块 | `stream_idle_timeout_resets_on_event` | mock 每 30s 一个 ping 持续 5 分钟 → 不超时 |
| `adapter.rs` 测试模块 | `stream_idle_timeout_zero_disables` | 配置 `stream_idle_timeout_secs = Some(0)` → 600s 静默不超时 |

### 6.2 必须不破的回归测试

- `harness/prompt.rs::drops_orphan_tool_use_blocks_from_replayed_assistant`
- `harness/prompt.rs::drops_entire_assistant_when_only_orphans_remain`
- `providers/protocols/anthropic.rs` 全部现有 SSE round-trip 测试（含 `test_build_endpoint_default` / `test_build_endpoint_custom` / `convert_messages` 系列）
- `providers/message.rs:316` 合成 ToolResult 兜底测试
- 其他 protocol（OpenAI/Gemini/Ollama）的全部既有测试——本 Step 1 不动其代码，必须零回归

### 6.3 集成验证（手动）

| 场景 | 方法 | 预期 |
|---|---|---|
| 真实 Anthropic 稳定流 | 启动服务 + webchat 发普通问句 | 正常响应，无回归 |
| 真实 kimi-for-coding | 同上切到 kimi-for-coding | 正常响应，无回归 |
| 人造 stall | `socat` / mock SSE server 在中途 sleep 90s | 60s 后干净失败 + 下一轮孤儿过滤器接管，无 panic |

socat stall 测试**只跑一次手动验证**，不写自动 integration test（写自动版需要起本地 mock SSE server，工程投入与回归收益不成比例；超时逻辑本身已被单元测试覆盖）。

## 7. Rollout 计划

按依赖顺序拆 2 个独立可 revert 的 commit：

| Commit | 改动 | 测试 | 风险 |
|---|---|---|---|
| **1** | 改动 1 (`delta.rs` 单行返回值 + 文案) | 2 个新单测 | 极低 |
| **2** | 改动 2 (`anthropic.rs` 加字段 + `adapter.rs` wrap + `provider.rs` 加 Option) | 3 个新单测 + CHANGELOG.md 英文条目 | 中 |

**Commit 顺序理由**：

- Commit 1 是 1 行纯外科手术，零依赖，先 ship 拿走最小风险价值
- Commit 2 引入新失败模式 + 新结构体字段，单独 commit 便于按需 revert

**Revert 策略**：任一 commit 在生产引发问题，只回滚该 commit，前置 commit 修复保留。

**版本号**：本 Step 1 不需要单独版本号，跟随后续日常 release（CalVer `YYYY.MM.DD`）。CHANGELOG 在 Commit 2 内同步更新。

## 8. Acceptance Criteria

Step 1 视为 ship 完成当且仅当：

1. ✅ 5 个新增单元测试全部通过（2 个 `delta.rs` + 3 个 `adapter.rs`）
2. ✅ `cargo test -p alephcore --lib` 零失败（排除 MEMORY.md 记录的 8 个 baseline 失败，参见 `project_baseline_test_failures.md`）
3. ✅ `cargo clippy` 零新增 warning
4. ✅ 手动跑一次 webchat 真实对话，Anthropic 原生 + kimi-for-coding 各一次，确认无回归
5. ✅ socat 触发一次人造 stall，确认 60s 后干净失败 + 下一轮 prompt 不含孤儿
6. ✅ CHANGELOG.md 新增英文条目，说明 idle timeout 新行为和默认 60s 阈值

## 9. Explicit Non-Goals（Step 1 本期不做）

下列项目在 Step 1 **明确不做**，避免 scope creep。这些将在 Step 2 / Step 3 各自的 spec 中处理：

- ❌ 调整孤儿过滤器逻辑（它工作得很好，不动）
- ❌ 引入 `CacheControl` 1h TTL（→ Step 2）
- ❌ 末条 user message 缓存（→ Step 2）
- ❌ system prompt 边界切分（→ Step 2）
- ❌ adaptive thinking 类型（→ Step 3）
- ❌ beta headers (`fine-grained-tool-streaming-2025-05-14`, `interleaved-thinking-2025-05-14`)（→ Step 3）
- ❌ 流式 partial JSON 增量解析（→ Step 3）

## 10. Permanent Non-Goals（永不做，钉死边界）

下列项目**永远不进入 Step 1 / 2 / 3 任何阶段**：

| 不做项 | 不做的根本原因 |
|---|---|
| **OAuth 令牌 (`sk-ant-oat`) + `claude-code-20250219` beta + Claude Code 工具名映射** | Aleph 是自己的 agent harness，**不是 Claude Code 客户端**。引入这套是把 Aleph 伪装成 Claude Code，与"一核多端"的自主 agent 定位冲突。openclaw 需要它是因为它代理 Claude Code 流量；Aleph 不需要 |
| **工具循环检测 (`resolveToolLoopDetectionConfig`)** | 违反 **R7 LLM 主权 + R10 笨循环**。检测循环就是替模型决定该不该继续。如果模型陷入循环，那是 prompt 层的问题，应在 prompt 修复，不在 harness 加防御逻辑 |
| **kimi tagged-text tool-call 文本协议、deepseek thinking wrapper** | Aleph 的 kimi-for-coding 走**真 Anthropic 协议**（`presets.rs:83-95` 已确认 base_url + protocol="anthropic"），不需要 wrapper 层。如果将来支持 kimi OpenAI 兼容口的 tagged tool-call，那归 OpenAI protocol 管，不归 Anthropic 协议 |

## 11. References

- 探索报告：本次 brainstorming 的 Aleph + openclaw 双向探索（保留在对话历史，未单独落盘）
- 孤儿过滤器原始修复：S2000 (2026-05-10)，参见 `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/`
- kimi-for-coding stall 事件背景：S2033 (2026-05-10)
- 架构红线 R7 (LLM 主权) / R10 (笨循环编排核心)：`CLAUDE.md` "Architectural Redlines" 章节
- openclaw 参考实现：`/Volumes/TBU4/Github/openclaw/src/agents/anthropic-transport-stream.ts`、`anthropic-payload-policy.ts`
- Aleph 现有协议代码：
  - `src/providers/protocols/anthropic/adapter.rs`
  - `src/providers/protocols/anthropic/sse.rs`
  - `src/providers/protocols/anthropic/proto_impl.rs`
  - `src/providers/anthropic/types.rs`
  - `src/providers/delta.rs`
  - `src/harness/prompt.rs`
  - `src/providers/message.rs`
