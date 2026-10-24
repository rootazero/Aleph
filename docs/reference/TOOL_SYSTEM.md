# Tool System

> AlephTool trait, built-in tools, and tool development guide

---

## ToolService — production stack

Harness consumers depend on `Arc<dyn ToolService>` exclusively:

```rust
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError>;
    async fn list(&self) -> Vec<ToolDefinition>;
    async fn describe(&self, name: &str) -> Option<ToolDefinition>;
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]>;
}
```

Production harness impl: **`ScopedToolService`** (`src/tools/scoped/`). For agent
turns, the Gateway builds one per request via
`gateway::execution_engine::tool_service_builder::build_request_tool_service`
and supplies it as `FlowRequest::tool_service` so each turn sees an
allow-listed view over the shared `LoopToolRegistry`.

`ScopedToolService` carries the HITL seams natively:
- `with_confirmation(requester)` — gates `requires_confirmation`
  tools through `ApprovalRequester` (wired at boot to
  `ChannelApprovalBridgeAdapter`).
- `with_turn_context(TurnContext)` — scopes the `TURN_CONTEXT` task-local for
  every tool call so HITL tools (`ask_user`, sandbox escalations, channel
  approval) can route back to the originating channel.

Tool authors implement `AlephTool` (typed) or `LoopTool` (untyped via
`RegistryToolAdapter`). The harness fallback `AgentHarnessRunner.tool_service`
is `NullToolService` (`src/tools/null.rs`) — production never reaches it
because Gateway always supplies the per-request override; a `NotFound` from
that service signals upstream wiring regression.

The pre-`ScopedToolService` Phase 2 decorator chain (`facade.rs` /
`dispatch.rs` / `registry.rs` / `middleware/` / `handlers/`, ~2700 lines) was
deleted in 2026-05-20; it was unreachable because agent-turn requests
overrode it. This is not a claim that all RPC dispatch is scoped. See
`docs/superpowers/specs/2026-05-19-hitl-loop-closure-design.md` §8.

---

## `tools.invoke` compatibility boundary / 兼容调用边界

Ordinary `tools.invoke` calls still dispatch through raw `ToolRegistry`, even
when boot supplies the canonical `ToolHandlerRegistry`. This is a **hard-floor
contract, not the unified scoped gate**. It reuses existing predicates to refuse:

1. Dangerous tools, static or registered confirmation declarations, and
   `ExecTier::Auto::asks_for_arguments` risks (no approval card is available).
2. Continuation-driven tools whose post-run hook this RPC cannot drive.
3. Operator-only tools for non-operators.
4. Tools outside the explicit member allowlist for non-operators.
5. Tools outside the selected agent's allowlist (production supplies agents).

`ALEPH_GATEWAY_TOOLS_ALLOW` remains an exact-name, operator-configured override
for the first two floors only; it does not bypass operator/member/agent checks.
PTY interaction stays on the dedicated `pty.*` capability surface; this raw
invocation boundary does not provide a second terminal route.

普通 RPC 没有真实 session、有效 exec tier、合并后的 global/agent/channel
`tool_permissions` 或可路由的 approval context，也不经过 agent-loop hooks / ledger。
因此保留 raw compatibility 的明确下限，不构造第二个 PermissionManager，也不把
临时 session 或默认配置伪装成完整上下文。完整 scoped 接入必须先补齐这些上下文；
当前下限会拒绝已知的审批风险，但不等价于执行全部用户策略。

---

## B4 canonical plugin dispatch / 规范插件派发

B4 (`d53821d7f`) seals the canonical plugin path. `ScopedToolService` captures
`CanonicalEntryLease`; existing permission, visibility, trust, confirmation,
actor, retry, Layer2, and before/after gates lead to an exact `DispatchVerdict`.
`PluginActivationAuthority` derives only from the task-local sealed verdict plus
registry-owned `ReadySnapshot` publication, not from discovery or caller claims.

`PluginCanonicalHandler` and `ExtensionManager::call_plugin_tool_with_authority`
revalidate provider generation, owner generation, mount identity, binding,
descriptor, caller, input, and publication before/after waits and immediately
before guest admission. Stale, replaced, or ownerless compatibility paths fail
closed. Direct mounted-only RPC and the raw `tools.invoke` boundary above retain
separate semantics; Cold tool-only WASM gains no activation proxy at discovery
time. B5 (2026-10-23) adds the explicit enable-driven activation path without
changing the callable authority; B6 now adds the real boot activation pass and full Tool/Plugin lifecycle acceptance; see the B6 addendum below.

B4（`d53821d7f`）封闭 canonical plugin 派发链：`ScopedToolService` 捕获
`CanonicalEntryLease`，既有 permission、visibility、trust、confirmation、actor、
retry、Layer2 与 before/after gates 形成精确 `DispatchVerdict`。
`PluginActivationAuthority` 只从 task-local sealed verdict 与 registry-owned
`ReadySnapshot` publication 派生，发现结果或调用者声明不授予该 authority。

`PluginCanonicalHandler` 与 `ExtensionManager::call_plugin_tool_with_authority`
在等待前后及 guest admission 紧前复检 provider generation、owner generation、
mount identity、binding、descriptor、caller、input 与 publication。陈旧、被替换或
无 owner 的兼容路径 fail-closed。direct mounted-only RPC 与上方 raw `tools.invoke`
边界仍保持独立语义；Cold tool-only WASM 在 discovery 期不获 activation proxy。B5
（2026-10-23）补上显式 enable 驱动的激活路径，且不改变 callable 权威；B6
现在补上真实 boot activation pass 与 Tool/Plugin lifecycle 验收；详见下方 B6
addendum。

## B5 plugin activation lifecycle / 插件激活生命周期

B5 (2026-10-23) wires exactly one thing: the explicit activation of eligible tool-only WASM through the existing plugin-control path. Discovery declares `Cold` and nothing else — no VM, no published tool surface, no registry revision change. The single production entry is `ExtensionManager::set_plugin_enabled(id, true)` → `reconcile_plugin_control_locked` (under `load_guard`) → capture the exact `RuntimeOwner` incarnation → release the guard → `RuntimeOwner::activate_owner` → re-acquire the guard → re-verify current owner (`Arc::ptr_eq`), provider `Ready`, registry row `Loaded`, identity and receipt present, scope still held. Only then does the outcome become `Mounted`; anything else is `Failed` at the Reconcile phase.

The process-global manager Arc installs a `Weak` activation context in `init_extension_manager` only; the retained activation task never strongly holds the manager, and an unattached context fails closed. `ToolHandlerRegistry` remains the sole callable authority, with the existing exact receipt / revision / generation fencing on Ready publication, revoke, and replacement. Disable revokes publication, drains and joins in-flight guest calls, then unloads and disposes the scope; stale owner, provider generation, mount identity, and lease all fail closed. Zahir observers subscribe before the snapshot, treat the snapshot as their anchor, receive live broadcast only, and resync through `Invalidated` on lag — there is no durable registry replay, and `SessionEventStore` remains the only session durability source.

Historical B5 validation: `cargo fmt --all --check` passed; `cargo check -p alephcore -p aleph-slot-api --all-targets` passed; targeted lib tests passed — extension lifecycle 57, tool registry 53, Zahir facade 23, session 352; `cargo test -p alephcore --lib` passed with 21488 passed, 0 failed, 20 ignored; `cargo clippy --workspace --all-targets` passed with warnings (a large `Result` `Err` variant at `src/extension/plugin_ops.rs:877` plus two pre-existing registry test lints). Historical B5 deferred scope: boot auto-activation, MCP lazy semantics, EffectClaim producer, automatic Safe Replay, universal durable scheduler, remaining capability kinds, durable external-effect exactly-once, and `src/harness/`.

B5（2026-10-23）只接一件事：eligible tool-only WASM 的**显式**激活，走既有 plugin control 路径。discovery 只声明 `Cold`：不启动 VM、不发布 tool surface、registry revision 不变。唯一生产入口是 `ExtensionManager::set_plugin_enabled(id, true)` → `reconcile_plugin_control_locked`（在 `load_guard` 内）→ 捕获 exact `RuntimeOwner` incarnation → 释放 guard → `RuntimeOwner::activate_owner` → 重新取回 guard → 复检 current owner（`Arc::ptr_eq`）、provider `Ready`、registry row `Loaded`、identity 与 receipt 齐备、scope 仍持有；全部成立才是 `Mounted`，否则 `Failed`（Reconcile 阶段）。

进程级 manager Arc 只在 `init_extension_manager` 装 `Weak` 激活上下文；retained activation task 不强持 manager；未 attach 一律 fail-closed。`ToolHandlerRegistry` 仍是唯一 callable authority，Ready publication / revoke / replacement 沿用既有 exact receipt / revision / generation fencing。disable ⇒ revoke publication ⇒ drain/join 在途 guest 调用 ⇒ unload/dispose scope；陈旧 owner、provider generation、mount identity、lease 一律 fail-closed。Zahir observer 先 subscribe 再取 snapshot，snapshot 是锚点，broadcast 只是 live notification，lag 时以 `Invalidated` 触发 resync；**没有** durable registry replay，durable recovery 只认 `SessionEventStore`（session durability）。

B5 历史基线验证：`cargo fmt --all --check` 通过；`cargo check -p alephcore -p aleph-slot-api --all-targets` 通过；定向 lib 测试通过——extension lifecycle 57、tool registry 53、Zahir facade 23、session 352；`cargo test -p alephcore --lib` 通过：21488 passed / 0 failed / 20 ignored；`cargo clippy --workspace --all-targets` 通过但带 warnings（`src/extension/plugin_ops.rs:877` 的 large `Result` `Err` 变体，另有两处既有 registry 测试 lint）。deferred：boot 期 auto-activation、MCP lazy semantics、`EffectClaim` producer、automatic Safe Replay、universal durable scheduler、其余 Capability kinds、durable external-effect exactly-once、`src/harness/`。

### B6 Tool/Plugin boot activation / B6 Tool/Plugin 启动激活

B6 keeps B5 discovery Cold and connects enabled, eligible tool-only WASM to the real `ExtensionManager::ensure_loaded` path. Discovery runs under `load_guard` with `loaded` unpublished; the exact Cold owners are captured, activation runs outside that guard, and a final guard-held proof settles each owner before `cache_state.loaded=true`. `boot_guard` serializes the whole pass. Boot and explicit enable share `owner_activation_proof`, which verifies the current owner incarnation, provider `Ready`, row `Loaded`, mount identity, batch receipt, and effect scope. An unattached activation context fails closed and leaves the provider `Failed`; one plugin failure does not abort other plugin activations.

Public `load_all` and `reload` remain discovery/mount primitives and do not implicitly activate Cold owners; they are serialized against the boot pass. Read-side catalog, describe, resolve, `plugins.list`, observer, and doctor paths have no activation entry. D-C therefore keeps Cold observable on those discovery-only paths, after an unattached-context failure, and during explicit control races. B6 proves only the Tool/Plugin slice. The full failure matrix is represented by the existing B5 lifecycle tests plus real WASM boot tests for single and concurrent `ensure_loaded`; at B6 time channel, terminal, memory, MCP, replay, scheduler, EffectClaim, and other Capability kinds remained deferred. B7 (below) wires exactly one Gateway provider slice — Feishu — and nothing else.

B6（Tool/Plugin 启动激活）保持 B5 的 Cold discovery 语义，把 enabled 且 eligible 的 tool-only WASM 接入真实 `ExtensionManager::ensure_loaded`。discovery 在 `load_guard` 内执行但不发布 `loaded`；捕获 exact Cold owner 后释放 guard，在 guard 外激活，再重新取 guard 用 proof settle 每个 owner，最后才写入 `cache_state.loaded=true`。`boot_guard` 串行化整个 pass。boot 与显式 enable 共用 `owner_activation_proof`，统一检查 current owner incarnation、provider `Ready`、row `Loaded`、mount identity、batch receipt 与 effect scope。未 attach activation context 时 fail-closed，provider 进入 `Failed`；单个插件失败不会阻断其他插件。

public `load_all` 与 `reload` 仍是 discovery/mount 原语，不隐式激活 Cold，并与 boot pass 互斥。catalog、describe、resolve、`plugins.list`、observer、doctor 等读侧没有 activation 入口。D-C 因此保留这些 discovery-only 路径、未 attach 失败和显式 control 竞态中的 Cold 可见性。B6 只证明 Tool/Plugin slice；在 B6 时 channel、terminal、memory、MCP、replay、scheduler、EffectClaim 及其他 Capability kinds 均 deferred。B7（见下）只接入 **一条** Gateway provider slice——Feishu——别的什么都没接。

### B7 Feishu provider vertical slice / B7 Feishu provider 垂直切片

B7 (2026-10-24, uncommitted) reuses the frozen provider lifecycle contract for exactly one Gateway provider: the Feishu channel (`src/gateway/interfaces/feishu/**`, plus `src/gateway/inbound_router/executor.rs`). `src/gateway/channel_registry.rs::ChannelRegistry` stays the only Gateway lifecycle authority; the channel-internal `ProviderHandle` is an admission fence over `ProviderState`/generation, and `src/gateway/interfaces/feishu/api_handle.rs` is a generation-fenced derived cache, not an authority. Every outbound action and every outbound HTTP call is admitted against the current generation (the API read guard spans the whole call including the token await, so `drain()` really waits for in-flight calls); inbound webhook and WS forwarding are admitted too (unadmitted webhook ⇒ `503`); the read side never starts or authenticates anything (the old "rebuild config + second `TokenManager`" fallback in `try_create_feishu_emitter` is gone).

Static in-process boundary (D-B): B7 proves authority and runtime-effect revocation, not hot unload or "code not mapped". B7 is **not** a unified Gateway, **not** a live Zahir facade, **not** EffectClaim producers / automatic Safe Replay / universal durable scheduler / durable external-effect exactly-once / MCP lazy, and **not** the remaining Gateway channels / terminal / memory providers. Known residuals: stop only aborts and awaits the top-level webhook accept task; it does not explicitly track or await each already-accepted webhook handler, so this is **not** a complete graceful handler drain. `FeishuApi::admit_inbound()` forms the enqueue quiescence point via the existing calls read guard: after a revoke, newly arriving inbound is fail-closed, while already-admitted sends can still be waited on by `drain()`. Also, `qa/channels` needs a bare-environment Python `websockets` package that is not installed by default. Validation in the B7 implementation round: cargo fmt/test/check/clippy/build passed (only pre-existing clippy warnings) and `qa/channels/run.sh all` returned rc=0 (phase 1 reachability 18/18, phase 2 Lark failure paths 6/6, phase 3 approval 9/9). Full details: `docs/reference/FEATURE_LOCATOR.md` §3.5g and `docs/reference/ARCHITECTURE.md`「B7 Feishu provider vertical slice」。

B7（2026-10-24，未提交）把已冻结的 provider lifecycle contract 复用到**一条** Gateway provider：Feishu channel（`src/gateway/interfaces/feishu/**`，外加 `src/gateway/inbound_router/executor.rs`）。`src/gateway/channel_registry.rs::ChannelRegistry` 仍是唯一的 Gateway lifecycle authority；channel 内部 `ProviderHandle` 只是 `ProviderState`/generation 上的 admission fence；`src/gateway/interfaces/feishu/api_handle.rs` 是 generation-fenced 的 derived cache，不是权威。channel 的每个 outbound 动作与 `FeishuApi` 的每个出站 HTTP 调用都按当前 generation 准入（API read guard 覆盖整段调用、含 token await，故 `drain()` 真能等住在途调用）；入站 webhook 与 WS 转发同样准入（未准入的 webhook 请求 `503`）；读侧不启动也不认证（`try_create_feishu_emitter` 里「重建 config + 第二个 `TokenManager`」的旧 fallback 已删除）。

静态 in-process 边界（D-B）：B7 证明的是 authority 与 runtime effect 的撤销，不是热卸载、也不是「代码页未映射」。B7 **不等于** Gateway 已统一、**不等于** Zahir live facade，也**不等于** EffectClaim producer / automatic Safe Replay / universal durable scheduler / durable external-effect exactly-once / MCP lazy，或其余 Gateway channel / terminal / memory provider。已知残余：stop 只 abort + await 顶层 webhook accept task；没有显式跟踪/等待每个已接受的 webhook handler，因此**不能**宣称完整的 graceful handler drain。`FeishuApi::admit_inbound()` 以现有 calls read guard 形成入队静止点：revoke 之后新入站的 inbound 是 fail-closed 的，而已准入的 send 仍可被 `drain()` 等待。另外，`qa/channels` 依赖默认未安装的裸环境 Python `websockets` 包。B7 实施轮验证：cargo fmt/test/check/clippy/build 通过（clippy 仅既有 warnings），`qa/channels/run.sh all` rc=0（phase 1 reachability 18/18、phase 2 Lark failure paths 6/6、phase 3 approval 9/9）。详见 `docs/reference/FEATURE_LOCATOR.md` §3.5g 与 `docs/reference/ARCHITECTURE.md`「B7 Feishu provider vertical slice」。

---

## Overview

Aleph's tool system provides:
- Type-safe tool definitions with automatic schema generation
- Built-in tools for common operations
- MCP (Model Context Protocol) integration
- Extension tools via WASM/Node.js plugins

**Location**: `src/tools/`, `src/builtin_tools/`

---

## AlephTool Trait

### Static Dispatch (Compile-time)

```rust
pub trait AlephTool: Clone + Send + Sync + 'static {
    /// Tool name (used in LLM tool_use)
    const NAME: &'static str;

    /// Tool description for LLM
    const DESCRIPTION: &'static str;

    /// Argument type (auto JSON Schema via schemars)
    type Args: Serialize + DeserializeOwned + JsonSchema + Send;

    /// Return type
    type Output: Serialize + Send;

    /// Execute the tool
    async fn call(&self, args: Self::Args) -> Result<Self::Output>;

    /// JSON interface (auto-implemented)
    async fn call_json(&self, args: Value) -> Result<Value> {
        let typed_args: Self::Args = serde_json::from_value(args)?;
        let result = self.call(typed_args).await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Get tool definition (auto-implemented)
    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            input_schema: schema_for!(Self::Args),
        }
    }
}
```

### Dynamic Dispatch (Runtime)

```rust
pub trait AlephToolDyn: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call(&self, args: Value) -> BoxFuture<'_, Result<Value>>;
}

// Blanket impl: Any AlephTool is also AlephToolDyn
impl<T: AlephTool> AlephToolDyn for T { ... }
```

---

## Built-in Tools

**Location**: `src/builtin_tools/`

### File Operations

| Tool | Description | Args |
|------|-------------|------|
| `file_read` | Read file content (windowed: line limit **and** token budget, whichever binds; message reports a resumable `offset`) | `path`, `offset?`, `limit?` |
| `file_write` | Write file | `path`, `content` |
| `file_list` | List directory | `path`, `recursive?` |
| `file_delete` | Delete file/dir | `path` |
| `file_mkdir` | Create directory | `path` |
| `file_chmod` | Change permissions | `path`, `mode` |

### Tree Search

**Location**: `src/builtin_tools/file_search/` — `walk.rs` (the one answer to
"which files does this repository consider its own", plus the denylist floor a
byte-reading face must bind), `scan.rs` (pure line matching), `grep.rs`,
`find.rs`.

| Tool | Description | Args |
|------|-------------|------|
| `grep` | Content search across a tree. `.gitignore`-aware, skips `.git` and binaries, capped and pageable. Match lines are 240-char **locators** — follow one with `file_read{offset,limit}` | `pattern`, `path?`, `glob?`, `ignore_case?`, `literal?`, `context?`, `limit?`, `offset?`, `files_only?`, `no_ignore?` |
| `find` | File discovery by glob. Same walk, paths only, sorted and pageable | `pattern`, `path?`, `limit?`, `offset?`, `no_ignore?` |

Three things about this pair are load-bearing:

- **`pattern` is a regex, so several terms are ONE call** (`foo|bar|baz`).
  There is deliberately no `multi_grep`; a second verb would buy per-pattern
  grouping at the price of a second registration surface, ~700 B of description
  billed on every request, and one action answering to two names.
- **They replace `bash`.** A `grep -r` does not read `.gitignore`, so one
  recursive run pours every hit under `node_modules/`, `target/` and `dist/`
  into the context window. `bash`'s own DESCRIPTION says so, and
  `tools/scoped/search_steer.rs` repeats it at call time as a non-blocking
  `<system-reminder>` when a shell command duplicates one of them. `rg` and
  `fd` are never steered — they are the sanctioned shell fallback.
- **`file_ops{operation:"search"}` is a different face, not a duplicate.** That
  one is file *management* (returns size/type/extension, feeds
  `organize`/`batch_move`/`stats` over any directory); `find` is code
  *navigation*. What they must not fork on — "which files exist" — is answered
  once, by `walk`.

### Code Execution

| Tool | Description | Args |
|------|-------------|------|
| `bash_exec` | Run bash command | `command`, `timeout?` |
| `code_exec` | Execute code snippet | `language`, `code` |
| `python_exec` | Run Python | `code`, `requirements?` |

### Web & Search

| Tool | Description | Args |
|------|-------------|------|
| `web_fetch` | Fetch a page and extract its text; with `prompt`, a page too large to return whole comes back as the sections matching it plus a handle to the rest (fetch by intent, see *Result budgets & recovery* below) | `url`, `extract_mode?`, `prompt?` |
| `search` | Search the web | `query` / `queries`, `limit?`, `recency?`, `domains?`, `exclude_domains?`, `full_content?`, `providers?` |

### Browser — 26 tools, two engines, one driver

Every browser tool routes through `ProfileManager::get_backend` (`src/browser/manager.rs`), which
maps a profile's `driver` to a `BrowserBackend`. Since 2026-09-06 the default driver is `cdp`
(`src/browser/cdp_backend/`), which speaks CDP directly through the Aleph-owned client in
`crates/aleph-cdp` and serves **both** engines:

| axis | wire values | what it decides |
|---|---|---|
| `driver` | `cdp` (default) · `managed` · `existing_session` | who drives the browser |
| `engine` | `obscura` (default, `[general.browser] default_engine`) · `chromium` | which process runs |

⚠️ Those are the **exact** strings serde reads and writes (`BrowserDriver::as_wire`,
`Engine::as_str`). `managed` is the playwright-cli driver and `existing_session` the Chrome
DevTools MCP one; the round that introduced this table nearly published them as `managed_cli` and
`chrome_mcp`, which parse as nothing.

The two axes are orthogonal, but only one of them is free. `driver = "managed"` or
`"existing_session"` **is** `engine = "chromium"` — `resolved_engine` does not consult
`default_engine` under those drivers at all, because a profile that names a legacy driver has
already named its engine and a global default must not drag it somewhere it cannot run. An explicit
`engine` on a legacy driver does not silently win either: it is a contradiction, and
`validate_engine_driver` refuses it **by name at load**, not as a silent fallback.

**The model's default observation is not a screenshot.** `browser_snapshot` returns a page-state
tree built by `src/browser/page_state/` from the DOM plus geometry — one builder, both engines —
carrying role, accessible name, states, rect and a generation-stamped `ref` per node. A `ref` dies
two ways and says which (`StaleRef{reason}`, `StaleReason::{Navigated, NodeGone, Unknown}`): the
**document** layer retires the table when the main frame's `loaderId` turns over, and the **node**
layer asks the resolved object `this.isConnected`. Neither covers the other's case, and on the
default engine that is load-bearing rather than belt-and-braces — see
[FEATURE_LOCATOR §3.12](FEATURE_LOCATOR.md) 第八轮 ⑯b. Every DOM-sourced
string in that tree is printed through one helper, `render::quote`, so a page cannot forge a
`[ref=…]` token into the model's view by putting one in a `placeholder`.

**Three tool faces carry the engine:**

| Tool | Argument | Meaning |
|---|---|---|
| `browser_open` | `engine?` | one-shot: which engine to *start* for a cold profile. Refused, by name, if that profile already runs the other one — its cookies live in that process. |
| `browser_session` | `action:"capabilities"` | the per-engine support table, as a tool result: rendered text plus a JSON form, each row stamped with the engine version it was measured on. |
| `browser_session` | `action:"switch_engine", engine, migrate?` | move a live profile to the other engine, carrying cookies, tab URLs + scroll, and per-origin localStorage. Returns the new page tree; every earlier `ref` is dead. |

Engine capabilities are a constant table (`src/browser/engine/capability.rs`) of exactly five rows —
`js_dialogs`, `drag`, `file_upload`, `pdf`, `insert_text` — and **every row names the `browser_*`
verb that dispatches it**. That rule is what keeps the table a menu rather than a brochure: a row
with no verb behind it advertises a capability no tool can reach (判据 §9), which is why `touch`,
`screencast`, `network_interception` and `multi_connection` are not rows.

The table reaches the model **as a tool result, not as description bytes** — `browser_session`'s
`DESCRIPTION` carries one sentence pointing at `action:"capabilities"`, and nothing in this tree
generates description text at runtime (`AlephTool::DESCRIPTION` is a `const`). The same table raises
`UnsupportedByEngine{engine, verb}` — the verb is named, so the model knows what it would be
switching for. `qa/browser_dual/run.sh caps` probes every row against the real binaries **by
effect**, which is that table's expiry check rather than a second copy of it.

#### Browser live resources: producer discovery · 生产者发现
<!-- browser-live-docs:begin -->
`browser_open` / `browser_navigate` remain the active agent tools (not removed); they feed `operation_publish` (`src/builtin_tools/browser_tools/operation_publish.rs`) and the explicit `BrowserLiveProducer` seam (`src/builtin_tools/browser_tools/present.rs`), which publishes `PreviewResource` records under the turn context and announces them with the gateway events `browser.live.resource` / `browser.live.closed`. A producer can never publish `Safe`: `Safe` is clamped to `Pending`, `Pending` is kept, `Refused` fails closed (close + revoke). `Safe` is set only by `browser.viewer.resource.decide` (explicit user decision, revision CAS). Contract and status: [BROWSER_LIVE_VIEWER.md](BROWSER_LIVE_VIEWER.md).

browser 工具产出「资源」而非 URL；production 批准路径只有用户决策 CAS。

- **共同边界 / Shared boundary**：Gateway 独占 CDP（Gateway owns CDP）；Panel/TUI 仅 I/O only 呈现；仅显式合法 `[gateway] browser_viewer_origin` 才 opt-in，默认（None / 非法）deny-only；producer 的 `Safe` 被钳为 `Pending`，`Safe` 只来自用户经 `browser.viewer.resource.decide` 的 revision CAS；绑定走精确 binding / owner / generation；#27 TUI paired/browser 已接线，Windows Credential Manager standalone probe 1/1 已通过；macOS/Linux provider 与真实 pairing/browser E2E 仍 blocked；`NetworkContextGrant` / proxy 未触碰；**not production ready**。
Gateway owns CDP; Panel/TUI are I/O only; opt-in only for an explicit valid `[gateway] browser_viewer_origin`, default deny-only; producer `Safe` is clamped to `Pending` and `Safe` comes only from the user's decision CAS; exact binding/owner/generation; #27 TUI paired/browser wiring is implemented; the Windows Credential Manager standalone probe passed 1/1; macOS/Linux providers and real pairing/browser E2E remain blocked; network grants untouched; not production ready.
<!-- browser-live-docs:end -->

### Generation

| Tool | Description | Args |
|------|-------------|------|
| `image_generate` | Generate image | `prompt`, `provider?`, `size?` |
| `pdf_generate` | Generate PDF | `content`, `template?` |

### Perception

| Tool | Description | Args |
|------|-------------|------|
| `snapshot_capture` | Capture AX tree + optional OCR | `target`, `region?`, `include_ax?`, `include_vision?`, `include_image?` |

### Session Tools

| Tool | Description | Args |
|------|-------------|------|
| `sessions_spawn` | Spawn sub-agent | `model?`, `thinking?`, `prompt` |
| `sessions_send` | Send to sub-agent | `session_key`, `message` |
| `sessions_list` | List sub-agents | - |

### Embedded Terminal

终端是纯 PTY 能力面。`pty.*` RPC 负责 session 生命周期、输入、resize、attach 和 screen；不存在 runtime agent 状态表、进程探测、`terminal_sessions_*` 观察工具或 agent panel。

客户端通过 `pty.list`、`pty.screen`、`pty.exit` 维护终端 tabs 和画面。PTY 输入仍受 `pty` ownership 与连接权限约束；写入路径不自动获得 exec/sandbox 命令策略权限。详见 [TERMINAL_RUNTIME.md](TERMINAL_RUNTIME.md)。 / The embedded terminal is a pure PTY surface. Its RPCs own session lifecycle, input, resize, attach, and screen delivery; runtime agent detection, observation tools, and the agent panel are removed.

### Memory Tools

| Tool | Description | Args |
|------|-------------|------|
| `memory_store` | Store fact | `content`, `tags?` |
| `memory_search` | Search memory with hybrid retrieval | `query`, `max_results?` |
| `memory_forget` | Delete fact | `fact_id` |

#### memory_search Tool

**Purpose**: Search personal memory for relevant facts and conversation history with intelligent redundancy elimination.

**Features**:
- Hybrid retrieval: Searches both compressed facts and raw transcripts
- Post-retrieval arbitration: Eliminates redundancy between facts and transcripts
- Priority-based selection: Higher similarity scores selected first
- Token budget management: Fits results within context window
- Importance scoring: Can filter low-value content (when integrated with ValueEstimator)

**Arguments**:
```rust
pub struct MemorySearchArgs {
    /// Search query (natural language)
    pub query: String,
    /// Maximum results to return (default: 10)
    pub max_results: usize,
}
```

**Output**:
```rust
pub struct MemorySearchOutput {
    /// Compressed facts (deduplicated)
    pub facts: Vec<FactResult>,
    /// Raw conversation transcripts (deduplicated)
    pub transcripts: Vec<TranscriptResult>,
    /// Original query
    pub query: String,
    /// Tokens saved through deduplication
    pub tokens_saved: usize,
}
```

**Example Usage**:
```json
{
  "tool": "memory_search",
  "args": {
    "query": "What are my coding preferences?",
    "max_results": 10
  }
}
```

**Architecture**:
```
memory_search(query)
  → Hybrid search (see [Retrieval](../memory/RETRIEVAL.md))
    → Facts + raw memories fallback
  → ContextComptroller.arbitrate(results, budget)
    → Detect redundancy via cosine similarity (threshold: 0.95)
    → Remove redundant transcripts when facts exist
    → Sort by similarity score (descending)
    → Trim to fit token budget
  → Return deduplicated results
```

**Configuration**:
- Similarity threshold: 0.95 (configurable via ComptrollerConfig)
- Token estimation: 4 chars per token
- Retention mode: Hybrid (facts prioritized, redundant transcripts removed)
- Max facts: 10 (configurable via FactRetrievalConfig)
- Max raw fallback: 10 (configurable via FactRetrievalConfig)

#### Retrieval tools — three stores, no overlap

Aleph exposes three complementary BM25/hybrid retrieval tools. Each searches a
**different store**, so their descriptions stay sharp to avoid tool-choice confusion:

| Tool | Store | Use when |
|------|-------|----------|
| `ctx_search` | Offloaded tool **output** (FTS5 content index) | A tool result shows `[Full output persisted: … Indexed N sections …]` and you need only the relevant slice instead of re-reading the whole build log / big grep / web fetch. |
| `recall_events` | **This session's** event timeline (`session_events`, BM25) | An earlier tool action, result, or error in *this* session was dropped from context by compaction and you need to recover what already happened. |
| `session_search` | **Past conversations across sessions** (memory summaries + transcripts) | You need facts or decisions from *other* sessions (long-term memory), not the current run. |

Rule of thumb: `ctx_search` = "what did that tool print?", `recall_events` =
"what did I already do this session?", `session_search` = "what happened in past sessions?".

### Meta Tools

| Tool | Description | Args |
|------|-------------|------|
| `skill_read` | Read skill definition | `skill_name` |
| `ask_user` | Ask 1–4 questions, park until answered | `question` + `choices?`, **or** `questions[]` (`id?`/`header?`/`question`/`choices?`/`multi_select?`/`secret?`) |
| `canvas_show` | Display in canvas | `content`, `type` |

---

## Tool Definition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Schema,  // JSON Schema
}

// Sent to LLM as:
{
  "type": "function",
  "function": {
    "name": "file_read",
    "description": "Read content from a file",
    "parameters": {
      "type": "object",
      "properties": {
        "path": { "type": "string", "description": "File path" },
        "encoding": { "type": "string", "default": "utf-8" }
      },
      "required": ["path"]
    }
  }
}
```

---

## Tool Server

**Location**: `src/tools/server.rs`

The Tool Server manages tool execution:

```rust
pub struct ToolServer {
    builtin_tools: HashMap<String, Arc<dyn AlephToolDyn>>,
    mcp_clients: HashMap<String, McpClient>,
    extension_tools: HashMap<String, ExtensionTool>,
}

impl ToolServer {
    pub async fn execute(
        &self,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolResult> {
        // 1. Check builtin tools
        if let Some(tool) = self.builtin_tools.get(tool_name) {
            return tool.call(args).await;
        }

        // 2. Check MCP tools
        if let Some((server, tool)) = self.find_mcp_tool(tool_name) {
            return self.mcp_clients[server].call(tool, args).await;
        }

        // 3. Check extension tools
        if let Some(ext_tool) = self.extension_tools.get(tool_name) {
            return ext_tool.call(args).await;
        }

        Err(Error::ToolNotFound(tool_name))
    }
}
```

---

## MCP Integration

**Location**: `src/mcp/`

Model Context Protocol for external tool servers:

```rust
pub struct McpClient {
    transport: Transport,  // Stdio, WebSocket, or HTTP
    tools: Vec<ToolDefinition>,
}

impl McpClient {
    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>>;
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
}
```

### MCP Configuration

```json5
{
  "mcp": {
    "servers": [
      {
        "name": "filesystem",
        "command": "npx",
        "args": ["-y", "@anthropic/mcp-server-filesystem"],
        "env": { "HOME": "/Users/user" }
      }
    ]
  }
}
```

---

## Tool Development Guide

### Step 1: Define Arguments

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MyToolArgs {
    /// Description shown to LLM
    pub required_field: String,

    /// Optional with default
    #[serde(default)]
    pub optional_field: Option<String>,
}
```

### Step 2: Implement Tool

```rust
use crate::tools::AlephTool;

#[derive(Clone)]
pub struct MyTool {
    // Any shared state
}

impl AlephTool for MyTool {
    const NAME: &'static str = "my_tool";
    const DESCRIPTION: &'static str = "Does something useful";

    type Args = MyToolArgs;
    type Output = String;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Implementation
        Ok(format!("Processed: {}", args.required_field))
    }
}
```

### Step 3: Register Tool

Registration is **not one place**, and the gap between "the model is told about
this tool" and "a call reaches it" has shipped as a bug four times
(`select_model`, `doctor`, `config_audit`, `plugin_manage` — the last one found
by a real-machine fixture, not by the 16k-test suite, because every in-process
test asked a *registration* surface whether the tool existed and every one of
them correctly said yes).

The sites, in the order a new tool needs them:

| # | File | What it buys |
|---|------|--------------|
| 1 | `executor/builtin_registry/definitions.rs` — `BUILTIN_TOOL_DEFINITIONS` | catalog row; the description starts being billed on every request |
| 2 | `definitions.rs` — `create_tool_boxed` | construction for `AlephToolServer` |
| 3 | `registry/tool_registry_impl.rs` — `execute_tool` match arm | **dispatch**; without it every call answers `Unknown tool` |
| 4 | `registry/struct_def.rs` | the instance field |
| 5 | `builder/constructor/mod.rs` | construction + struct init (pass the shared `ToolContext` handle if the tool resolves paths) |
| 6 | `builder/core_tools.rs` — `reg(...)` | registry-map row (`agent_init` completes the model's list from here) |
| 7 | `builtin_registry/groups.rs` | Panel display category |
| 8 | `config/types/tools.rs` — `default_core_tools()` | schema-resident vs collapsed behind a `get_tool_schema` round-trip |
| 9 | `tools/adapters/registry_adapter.rs` — `READ_ONLY_TOOLS` | read-only ⇒ idempotent ⇒ auto-retry ⇒ callable under the `Ask` / `Plan` tiers |
| 10 | `tools/fallback_registry.rs` — `ToolFamily::from_name` | which alternatives are suggested when it fails |

1–3 and 6 are enforced: `builtin_registry/dispatchable.rs` recovers both the
advertised set and the dispatchable set **from the source text** (not from a
list someone maintains) and fails naming the tool that is in one and not the
other. 4–5 are compile errors. 7–10 are silent if missed — a Panel row that
renders as a generic gear, a tool that costs a round-trip to call, a pure read
that the `Ask` tier stops to ask about.

---

## Tool Filtering

**Location**: `src/thinker/tool_filter.rs`

Control which tools are available:

```rust
pub struct ToolFilter {
    /// Whitelist (if set, only these tools available)
    pub allowed: Option<HashSet<String>>,

    /// Blacklist (always excluded)
    pub blocked: HashSet<String>,

    /// Require confirmation for these
    pub require_confirmation: HashSet<String>,
}
```

### Configuration

```json5
{
  "tools": {
    "allowed": ["file_read", "web_fetch", "memory_*"],
    "blocked": ["bash_exec"],
    "requireConfirmation": ["file_write", "file_delete"]
  }
}
```

---

## Tool Result

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}
```

### Ingress: 结果在进入上下文之前要经过什么

唯一入口 `src/tool_output/ingress.rs::clean_for_ingress`，由
`tools/scoped/dispatch.rs::apply_layer_two` 调用一次：

```text
value (serde_json::Value —— 文本字段仍带真换行)
  │
  ├─ hoist_inline_images        图片 → 带外 vision 通道（含 MCP content 块）
  ├─ harvest_outbound_media     _media → 产物库
  ├─ 1. 每工具压缩              无条件，字段级
  ├─ 2. 内容类型清洗            仅在已超预算时，字段级
  └─ 3. 扁平化 → apply_result_budget（persist 原文 / 内联信号 / 截断）
```

**给写新工具或新 adapter 的人的四条硬约束**（每一条都对应一次真实的静默失效）：

1. **不要在交给 dispatcher 之前把自己的结果 `to_string`。** `Value::to_string()`
   转义每一个 `\n` 并把整个结果压成一行，而这条链上的**每一个**清洗器都按行路由——
   log / search / diff / json 四个缩减器、错误蒸馏器、以及 `compressor.rs` 的三个
   DevTools 策略。MCP adapter 曾这么做，结果那四条路全是死的，而且蒸馏器还会拿 JSON
   信封的前 400 字符冒充"关键错误"替换整个结果。
2. **图片要留在结构里、留在 `hoist_inline_images` 认得的位置。**
   支持 `{image_base64, format}`（顶层或 `data` 下）与 MCP 的
   `content[].{type:"image", data, mimeType}`。字符串里的 base64 找不回来，只会被
   当文本计费然后截断成解不开的片段。
3. **要重写文本字段就走 `tool_output::fence::rewrite_interior`**，别整体替换——
   不可信内容围栏是结构不是内容，见 [SECURITY.md](SECURITY.md#content-sanitization)。
4. **任何会丢内容的 stage 都欠调用方一份原文。** `clean_for_ingress` 的
   `full_original` 是 `apply_result_budget` 落盘的那一份，它决定被丢掉的行还能不能
   靠 `ctx_search` / `read_file` 挖回来。压缩器一度不设它，理由是"压缩产物按构造小于
   预算所以落盘走不到"——真实 `take_snapshot` 压缩后 13 585 token / 8 000 预算，当场
   走到，落盘的是压缩正文，被丢掉的 443 个节点就此不可恢复。**只有"从不删内容"的
   stage 才可以传 `None`**（剥 ANSI 转义是唯一的例子）。

**真机 QA（2026-08-04）**：隔离 `ALEPH_HOME` + 真实 `chrome-devtools-mcp` + 记录请求体的
mock LLM，实测确认 MCP 截图作为可解码 PNG 到达模型、三个工具结果围栏开闭配对、快照压缩
保住全部 660 个控件、落盘的是未压缩原文。**未覆盖**：`metadata.images` 只有 Anthropic
协议消费，OpenAI 系（`openai_chat` / `responses`）的 ToolResult 臂丢弃图片（API 约束，见
FEATURE_LOCATOR §3.14「已定位·未做」）。

详见 [FEATURE_LOCATOR §3.14](FEATURE_LOCATOR.md)。

### Result budgets & recovery · 预算与找回

> Numbers live in the constants named here; this section does not restate them (§1).
> 数值只住在下面点名的常量里，本节不抄数字。

**Three budgets, one source each · 三个预算，各有唯一来源**

| Budget | Owner | What it bounds |
|---|---|---|
| Per result (Layer 2) | `tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS`, or a tool's `AlephTool::MAX_RESULT_TOKENS` | One result. Over it, the original is offloaded (persisted + indexed) and the model gets the recovery footer. **No builtin declares `MAX_RESULT_TOKENS`** — a larger budget belongs only on output that *is* the window the model chose; a merely large result (a build log, a page) is better offloaded and searched. The registry census `result_budgets_come_from_declarations_and_default_everywhere` pins it. |
| Per turn (Layer 3) | `tools::turn_budget::DEFAULT_MAX_TURN_TOKENS` | All results of one Think→Act turn. Past it, `TurnResultBudget::record` spills **the result it was just handed** (the only one still rewritable) — unless it carries its own call's persist marker (`result_processing::carries_own_persisted_marker`, the derivation Layer 2 uses; a marker line quoted in the payload does not count) or is no larger than the offload floor. It credits what the spill removes at least (everything above the floor or `turn_budget::spill_residue_tokens`), and `result_processing::spill_replacement` leaves no more than that — cutting, with a note, when the write cannot happen. The floor is measured on the store the spill writes through: `runner_impl` and `subagent_spawner` hand the budget the same session-scoped `Arc` they put in `HarnessDeps::result_store` (`TurnResultBudget::with_result_store`). The harness spill (`act.rs`) replaces the result with exactly that `spill_replacement`. |
| Read window | `result_processing::read_backstop_tokens()` (= `MAX_RESULT_BUDGET_TOKENS` clamped by the window ceiling) | The read family (`result_processing::is_read_family`: the builtin `file_read`). **Not offloaded at Layer 2** — it resolves no budget there and is truncated at the read window instead, because an offloaded read hands back a marker to read again. **Layer 3 does not exempt it**: the per-turn budget is the only per-turn bound on reads, and a spilled read is persisted and indexed (a re-read, not a loss). |

Neither layer swaps a result for a footer at least as large as the result: both measure one floor,
`result_processing::offload_floor_tokens` — the call's marker on the store the write goes through, plus the
longest hint line the footer can carry (a preview grown by its scrub can exceed that bound). 两层用同一个地板。

An **empty recovery set** (neither `ctx_search` nor `file_read` callable) means no offload at all — a path the
model has no tool to open is a dead handle. `offload` checks `RecoveryTools::is_empty` for every writer, and
also declines after the write when the footer would name no callable reader (only `ctx_search`, and the index
did not take the blob); the writer cuts head and tail instead and says the rest was not saved.

Context management is on by default (a missing `[context_budget]` is on; `enabled = false` opts out), so a default
install runs these ingress gates **and** a per-run `ContextBudget` behind them (preflight pruning, calibration,
`context.breakdown` `messages`). With `enabled = false` only the two ingress gates remain and no window ceiling is
installed. 默认开启；显式关掉时只剩入口两道闸。

Small-window models (context management on): boot installs a ceiling from `turn_budget::budget_for_window` (window fractions, clamped to
`MAX_RESULT_BUDGET_TOKENS` / `DEFAULT_MAX_TURN_TOKENS`); the per-result ceiling caps **every** per-result budget,
the read window included. `tool_output::scale_to_budget` sizes digests/profiles against
`tool_output::KNOB_REFERENCE_BUDGET_TOKENS` (the budget the knob defaults were sized for), so a smaller budget gets a
proportionately smaller artifact.

**The recovery footer · 找回句柄** — every writer of an offloaded original goes through
`result_processing::recovery_footer_for(store, call_id, tool, full, threshold, RecoveryTools)`: it stores
`tool_output::render::line_preserving(full)` (a one-line JSON envelope becomes real lines, so the index gets real
sections and `file_read` real lines), indexes it for `ctx_search`, and names **only the retrieval tools the model can
call**. Who knows that set:

| Writer | Where the `RecoveryTools` come from |
|---|---|
| Dispatcher, Layer 2 (`scoped/dispatch.rs::apply_layer_two`) | `ScopedToolService::recovery_tools()` — registered + allowed + not hidden — intersected with any outer scope (`dispatch_recovery_tools()`) |
| Harness Layer-3 spill | `ToolService::recovery_tools()` — decorators forward it; `AllowlistToolService` narrows it to the agent's allow set |
| A tool offloading its own output (browser offload, `web_fetch` fetch by intent) | `result_processing::dispatch_recovery_tools()` — a task-local scoped around the tool's execution by the dispatcher and by `AllowlistToolService` (`with_recovery_tools`, which intersects with an outer scope: an inner dispatcher cannot widen a wrapper's narrowing); `RecoveryTools::ALL` only outside a dispatch, where nothing about the gates is known |

A fenced source (external content) gets no "First sections:" preview in its footer — the preview would be untrusted
text outside the fence. `ctx_search` returns whole sections, each scrubbed and fenced on its own
(`ContentSource::OffloadedToolOutput { tool }`), fitted to its budget by measurement.

**`web_fetch` sizing · 定尺** — the content cap is derived from the budget Layer 2 will hold the result to
(`chars_for_result_token_budget(resolve_result_budget(web_fetch, …))`); the policy's `max_content_length` can only
lower it. `fit_to_budget` measures the whole result the way Layer 2 does (flattened JSON: fence, URL, title, focus
marker) and shrinks the cap proportionally, at most `FIT_ATTEMPTS` times, so the result arrives inline instead of
being offloaded again. A result still over after those attempts is bounded by Layer 2.

**Fetch by intent · 按意图取页** (`builtin_tools/web_fetch/intent.rs`) — `web_fetch` with a `prompt` on a page
longer than that cap: the page is extracted whole (bounded by `WebFetchTool::INTENT_EXTRACT_MAX_CHARS`);
`index_page` offloads and indexes it **once** through `recovery_footer_for` into the same store `ctx_search`
reads; `sections_for_prompt` BM25-searches **this page's own sections** with the prompt
(`ContentIndex::search_source` — the source restriction is in the query, not a filter over the session's pooled top
hits) and returns them in page order within the cap, fenced, followed by the footer; refitting re-selects
sections only. No match → the page head, and the note says so; a page the index did not take (or an index that
cannot answer) → the page head, and the note says it could not be searched, not that nothing matched. Layer 2
recognises a result carrying the marker of its own call's blob (by the blob's file name, as written or
JSON-escaped) and never persists it again — the blob is named by the call id, so a second write would replace
the page with the sections result. Ingress does not rewrite such a result (a field cut could drop the marker
line); over budget, Layer 2 keeps the footer (marker line and hint) whole and cuts everything else around it,
text after the footer included.

**`browser_exec` steps · 过程内的多次卸载** — each snapshot step a procedure cuts is offloaded under its own key
(`browser_exec-step-N`, for both the blob file and the index source), so a second cut step does not replace the
first step's blob or its sections. Pure retrieval (R7), no second index; prompted
fetches bypass the URL cache (it holds capped pages).

Why and how this round changed them → [FEATURE_LOCATOR §2.21](FEATURE_LOCATOR.md).

---

## Memory System Components (Phase 2)

### TranscriptIndexer

**Purpose**: Near-realtime indexing of conversation transcripts with chunking support.

**Features**:
- Sliding window chunking for long conversations
- Configurable chunk size and overlap
- Sentence-boundary aware splitting
- Token estimation (4 chars per token)

**Configuration**:
```rust
pub struct TranscriptIndexerConfig {
    pub max_tokens_per_chunk: usize,  // Default: 400
    pub overlap_tokens: usize,         // Default: 80
    pub enable_chunking: bool,         // Default: true
}
```

**Usage**:
```rust
let indexer = TranscriptIndexer::new(database);
let chunks = indexer.chunk_text(&long_conversation);
```

No embedder: the indexer writes Markdown-backed raw records with metadata in `raw_memory_index`; recall over them
is the substring transcripts leg of `memory_search`.

### ValueEstimator

**Purpose**: Importance scoring for memory entries to filter low-value content.

**Features**:
- Signal-based detection (8 signal types)
- Score range: 0.0 (low value) to 1.0 (high value)
- Length bonus for longer conversations
- Batch estimation support

**Signals**:
- **Positive**: UserPreference (+0.25), Decision (+0.20), PersonalInfo (+0.30), FactualInfo (+0.15)
- **Negative**: Greeting (-0.30), SmallTalk (-0.20)
- **Neutral**: Question, Answer (combined +0.10)

**Usage**:
```rust
let estimator = ValueEstimator::new();
let score = estimator.estimate(&memory_entry).await?;

if score > 0.7 {
    // High-value content, prioritize for compression
}
```

### ContextComptroller

**Purpose**: Post-retrieval arbitration to eliminate redundancy and manage token budget.

**Features**:
- Redundancy detection via cosine similarity (threshold: 0.95)
- Priority-based selection (similarity score descending)
- Token budget enforcement
- Three retention modes: PreferTranscript, PreferFact, Hybrid

**Configuration**:
```rust
pub struct ComptrollerConfig {
    pub similarity_threshold: f32,     // Default: 0.95
    pub retention_mode: RetentionMode, // Default: Hybrid
}
```

**Usage**:
```rust
let comptroller = ContextComptroller::new(config);
let budget = TokenBudget::new(10000);
let arbitrated = comptroller.arbitrate(retrieval_result, budget);

// arbitrated.facts: Deduplicated facts
// arbitrated.raw_markdown: Deduplicated transcripts
// arbitrated.tokens_saved: Tokens saved through deduplication
```

### CompressionDaemon

**Purpose**: Background scheduler for periodic memory compression.

**Features**:
- Configurable check interval (default: 1 hour)
- Idle detection (default: 5 minutes idle required)
- Activity tracking
- Graceful start/stop
- Error handling and logging

**Configuration**:
```rust
pub struct CompressionDaemonConfig {
    pub check_interval_seconds: u64,   // Default: 3600 (1 hour)
    pub idle_threshold_seconds: u64,   // Default: 300 (5 minutes)
    pub enabled: bool,                  // Default: true
}
```

**Usage**:
```rust
let daemon = Arc::new(CompressionDaemon::new(config, || async {
    compression_service.compress().await
        .map_err(|e| e.to_string())
}));

// Start daemon
let handle = daemon.start();

// Record activity to reset idle timer
daemon.record_activity();

// Stop daemon
daemon.stop();
```

**Integration Example**:
```rust
// Create compression service
let compression_service = Arc::new(CompressionService::new(
    database.clone(),
    provider.clone(),
    embedder.clone(),
    CompressionConfig::default(),
));

// Create daemon with compression callback
let daemon_config = CompressionDaemonConfig::default();
let daemon = Arc::new(CompressionDaemon::new(daemon_config, {
    let service = compression_service.clone();
    move || {
        let service = service.clone();
        async move {
            service.compress().await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
}));

// Start background compression
daemon.start();
```

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - How tools are invoked
- [Extension System](EXTENSION_SYSTEM.md) - Plugin-based tools
- [Security](SECURITY.md) - Tool execution safety
- [Memory System](MEMORY_SYSTEM.md) - Memory architecture and design
