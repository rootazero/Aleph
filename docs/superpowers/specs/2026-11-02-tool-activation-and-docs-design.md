# Tool 首次调用激活与文档事实重整设计 / Tool First-Call Activation and Documentation Reconciliation Design

**日期 / Date:** 2026-11-02
**状态 / Status:** 已批准并实施 / Approved and implemented
**范围 / Scope:** canonical Tool cold activation、`FEATURE_LOCATOR.md` 历史词典重整、全 `docs/` 可验证链接清理

## 意图 / Intent

本阶段关闭一个已经由源码证明的边界缺口：ExtensionManager 可以保留 Cold 的 tool-only WASM owner，也可以通过显式 boot/enable 路径完成 activation，但 Cold declaration 当前不进入 canonical callable registry；canonical handler、`PluginActivationAuthority` 和 `ReadySnapshot` 都要求 Ready。因此经过权限与 scope 的真实 Tool 调用无法触发第一次 activation。

The phase closes one source-proven boundary gap: ExtensionManager can retain a Cold tool-only WASM owner and activate it through explicit boot/enable paths, but a Cold declaration is absent from the canonical callable registry. Canonical handlers, `PluginActivationAuthority`, and `ReadySnapshot` all require Ready, so a permitted real Tool call cannot trigger first activation.

同时，`FEATURE_LOCATOR.md` 的历史词典仍把当前事实、历史证据和 deferred 方案混在一起；全 `docs/` 扫描发现 228 个缺失目标，但这些结果包含真实断链、已删除历史文件、代码示例伪链接、外部路径和占位符。文档工作需要分类后修复，不能批量伪造目标。

## 目标 / Goals

- 让 canonical Tool 的 Cold declaration 能被同一 `ToolHandlerRegistry` 表达为可发现但尚未 Ready 的 activation entry。
- 让一次已经通过 permission/scope 的真实 Tool 调用触发 owner 的 retained single-flight activation；mount、identity、owner incarnation、provider generation 和 canonical publication 全部成功后，才创建 Ready proof 并执行 guest。
- 保持 read-side `describe`、`resolve`、`catalog` 不启动 provider，不执行 WASM，不创建 guest instance。
- 保持 stop/reload 的 admission fence：关闭新调用，等待已持有 permit 的调用，清理精确旧 owner；旧 completion、lease 或 snapshot 不能影响 replacement。
- 维持现有 authority：`ToolHandlerRegistry` 是 callable source，`RuntimeOwner` 持有 declaration incarnation、scope、activation/stop task 和 receipt，`ProviderHandle` 是 provider state/generation authority，`OwnershipTree` 仍是其自身 owner generation authority，`MemoryRuntime` 不参与 Tool activation。
- 把 `FEATURE_LOCATOR.md` 重整为 current facts、historical evidence、deferred boundaries 三个明确层次，保留历史内容但不再让历史状态看起来是当前实现。
- 分类并修复全 `docs/` 中能够由仓库事实验证的相对链接；历史删除目标必须改成当前证据链接或明确标注 unavailable，不伪造文件。

## 非目标 / Non-goals

- 不新增 Capability kind、第二 callable registry、第二 owner/generation/disposer map 或 universal lifecycle supervisor。
- 不把 `NEXT_OWNER_GENERATION` 合并到 `OwnershipTree` generation；源码已经明确它是 declaration incarnation ID。
- 不把 Feishu、Channel、Terminal、Memory、MCP 或其他 deferred kind 伪装成 Tool mount；本阶段只处理 tool-only WASM。
- 不把 MemoryRuntime 改成 universal scheduler、所有 writer 的停止闸、进程隔离、native unload、RSS 回收或 exactly-once 外部效果。
- 不改变 transport compatibility 的 mounted-only 语义，除非当前 canonical dispatch 契约明确需要共享内部执行路径。
- 不把代码块、inline code、外部 URL、`url`、`about:blank` 或示例中的 Rust symbol 当成 Markdown 文件链接修复目标。
- 不重写 research/spec/run 文档中的历史证据为当前生产事实。

## 约束 / Constraints

- 冷声明和 Ready publication 必须继续由同一个 canonical registry 管理；activation entry 不能绕过 registry 的 permission、scope、publication 或 lease 检查。
- Cold entry 不得携带伪造的 `ReadySnapshot`、provider generation 或 WASM binding。Ready proof 只能在实际 publication transaction 成功后由 registry 创建。
- 首次调用必须沿用现有 sealed `PluginActivationAuthority` 设计：外部输入、transport metadata、replay 或普通 handler 不能自行 mint proof。
- caller cancellation 不得取消已经开始的 owner activation；既有 `RuntimeOwner::activate_owner` retained task 和 watch completion 语义必须保留。
- 不得持有 lifecycle、registry 或 ownership 锁跨 await/I/O；不 abort、detach、替换或手工 join 已准入生产任务。
- Rust 注释使用英文；持久文档使用中英双语。

## 方案 / Approaches

### 方案 A：同一 canonical registry 的 Cold activation entry（采用） / Cold activation entry in the canonical registry (selected)

在现有 `CanonicalToolState` 中增加 Cold declaration 的明确表示和 registry-owned receipt/dispatch 路径。Cold entry 只携带可信 declaration identity、owner incarnation、scope 和 activation callback，不携带 Ready proof。Permission/scope admission 通过后，entry 调用现有 `RuntimeOwner::activate_owner`；activation 完成后从同一 registry 读取当前 Ready entry，在当前 gate verdict 下创建 registry-owned `PluginActivationAuthority`，再执行精确 Ready handler。Cold entry 的 replacement/revoke 仍使用同一 registry transaction 和 owner receipt。

Extend the existing `CanonicalToolState` with an explicit Cold declaration representation and registry-owned receipt/dispatch path. A Cold entry carries only trusted declaration identity, owner incarnation, scope, and an activation callback; it carries no Ready proof. After permission/scope admission, it calls the existing retained `RuntimeOwner::activate_owner`. Once activation completes, it reads the current Ready entry from the same registry, creates the registry-owned `PluginActivationAuthority` under the current gate verdict, and invokes the exact Ready handler. Replacement and revocation use the same registry transaction and owner receipt.

优点是保持一个 callable source、一个 receipt authority 和现有 Ready proof；代价是 registry 需要区分 Cold declaration 与普通 compatibility entry，并增加一次 activation 后的 current-entry rebind。

### 方案 B：在 ExtensionManager 外部路由首次调用 / Route first calls outside the registry

让 executor 先从 ExtensionManager declaration index 查找 Cold owner，激活后再回到 registry。它较容易复用现有 owner，但会让 ExtensionManager 成为第二个 callable lookup authority，并使 read/resolve/call 的 source of truth 分裂，因此不采用。

### 方案 C：启动时提前 activation / Activate every eligible owner at boot

把所有 Cold owner 在启动或 enable 时提前变成 Ready。它避免了首次调用路径，但违背 lazy activation 目标、扩大启动资源和失败面，也不能证明 read-side no-start 与 first-call behavior，因此不采用。

## Rust 行为契约 / Rust behavior contract

### Read side / 读侧

- `describe`、`catalog` 和 `resolve` 可以返回 Cold declaration metadata 或明确的 deferred-callable result，但不得调用 `activate_owner`、mount WASM、创建 guest instance 或发布 Ready snapshot。
- 任何 read-side result 都必须保留 declaration identity，并在 owner replacement 后失效；不得成为可调用的 Ready lease。

### Call side / 调用侧

- canonical permission/scope gate 先于 activation；拒绝调用不启动 provider。
- 同一 Cold owner 的并发调用共享一个 retained activation task；caller cancellation 不取消 activation。
- activation 成功的唯一顺序是：`Cold → Starting(generation) → mount/identity checks → canonical Ready publication → Ready handler invocation`。
- mount、identity、owner incarnation、provider generation、publication revision 任一检查失败都 fail closed，并清理本次 exact owner scope。
- stop/reload 先关闭 admission，再等待已获得 permit 的 guest call，撤销旧 receipt，最后允许 replacement；旧 Ready proof、completion 和 lease 均不能调用新 owner。
- activation 后的 execution 必须进入现有 `with_plugin_publication` / sealed authority 路径，不能直接调用 WASM binding。

## 文档重整 / Documentation reconciliation

### `FEATURE_LOCATOR.md`

保留历史条目的原文证据，但重新组织入口：

1. **Current facts / 当前事实**：只描述当前源码已经证明的 authority、Tool cold activation、provider/owner generation 区分、MemoryRuntime 边界和实际 production wiring。
2. **Historical evidence / 历史证据**：集中保留旧轮次、已完成设计、旧的 deferred 状态，并在入口注明这些段落不覆盖 current facts。
3. **Deferred / NO-GO**：集中列出未接线的 Capability kinds、Feishu → Zahir、MCP lazy、EffectClaim、Safe Replay、universal durable scheduling 等，并链接到拥有这些结论的 reference/research 文档。

每个保留的历史主题继续保留 status 与 evidence link；current owner/link 由前置 `Current state / 当前现状` 的 authority map 和各 owning reference 文档提供，避免在 160 个历史条目中复制当前契约。无法恢复的历史文件标为 unavailable，不使用悬空链接冒充证据。

### 全 `docs/` links / 全 `docs/` 链接

扫描器只把 Markdown 链接语法中的 repository-relative target 作为文件链接候选，并跳过 fenced code、inline code、external/protocol URL、absolute/home path、placeholder 和示例字符串。每个缺失候选按四类处理：

- current target：修正相对路径或改为实际存在的 source/reference target；
- historical evidence：迁移到当前仍存在的 plan/spec/run/source evidence，或明确标注 historical artifact unavailable；
- code/example：保留代码内容，不作为 broken-link failure；
- external/placeholder：保留语义并从 repository link contract 排除，必要时补充说明。

验证输出需记录总扫描文件、原始命中、跳过项、修复项、迁移项和明确 unavailable 项；不得只报告“无断链”而隐藏分类结果。

## 证据与验证 / Evidence and validation

- 新增或修改真实效果测试：Cold declaration read-side no-start；首次 canonical call single-flight activation；permission denial no-start；activation failure cleanup；stop/reload with in-flight permit；stale old proof after replacement。
- 复用现有 `src/extension/lifecycle.rs` 的 real-WASM coverage，不复制已经通过的 boot activation、exact disposal 和普通 stale-generation tests。
- 运行受影响的 extension/capability tests、`cargo check -p alephcore --tests`、`cargo check -p alephcore --bin aleph-server`，必要时运行针对性 clippy；每次 Cargo 前执行 `python qa/browser_live/check_memory.py`，使用 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` 串行运行。
- 对全 docs link scan 运行结构化分类检查、UTF-8、尾随空白、Markdown diff check 和新增/修复目标存在性检查。
- 获取 fresh read-only review，重点审查 authority 是否重复、Cold entry 是否绕过 Ready proof、stop/reload 是否留下旧 lease、历史文档是否被误报为当前，以及链接分类是否伪造证据。

## 风险与取舍 / Risks and trade-offs

把 Cold declaration 进入 canonical registry 会扩展 registry state shape 和 resolve/read API，需要保证现有 consumers 不把 Cold entry 当成 Ready callable。最主要风险是用普通 `ready=None` entry 偷渡 activation，从而绕过插件 proof；实现必须使用显式 Cold representation 和 registry-owned rebind。链接全量清理会触及大量历史文档；分类 ledger 比单纯删除链接更慢，但能保留历史可追溯性并避免虚假修复。

## 待计划 / Remaining work

用户已批准本设计，实施已在 `docs/superpowers/plans/2026-11-02-tool-activation-and-docs-plan.md` 中完成拆解；生产修改、文档修改与验证证据记录在对应 run record。后续只需完成最终 review、账本收尾和本地 commit。
