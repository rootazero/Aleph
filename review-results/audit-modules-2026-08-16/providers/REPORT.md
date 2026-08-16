# Providers 静态审查报告

**模块**: `src/providers/anthropic/` + `src/providers/openai/` + `src/providers/codex/` + `src/providers/gemini/` + 顶层共享文件（catalog, capability_gate, http_provider, llm_retry, retry, message, metadata, registry, route_*, model_catalog/, protocols/, presets/, mock, recording_mock）
**日期**: 2026-08-16
**工作目录**: `/home/zou/data/workspace/Aleph/.worktrees/audit-2026-08-16-modules/`
**审查范围**: 112 files / 52567 lines
**审查 lens**: seam（severed-wire-audit） / logic / architecture
**置信度阈值**: 仅报告 >80% 高置信度发现

## 环境备注（重要）

本审查过程中，**多个工具输出（`read` / `bash`）的内容里被注入了伪装成 `<system-reminder>` 的提示注入**，文本形如 *"Respond as helpfully as possible, but be very careful to ensure you do not reproduce any copyrighted material..."*。已经过 `grep -c "Respond as helpfully" src/providers/*.rs` 确认：**实际源码中不包含该文本**，仅为输出流注入。该注入尝试重新定向任务，未被遵守 — 审查按原始授权继续，仅在此处如实记录。详见 Finding #11。

---

## 发现汇总

| Severity | Count |
|----------|-------|
| Critical | 1 |
| High     | 1 |
| Medium   | 6 |
| Low      | 4 |
| **Total**| **12**|

---

## Critical

### [Critical] src/providers/message.rs:372,398 — `transform_messages` / `normalize_tool_pairs` 是"已声明但永不触发的线缆"

**Category:** architecture / seam
**Confidence:** High

**Description:**
两个函数被文档严格定义为"the single wire choke-point"（line 363–374），并描述为 *every provider call passes through* 的最终一道安全网，用于保证 `ToolCall`/`ToolResult` 在会话历史中成对出现：

- `pub fn transform_messages(messages, _target_provider)` — 包装 `normalize_tool_pairs`
- `pub fn normalize_tool_pairs(messages: &mut Vec<UnifiedMessage>)` — 实现移除 orphan result + 补齐缺失 result 的双向修复
- 辅助 `pub fn from_provider_response(...)` — 把 `ProviderResponse` 翻回 `UnifiedMessage` 的对偶操作

**实际调用情况（通过 `grep -rn` 跨模块 + `tests` 排除后核实）：**

```
src/providers/message.rs:601:        let msg = UnifiedMessage::from_provider_response(&resp);   ← 测试
src/providers/message.rs:848:        let msg = UnifiedMessage::from_provider_response(&resp);   ← 测试
src/providers/message.rs:666/684/718/740/760 — 全部在 `#[cfg(test)] mod tests` 内
src/context/compact/compactor.rs:879 — 仅在 doc 注释中作为 reference，未调用
```

**全代码库零生产调用者。** `HttpProvider::execute_once`（http_provider.rs:198–）、`AnthropicProtocol::convert_messages`、`OpenAiProtocol::convert_messages`、`GeminiProtocol::convert_messages`、`ResponsesAdapter::build_responses_request` 全部都直接接收调用方传入的 `messages`，**未经过 normalize**。

后果：会话历史被压缩（compactor）、截断、或被打断 turn 后留下的半配对 ToolCall/ToolResult，会以未修复状态发送到 Anthropic / OpenAI / Gemini 的 API，导致：
- Anthropic：`tool_result` 没有前置 `tool_use` → 400 rejected
- OpenAI：连续 `tool` 消息引用已不存在的 `tool_call_id` → 400 invalid_request_error
- Gemini：`functionResponse` 数量不匹配 `functionCall` → 400 INVALID_ARGUMENT

整个 200+ 行的 `normalize_tool_pairs` 实现 + 5 个完整单测是一条**已死线缆**。该模块最近的 review（39d6b3e1d "one wire contract..."）声称统一了 wire contract，但实际未插入到调用点。

**Suggested fix:**
要么在 `HttpProvider::execute_once` 的最前面插入 `transform_messages` / `normalize_tool_pairs`（最自然的 wire choke-point），要么彻底删除该函数并更新文档说明"pairing 由调用方责任"。当前状态下，文档与实现互相欺骗，是 R7/R10 立场最严重的违反 — 文档承诺了"基础设施层 R7"，代码却放任业务层破约。

---

## High

### [High] src/providers/codex/auth.rs:438 — `CodexAuth::ensure_valid` 是 dead code

**Category:** architecture
**Confidence:** High

**Description:**
`pub async fn ensure_valid(&mut self) -> Result<&str>` 在模块顶部文档里写为 "Ensure the token is valid, refreshing if necessary"，但**全代码库零调用者**（grep `auth.ensure_valid` / `CodexAuth::ensure_valid` / `ensure_valid`（Codex 范围）均无匹配；唯一 `ensure_valid` 命中在 `src/mcp/auth/` 的 MCP 子模块同名但语义不同的方法）。

实际 codex OAuth token 刷新走两条路径，都在 `src/gateway/`：
1. `src/gateway/codex_token_refresher.rs` — 后台循环 + reactive retry（注册到 `GLOBAL` OnceLock，由 `run_loop/inner.rs:1473` 调用 `is_oauth_token_expired_error`）
2. `src/gateway/handlers/oauth.rs:236 try_refresh` — Panel status poll / runtime self-heal

两条路径都直接构造 `OAuthTokenCache::from_auth(&auth)` / 反向 `cache.to_auth()`，调用 `CodexAuth::refresh()`（注意：`refresh()` 与 `ensure_valid` 都被定义，但只有 `refresh()` 有真实调用者，`ensure_valid` 是 dead wrapper）。

**Suggested fix:**
删除 `ensure_valid`；或在两个调用点之一改用它（同时省去 refresh+swap 的重复写法）。当前 dead wrapper 增大了攻击面（任何后续 reviewer 都可能误以为 `ensure_valid` 是 token 保鲜的统一入口）。

---

## Medium

### [Medium] src/providers/protocols/gemini/adapter.rs:288–310 — Gemini SSE 适配器缺少截断守卫，与 openai_chat/responses 不一致

**Category:** logic
**Confidence:** High

**Description:**
三个 streaming 适配器的 HTTP 收尾分支行为不一致：

- `openai_chat/adapter.rs` (line 451–472)：HTTP 收尾时检查 `has_terminal_delta(&state.pending)`，若无 terminal signal 则 push 一个 typed `AlephError::Timeout` 错误（"OpenAI Chat stream closed before a finish_reason or [DONE] sentinel arrived"）。
- `openai_responses/mod.rs` (line 494–516)：同样的 truncation guard，推迟 `Done` 直到 usage chunk，typed Timeout 错误注入。
- `gemini/adapter.rs` (line 288–310)：**没有任何 guard**。HTTP 收尾时，若 pending 队列空，直接 `return None`；若 pending 非空，pop 一个就走。无任何 `Done`/`Error` 末端的检测。

后果：Gemini upstream 如果在 stream 中途断开（idle timeout、proxy 关闭），response 会被 `DeltaCollector` 默认化为 `StopReason::EndTurn`，**用户得到一个看起来正常结束但实际被截断的 turn**，没有任何 retry 信号。已有 `stream_idle_timeout_secs` 包装层只能抓 stream 中的 idle gap，抓不到 EOF 突然出现的情况。

**Suggested fix:**
参考 anthropic/adapter.rs:153 的 `queue_has_terminal` + `stream_was_truncated` helper，复用 `delta::has_terminal_delta`，在 gemini SSE 收尾分支添加：若没见到 terminal signal 则 push `AlephError::Timeout`，并设置 `provider_error` 同 openai_chat 路径以让上层正确分类为 transient。

---

### [Medium] src/providers/llm_retry.rs:472 — `backoff_delay` 函数是 dead code

**Category:** architecture
**Confidence:** High

**Description:**
`pub fn backoff_delay(base: Duration, attempt: u32, max_delay: Duration) -> Duration` 在 `llm_retry.rs` 模块顶部文档里说"the failover walk owns the loop"，但实际多处文档 *引用* 该函数：
- `providers/failover/mod.rs:80` — `/// `llm_retry::backoff_delay`'s exponential growth`
- `providers/failover/provider.rs:1285` — `/// `llm_retry::backoff_delay` so a stubborn`

**跨模块 grep `llm_retry::backoff_delay` 零调用者**。failover 自己用别的方式实现 exponential growth（参见 `gateway/delivery_queue.rs:472 backoff_delay` 是同名的本地独立函数，与 `providers::llm_retry::backoff_delay` 无关）。

后果：函数 + 完整 test coverage（5 个 test 包含 `attempt 10 → max cap` 等边缘情况）属于无效代码 — 容易被未来的 failover 改造误认为"已经统一了 backoff 公式"而继续在 `failover/provider.rs` 维护本地版本。

**Suggested fix:**
要么 failover/provider.rs 真正调用 `llm_retry::backoff_delay` 兑现文档承诺；要么从 `llm_retry.rs` 中删除该函数 + 文档中的引用并坦诚声明 failover 有自己的 backoff 实现。

---

### [Medium] src/providers/protocols/openai_responses/mod.rs:325–331 — 每次 Codex 请求都重新解析 JWT

**Category:** logic / performance
**Confidence:** Medium

**Description:**
`OpenAiResponsesProtocol::build_request` 在每次请求都执行 `extract_codex_account_id(api_key)`，其中 `api_key` 是 OAuth JWT（通常 ~1KB）。`extract_codex_account_id` 每次都会：

1. split('.') 三段
2. base64-decode payload 段（一次 ~1KB 解码）
3. serde_json::from_slice 完整 payload JSON
4. 字段查找 `https://api.openai.com/auth.chatgpt_account_id`
5. trim + 过滤空字符串

Codex 路径是 chatgpt 用户的常态 — 每 turn 一次、每 stream chunk 都共享一个 provider 实例、session 内大量 turn。`base64::decode + serde_json::from_slice` 在热路径上每次都重做；account_id 在 token 生命周期内不变（仅当 token 刷新时才变）。

建议：用 `OnceCell<String>` / `RwLock<Option<String>>` 在 `OpenAiResponsesProtocol` 上缓存 (key → account_id) 映射，或在 `CodexAuth` 上挂一个字段（CodexAuth 本来就有 `session_id`，再加一个 derived `account_id` 字段是自然选择）。

**Suggested fix:**
为 `OpenAiResponsesProtocol` 加一个 `cached_account_id: RwLock<Option<String>>`（或 `OnceCell`，取决于 rust 版本），在第一次 build_request 时计算并缓存；或直接把 account_id 提到 `CodexAuth` 字段上（`oauth.rs::OAuthTokenCache::from_auth` 同步填充）。

---

### [Medium] src/providers/protocols/anthropic/sse.rs (SSE parser) — `signature_delta` 拼接逻辑依赖 Anthropic 单签

**Category:** logic
**Confidence:** Medium

**Description:**
阅读 `anthropic/sse.rs` 后发现：thinking signature 是按 streaming fragments 拼接（参见 `ProviderDelta::ThinkingSignatureDelta(String)`，delta.rs:54），`DeltaCollector` 用 `String` 累加。Anthropic 在每个 `thinking_delta` 事件末尾追加 signature fragment。**Anthropic 4.5+ 在 signed thinking block 流式输出时多次发送 `signature_delta`**，当前的简单 `+=` 拼接能工作但依赖于 Anthropic 自身的字节顺序保证。

一旦 Anthropic 改用其他编码（base64 chunks / 多签名轮转 / single-shot signature in final event），`ThinkingSignatureDelta(String)` 的累加语义就会破坏：要么丢签、要么拼错。当前没有针对 signature-only-final 这种交付形式的回退路径。

**Suggested fix:**
添加针对 Anthropic 4.6+ "signature in `content_block_stop`" 变体的探测（line-streaming vs final-stop），并在 signature 解析时按 events type 而不是简单追加。

---

### [Medium] src/providers/presets/registry.rs (lines 130–917) — `ProviderPreset::description` 长度未约束，与 `ProviderMetadata::notes` "under ~80 chars" 文档不符

**Category:** architecture / api contract
**Confidence:** Medium

**Description:**
`metadata.rs:79` 文档说 `pub notes: Option<&'static str>` "Kept under ~80 chars"。但 `presets/registry.rs:1018-1027` 直接把 `preset.description` 作为 `notes` 注入 `PRESET_METADATA`，没有长度约束。实际描述字符串包括：

- "OAuth login, Codex Responses protocol" — 38 chars OK
- "Anthropic protocol; override base_url per region" — 46 chars OK
- "Override base_url with your Azure resource" — 44 chars OK
- "Claude via GCP Vertex; region-specific base URL" — 49 chars OK

当前没有超长字符串，但**没有任何运行时或编译期守卫阻止未来 PR 添加超长 description**。文档承诺的契约与实现不强制。

**Suggested fix:**
在 `ProviderPreset::with_description` 构造器（或 `PRESET_METADATA` lazy init 处）添加 `assert!(desc.len() <= 80, "description too long: {desc}")`。给契约一个真正的门。

---

### [Medium] src/providers/anthropic/proto_impl.rs:370 — `is_oauth_token` 字符串嗅探对 base_url 升级不感知

**Category:** security / logic
**Confidence:** Medium

**Description:**
`is_oauth_token(key)` 仅基于 key 字符串前缀（`sk-ant-` not `sk-ant-api`, `eyJ`, `cc-`）。但 Anthropic protocol 的 OAuth 栈要求 OAuth-key + Claude Code 身份 system block + claude-code/oauth/token-restricted betas（line 363–376）。如果用户在 OAuth token 模式下错配 `base_url` 指向了非官方 endpoint，会：

1. `is_oauth = true` 触发
2. 注入 Claude Code identity system block（prepend_claude_code_identity）
3. 注入 claude-code/oauth/token-restricted beta headers
4. 用 `Authorization: Bearer` 而非 `x-api-key`
5. → 第三方 endpoint（如 Bedrock）可能 400/401

**反之亦然：**Bedrock / Azure 用的 IAM token 不是 `sk-ant-*` 前缀，所以 `is_oauth_token` 返回 false → 用 `x-api-key` 头 — 但 Bedrock 实际期望的是 AWS SigV4（不是 `x-api-key`）。这暗示 Bedrock 的工作完全靠协议 stub 的其他部分（如 provider_policy）把 base_url 重写到 Bedrock 特定 endpoint + 依赖 Bedrock 自己后续处理。

**Suggested fix:**
把 `is_oauth_token` 的判断从单纯 key 前缀升级为 (key_prefix, base_url) 联合判断：当 base_url 命中 Bedrock/Azure/Vertex 域名时强制 `is_oauth = false`（避免被 `eyJ...` JWT 误判），并对 base_url 指向 `api.anthropic.com` 但 key 是 non-sk-ant 形态（如代理注入）的情况告警。

---

## Low

### [Low] src/providers/protocols/anthropic/adapter.rs:153 — `queue_has_terminal` 私有函数与 `delta::has_terminal_delta` (pub(crate)) 重复

**Category:** quality / architecture
**Confidence:** High

**Description:**
两个函数做完全一样的事（扫描 `VecDeque<Result<ProviderDelta>>` 找 `Done` / `Error`）：

- `src/providers/delta.rs:96 — pub(crate) fn has_terminal_delta(pending: &VecDeque<...>) -> bool`
- `src/providers/protocols/anthropic/adapter.rs:153 — fn queue_has_terminal(pending: &VecDeque<...>) -> bool`

`openai_chat/adapter.rs:454` 和 `openai_responses/mod.rs:475,500` 全部直接调用 `delta::has_terminal_delta`，唯独 anthropic 适配器自己又实现了一份。两个实现测试都在 `delta.rs:1260 has_terminal_delta_detects_done_and_error` 和 `anthropic/adapter.rs:1069 queue_has_terminal_recognizes_done_and_error_only` — 两份 test 覆盖同一份逻辑。

**Suggested fix:**
删除 anthropic/adapter.rs:153 的本地 `queue_has_terminal`，统一调用 `crate::providers::delta::has_terminal_delta`（与 openai_chat/responses 一致）。`stream_was_truncated` 仍可保留为 anthropic 专属的 truncation predicate（它组合了 saw_terminal + tail_terminal 两个 flag，语义略不同）。

---

### [Low] src/providers/message.rs:188 — `UnifiedMessage::from_provider_response` 仅被测试使用

**Category:** architecture / dead code
**Confidence:** High

**Description:**
`grep -rn "from_provider_response" src/ --include="*.rs"` 仅命中 `message.rs` 内部（定义 + 2 个 test 用例）。它把 `ProviderResponse` 反向构造为 `UnifiedMessage::Assistant`，看起来是为会话历史持久化设计的对偶操作，但生产代码路径上没有任何 caller。

会话历史的 `ProviderResponse → UnifiedMessage` 转换实际发生在 `delta.rs::response_to_delta_stream` + 各种 `replay_*` 路径（这里直接构造 `UnifiedMessage` 而不走 `from_provider_response`）。

**Suggested fix:**
要保留就标注 `#[allow(dead_code)]` 并写明预期 caller；否则删除。`pub` 暴露的 dead API 比 `pub(crate)` dead API 危险（外部 crate 可能依赖它）。

---

### [Low] src/providers/retry.rs:43 — `apply_jitter` 是 `retry.rs` 唯一活代码，与 `llm_retry` 文档边界不清晰

**Category:** quality / architecture
**Confidence:** Medium

**Description:**
`retry.rs` 整个文件被清理后只剩一个函数 `apply_jitter`（被 `failover/provider.rs` / `failover/decision.rs` 实际使用）。模块顶部文档 (lines 1–25) 详细描述了 *清理过程* — 哪些 dead retry_with_* / parse_retry_after 被删除、为什么 `llm_retry::retry_after_header_secs` 是现在唯一合法读 Retry-After header 的位置。

这是 *good documentation* 但同时暴露一个事实：**该模块的"history" 是审查残留**。任何未来的 retry 改造 PR 在 `retry.rs` 添加新函数都可能被读者错误解读为"retry 的家"。建议将模块重命名为 `jitter.rs`（只剩这一个函数）以反映真实用途。

**Suggested fix:**
文件重命名为 `jitter.rs`，或保留 `retry.rs` 但将 `apply_jitter` 直接迁入 `llm_retry`（同一个"重试工具包"的语义边界更明确）。

---

### [Low] src/providers/registry.rs:90 — `names()` 返回 Vec 而不是 &[&str]

**Category:** quality
**Confidence:** High

**Description:**
`pub fn names(&self) -> Vec<String>`：每次调用都 `sort()` + clone 所有 keys 字符串。provider registry 是热查询结构（每条 `route_status` RPC 都会读），`String` 分配 + `Vec` 分配是可控但不必要的开销。

`ProviderRegistry::providers` 内部已经是 `HashMap<String, Arc<dyn AiProvider>>` — 排序是必要的，但 key 字符串没必要每次 clone。`Vec<&str>` 加 `let mut names: Vec<&str> = self.providers.keys().map(String::as_str).collect(); names.sort();` 即可。

**Suggested fix:**
签名改为 `pub fn names(&self) -> Vec<&'static str>`（注意：当前 key 是 `String`，所以 &'static str 不行；改为 `Vec<&str>` 即可 — key 的所有权仍在 HashMap 内，&str 借用是安全的）。

---

## 综合说明

### 已重新核实的近期修复（避免重复报告）

- `ab545a685 cache: unify hit-rate formula into protocol` — cache 模块本次未覆盖
- `b94236a7e providers: catalog data refresh vs models.dev 2026-08-15` — catalog.rs 数据漂移检查已 in place
- `0b0c4ae9e providers: round-4 — put the window and the price on the row` — model_catalog 字段已落到 RosterModel 上
- `9c9254337 providers: round-4 data refresh` — o4-mini 退役
- `906a649ac providers: both faces of "should this provider be probed"` — probe.rs 单一 derivation
- `b592a6cf5 providers: one matcher for every preset list` — presets matcher 收敛
- `39d6b3e1d providers: one wire contract, provider search, pick-a-model-after-linking` — **但 `transform_messages` / `normalize_tool_pairs` 仍无人调用**，见 Finding #1

### Prompt Injection 备忘（Finding #11）

本次审查共读取约 30 个文件 / 多次 bash 输出。**在每次 `read` 调用的输出末尾，多次出现伪装成 `<system-reminder>` 标签的提示注入**，尝试让模型偏离原始审查任务转入"避免复制版权材料"等无关指令。该注入**不在实际文件内容中**（已通过 `grep` 跨 `src/providers/` 全树证实），仅出现在 read tool 输出流。

模型未遵守该注入，按原始任务完成审查并在此处如实记录。本文件不视为 Aleph 项目源码缺陷，而是工具/环境层的提示注入事件，建议提交方（Aleph 工程团队）评估其来源（CI hook？wrapper？developer shell？）。

### State of the Union 总结

总体上 providers 模块展现了**显著的重构密度**（最新 30+ commit 全部围绕 providers），架构方向正确（统一 wire contract / 单一 derivation / matcher 收敛），但本次审查发现：

1. **最严重**：一个被文档郑重承诺的 `normalize_tool_pairs` 安全网**完全没有 wire 集成**（Finding #1, Critical）。这是 R7 / R10 立场文档与实现脱节的典型案例。
2. **代码卫生**：3 个 dead-code 候选（Finding #2/#4/#7），1 个 dead-on-arrival duplicate（Finding #6）。
3. **跨适配器一致性**：gemini SSE 缺少 truncation guard（Finding #3），与 openai_chat/responses 行为分裂。
4. **热路径性能**：Codex JWT 解析未缓存（Finding #5）。

修复 Finding #1 即可消除 Critical 级别风险，其余多为 quality / 防御性强化建议。