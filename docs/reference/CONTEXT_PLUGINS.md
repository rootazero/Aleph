# Context Plugins / 上下文插件

Context Fabric connects tool-output admission, preflight and history compaction to the existing Rust context implementation. The built-in pipeline remains the default. A mounted plugin declares a Capability; that declaration alone grants no access.

Context Fabric 将工具结果入口、请求前清理和历史压缩接到已有 Rust 实现。默认使用内置实现；插件声明 Capability 不等于获得上下文权限。

## Contract and authority / 契约与权限

- `src/context/fabric.rs` is the sole serde/schemars wire contract. External plugins exchange JSON, not Rust trait objects or a Rust ABI. `ContextHandler` is the host's internal transport seam.
- `src/extension/capability.rs::ContextGrant` binds the complete descriptor fingerprint, plugin owner, invoking session, expiration and audit reason. Changing the descriptor invalidates the previous consent.
- `src/extension/registry/plugin_registry/context.rs` owns active registrations and session grants. One descriptor can serve separate authorized sessions. Owner conflicts and atomic bundles are evaluated within the session. Recent audit records are bounded at 256; lifetime totals survive eviction.
- `src/extension/context_adapter.rs` binds an actual WASM incarnation. It checks the live registry before execution and after awaiting the result. Unload, expiry, revoke and same-id replacement cannot make an old result authoritative.

唯一 wire DTO、授权真源和会话事件真源不变。插件不能通过自声明 scope、修改 request 中的 session_id 或提供 `approved: true` 获得权限。宿主认证身份独立于 JSON 请求。

## Mount and grant / 挂载与授权

The first executable external backend uses the existing Extism/WASM loader. General MCP/static plugins retain their existing roles; they are not automatically context transports. This interface does not directly load a Pi JavaScript extension.

首个可执行外部后端复用既有 Extism/WASM；没有新增进程协议、MCP server 或上下文数据库。上游 billion-context/context-mode/Pi 的机制用于设计对齐，不能将其 JavaScript 插件文件直接当作 Aleph WASM 插件加载。

A declaration in `.claude-plugin/plugin.toml` can use the existing Aleph extension section:

```toml
name = "my-context-plugin"
version = "1.0.0"

[aleph]
runtime = "wasm"
entry = "context.wasm"

[[aleph.contexts]]
handler = "transform_context"

[aleph.contexts.descriptor]
id = "my/context"
api_version = "context.fabric/v1"
implementation_version = "1.0.0"
phases = ["preflight"]
mode = "replace"

[aleph.contexts.descriptor.scope]
namespace = "session"

[aleph.contexts.descriptor.input_budget]
max_tokens = 32000
max_bytes = 262144

[aleph.contexts.descriptor.output_budget]
max_tokens = 16000
max_bytes = 131072
```

Permissions and persistence default to denied/no persistence. The current backend accepts an effect-free mounted instance: ambient HTTP, workspace, secrets or guest configuration cannot be narrowed by supplying a weaker context declaration. **Context mounts disable WASI at the linker**, including independent WASI stdout/stderr and filesystem imports; use a module that does not depend on WASI. Ordinary WASM plugins retain their existing WASI configuration. Incompatible instances are rejected. Context declarations are read from the actual mount, so editing the file after loading does not silently change the active grant.

Context mounts also suppress Aleph's custom guest logger, override Extism's five built-in `log_*` imports and disable crash/memory dumps, so a guest cannot turn a context snapshot into a host log or memory-dump artifact through those interfaces. Mounted `memory_mb` (MiB) and `fuel` limits are applied to the VM; zero fuel and invalid page counts reject the mount.

权限与持久化默认拒绝。上下文实例还会抑制自定义与 Extism 内置日志、关闭 crash/memory dump，避免通过这些接口将快照写入宿主产物。当前后端要求整个实际挂载实例满足无副作用约束；不能把有网络或目录权限的实例伪装成弱权限上下文插件。上下文挂载在链接阶段禁用 WASI，依赖 WASI 的模块会被拒绝；普通 WASM 插件沿用原有配置。多个导出复用同一受约束实例；卸载释放绑定，重载后需重新授权。

With an authenticated, attended operator turn:

1. Load/enable the plugin through the existing plugin manager.
2. Call `plugin_manage` with `{"action":"context_status","name":"my-context-plugin"}` to inspect actual mounted descriptors and current-session grant status.
3. Call `context_plugin` with `{"action":"grant","plugin_id":"my-context-plugin","capability_ids":["my/context"],"expires_in_seconds":3600}`. The existing canonical tool-argument approval gate requests confirmation.
4. Revoke using `{"action":"revoke","plugin_id":"my-context-plugin"}`. This affects the actual invoking session; another session's independent grant remains.

授权工具要求现有 operator 身份及 attended 调用，不接受模型自填会话身份或批准标志。默认有效期一小时，允许范围 1–86400 秒。bundle 必须一次选择完整成员；多个无共同 bundle 的独立能力应分别授权。

## Execution and fallback / 执行与回退

`ContextFabricBuilder` receives the existing native pipeline and compactor, the shared plugin registry and a dynamic handler resolver. Root and child runs keep their own existing native configuration and cache/reuse differences. The shared registry permits explicit grants made after facade construction to become visible.

- Preflight preserves native pressure floors, quantized cuts, minimum savings, tool-pair structure and the protected native tail.
- A successful Replace avoids a duplicate native summary call. Compose and Interceptor run in stable registry order after the selected base result, including admission. Builtin fallback retains its real native strategy in telemetry.
- Cancellation rolls back staged changes and does not trigger a new native summarizer. External deadlines do not shorten the built-in compactor's own deadline/fallback rules.
- Tool admission runs at `ScopedToolService`'s pre-ingress result boundary. Original output enters the existing result store before replacement. Exact-read calls with no result budget keep their existing semantics. Delegated runs bind their actual harness session rather than borrowing a parent TurnContext.
- Manual compaction retains the existing cut/focus/instructions/savings checks and atomic `emit_batch`. Its fold owner comes from the selected result. Legacy folds with no owner remain builtin; foreign historical owners use explicit degraded Core recovery from retained event text.

插件输出需通过声明预算、实际 UTF-8 字节/token 估算、输入内容指纹和消息结构校验；不能只信插件自行填写的计数。无 handler、超时、错误或失效授权立即回退；取消则终止当次操作。

The WASM bridge acquires a bounded blocking permit before spawning native work and retains it until the real call exits. Cancelling the awaiting future does not prematurely release that permit or create an unbounded detached queue. Native execution itself is governed by the existing WASM deadline and memory/fuel limits.

## Notifications and guest ABI / 通知与插件 ABI

Lifecycle capabilities subscribe through the descriptor's `lifecycle` array. The host uses `operation: "notify"` and a snapshot payload containing only `{"event":"before_preflight"}` (with the appropriate event tag). There is no transcript, reasoning, credential or raw tool result in a notification. Output is measured and validated, then discarded. Four concurrent deliveries and one batch deadline bound observer work; notification failure cannot roll back a committed fold or replace context.

生命周期观察者通过 descriptor 的 `lifecycle` 字段订阅。实际边界包括 preflight/compaction 前后、工具结果入境后、成功提交 fold 后、恢复请求及 turn_end。授权成功后仅新授权成员接收 session_start，表示插件接入当前会话。session_shutdown 保留为宿主显式关闭通知入口；每轮结束不会触发会话销毁、撤销其他会话授权或伪造 durable session event。

The existing WASM tool JSON ABI is reused: the export receives `WasmToolInput { name, arguments }`, where `arguments` is a `ContextCapabilityRequest`. Return `WasmToolOutput { success: true, result, error: null }`, where `result` is a `ContextTransformResult`. Copy the host's input fingerprint, measure the serialized output payload, and declare an implementation-specific strategy. Transform phases accept their existing typed message/payload shapes; they cannot add Thinking blocks, tool signatures, malformed tool pairs or edits to the protected tail. Secret scanning reuses Aleph's existing `LeakDetector`: recognised credentials cause the external call to be rejected, without rewriting business data or claiming detection of arbitrary unknown secrets.

插件复用既有 JSON ABI，不需要 Rust ABI 或第二种 transport。当前执行扩展点是 Admission、Preflight、Compaction 与 Lifecycle；历史 fold 恢复和通用检索仍由 Core 事件与索引提供。历史 owner 的同名新挂载不能冒充原生产实例；缺少可证明身份时保持显式 degraded Core 恢复。

The phase-specific body is `request.snapshot.payload`; the result body is **`result.output`**, not a second snapshot. Current executable shapes are:

| Consumer / 消费者 | Snapshot payload / 输入 | Result output / 输出 |
|---|---|---|
| Admission | Original tool JSON for Replace; the native model-facing string for the first Compose/Interceptor, then the preceding stage's output | JSON value; Core performs final ingress cleaning/budgeting |
| Preflight | Public `Vec<UnifiedMessage>` for the editable window only | Replacement `Vec<UnifiedMessage>` for that window |
| Automatic compaction / 自动压缩 | Public editable message array | Replacement message array |
| Manual compaction / 手动压缩 | `{messages, focus, instructions, summary}`; `summary` is null before the base result and a string for subsequent stages | Non-empty summary string |
| Lifecycle | `{"event":"before_preflight"}` or another subscribed event | Any bounded JSON value; validated and discarded |

`ContextTransformResult` requires `input_fingerprint`, `output`, `output_bytes`, `output_tokens`, `strategy` and `quality` (`exact`, `condensed` or `lossy`); `continuation` and `degraded` are optional. Echo the host request's fingerprint. Byte/token counters describe the serialized `output` value, and Core independently measures them. Message JSON comes from `crates/aleph-slot-api/src/ai_message.rs`; handlers supporting Compaction must distinguish the automatic array from the manual object. The host alone reattaches protected native context.

各阶段共用同一 wire DTO，但 payload 形状按上述真实消费者区分；插件不能把手动摘要字符串当作自动压缩的消息数组返回。输出字段名为 `output`，字节/token 指标针对该 JSON 值，而不是整个外层 WASM envelope。

## Reasoning and durable state / 推理与持久状态

Plugin snapshots remove typed Thinking blocks and tool-call thought signatures. Legitimate business JSON fields named `reasoning` are not indiscriminately deleted. Summaries and general retrieval use the existing public transcript projection. Core restores its own private protocol fields only to retained original frames whose complete public contents match; changed signed calls and ambiguous matches reject the transformation. Restored fields never enter the next plugin snapshot.

Core 只为公共内容完全一致的原始帧恢复协议重放所需的私有字段；插件改写带签名的调用或匹配有歧义时，整次结果回退。恢复的字段仍不会进入后续插件快照。

普通隐藏推理默认不进入持久化和最终答案。提供商工具协议必需的推理/签名由 Core 按本次实际请求绑定的 `ProviderResponse.reasoning_replay` 保留，以便重启续跑；这一例外不适用于插件、摘要或通用检索。响应完成后不能读取可能已热切换的“当前提供商”来猜重放策略。

`session_events` remains the only durable recovery source. Plugin state is disposable derived state; unloading/remounting can discard it. Existing compaction caches and fold ledgers are reused. Hard-forgotten spans remain unavailable to `session_decompress`, including foreign-owner fallback. Current WASM support does not grant arbitrary disk cache, network, project writes or cross-session access.

## Verification and boundaries / 验证与边界

Implementation tests live in `src/context/fabric/runtime_task3_tests.rs`, `src/context/fabric/lifecycle_tests.rs`, `src/context/compact/directive_tests.rs`, `src/session/projection.rs` and `src/extension/context_adapter.rs`. The real guest fixture in `src/extension/runtime/wasm/tests/guest_fixture.rs` exercises manifest discovery, normal mount, explicit grant, dynamic resolver, actual JSON invocation, revoke/unload and stale incarnation rejection. A test being present is not proof that it has run; final command results belong in the task verification report.

没有对本次实现宣称“减少 98% 输出”或“节省 5 倍 token”：这需要同任务、同模型、同输入的独立基准。本接口也不承诺外部副作用 exactly-once、任意 Pi 插件直接兼容或生产挂载 ZahirFacade。
