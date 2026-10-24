# Aleph Modularization P0–P6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 保持单二进制完整默认体验，建立可外部消费的单定义契约边界，并沿 P0–P6 交付安全、可审计的模块替换。 / Preserve complete defaults while enabling safe, observable replacement.

**Architecture:** P0 下沉完整的 trait/纯值/固有方法闭包，旧路径 re-export；实现、会话权威、安全策略留 Rust Core。后续复用现有 Registry、OwnershipTree、Lease、Visibility 与 generation，不创造第二个 host。 / Contracts move; authority stays.

**Tech Stack:** Rust 2021 / MSRV 1.95；serde、serde_json、async-trait、thiserror、schemars、shared/protocol；必要 sha2/hex/chrono/tokio(sync)/http/bytes；Node/Bun 隔离 stdio sidecar（P6）。

**Spec:** `aleph_modularization_plan.md`（完整 §0–16；本计划补充闭包与验收，不修改原方案）。

## Global Constraints

- 用户已批准完整实施清单及 Task 0 QA 范围，先修复可信 macOS 内存探针，再按 P0→P6 推进；文档未提交，源码/行为各自的 review 与验收门不因批准而解除。 / Approved for staged implementation; evidence gates remain binding.
- 基线 `main@fac11b6de`，当前专用分支 `refactor/modularization-p0`；保留三个未跟踪用户文档，不 stage、commit 或擅自安装工具链。构建仅在真实内存门通过后执行，计划修订阶段未运行 cargo。
- `.` 已由主 agent 实测不存在；本次唯一授权落点 `.`，旧文档挂载点不是可切换仓库。以后实施先确认 git root，不复制或切换检出。
- 包名与目录沿原方案：`aleph-slot-api` / `crates/aleph-slot-api`；Rust import `aleph_slot_api`。不要另造 `aleph-api` 同义包。
- API 不依赖 alephcore、registry、config 编排、数据库、reqwest、axum、extism 或具体后端。新增非已批准依赖须先证明必要闭包并重新批准。
- 用户已选扩展轻量依赖；保留已有字段、serde、错误文本、固有方法和调用语义，不借迁移重新设计契约。代价：公开类型绑定第三方版本，未来升级需兼容审查。
- 所有生产功能始终编译；不以 feature 裁剪官方发行版。测试 feature `loom` / `test-helpers` 不属于产品裁剪。
- 不改 `src/harness/` 或 12 文件棘轮，不拆冻结遗留文件，不删/放宽安全 guard，不将 session/state/events 权威搬入 API。
- `shared/protocol` 是跨语言 wire 唯一来源；P0 在 inventory/review 硬 gate 关闭后才能写入/冻结 Hook@1/AiProvider@1/BrowserManagerContract@1；P2 接 Hook 生产授权/lease，AI/browser bridge 另行规划、P3 接线，不谎称已接线。
- 已有生产 ProjectionHost mount/install/await-ready；`src/capability/projection_host.rs` 构造 ZahirFacade。不得依据旧 AGENTS 备注重复挂载。
- 每次 cargo 前运行 `python qa/browser_live/check_memory.py`，确认可用内存 **>=4294967296 bytes**；不足停下。以下每条 cargo 命令都受此约束，包括 just 内部 cargo。
- GlobalStatus：计划已批准/未提交，Task 0 QA 实施中，P0 产品尚未开始。基线 Darwin probe 退出2；历史只读 `LC_ALL=C /usr/bin/vm_stat` 实验为 pageSizeBytes=16384、freePages=7452、freePhysicalLowerBoundBytes=122093568(<4294967296)。不是项目 probe 通过，也不是完整 macOS available 估计；真实正向门仍 BLOCKED，即便脚本支持 Darwin 也不能绕4GiB。续接状态与实测证据见 `.superpowers/sdd/2026-10-14-modularization-p0-implementation/progress.md`。
- TDD：新增测试先记录预期失败，再迁移，再运行；既有测试只记录真实基线，不能要求它们先失败。禁止把 0 tests 当通过。
- 每任务独立 reviewer gate；若以后批准提交，仅显式列出该任务文件，禁止 `git add .`；本次不执行提交。
- 中文说明 / English code comments；本计划新命名均明确为拟新增，不冒充现有 API。

## Review Focus

1. 拒绝理由包含 `401`/`429`/timeout：固定拒绝头仍优先，Permission/Repeated 不产生绕过 ladder（Task 2）。
2. 同型但不同定义、旧 serde 缺字段、隐藏固有方法闭包：跨旧/新路径赋值与 golden JSON 必须成立（Tasks 2–4）。
3. 通道取消先于订阅、无订阅者、能力声明却未实现：不得丢取消或返回成功 no-op；loom 语义不变（Task 4）。
4. 文件迁移令 include_str!/census 只看 re-export 或测试字符串：必须读真正权威生产源码且 mutation 能令守卫失败（Task 9）。
5. 外部 crate 宏/Value/协议依赖缺失：五个消费者（保留原方案至少三个目标：三个既有加 AI/browser 非空 mock）只直依赖 API，须编译且无 core 反向边（Task 10）。
6. Hook 未知事件/版本/字段或 Observer 越权，以及 post 伪造 Core metadata：strict decode/match 拒绝，合法业务 JSON 按槽 schema 校验（Tasks 5–6）。
7. AI 借用/取消/流错误与 browser 必要能力缺失：外部 mock 验证，不把 downcast/backend 状态塞进 API（Tasks 7–8）。

---

## File Structure & Dependency Gates / 文件与依赖

- 新 `crates/aleph-slot-api/src/{lib,error,text,tool,descriptor,concurrency,metadata,search,embedding,channel,cancellation,approval,webhook,hooks,slot}.rs`：按领域分文件，仅契约闭包。
- 新 `crates/aleph-slot-api/src/{ai,ai_message,ai_response,ai_delta,ai_reasoning,browser}.rs` 与 `tests/{ai_compat,browser_compat}.rs`：新增纯 port；Core adapter 不在 P0。
- 新 `crates/aleph-slot-api/tests/{error_compat,tool_compat,search_compat,channel_compat,hook_compat}.rs`：迁移与新增的纯契约回归。
- 新 `shared/protocol/src/hook.rs`：唯一 HookEnvelope/response wire；在现有 `shared/protocol/src/lib.rs` 导出。
- 新 `tests/modularization_contracts.rs`：旧路径类型身份、core 桥接与依赖方向断言；integration 用 `test-helpers`。
- 新 `examples/modularization/{tool,search,channel,ai,browser}/{Cargo.toml,src/lib.rs}`：三个既有消费者加两个非空 mock，各自 `[workspace]`，不加入主 workspace。
- 根 `Cargo.toml` 添加 API member/core path dependency；`Cargo.lock` 仅由实际 cargo 更新。实现目录与原路径只改契约引用/re-export，不移动具体 backend。
- DAG：0 preflight→1→2→3；1→4；2–4→5→6；1/2→7 inventory→review关闭→writing/freeze；1→8 inventory→review关闭→writing/freeze；2–8→9→10→P0验收→11(P0.5)→12(P1)/13(P2)→14(P3)/15(P4)/16(P5)→17(P6)。7/8 未关闭不得写/冻结或以 mock 绕过；无可信内存 probe 时重构实施（会话跟踪任务 #3，非本文搜索 Task 3）明确 BLOCKED，所有 cargo 同样阻塞。
- P0 共 **10 个实施任务 + 1 个环境前置 Task 0**；P0.5–P6 共 **7 个后续路线任务**。未核实 source closure 都是 future gate，不宣称 P0 全部 implementation-ready。

### Task 0: Environment preflight / 已批准的 macOS 内存 QA 前置

**Proposed scope:** `qa/browser_live/check_memory.py`（唯一必要 QA 修改）；本轮只读路径搜索确认 `qa/browser_live/tests/revoke-live.spec.ts`、`qa/browser_live/tests/viewer.spec.ts` 存在，未发现相邻 Python unittest 文件，不编造 test 路径、不改这些 browser E2E。fixtures 现位于同文件 `self_test()`。
- [ ] 主 agent 已实测 `python qa/browser_live/check_memory.py` 退出 2：`ERROR: unsupported platform for real probe: 'Darwin'`；unsupported 不是绿，也不能绕过 4GiB。此处记载上游证据，本轮未运行 probe。
- [ ] 用户已进一步批准 **psutil-style available 估计**，替代最初 free-only 分派口径：单次 `LC_ALL=C /usr/bin/vm_stat`（无 shell argv、局部 env、15s timeout），计算 `pageSizeBytes × (Pages free + Pages speculative + Pages inactive)`。Apple `vm_stat` 的 free=kernel.free_count−speculative；psutil 的 available=kernel.free_count+inactive（尚未减显示 free 的 speculative），因此三字段相加不重复计数。只依赖 stdlib，不安装 psutil、不加入 active/purgeable/compressed；估计值不保证可立即分配。保留旧 `parse_macos_vm_stat(text)` free-only helper 行为/fixtures，新 available parser 复用严格词法。source/formula/审批证据见 SDD `macos-available-evidence.md`，独立 review 后才启用 Cargo。
- [ ] 历史 free-only 实验为16384×7452=122093568，旧 real probes 亦低于4GiB。新口径只读快照为16384×(39466+1934+341674)=6276284416 bytes（约5.85GiB），**不是更新后项目 probe 通过**；当前仍须实现/审查并得 real CLI0。不得将历史或公式实验当权限缓存，未运行 cargo。
- [ ] 缺 header/三项必需 field、重复字段、非正页大小、非法单位/负数/非整数、timeout/nonzero exit 全部 error/fail-closed；页计数0有效，不是猜默认。保留 CLI/`available_bytes=... required_bytes=... ALLOW|BLOCK`、退出码 0/1/2、Windows/Linux 语义和 `>=4294967296` inclusive 边界。
- [ ] 扩充既有 self-test：三字段各自 malformed/单位/重复/missing/zero、just-below/equal/above、4096/16384页大小、三个 counter 独立贡献、probe failure/env/argv fixture；Darwin 原 unsupported 断言替换为真实分派 mock 验证，未知平台仍拒绝。父环境/调用方 overrides 完整保存校验且在失败路径还原，每个 expected subprocess exactly-once-with，套件其余 subprocess 默认拒绝；父环境污染、caller mutation、额外 shell call 三个 mutant 必须被拒绝。现有命令 `python qa/browser_live/check_memory.py --self-test`，fixtures仍在同文件，不编造额外 unittest 文件。
- [ ] 修复后本机执行 `python qa/browser_live/check_memory.py` 得 real positive（退出0且 bytes>=阈值），然后**每一条** cargo/内部调用 cargo 的 just 前重测；只有 fixtures 绿或 real 低于4GiB都不得编译。没有可信 probe/real positive 就停止 Rust 实施与 Cargo 验收。主 agent 随后只执行了既有 `python qa/browser_live/check_memory.py --self-test`，结果 `self-test OK`；这是既有 fixtures 基线，不是 Darwin real probe 通过，也不代表拟新增 QA 已实施。

### Task 1: P0 crate skeleton & error authority / 完整错误闭包

**Files:** Create `crates/aleph-slot-api/{Cargo.toml,src/lib.rs,src/error.rs,src/text.rs,tests/error_compat.rs}`; Modify `Cargo.toml`, `src/error.rs`, `src/utils/text_format.rs`; Test `tests/modularization_contracts.rs`.
**Interfaces:** 保留 `AlephError`、`ErrorClass`、`AlephException`、`Result<T>` 及全部现有构造器/From/Display；`pub const fn class(&self) -> ErrorClass`、`pub const fn is_transient(&self) -> bool`、`pub fn user_friendly_message(&self) -> String`。移动 `pub fn truncate_text(text: &str, max_chars: usize) -> String` 的单定义，旧 utility 路径 re-export；其余四种截断算法留 core。

- [ ] 创建 error golden 测试：Clipboard 的 Display=`Clipboard error: access denied`；Cancelled.class=Recoverable；InvalidResponse=Transient；RequestRejected=Fixable；is_transient 不扩大到 ProviderError/McpTimeout；JSON/std::io From 字符串不变。
- [ ] 创建 UTF-8 测试：`truncate_text("中文🙂", 2)=="中文..."`，上限 0 为 `""`；MissingInput 展示用 100 **字符**软上限再加 `...`，不能改成 100 bytes。
- [ ] Run `cargo test -p aleph-slot-api --test error_compat`；首次应因 package 尚不存在失败，记录后再创建 member。
- [ ] 建 crate 并完整迁移错误/方法/helper；不得在 core 为外来类型添加 inherent impl（Rust 禁止），不得复制 formatter。迁移原 error tests 到 API，core 类型身份测试保留。
- [ ] Run 同一命令及 `cargo check -p alephcore --lib --bins`；通过含非零 error tests 和旧路径赋值编译。记录新的依赖树，不能出现 core。

### Task 2: P0 tool contract closure / 工具、分类与描述符

**Files:** Create API `src/{tool,descriptor,concurrency,metadata}.rs`, `tests/tool_compat.rs`; Modify `src/tools/{service,descriptor,error_kind,concurrency}.rs`, `src/tools/handlers/mod.rs`, `src/thinker/nudges.rs`, `src/tool_metadata/` 纯类型定义文件, `src/session/events.rs`; Test `tests/modularization_contracts.rs`.
**Interfaces:** `ToolHandler: Send + Sync + 'static` 的 `async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>`、`fn definition(&self) -> ToolDefinition`、默认 `concurrency_claim`=global、`fences_output` 的既有 MCP 默认，以及第五方法 `bind_visible_servers`（默认 None、纯 McpServerFilter alias）；`ToolError::kind`/`is_retryable`、RefusedBy、ToolErrorKind/classifiers；ToolDefinition/Metadata、ToolSource、ToolCapabilityDescriptor/DescriptorError、ReplayPolicy、ImplementationContract、ReplayContractFingerprint、ToolCallIdentity；ToolOutput/Metadata/ToolImage 与 protocol Presentation。`ToolService` 及执行、准入、重试实现留 core。

- [ ] 在 `tool_compat` 创建 typed↔Display 分类矩阵：RefusedBy::ALL 的理由分别含 `401`/`429`/timeout，均 Permission；Repeated 不 admits_ladder；Cancelled 不 admits_ladder；ApprovalExpired/Cancelled 均 is_retryable 且 kind.is_transient；每个 kind.label 不变。
- [ ] 创建描述符 golden/fingerprint 测试：合法往返、字段/schema/safety 不匹配拒绝、未知 fingerprint 版本拒绝、Safe 缺 implementation contract 拒绝；不能造 revision=1 来投影 metadata。
- [ ] Run `cargo test -p aleph-slot-api --test tool_compat`；第一次 import 不存在失败，之后同命令应非零通过。
- [ ] 完整移动工具契约与现有纯算法/对应单测。ConcurrencyClaim/ExclusiveScope 连同构造器、normalize/conflict/batch helper 保持单定义；保留原跨种类矩阵：Paths/Nodes 不冲突，Sessions/其他种类仍保守冲突，双斜线归一化；不引入新调度策略（精确批准判据见 Task2 caller-gates）。
- [ ] 将 CROSS_BATCH_REFUSED_CAUSE 单定义下沉并在 thinker 旧路径 re-export；分类器不能反向 import thinker。保持固定头优先、Repeated 与原非绕过 ladder；原常量字节不能改。
- [ ] 固有投影方法 `ToolDefinition::to_metadata_definition`、descriptor `to_metadata_definition`/`to_unified_tool` 需要返回类型闭包：将其引用的 ToolCategory、metadata ToolDefinition、metadata ToolSource、UnifiedTool 及纯字段/default/helper 下沉 `metadata.rs`，旧路径 re-export；不移动 ToolCatalog/Registry。若发现非纯字段或未批准依赖，停在 gate，不能删方法或改为扩展 trait 悄悄破坏 source API。
- [ ] 只从 session events 移 ToolOutput/ToolOutputMetadata/ToolImage；Presentation 保留 protocol 单定义。SessionEvent、ToolCallIdentity 的事件使用点、state/recovery 权威留 core；按测试归属拆分，事件存储测试不搬。
- [ ] Run `cargo test -p aleph-slot-api --test tool_compat`、`cargo test -p alephcore --lib tools::`、`cargo test -p alephcore --lib session::`；验收 API 无 core import，旧新类型双向赋值，投影结果完全相同。

### Task 3: P0 search & embedding / 搜索与嵌入

**Files:** Create API `src/{search,embedding}.rs`, `tests/search_compat.rs`; Modify `src/search/{mod,provider}.rs` 及 SearchOptions/Recency/SearchResult 当前定义文件, `src/memory/embedding_provider.rs`；`src/search/providers/capability_census.rs` 仅将 OPTIONS_SRC 重新指向迁移后实际 mapper owner（九个 provider 输入和扫描断言仍留 Core）。
**Interfaces:** 保留 `async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>>`、`name`、`is_available`、`capabilities(&self, options: &SearchOptions)`；SearchOptions/Recency/SearchResult/SearchCapabilities；EmbeddingProvider 的 embed/embed_batch 签名原样移动，返回 Task 1 AlephError，不引入另一个错误 enum。

- [ ] 添加 `search_compat`：SearchCapabilities 默认三位 false；旧 options JSON 的默认值与 Recency 序列化不变；SearchResult::new 的 title/url/snippet 保持；Year 不得被声称为 Bing 支持。失败测试先引用 API 新路径。
- [ ] Run `cargo test -p aleph-slot-api --test search_compat`，记录缺失 exports 的失败。
- [ ] 同时移动完整 options/recency/result 的 builder 和纯 mapper，保住同处派生“排序承诺→实际 wire”；移 EmbeddingProvider trait 与签名闭包，配置、URL validation、RemoteEmbeddingProvider、HTTP 请求与 registry/handle 留 core。
- [ ] 移 provider 纯 trait 测试及其 Arc Send+Sync 断言；依赖实际 tokio test 宏的测试可留 core 或用 API dev-dependency（不是生产 runtime 扩权）。
- [ ] Run 同一测试及 `cargo test -p alephcore --lib search::`、`cargo test -p alephcore --lib memory::embedding_provider`；保留真实请求-builder/capability census 测试，不以 trait mock 替代它。

### Task 4: P0 channel complete closure & sync / 通道完整闭包

**Files:** Create API `src/{channel,cancellation,approval,webhook}.rs`, `tests/channel_compat.rs`; Modify `src/gateway/{channel,cancellation,channel_approval,webhook_receiver}.rs`, `src/exec/{socket,allowed_decisions}.rs`, `src/exec/approval/types.rs`, `src/sandbox/exec_approval/gate.rs` 纯 outcome 定义位置；`src/sync_primitives.rs` 只检查，不修改。
**Interfaces:** Channel/ChannelFactory/NativeStreamHandler 与全部消息/ID/health/capability/stream/attachment/conversation 类型；ChannelState、InboundMessageSender、CancellationToken；ChannelApprovalCapability/RenderedApproval/PendingApproval/ApprovalAction/AuthorizationResult；ApprovalRequest/CommandApprovalRequest/ApprovalDecisionType/default helper；WebhookHandler。`async fn create(&self, config: Value) -> ChannelResult<Box<dyn Channel>>`、create_with_id 默认委托，所有 optional 默认方法保留。

- [ ] 添加 `channel_compat`：未实现 optional 方法返回 UnsupportedFeature；declared=true/false 两种诊断；broadcast 两个 subscriber 均收到；take_receiver 每次 Some（不可改成单消费者）；cancel-before-subscribe 可收到通知；无人监听 send 返回原消息。
- [ ] 添加审批 wire 测试：旧 Command 请求缺 reason/allowed_decisions，reason=None，full_set 顺序 `[AllowOnce, AllowAlways, Deny]`；enum kebab-case 不变；clamped_for 向下逐级而不是任意改为拒绝；未认证/Denied 渲染去掉键盘。
- [ ] Run `cargo test -p aleph-slot-api --test channel_compat`，记录尚未导出失败。
- [ ] 移通道完整契约/defaults，包括私有 unsupported_feature、ChannelState 与 cancellation。chrono/tokio sync 是明确批准的闭包；保留 broadcast 容量（cancel=16）、SeqCst、try_read fallback=Disconnected，不换 tokio-util token。
- [ ] WebhookHandler 用 `http::HeaderMap` 和 `bytes::Bytes` 单定义（与 axum re-export 类型身份一致）；server/router/HMAC/挂载表/状态检查留 core，WEBHOOK_ROUTE_PREFIX 与回调 prefix 继续单定义并保持旧消费者引用。
- [ ] 审批 enum/纯 clamped_for/full_set 下沉；to_outcome_within 返回的 ApprovalOutcome 必须连同完整纯值定义下沉再原 gate 路径 re-export；审批执行/timeout/权限 floor 判定留 core。仅 full_set 是 serde 默认闭包，不把 for_confirm_gate/floor 策略迁入 API。
- [ ] sync 事实锁定：当前 `src/sync_primitives.rs` 的 Arc/Mutex/原子均 **始终 std**；AsyncRwLock 是 tokio；loom 用例 `cfg(all(test, feature="loom"))` 自行直接 import loom。API 可 re-export相同 std/tokio 类型，不做全局 loom swap，不将直接 loom 测试改成 std。迁出测试如直接用 loom，原 cfg 与 dev-dependency/feature 显式保留。
- [ ] 保留 `paradigm_for_channel_type` 在旧 core 文件（它返回 thinker InteractionParadigm，非 Channel trait 闭包）；ChannelApprovalExt 如为纯 trait/default 转移，任何具体 channel/manager/helper 留 core。
- [ ] Run 同一测试、`cargo test -p alephcore --lib gateway::`、`cargo test -p alephcore --lib exec::`、`cargo test -p alephcore --lib --features loom loom`；记录原 loom 用例清单与非零命中，数量/断言不可减少，不宣称 loom 覆盖 tokio broadcast。

### Task 5: P0 strict Hook@1 wire & legacy compatibility / 严格协议

**Files:** Create `shared/protocol/src/hook.rs`, `shared/protocol/tests/hook_wire.rs`, API `src/hooks.rs`, `tests/hook_compat.rs`; Modify `shared/protocol/src/lib.rs`, `src/extension/types/hooks.rs`, `src/extension/hooks/producer_census.rs`.
**Interfaces:** legacy HookEvent/MatchSubject/ALL/canonical_name/match_subject/supports_interceptor、SESSION_SOURCE_STARTUP/SESSION_SOURCES_FIRED、HookKind/HookPriority/HookAction 单定义下沉/re-export，serde 不变；HookConfig/executor 留 Core。以下是**拟新增** protocol 类型，不复用 legacy executor HookResult：
```rust
pub const HOOK_CONTRACT_VERSION: u32 = 1;
pub struct HookEnvelope { pub version: u32, pub session_id: String, pub turn_id: Option<String>, pub slot: String, pub event: HookInput }
pub enum HookInput {
    ToolPre { call_id: String, tool: String, input: Value, original_input: Value },
    ToolPost { call_id: String, tool: String, result: Value },
    Prompt { layers: Vec<PromptLayerView> },
    Compaction { messages: Vec<CompactionMessage>, required_pairs: Vec<ToolPair>, required_session_markers: Vec<String> },
    Continue { iteration: u32, limit: u32 },
    IngestFilter { records: Vec<IngestRecord> },
    Observer { lifecycle: ObserverLifecycle },
}
pub enum HookResponse {
    ToolPre { action: ToolPreAction }, ToolPost { replacement: Option<Value> },
    Prompt { append: Vec<String>, replace: Vec<ReplaceLayerRequest> },
    Compaction { summary: CompactionSummary }, Continue { request: bool, reason: Option<String> },
    IngestFilter { keep_ids: Vec<String> }, Observer { ack: ObserverAck },
}
pub enum ToolPreAction { Continue { updated_input: Option<Value> }, Deny { reason: String }, Ask { reason: String } }
pub struct PromptLayerView { pub id: String, pub text: String, pub replaceability: Replaceability }
pub enum Replaceability { Pinned, Replaceable }
pub struct ReplaceLayerRequest { pub replaceable_layer_id: String, pub text: String }
pub struct CompactionMessage { pub id: String, pub role: String, pub text: String, pub tool_call_id: Option<String> }
pub struct ToolPair { pub call_id: String, pub result_message_id: String }
pub struct CompactionSummary { pub text: String, pub preserved_pairs: Vec<ToolPair>, pub session_markers: Vec<String> }
pub struct IngestRecord { pub id: String, pub content: Value }
pub struct SessionLifecycleContext { pub session_id: String }
pub struct TurnLifecycleContext { pub session_id: String, pub turn_id: String }
pub struct SessionEndContext { pub session_id: String, pub reason: String, pub outcome: String }
pub struct TurnEndContext { pub session_id: String, pub turn_id: String, pub reason: String, pub outcome: String }
pub enum ObserverLifecycle { SessionStart { context: SessionLifecycleContext }, SessionEnd { context: SessionEndContext }, TurnStart { context: TurnLifecycleContext }, TurnEnd { context: TurnEndContext } }
pub struct ObserverAck { pub records: Vec<String> }
pub enum HookWireError { UnsupportedVersion(u32), Malformed(String), EventMismatch, InvalidResponse(String) }
pub fn decode_hook_v1(json: &str) -> Result<HookEnvelope, HookWireError>;
pub fn validate_response(input: &HookInput, response: &HookResponse) -> Result<(), HookWireError>;
```
- [ ] 生命周期事实矩阵：session_start/session_end 的 envelope.turn_id=None；tool.pre/tool.post/prompt/compaction/continue/ingest_filter 必须 Some(nonempty)，turn_start/turn_end 同样 Some(nonempty)。session_id/slot 必须非空，四 Observer context 的 session_id 必需非空并与 envelope 一致；turn context 也非空且匹配。IDs 仅 correlation，不赋予 authority。ingest 是否真实独立非turn事件本轮未核实，P0 review 必须确认；若独立，明确批准非turn工程选择与 Option 判据，不造假 turn。
- [ ] ended reason/outcome 是 Core 提供的 opaque nonempty 事实，范围为非空 String（非插件推理、非 Core 对象）；四个生命周期非空 golden 各覆盖 None/Some、reason/outcome，错误/空 ID 拒绝。ObserverAck 只有 records，不能添加 control fields。
- [ ] `HookInput`/`HookResponse` use `#[serde(tag="event", content="data")]`；明确 wire tags `tool.pre/tool.post/prompt/compaction/continue/ingest_filter/observer`。ToolPreAction 用 `#[serde(tag="action")]` 的 `continue/deny/ask`；其余 enums snake_case。所有 struct 与 variant 字段 `deny_unknown_fields`，必填不补 default，Option 接受 absent/null 并输出 null；version 必须等于 protocol 唯一 `HOOK_CONTRACT_VERSION`（自定义 Deserialize 与 decode 同规则），API/Core 只 re-export/引用，不另写 magic 1。响应用拟新增 `HookResponseEnvelope { version: u32, response: HookResponse }` 统一版本 guard，不生成第二个同名 wire enum；未来版本不降级。outer event tag 与 typed payload 单定义，unknown variant/field/version 拒绝。
- [ ] strict 外层不是封禁合法业务 Value：input/result/content 由 P2 Core 当前 slot 的 schema 验证；ToolPost 只能替换业务 result，wire 没有 metadata/served_by/backend/generation/lease/safety 写入口，嵌在业务 JSON 的同名键也不得提升为 Core 权威。Prompt replace 只请求已声明 Replaceable 的层；Pinned ID 验证失败，无 pinned 替换 action。
- [ ] `validate_response` 同事件才可成功：Observer 仅 Ack（records 是记录而非控制）；compaction 在 P0 用纯 validator 对真实 messages 事实验证：required_pairs 必须是消息中真实 call/result 配对全集，未知/非真实 ID、重复 pair 均拒绝；输出 preserved_pairs 与全集一一相等（非只比较长度）。输入 required_session_markers 是 Core opaque facts（Vec<String>），输出 markers 必须全集一致，缺失/伪造/重复拒绝；summary text 自由，不用 LLM 判断质量。当前 CompactionMessage 简化 shape 未认证真实 messages 字段闭包，写/冻结前必须审阅真实结构与 producer 投影，不能凭 role/text 编造配对证据。keep_ids 必须为输入 ID 子集且不重复，Continue.request 在 iteration>=limit 拒绝。纯值检查不授权；P2 校验 slot/schema/grant/lease 后才执行，不在 P0 宣称效果。
- [ ] 新测试 `hook_v1_golden_all_events` 用七个非空 fixtures 逐字段 roundtrip；`hook_v1_unknowns_reject`：`assert!(decode_hook_v1(version_2).is_err()); assert!(decode_hook_v1(unknown_event).is_err()); assert!(decode_hook_v1(extra_outer_field).is_err()); assert!(decode_hook_v1(extra_variant_field).is_err());`；`hook_v1_business_json_survives`：`assert_eq!(decoded_input, json!({"custom":{"x":1}}));`。
- [ ] P0 新测试对 envelope 与响应 version/unknown variant/outer+variant unknown fields 的全部负例同时走 `serde_json::from_str`/`serde_json::from_value` 和 decode helper；响应 envelope 与 HookResponse 本体均 strict。`compaction_facts_reject_missing_forged_duplicate` 覆盖缺/伪造/重复 marker、pair、非真实/未知 call/result、全集缺项；`prompt_replace_ids_reject` 拒绝 Pinned/unknown layer ID/重复 replace ID，仅用输入 layers 即可判定，不能拖到 P3。
- [ ] 新测试 `hook_v1_effect_boundary`：`assert!(validate_response(&pre,&post).is_err()); assert!(validate_response(&observer,&pre_response).is_err()); assert!(validate_response(&prompt_pinned,&replace_pinned).is_err()); assert!(decode_response_with_metadata.is_err()); assert!(validate_response(&compact,&missing_pair).is_err());`；`legacy_hook_golden_unchanged`：24 个 legacy canonical/PascalCase aliases 全覆盖，priority `[-1000,-100,0,100]`、未知 kind=Observer、HookAction golden 不变。
- [ ] 先 Run `cargo test -p aleph-protocol --test hook_wire` 与 `cargo test -p aleph-slot-api --test hook_compat`，缺新符号失败；再定义以上闭集/validator 与 legacy re-export，同命令非零通过。API `pub use aleph_protocol as protocol`；不把新 response alias 成旧 executor HookResult。
- [ ] 同步 producer_census 的 `include_str!("../types/hooks.rs")` 指向 API 的 legacy enum 权威；新 wire enum 不替代旧 producer 集。Task 9 mutation 验证；Run `cargo test -p alephcore --lib extension::hooks`，Claude Code hookSpecificOutput 生产路径不变。

### Task 6: P0 Slot identity & typed Hook trait / 显式新契约

**Files:** Create API `src/slot.rs`; Modify API `src/{hooks,lib}.rs`; Test `tests/hook_compat.rs`.
**Interfaces:** 拟新增 `SlotId(pub String)`、`ContractVersion(pub u32)`；protocol `HookEventTag` 七个与 HookInput 同名的无 payload variants，`HookInput::event_tag(&self) -> HookEventTag`；API re-export，不依赖 legacy HookEvent 做 wire tag。`HookDescriptor { id: String, slot: SlotId, event: protocol::HookEventTag, kind: HookKind, priority: HookPriority, contract_version: ContractVersion }`；`HookContext { envelope: protocol::HookEnvelope }`；`HookError { message: String }`（Display=message）；`Hook: Send + Sync` 的 `fn descriptor(&self) -> &HookDescriptor` 与 `async fn execute(&self, ctx: &HookContext) -> Result<protocol::HookResponse, HookError>`。只有一个 event 来源；调用方先比 descriptor tag/version，再 execute，再 Task 5 validate_response，任何不匹配拒绝。
- [ ] `typed_hook_context_matches_wire`：`assert_eq!(ctx.envelope.event.event_tag(), mock.descriptor().event); assert_eq!(mock.descriptor().contract_version.0,protocol::HOOK_CONTRACT_VERSION); assert!(validate_response(&ctx.envelope.event,&response).is_ok());`；`observer_cannot_control_execution`：Observer mock 仅返回 Observer Ack，跨事件 response 验证失败；`assert_send_sync::<Box<dyn Hook>>();`。
- [ ] 拟新增纯 guard `validate_hook_request(descriptor: &HookDescriptor, envelope: &protocol::HookEnvelope) -> Result<(), HookError>`；descriptor tag/version mismatch 在 execute 前 fail-closed，两类 mock 负例 `assert_eq!(call_count,0)`。execute 后必须调用 Task 5 `validate_response`，跨事件/非法替换响应不被消费；此处测试纯调用边界，不声称生产授权已接线。
- [ ] 先 Run `cargo test -p aleph-slot-api --test hook_compat` 缺符号失败；定义以上纯契约/newtype serde（不推理授权），再同命令非零通过及 `cargo check -p aleph-slot-api --all-targets`。System 类型可表达不等于插件有权。
- [ ] 根 re-export `async_trait::async_trait`、`serde_json`/`json!`/`Value`、protocol 与 sync/http/bytes；不引 protocol→API 环。P0 只编译/mock，P0.5/P2 再生产排序/adapter/schema/授权/错误接线。

### Task 7: P0 AiProvider@1 pure port / AI 契约非后端迁移

**Files:** Create API `src/{ai,ai_message,ai_response,ai_delta,ai_reasoning}.rs`, `tests/ai_compat.rs`, `examples/modularization/ai/{Cargo.toml,src/lib.rs}`; candidate Modify `src/providers/{mod,adapter,message,delta,reasoning_replay}.rs`, `src/agents/thinking.rs`, `src/thinker/prompt_builder/mod.rs` 的纯值定义/re-export。
**Source anchors:** `src/providers/mod.rs:250–414` 现有 AiProvider；`src/providers/adapter.rs:29–165` RequestPayload fields/default/builders；`src/providers/delta.rs:33–89,608–610` ProviderDelta/DeltaSink。现有 `as_http_provider(&self) -> Option<&http_provider::HttpProvider>`（`src/providers/mod.rs:351`）直接暴露 backend，不能原样下沉。
**Interfaces:** 拟新增纯 `AiProviderContract: Send + Sync`，API `pub use AiProviderContract as AiProvider`，@1 是契约版本不是第二 registry。保留现有纯方法 exact signatures：
```rust
fn process<'a>(&'a self, payload: RequestPayload<'a>) -> Pin<Box<dyn Future<Output=Result<ProviderResponse>> + Send + 'a>>;
fn execute_streaming_dyn<'a>(&'a self, payload: RequestPayload<'a>, sink: &'a dyn DeltaSink) -> Pin<Box<dyn Future<Output=Result<ProviderResponse>> + Send + 'a>>;
fn name(&self) -> &str; fn color(&self) -> &str;
fn supports_native_tools(&self) -> bool; fn supports_streaming(&self) -> bool;
fn protocol(&self) -> Cow<'_, str>;
fn model_behavior_override(&self) -> Option<Cow<'_, str>>;
fn behavior_hint(&self) -> Option<Cow<'_, str>>;
fn serving_model_hint(&self) -> Option<Cow<'_, str>>;
fn serving_provider_hint(&self) -> Option<Cow<'_, str>>;
fn reasoning_replay(&self, model: Option<&str>) -> ReasoningReplay;
fn clears_tool_results_server_side(&self) -> bool;
// DeltaSink: Send + Sync, async_trait
async fn on_delta(&self, delta: &ProviderDelta);
```
- [ ] **先交 inventory 身份矩阵→独立 review 关闭→才可写入/冻结**：逐类型列字段、固有 methods、defaults、返回类型、dependency、旧/新 exact path 身份（legacy Core trait 与新 port 明确不同）；全部 response/message/reasoning 未认证闭包逐项关闭，不边写边候选。DTO 迁移验证旧→新与新→旧 Rust 直接赋值，serialize 比较不证明身份。
- [ ] 闭包包括 RequestPayload<'a> 的全部现有字段/builders/default（messages、system_prompt/system_blocks、tools、think_level、temperature、max_tokens、tool_choice、model、metadata），UnifiedMessage 完整纯值/固有方法、SystemPromptPart、ThinkLevel、ToolChoice、ProviderResponse/NativeToolCall/StopReason/TokenUsage、ProviderDelta、DeltaSink 与 ReasoningReplay 纯 default 算法；Task 2 metadata ToolDefinition 单定义。**候选 gate**：message/reasoning/其余 response methods 未在本次预算内完整读，实施前 bounded inventory 每个字段/返回类型/依赖，审阅纯闭包名单；不删除方法或把未知闭包声称已核实。
- [ ] 新 trait default 保留现有 flags/Cow/hints/reasoning 行为，streaming default process 后 replay sink（不能成功但静默丢 deltas）；错误沿 Task 1 Result。不引 BoxStream/futures：此 port 用 std Future+sink，adapter 的 reqwest/BoxStream 不属于它；replay 纯 body 与 `collect_response_deltas` 单份纯 helper 从 delta 抽取；有状态 DeltaCollector/HTTP/protocol adapters 留 Core（source-review §10 与 main match-boundary 已关闭源码归属门，不代表实现验收）。
- [ ] P0 不搬旧 core trait 的 backend 方法、不改生产 wrappers。候选兼容 gate：Core 原 AiProvider 的所有方法/default/as_http_provider backend coupling 原样保留，生产 wrappers 调用必须编译且既有 provider/default-stream 测试非零；新 mock 只证明 new port，不证明 old dyn 相同。Core 到 API port 的 pure adapter/HttpProviderAccess 分离须 P3 bridge 前独立计划（无第二 registry），精确 census `as_http_provider` 的 impl 与调用，再批准 bridge；已定位 `http_provider.rs:965`、`metering.rs:416`、`think_level_provider.rs:121`、`model_override_provider.rs:69`，尚非完整消费者清单。两 trait 是 legacy bridge 边界不是重复纯 DTO，P0 API port 外部可用但不能声称旧 dyn 已自动接入。
- [ ] `ai_borrowed_payload_and_sink` 外部 mock 返回 `ProviderResponse::text_only`（现有 `src/providers/mod.rs:425` 调用），借用本地 messages/sink：`assert_eq!(response.text_content(),"echo"); assert_eq!(sink_text,"echo"); assert!(!mock.supports_streaming());`；`ai_error_and_drop_cancel`：`assert!(matches!(result,Err(AlephError::Cancelled))); drop(pending_future); assert_eq!(drop_count,1); assert_eq!(sink_after_drop,0);`，不承诺取消远端已经发生的副作用。
- [ ] 先 Run `cargo test -p aleph-slot-api --test ai_compat` 缺 port 失败；建非空 mock（process/name/color，default stream 真实 replay，Pending Future Drop 计数），再同命令非零通过；`cargo check --manifest-path examples/modularization/ai/Cargo.toml` 验证非 'static payload 与对象安全，cargo tree 无 core，唯一 normal dependency API。

### Task 8: P0 BrowserManagerContract@1 / 统一管理 port

**Files:** Create API `src/browser.rs`, `tests/browser_compat.rs`, `examples/modularization/browser/{Cargo.toml,src/lib.rs}`；P0 不修改 manager/backend 的生产实现。
**Source anchors:** 以1–700 bounded read 完整读取 `src/browser/backend.rs` trait/effect 定义（38 async delegate 动作 + 1 纯值 effect accessor；`as_any` 是 backend identity，非 delegate）；以1–650 bounded read 读取 `src/browser/types.rs` 全部生产 DTO 定义及部分测试（尾部22行测试未读）；`src/browser/manager.rs:519–578` principal-scoped profile、per-call live SSRF、shared engine/tab/route/recording registry 必须留 Core，mutable owner/session/config 不抽出。
**Interfaces:** 拟新增 `BrowserManagerContract: Send + Sync` 不是 legacy `BrowserBackend` 同一身份；`BrowserRequest { profile: String, operation: BrowserOperation }`、`BrowserCapabilities { supported: Vec<BrowserFeature> }`、`BrowserContractError` 原三变体 Unsupported/Cancelled/Execution(String) **仅为未冻结候选，不能覆盖安全语义**。required `fn capabilities(&self) -> BrowserCapabilities`、`async fn execute(&self, request: &BrowserRequest) -> Result<BrowserReply, BrowserContractError>` 与纯 `validate_browser_claim(required: &[BrowserFeature], offered: &BrowserCapabilities) -> Result<(), BrowserContractError>`。Operation/Reply/Feature **不能冻结为旧六项候选**；以下全 inventory 每行→独立 operation/feature/tagged reply，所有 Result 的 error 当前为 BrowserError，错误值/诊断闭包也需 review。

| Delegate inventory / 输入（省略共同 &self） | 成功返回 T / 不可丢事实 |
|---|---|
| open_tab(url:&str); close_tab(tab_id:&str); list_tabs() | TabId; (); Vec<TabLine>（全部 tab info，不能缩为 id/url） |
| navigate(tab_id:&str,url:&str); click(tab_id:&str,target:ActionTarget) | (); () |
| type_text(tab_id:&str,target:ActionTarget,text:&str); fill(tab_id:&str,target:ActionTarget,value:&str); hover(tab_id:&str,target:ActionTarget) | (); (); () |
| scroll(tab_id:&str,target:ActionTarget,direction:ScrollDirection); screenshot(tab_id:&str,opts:ScreenshotOpts) | (); ScreenshotOutput { png_bytes:Vec<u8> }，不得虚构路径替代 bytes |
| snapshot(tab_id:&str); snapshot_presented(tab_id:&str,max_chars:usize) | SnapshotOutput; PresentedSnapshot（全文、截断、chars budget 全保留） |
| evaluate(tab_id:&str,js:&str); select(tab_id:&str,target:ActionTarget,value:&str) | String 实际值；() |
| press_key(tab_id:&str,key:&str); history(tab_id:&str,nav:HistoryNav); dblclick(tab_id:&str,target:ActionTarget) | (); (); () |
| wait_for(tab_id:&str,condition:&WaitCondition,timeout_ms:u64); console_messages(tab_id:&str); network_log(tab_id:&str) | bool（timeout=Ok(false)）；String；String |
| route_add(tab_id:&str,rule:NewRouteRule); route_list(); route_remove(rule_id:&str); route_clear(tab_id:&str,scope:RouteScope) | RouteRuleInfo; Vec<RouteRuleInfo>; RouteRuleInfo（含 final hit count）；usize |
| record_start(tab_id:&str,options:RecordStartOptions); record_stop(tab_id:&str,recording_id:Option<&str>); record_status(tab_id:&str) | RecordingStatus（resolved path）；RecordingReceipt（verified receipt）；Option<RecordingStatus> |
| pdf(tab_id:&str,output_path:&Path); switch_tab(tab_id:&str); handle_dialog(tab_id:&str,action:&str,prompt_text:Option<&str>) | (); (); ()（路径请求/实际 effect 不伪报） |
| drag(tab_id:&str,from:ActionTarget,to:ActionTarget); upload(tab_id:&str,target:Option<ActionTarget>,paths:&[String]); resize(tab_id:&str,width:u32,height:u32) | (); (); () |
| emulate(tab_id:&str,opts:&EmulateOptions); save_state(path:&Path); load_state(path:&Path); cookies(op:&CookieOp); fill_form(tab_id:&str,fields:&[(ActionTarget,String)]) | (); (); (); String; usize（真实 filled count） |
| last_effect_verification()（sync 值 accessor，不搬 latch） | Option<EffectVerification>：None≠Verified，Verified/Skipped(Engine)/Failed 不合并 |

- [ ] 已读 DTO 字段：ActionTarget=Ref{ref_id}/Coordinates{x:f64,y:f64}；HistoryNav=Back/Forward/Refresh；WaitCondition=Text/TextGone/Selector/SelectorGone/UrlContains(String)/Time(u64)；ScreenshotOpts{full_page,format} defaults=false/png；SnapshotOutput{snapshot_text,page_url:Option<String>,page_title:Option<String>,ref_count:usize,state_json:Option<Value>}；PresentedSnapshot{snap,text,truncated,full_text:Option<String>}，truncated 必须全文可恢复；ScrollDirection/400px 与固有 wheel_delta 单定义。EmulateOptions 六个 optional 字段/color/geolocation/network/cpu/headers/user_agent、CookieOp 五变体及 leaves/validate/helper/default 全部进入 review，不能复制纯方法或删校验。
- [ ] **P0 写/冻结硬 stop：未认证闭包**为 `src/browser/tab_registry.rs` TabLine（本轮仅1–95，没有定义位置）、`src/browser/cdp_backend/routes.rs` NewRouteRule/RouteRuleInfo/RouteScope、`src/browser/cdp_backend/recording.rs` RecordStartOptions/RecordingReceipt/RecordingStatus、`src/browser/engine.rs` Engine 与 effect helper 的策略边界、`src/browser/error.rs` 错误，以及 browser tool/schema→manager 全映射/截图路径输出。本轮未读这些完整定义，不能声称全部 closure 已核实；实施前逐字段/固有方法/serde/schema/依赖/旧新 path/返回值 inventory + 独立 review 关闭才写冻结 @1，不能拖到 P3 补已冻结 v1。非纯 helper 留 Core，未经批准依赖停下。
- [ ] **error 写/冻结前硬 gate**：先读 `src/browser/error.rs` 全闭集与生产分类调用点，提交旧 variant→结构化 API error→fallback eligibility 矩阵；优先复用 Task 1 AlephError/Task 2 RefusedBy，若需新 variant 逐项 source 证明，不凭空定最终形状。矩阵必含 Permission/security denial、revoke、lease invalid、approval failure→绝不 ordinary execution/fallback；Unsupported、Cancelled→非 ordinary；仅已认证 ordinary execution/timeout 才由 Core 显式 fallback 策略消费。未知/无法分类默认不进 execution fallback，不用 text regex 猜安全。本轮未读旧 error，gate 未关闭。P0 `browser_typed_permission_never_ordinary` 非零测试确认 typed Permission 不被压成 Execution；安全权威仍在 Core，不由插件同名 metadata 提权。
- [ ] 全 38 delegate + effect 事实各有 operation/reply/feature 1:1 mapping test，动态派生 inventory 非空并 count 相等、缺任一映射失败；reply tag 与请求配对 guard，保留 None/false/error/路径/bytes/截断真实值。`browser_all_delegates_map_losslessly` 和 `browser_schema_inventory_complete` 非零是 P0 完成前提，六项不能代表全组。required 方法无成功 no-op defaults；不可用 Eval/脚本冒充 click/fill 等绕过 capability/grant，legacy wait 的真实 default 不在纯 port 搬运行时。请求 profile 只是标识非授权 token，Core 校验 session/grant/lease/SSRF。
- [ ] `browser_claim_requires_every_feature`：`assert!(validate_browser_claim(&[OpenTab,Snapshot],&only_open).is_err()); assert!(validate_browser_claim(&[OpenTab],&only_open).is_ok());`；`browser_mock_effect_and_cancel`：open 返回非空 tab，navigate 修改 mock URL，list 返回修改结果；`assert_eq!(tabs[0].url,"https://example.test/new"); assert!(matches!(missing,Err(BrowserContractError::Unsupported(_)))); drop(pending); assert_eq!(drop_count,1);`。
- [ ] 先 Run `cargo test -p aleph-slot-api --test browser_compat` 缺 port 失败；定义闭集与 validator、实现非空外部 mock，再同命令非零通过及 `cargo check --manifest-path examples/modularization/browser/Cargo.toml`、cargo tree，无 core/manager/backend 重依赖。外部 mock 编译不是生产 adapter 已完成。
- [ ] MCP 必须 adapter→统一 manager+现有 registry；native/state adapters 是 P3 future 路线，不反推 P0 闭包可不完整。P0 mock 仅证明新 port，P3 用全 inventory（含 evaluate 真值、wait false、effect 三态）逐项验证 adapter，缺 required 拒绝认领，不允许直接工具组接管或第二 owner/session/lease/state map。

### Task 9: P0 source-census & dependency guard / 防误绿

**Files:** Create `tests/modularization_contracts.rs`; Modify `src/capability/census.rs`, `src/search/providers/capability_census.rs`, `src/extension/hooks/producer_census.rs` 及实际包含 error_funnel_census 的现有文件；Test 现有 harness budget guard（不编辑 harness）。
**Interfaces:** 测试读取 API 真实 enum 权威，core 注册/效果 producer 仍取 Core；Cargo metadata 只证明 package normal edges，不能证明 Rust import 边。现有 `src/extension/hooks/producer_census.rs:45–57` corpus 固定 CARGO_MANIFEST_DIR+全局 OnceLock，`:233–255` enum_body 固定 include_str，无 fixture scanner 入口。

- [ ] 创建 core 旧路径↔API 新路径双向赋值测试，覆盖错误、工具、输出、搜索、通道、hook；记录每个迁出 cfg(test) 的“旧模块→新测试位置→测试名”清单，迁前迁后数量可解释且断言保留。
- [ ] Run `cargo test -p alephcore --features test-helpers --test modularization_contracts`；新 API 身份测试不得通过复制类型或 JSON 转换。
- [ ] 更新 include_str!/相对路径/file!/CARGO_MANIFEST_DIR 的根推导；enum census 从 API 实际 enum 读取，core producer 扫描依然完整；若找不到权威 enum 明确 panic，不能接受空集合。
- [ ] 先拟抽 **test-only** 共享 parser：`parse_hook_enum(raw: &str) -> Result<HookEnumInventory, GuardError>`、`scan_hook_producers(raw_enum: &str, corpus: &[(String, String)]) -> Result<ProducerReport, GuardError>`、`scan_hook_fixture(root: &Path, authority: &Path) -> Result<ProducerReport, GuardError>`。类型均拟新增；production-root test wrapper 与 fixtures 调同函数，显式 root/raw input，不跨 root 复用全局 OnceLock、不让 include_str 绑定 fixture 权威；不动生产 harness/安全策略。enum opening 缺失/空 variants/空 event 集必须失败。
- [ ] producer fixtures `tests/fixtures/modularization/{clean,missing_producer,missing_enum,empty_enum}/`：clean 包含权威 legacy enum+完整 firesites；missing_producer 删 session_start startup dispatch、missing_enum 只有 re-export、empty_enum 无 variants。双方向测试：clean/production root 非零通过；删除每一真实 fire-site 报具体缺 event，缺/空 enum fail；按 clean→bad→clean 与 bad-root→clean-root 顺序扫描结果不受 cache 污染。consumer identities 另测旧↔新直接赋值，不能拿 fixture green 替代。
- [ ] dependency/import guard **独立验收**：metadata normal edges 检包依赖；拟 `scan_contract_imports(root: &Path) -> Result<ImportReport, GuardError>` 由可复用 AST parser 或已条件批准 dev-only syn 实现。alias/glob/qualified/reexport fixtures 分别含 `use alephcore as c`、`use alephcore::*`、`alephcore::providers::...`、`pub use alephcore::...`；合法 API imports 必须通过，四类越界必须失败，不能一行 metadata 自证 imports。parser 复用/实现及 fixture 未核实则此 gate 未关闭，不报告 guard 完成。
- [ ] 检查 API dependency whitelist 和 cargo metadata 的生产 normal edges，无 alephcore/backends；dev-dependency 不混成生产边。P4 backend guard 不在 P0 强禁现有合法 gateway 编排引用。
- [ ] Run `cargo test -p alephcore --lib census`、`cargo test -p alephcore --lib error_funnel`、`cargo test -p alephcore --lib budget`；记录具体命中测试名/非零数，必要时按输出中的完整名补跑，不可 0 tests 过关。

### Task 10: P0 external consumers & full acceptance / 五消费者与总验收

**Files:** Create `examples/modularization/{tool,search,channel,ai,browser}/{Cargo.toml,src/lib.rs}`; Test API、core、workspace、受影响 desktop/shared 与现有 integration。
**Interfaces:** 五示例每个 manifest **唯一 normal dependency** `aleph-slot-api = { path = "../../../crates/aleph-slot-api" }`；各自 `[workspace]`；通过 API re-export 使用 async_trait/Value/json!/protocol，不依赖 alephcore。

- [ ] 写 tool 示例覆盖 ToolHandler 五方法：实现 invoke/definition，实际调用 concurrency_claim/bind_visible_servers/fences_output 三个默认方法；definition 用完整已迁入字段、ToolSource::Builtin、Metadata::default；invoke 返回 value+metadata 完整 ToolOutput，不使用未核实的 ToolOutput::from_value。
- [ ] 写 search 示例实现 search/name/is_available，返回 `SearchResult::new(title, url, snippet)`；options default；显式检验 capabilities 默认 false；不请求网络。
- [ ] 写 channel 示例实现 Channel 的 info/state/start/stop/send，以及 ChannelFactory::create；state 用 `ChannelState::new(16)`；发送结果使用迁入 SendResult 的实际字段，不虚构 constructor。测试可编译 create_with_id 默认委托、optional 方法错误与 webhook/approval 返回类型。
- [ ] 在迁移前分别 Run `cargo check --manifest-path examples/modularization/tool/Cargo.toml`、search、channel 的同构命令；首次 API 不存在应失败；迁移后全部通过。`cargo tree --manifest-path examples/modularization/tool/Cargo.toml`（另两份亦跑）不能含 alephcore，manifest 不能加 async-trait/serde_json 直依赖蒙混过关。
- [ ] 已核实 builder calls：`SearchResult::new(String, String, String)`；`OutboundMessage::text(conversation_id, text).with_reply_to(message_id)`；`ChannelState::new(buffer_size)`；`ChannelId/ConversationId/UserId/MessageId::new(id)`；`ToolDefinition::from_descriptor(&ToolCapabilityDescriptor)`。所有调用保持现有参数语义，不引入新组装 API。
- [ ] 所有 cargo/调用 cargo 的 just 前分别 Run `python qa/browser_live/check_memory.py`，可用内存 >=4294967296 bytes 才继续（不是只对第一条检查）。先审批生成副作用并 `just _stage-shell-placeholders`，**在 workspace check 前**；再 `cargo fmt --all -- --check`、`cargo check --workspace`、`cargo check -p alephcore --lib --bins`、`cargo test -p aleph-slot-api`、`cargo test -p alephcore --lib`、`cargo test -p alephcore --bins`、`cargo test -p alephcore --features test-helpers --tests`、`cargo test -p aleph-panel --lib --no-run`、`just test-shared`。预期退出 0，保留实际耗时/非零测试数；panel no-run 仅编译不是运行通过。
- [ ] 对 cargo metadata 确认的受影响 desktop package 执行 check（workspace 已覆盖本机可构建目标）；跨平台 runner 运行 macOS/Linux/Windows channel consumer/check，缺 runner 标记未验证，不声称兼容。
- [ ] placeholder 已在 workspace check 前准备；重新内存检查后 Run `cargo clippy --workspace --all-targets`，不引入新诊断，基线问题分别报告。本次所有 checkbox 均未执行。
- [ ] reviewer 对照原方案 §9/§13：完整契约 crate+三消费者+旧调用编译+原安全 census/loom，不接受只有叶子结构空壳；记录依赖变化与未经验证平台；P0 gate 通过后继续制定并推进 P0.5 精确子计划，不逐阶段机械重问。仅出现新增重大取舍、未经批准依赖或真实阻塞时暂停澄清。

## Follow-on Roadmap / 后续阶段（非当前 implementation-ready）

后续每任务先写精确子计划，固定实际签名/测试 fixture/命令后再实现；下面 Files 是目标范围，Interfaces 是原方案意图，不冒充已存在 API。P0 不能被当成整个 P0–P6 完成。

### Task 11: P0.5 versioned registries & privilege fix

**Files:** `src/capability/` registry、`src/tools/registry.rs`、`src/extension/{lifecycle,mod,skill_ops}.rs`、hook registry/executor、`src/gateway/execution_engine/tool_service_builder.rs`。
**Interfaces:** 抽 VersionedRegistry<T>、工具/Hook 强类型外壳；register/replace/unregister/snapshot/revision/subscribe/close 保持工具语义。build_request_tool_service 每请求一代 hook 快照。
- [ ] 准备插件状态变更与 user hook 热重载并发 fixture：统一写入口，一次 revision、完整上一代或下一代，保留原 try_read/read 测试；close/lag/replaced registration 不误报成功。
- [ ] 修 System 自封：用户/插件不能进入 System；插件 High 未授权降级并可见；这项显式安全变化是“纯重构 PR”的例外，独立审查，不谎称所有行为不变。
- [ ] 验收改写链最终输入实际经过 deny/operator/confirmation；ask 无通道=NobodyAsked；original/final 审计保留；Hook ownership 用现有 tree，无第二份 owner map。
- [ ] 运行 Task 10 验证集+实际工具/hook/lifecycle guard；依赖 P0，不改变 harness。

### Task 12: P1 unified tools & Claude Code

**Files:** `src/tools/handlers/`、`src/extension/{plugin_ops,lifecycle}.rs`、`src/tool_metadata/registry/`、现有 manifest/json_output 与兼容 integration fixtures。
**Interfaces:** PluginToolHandler 复用 call_plugin_tool 与既有沙箱/credentials；ToolCatalog 只为 Registry projection。
- [ ] 先固定真实热门 Claude Code 插件版本/内容哈希/许可证测试夹具，再断言 hook 改写、阻断与 skill 可用的真实效果；新增资源免替换授权。
- [ ] 注册/卸载/替换后 Registry 与 Catalog 同代；invoke 只走唯一 Registry；第三方输出 fence=true；不能复制 ExtensionManager 状态。
- [ ] 运行插件 registry 集成+兼容效果测试+Task 10 验证集，禁止以“加载成功”代替效果验收。

### Task 13: P2 resolver, minimal grants & contract freeze

**Files:** `src/capability/` resolver、config slots/live_apply、`src/extension/{plugin_trust,plugin_state}.rs`/plugins 配置、`shared/protocol/src/hook.rs`、API 契约。
**Interfaces:** 原方案 SlotResolver→纯 SlotPlan；仅 routing/config，不替 LLM 选择恢复策略。P5 授权管理依赖倒置：**P2 先实现最小 grant enforcement**，P5 才完善 UI/审计管理。
- [ ] 先冻结 grant 记录 content hash+item，默认拒绝与加载 trust 独立；无批准通道/无人值守不得授予；过期/撤销/更新 hash 的认领立即失效。兼容清单不能隐式认领单槽。
- [ ] 测五层覆盖（session/project/user/claims/builtin）、同层 single 冲突 Conflicted、disable、version mismatch；不把 conflict/denial 的错误值消费成许可。
- [ ] `fallback_never_bypasses_security`：安全拒绝/revoke/失效 lease/审批失败/Interceptor deny 即使 fallback=builtin 也 `assert_eq!(builtin_calls,0);`。仅普通 execution error/timeout 且显式 builtin 才可回退；对 builtin 自身新 lease 重新完整准入/审批/并发/fence，不继承旧准入；`fallback_reenters_builtin_admission`：`assert_ne!(new_lease,old_lease); assert_eq!(admission_count,2); assert!(audit.fallback); assert!(output.fenced);`。embedding 任意错误均 `assert_eq!(fallback_calls,0);`，错误给模型，不由 harness 挑恢复策略。
- [ ] 测热换旧 lease 失效、旧调用可跑完、新调用新代；SearchHandle 重建失败保留旧代并如实 Restart；共享 live_apply 不另写配置入口。
- [ ] 消费 P0 已冻结 Hook@1 的 strict decode/response validator；生产 adapter 另外验证 slot schema、grant/lease、Core metadata 保留与 Observer 无效果，`post_business_rewrite_preserves_authority`：`assert_eq!(after.business,new_value); assert_eq!(after.served_by,before.served_by); assert_eq!(after.generation,before.generation); assert_eq!(after.safety,before.safety);`。破坏性 wire 升级保留旧适配器至少一个 CalVer 发布周期。
- [ ] 运行 resolver/grant/live_apply/lease 效果集成+Task 10；记录 SlotPlan 是 projection/配置解析结果，不替代 Registry/SessionEventStore 权威。

### Task 14: P3 behavior & non-tool slots

**Files:** thinker prompt layers/contract/budget、extension hook adapters、search providers/handle、browser tool registration、memory embedding/index rebuild；不编辑 harness。
**Interfaces:** prompt replaceability 默认 Pinned；插件独立预算；已有 stop verifier seam 承载 turn.continue，资源 scope 只作并发约束。
- [ ] 全量普查层及渲染点写 guard：固定六层至少包含 security/operating_envelope/protocol_tokens/tool_runtime_state/runtime_capabilities/memory_protocol；replaceable 初选 soul/strategy/guidelines/language/citation_standards/skill_instructions，逐层安全审查后批准。新增层默认 Pinned。
- [ ] 测固定层替换拒绝、可替换层 grant、核心顺序不变、插件预算不能挤占核心；reachable_layers/scaffold_bytes_ratchet/no_sentence_is_stated_twice 保留适用边界。
- [ ] compaction 工具配对/会话标记失真时拒绝摘要并用 builtin+审计；turn.continue 每用户回合默认最多 3 次，可配置、不能由插件重置，到限向模型/用户报告；不加入 harness 恢复策略。
- [ ] MCP/WASM 搜索、HTTP/MCP embedding 每个至少一条真实效果 E2E；browser MCP 经 adapter→统一 manager+registry，覆盖 required 全动作才认领，缺能力 `assert_eq!(active_claims,0);`，evaluate fixture `1+1` 实际结果 `assert_eq!(value,"2");` 而非脚本回声，不直接工具组接管。
- [ ] `embedding_same_dim_new_space_rebuilds`：fingerprint 含 provider/model/revision/normalization 等语义空间事实，不只 dimension；同维度不同 fingerprint 也新索引回填+原子 provider/index 配对切换。`assert_ne!(new.fingerprint,old.fingerprint); assert_eq!(during.query_pair,old_pair); assert_eq!(failed.query_pair,old_pair); assert_eq!(committed.query_pair,new_pair); assert_eq!(new_provider_queries_old_index,0);`；失败保留旧 provider+旧索引，绝不新 embedding 查旧索引。
- [ ] 验收 memory.ingest_filter、session.lifecycle observer-only 与 prompt/tool/compaction/continue 全行为目录，不以只做工具槽位宣布 P3 完成；运行相关 E2E+原 safety guards。

### Task 15: P4 default distro & directional guard

**Files:** 拟新增 `crates/aleph-distro-default`；server startup 组装调用；gateway interfaces/search providers 的依赖方向 guard。
**Interfaces:** 原方案 `register_defaults(&mut SlotTable)` 是设计示意，SlotTable/run/provide 不是已核实公共 API；先批准 binary↔core↔distro 无环组装契约，不照抄无法链接的示例。
- [ ] 审查 crate DAG，官方二进制始终链接全 builtin；冻结 start 文件只换注册调用，不结构性重构，不改变现有 ProjectionHost mount 顺序。
- [ ] 默认功能/slots 快照与基线 diff 为空（缺凭据能力显示 Unconfigured）；native 编译进去不等于选中，必须 config 显式 use。
- [ ] 依赖方向 guard 用 AST/import 解析覆盖 alias/glob/qualified path/re-export；越界样例必须报错，合法 backend I/O/协议引用不得误杀；不得靠字符串 grep 自证。
- [ ] 运行默认二进制功能 QA+Task 10；独立测编译时间，P5 后是否拆实现 crate 再决策，不许宣传未测收益。

### Task 16: P5 observation, grant management & audit

**Files:** CLI/RPC/tool I/O、Leptos Panel views、会话状态投影、plugin grant store/audit、tool output metadata。
**Interfaces:** slots list/doctor/set/reset、hooks list、plugin grant/revoke/status；slots.list/slots.set 工具；授权本身不得模型调用。UI 只消费投影，不新增业务权威。
- [ ] 继承 P2 默认拒绝/hash enforcement，完善持久化 grant/revoke/失效审计（slot/source/hash/item）；不因 trust.enforce=false 开放高权限。
- [ ] 实际渲染所有 Active/Fallback/Failed/Conflicted/Disabled/Unconfigured；doctor 预埋冲突/hash/version/unauthorized/fallback 次数；hooks list 展示真实执行顺序与降级。
- [ ] served_by（slot/backend/generation）到每次工具结果、错误、CLI、Panel、会话状态；对每份展示指出渲染调用行，不能只有日志或 serializer。
- [ ] 真机核对 set/reset 走工具/live_apply，grant 只人类授权；验证 CLI/Panel/session 三表面+审计，运行 UI/RPC/integration 的阶段计划命令。

### Task 17: P6 ecosystem & distribution examples

**Files:** 拟新增 WASM `aleph-plugin-sdk`、npm `@aleph/plugin-sdk`、distro template、pi manifest/resource adapter、Node/Bun sidecar、marketplace npm source 与 QA fixtures。
**Interfaces:** 独立进程 jiti TS/JS+stdio JSON-RPC+唯一 HookEnvelope；无 V8/deno_core/dlopen/Lua 新架构，无猜造 Bridge::register_extension。
- [ ] pin pi package 版本/源码 hash，生成真实 API A/B/C 覆盖报告；已核实 pi before_agent_start 返回 `systemPrompt?: string`、turn_end boundary 可 continue；omp systemPrompt 为 string[] 且 turn_end notification-only，不能混用。forceSystemPrompt 只称语义别名，不写成公共 API 字段。
- [ ] A 档 registerTool/tool_call/tool_result/before_agent_start/session lifecycle/command/MCP/message 逐项真实 fixture 证明效果；先核实每个字段/入口确属 pinned pi，缺失 API 报 unsupported，不发明 shim 能力。B 档按需求排期；C 档 TUI/shortcut/themes/virtual model 明确不支持。
- [ ] npm 安装 hash 绑定包含锁解析结果；升级高权限失效；资源 skills/prompts 无 runtime、themes 明确忽略；按需 runtimes 下载 Node/Bun，缺运行时/版本不兼容给可见失败。
- [ ] sidecar timeout/crash：Interceptor fail-closed、Observer 只记录；退避重启计数/原因可见；默认 sandbox 文件/网络权限，提升逐项 grant；真机验证三平台隔离，不声称有 100% 生态兼容。
- [ ] 搜索/检索/工具/hook 四插件从市场安装→授权替换→热换→回退/reset 全链真机 QA；WASM/npm SDK 与自定义 native 模板独立编译、无 core reverse edge。原方案 distro 的 anyhow/组装依赖是发行版例外，不污染“第三方实现只依赖 API”。

## Remaining Approval Gates / 必须确认的剩余取舍

- 整体实施授权已获得，不重复索取阶段性确认；metadata/outcome/helper 闭包如超出已批准轻量依赖仍须单独重新评估，不能用删固有方法规避。
- 计划未提交；方向已定（严格 Hook、纯 AI/browser、browser MCP manager adapter）。Task 0 QA scope 已授权且正在实施，未经可信 real positive 不运行 cargo。Tasks 5/7 的纠正后 source closure/调用者边界已由 main 关闭（见 SDD implementation-contract/source-review/match-boundary），不是源码实现验收；Task8 原报告未接受，独立 source-review 仍为 P0 写/冻结前 gate；P0 通过还需全 coverage 非零验收，不宣称 implementation-ready。AI legacy bridge 在 P3 前另独立 pure adapter 计划；P0 @1 不等于 ABI/生产 bridge 完成。

| 新依赖 / Dependency | 范围与必要性 / Rationale | Approval |
|---|---|---|
| sha2/hex/chrono/tokio(sync)/http/bytes | 已批准契约闭包，不扩 backend/runtime | 已批准 |
| futures-core | sink+std Future port 无需；仅未来证明 stream closure 必需时申请，不能升级为 reqwest/futures 全栈 | 未申请/未批准，不在 P0 加入 |
| syn（dev-only） | Task 9 AST import fixture/guard 若现有 parser 无法复用，限定测试，覆盖 alias/glob/qualified/re-export | 已条件批准；仍须先确认现有 parser 可否复用，不进入生产 API |
| url2.5 / tracing0.1 | Task7 现有 URL parser / response diagnostics 单份闭包；不扩 HTTP/backend | 用户已批准复用 |
| 其他 message/reasoning 闭包依赖 | Task 7 inventory 必须列类型/方法与来源；不把未知依赖当批准 | 未知新依赖仍需证明/批准 |
- P3 全层普查后的 replaceability 表、插件预算数值与默认续跑 3 次需真实行为校准；未定值不在 P0 偷实现。
- P4 无环发行版 builder 及 native 组装 API 必须另行批准；当前仅确认上列真实类型 builders，不承诺 SlotTable/provide/run 已存在。
- P6 pinned pi/npm 版本、真实插件测试夹具、sandbox 平台保证与分发策略需阶段审查；omp memory/compaction 配置不当作 pi API，sharpshooter 未核实不承诺。

## Planning-stage Self-review / 计划修订阶段自审（历史记录）

以下记录计划修订阶段的验证与未做项，不代表后续实施进度；当前授权见 GlobalStatus，续接与真实测试见 SDD ledger。

- 覆盖：§0–3/8/9→Tasks 1–4/7–10/12/15；§4/5/12→13/16；§6→5/6/11/14；§7→14；§10→12/17；§11→16；§13–16→DAG/global/gates。P0=10、后续=7，共17，AiProvider/browser 纯契约与外部 mock 纳入；生产 bridge gate 未关闭不宣称全接线。
- 类型检查：API 包命名统一，ToolSource 与 metadata ToolSource 分域单定义；protocol→API→core 单向；固有方法返回类型、宏、Value、Presentation、sync 和审批默认闭包均有验收步骤。
- 验证纪律：本轮先完整读324行计划（含50KB截断后的末3行），再8次 bounded source读取/搜索（backend、types、manager、tab_registry局部、producer_census局部、memory脚本、两次QA路径查找）；未重新全仓扫描、未读 skill。backend 38动作+effect 全列表已读，DTO/schema闭包仍有显式硬 gate，AI/Hook事实闭包也未认证。所有测试/编译命令是**未来需实测命令，不是已通过结果**；整体文档待批准、memory blocked、P0未完成。
- ReviewFocus 的短拟新增测试：Task 2 `refusal_header_beats_status_hint`：`for hint in ["401","429","timeout"] { assert_eq!(refused(hint).kind(),Permission); } assert!(!Repeated.admits_ladder());`；Tasks 2–4 `legacy_identity_and_serde_defaults`：`let _: api::ToolOutput = old_output; assert_eq!(old_json,new_json); assert_eq!(decoded.reason,None);`；Task 4 `cancel_before_subscribe_is_observed`：`token.cancel(); assert!(late_receiver.recv().await.is_ok()); assert!(state.take_receiver().is_some()); assert!(state.take_receiver().is_some());`；Task 9 mutation 入口见 fixture 步骤；Task 10 `external_manifest_has_only_api`：`assert_eq!(normal_dependencies,["aleph-slot-api"]); assert!(!normal_tree.contains("alephcore"));`。代码为测试断言片段不是已有函数，setup 使用各任务现有真实类型，非编造 constructor。
- 实际验证 / Actual validation：主 agent 执行既有内存探针 `--self-test`，结果 `self-test OK`；真实 Darwin probe 退出 2（unsupported）；只读 vm_stat 实验的 free-page 下界低于门槛，不证明完整 available 内存不足，也不允许据此宣称编译前置通过。git status 仅多出本计划，三个原用户文档未动，index 无改动。
- 未做 / Not done：未改原方案/其他用户文档/产品或 QA 源码/AGENTS/CLAUDE；未运行 Cargo 构建/测试、安装、stage 或提交；未执行图重建、跨平台 QA、性能测量、真实插件兼容验证。产品任务未开始，只有本计划供审阅。
