# Core Panel bytes admission design / Core Panel bytes 准入设计

Date / 日期: 2026-10-31

Status / 状态: **Approved on 2026-10-31; implementation in progress under the Core byte-admission plan. / 2026-10-31 已批准，按 Core byte-admission plan 实施中。**

Task / 任务: todo #7, dependency of the [live publication plan](../plans/2026-10-30-panel-community-assets-live-publication-plan.md). Builds on the [production audit](../../reports/panel-community-live-publication-audit.md).

## Intent and success / 意图与验收

Resolve the missing Core-owned producer: an admitted Panel declaration must refer to immutable, exact SRI-verified bytes from its lifecycle-owned plugin and carry its internal visibility scope. Discovery and later byte reads must observe the same committed generation. The user selected **retain Core scope**: reuse `visibility::visible_to` for both; without trusted project context, expose Global only.

补齐缺失的 Core producer：已准入 Panel declaration 必须关联其 lifecycle-owned plugin 的 immutable exact bytes（经 SRI 验证），并保留内部 visibility scope。discovery 与后续 bytes 读取必须观察同一 committed generation。用户已选择 **保留 Core scope**：两者共用 `visibility::visible_to`，没有可信 project context 时只可见 Global。

Success for #7 means a real lifecycle transition produces and replaces that state, with runtime tests for bytes, scope, withdrawal, version replacement and revision consistency. A declaration-only map or a test-only producer does not complete #7. HTTP authentication, route registration and browser credential transport remain tasks #2–#5; #7 does not claim their safety or completion.

#7 的成功标准是真实 lifecycle transition 能产生并替换该 state，且 bytes、scope、撤回、跨版本替换及 revision 一致性有 runtime 测试。只有 declaration map 或 test-only producer 不算完成。HTTP 认证、route 注册和浏览器凭据仍属于 #2–#5，本设计不宣称其安全性或完成状态。

## Approaches / 方案比较

1. **Global-only producer.** Smaller change and no project-context ingress, but project-scoped plugins lose Panel support and discovery/bytes would have a special scope rule. Not selected. / 只发布 Global，实现较小，但 project plugin 失去 Panel 支持，且会产生特殊 scope 规则；未选择。
2. **One Core publication state retaining scope — selected.** Move the existing byte store into Core and extend its records with internal scope. Derive discovery and bytes from that one state. Costs a store migration and snapshot API adjustment, but preserves project isolation without another registry/trust/revision authority. / **已选择**：把既有 store 移入 Core，在 record 内保留 scope，从同一 state 派生 discovery/bytes。代价是 store 迁移与 snapshot API 调整，收益是不新增 registry、trust 或 revision authority。
3. **Gateway-owned filesystem publisher.** Rejected by the hard boundary: it would reconstruct admission from roots/manifests/client paths and make Gateway a second authority. / Gateway 从 filesystem 自行发布违反既定边界，会重建 admission 并成为第二 authority，不采用。

## Ownership and dependency direction / 所有权与依赖方向

Move `PanelAssetPublicationStore` and its existing identity, declaration validation, SRI and exact-byte logic from `src/gateway/server/panel_asset_route.rs` into a Core-owned module under `src/extension/`. This is a relocation and extension, not a second implementation. Preserve and repair its meaningful tests. Gateway can depend on the Core facade; Core must not import `gateway::server`. Shared wire types remain in `aleph_protocol::panel`; WASM continues depending on Protocol, never Core.

把既有 store、identity、declaration 校验、SRI 与 exact-byte 逻辑迁至 `src/extension/` 下的 Core module，是迁移并扩展，不是第二实现；保留并修复有意义的测试。Gateway 依赖 Core facade，Core 不依赖 `gateway::server`。wire 类型继续位于 Protocol，WASM 只依赖 Protocol，不依赖 Core。

Replace the current independently stored `panel_snapshot` with one lock-protected Core publication state. It contains the sole committed revision and the existing store's records; each record carries its admitted declaration, immutable bytes and `ScopeKey`. Snapshot declarations are read models derived from those records. Do not add a separate scope map, byte registry, trusted-plugin list or revision counter. Diagnostic outputs may be retained separately because they do not authorize publication.

用单一 lock-protected publication state 替代独立存储的 `panel_snapshot`：只持有一个 committed revision 与既有 store records，每个 record 包含 admitted declaration、immutable bytes 与 `ScopeKey`。snapshot 是这些 records 派生的 read model；不另建 scope map、bytes registry、trusted-plugin list 或 revision counter。诊断可以单独保留，但不参与授权。

The Core facade remains owned by `ExtensionManager`, backed by `Views`. No generic publication tool or external write API is introduced: this producer is part of existing lifecycle projection, not a new externally composable surface. `panel.capabilities.snapshot` remains the discovery transport. No Zahir runtime mount is involved.

facade 仍由 `ExtensionManager` 持有，`Views` 支撑。不新增 generic publication tool 或外部 write API：producer 是既有 lifecycle projection 的组成部分，不是新的外部可组合入口。discovery 继续使用 `panel.capabilities.snapshot`，不涉及 Zahir runtime mount。

## Lifecycle admission and publication / Lifecycle 准入与发布

`lifecycle::after_transition` remains the sole caller of `republish_plugin_projections`, serialized by the existing `load_guard`. Refactor the derivation so it clones the necessary registry facts under the existing read lock, then releases that registry lock before filesystem I/O. Other plugin-owned projections keep their existing behavior.

保留 `after_transition` 唯一调用 `republish_plugin_projections` 的 chokepoint，并沿用 `load_guard` 串行化 lifecycle writers。在既有 registry read lock 内取得必要事实，释放该锁后才进行文件 I/O，其他 plugin projection 行为保持原状。

For each nonduplicate candidate accepted by the existing `derive_panel_declaration` / snapshot validation:

- Use only `PluginRecord.panel.entry` from the lifecycle record. Never use generic plugin entry, browser declaration, client path, URL or client-selected revision. / 只使用 lifecycle record 的显式 `panel.entry`，不使用 generic entry、browser declaration、client path/URL 或 client-selected revision。
- Resolve the server-owned record's root and the validated relative entry; canonicalize both, require the entry to remain under that root and be a regular file. Read only that one file. Do not expose either path in declarations, byte results or diagnostics. / canonicalize server-owned root 与 validated entry，要求文件仍在 root 下且为 regular file；只读一个文件，root/path 不进入 declaration、bytes result 或诊断。
- Bound the read, hash those exact bytes with the existing SHA-256/SRI validator, and admit only a match. Retain immutable bytes in memory; reads never reopen a path. / 有界读取后复用 SHA-256/SRI 校验，只准入匹配 bytes；内存保存 immutable bytes，读取不再打开文件。
- Keep the record's existing `ScopeKey` internal. A failed file read, containment/size/SRI check or invalid/duplicate declaration omits that Panel record from the new generation and emits a fixed diagnostic code. It does not block the plugin's other capabilities. / 内部保留 record scope；file、containment、size、SRI、invalid/duplicate 失败则在新 generation 中省略该 Panel，并记录固定 code，不阻塞该 plugin 其他能力。

Use bounded reads of **4 MiB per entry** and **64 MiB of admitted module bytes per generation**, with canonical key order making budget admission deterministic. These are proposed implementation defaults chosen by the assistant, not pre-existing limits or user requirements. Oversized/over-budget records are omitted with a fixed code; no new configuration or HTTP knob is added. The builder may optimize ownership of buffers but must retain these bounds and exact-byte semantics.

建议限制为每 entry **4 MiB**、每 generation 已准入 module bytes 总计 **64 MiB**，按 canonical key 顺序确定 budget admission。这些是 assistant 提议的默认值，并非既有规则或用户要求；超限记录省略并使用固定 code，不新增配置或 HTTP knob。builder 可优化 buffer ownership，但须保留限制与 exact-byte 语义。

Build a replacement state privately, then swap the entire state under one write lock. Readers see the old complete state or the new complete state. A transition has completed its Panel publication only after this swap. A failure in a newly declared entry must not retain the old entry as fallback. Filesystem mutation after admission cannot alter returned bytes; they change only through a later successful lifecycle publication. Canonical containment and SRI checks do not claim a general sandbox against an adversarial process rewriting the filesystem while it is read.

先构建完整 replacement state，再在一个 write lock 内整体 swap；读者只看到完整旧 state 或完整新 state。Panel publication 的 transition 完成点就是 swap。新 entry 准入失败不能回退到旧 entry。准入后的 filesystem 变动不改变返回 bytes，只能由后续 lifecycle publication 更新。canonical containment 与 SRI 不宣称可通用防御另一个恶意进程在读取期间重写 filesystem。

Keep `derive_panel_declaration` as the declaration/trust validation source; do not duplicate it in the producer. Perform final admission validation against the policy observed for that publication, with a short policy-read/commit lock ordering so a stale off-lock derivation cannot overwrite a newer trust-policy observation. No lock is held across I/O. Trust remains the established **load/admission gate**: `set_owner_trust_policy` / `set_plugin_trusted` do not tear down already admitted running plugins; disable/reload or another existing lifecycle publication applies the changed policy to Panel admission. This preserves `plugin_ops` semantics and does not promise instantaneous untrust revocation. This preservation is the assistant's proposed compatibility decision for approval.

继续以 `derive_panel_declaration` 作为 declaration/trust 判据，不在 producer 重写。commit 前以该次 publication 观察到的 policy 完成最终准入校验，使用短时 policy-read/commit 的一致锁顺序，避免旧 off-lock 派生覆盖较新 policy observation；I/O 期间不持锁。trust 沿用既有 **load/admission gate**：setter/trust 操作不即时 teardown 已运行且已准入 plugin，disable/reload 或既有 lifecycle publication 将新 policy 应用到 Panel admission。因此不承诺即时 untrust 撤销；这是 assistant 提议、待本设计批准的兼容性决定。

## Revision, replacement and withdrawal / Revision、替换与撤回

Allocate one new revision from the committed Core state per existing lifecycle publication. Every admitted declaration has that revision, including unchanged entries. A scoped snapshot retains the same revision after filtering. Byte reads check the current state and its canonical identity/entry; they do not accept an independently chosen revision as authority. Revision arithmetic must not wrap; an allocation failure must refuse publication reads rather than leave a stale generation available as current.

每次既有 lifecycle publication 从 committed Core state 分配一个新 revision，所有 admitted declarations（包括未变化 entries）使用它。scope 过滤不另造 revision。bytes 读取只匹配当前 state 的 canonical identity/entry，不把独立输入 revision 当权威；revision 不可回绕，分配失败须拒绝 publication reads，不能把旧 generation 继续冒充 current。

Whole-state replacement removes every omitted/disabled/unloaded/untrusted-on-republication tab and every old plugin version. It also prevents stale reinsertion after retraction because consumers cannot write records and only the lifecycle writer builds the next state. No growing tombstone/history table is needed for this process-local read model. Existing low-level replacement checks remain useful but are not the current-generation authority. A read that already obtained bytes before withdrawal may finish; reads linearized after withdrawal refuse. Browser cancellation and late-result cleanup remain separate loader responsibilities in task #4.

整体 replacement 移除被省略、disable/unload 或重新 publication 时不再受信任的 tab，并移除旧 plugin version。consumer 不能写 record，只有 lifecycle writer 构建下一 state，因此撤回后不能 stale reinsertion，不需要不断增长的 tombstone/history table。既有低层 replacement 检查仍有用，但不是 current-generation authority。撤回前已取得 bytes 的读取可完成，撤回后线性化的读取拒绝；browser cancellation 与 late-result cleanup 仍由 #4 处理。

Revision remains process-local; no persistence, replay, background refresh or notification channel is added. / revision 仍是 process-local，不新增持久化、replay、background refresh 或 notification channel。

## Discovery and byte-read contract / Discovery 与 bytes 读取契约

Provide narrow Core reads of the current scoped snapshot and an exact published asset, each receiving an internal `VisibilityCtx`. Both call the existing `visible_to` against the record's retained `ScopeKey`. A byte lookup returns immutable bytes plus the admitted declaration from one state read; no root/path, token, manifest or editable record is returned. Asset selection uses a validated logical tab identity and exact current canonical entry; version/revision comparisons may reject stale expectations but never select an older state. Generic absence/refusal is sufficient for callers.

提供接收内部 `VisibilityCtx` 的窄 Core scoped snapshot / exact asset reads，两者对 record 的内部 scope 使用既有 `visible_to`。bytes lookup 在同一次 state read 中返回 immutable bytes 与 admitted declaration，不返回 root/path、token、manifest 或 editable record。selection 只使用 validated logical tab identity 与当前 canonical entry；version/revision 比较只能拒绝 stale expectation，不能选择历史 state；对 caller 使用 generic absence/refusal 即可。

Keep `panel_ui_capability_snapshot()` as a compatibility wrapper using **`VisibilityCtx::default()`**, not `from_project_root(None)` (which falls back to daemon CWD). The existing contextless `panel.capabilities.snapshot` handler therefore becomes Global-only. Add no request parameter that turns a client path into a trusted project. Trusted project-bound Gateway context is an explicit task #2 dependency; until then project-scoped state exists in Core but is unavailable through that contextless transport.

保留零参数 snapshot API，但使用 `VisibilityCtx::default()`，不用会回退 daemon CWD 的 `from_project_root(None)`。既有无 context RPC 因而只返回 Global；不新增把 client path 变为 trusted project 的 request parameter。可信 project-bound Gateway context 是 #2 的明确依赖；此前 Core 可以保留 project state，但无 context transport 不暴露它。

`VisibilityCtx` is a Core visibility input, **not an HTTP authentication proof**. Do not promote the old caller `authorized: bool` into this facade. As part of moving the store, remove that boolean from its data-read API; the unmounted Gateway module may retain only a thin Core re-export/adapter as needed. This mechanical removal does not finish #2: a production route must still create an authenticated caller, derive permitted project context from server-owned state and repeat Origin/TLS/client/rate-limit checks. No route is registered in #7.

`VisibilityCtx` 仅是 Core visibility input，**不是 HTTP 认证证明**。不能把旧 `authorized: bool` 搬成 facade 授权。迁移 store 时移除 data-read API 的 boolean，未挂载 Gateway module 只按需保留薄 re-export/adapter；该机械移除不代表 #2 完成：route 仍须创建 authenticated caller，从 server-owned state 派生其许可 project context，并重复 Origin/TLS/client/rate-limit 检查。#7 不注册 route。

## Tests and verification / 测试与验证

Each implementation task must run a minimal failing contract test before production changes, confirm the actual failure reason, then run the passing test. Repair the known stale-record fixture so the stale attempt has valid matching SRI; otherwise it tests integrity failure rather than revision refusal. Do not claim generic-field tests unless such input is actually supplied.

每个实现 task 先跑最小失败契约测试，确认实际失败原因再改 production，并验证 GREEN。修复已知 stale fixture：stale attempt 必须具有匹配 SRI，否则只覆盖 integrity 失败。未实际输入 generic fields 时不得宣称相关测试覆盖。

Required runtime cases / 必需 runtime cases：

- Lifecycle-owned temporary plugin root with explicit Panel entry yields the exact file bytes, matching SRI, declared identity and one source revision. A different generic plugin entry cannot influence the result. / 临时真实 plugin root 的显式 Panel entry 产生 exact bytes/SRI/identity/revision，generic entry 不影响结果。
- Missing file, bad SRI, invalid/escaping entry, directory, duplicate key and size/budget failures remove the Panel record without exposing roots or breaking other plugin capabilities. Symlink containment is tested where the platform permits creating links, with an explicit skip otherwise. / 错误与超限省略 record，scope/root 不泄漏，不影响其他能力；symlink 在允许创建的平台验证，否则记录 skip。
- Global and two project scopes: snapshot and bytes agree for Global-only and each trusted internal context; unknown/missing context cannot read either project's record. Contextless RPC returns Global only even when the daemon CWD is a project. / Global/两个 project 验证 discovery 与 bytes 一致，无/未知 context 不读 project，RPC 不借 cwd 扩权。
- Disable/unload, failed reload, tab change and cross-version replacement withdraw old records; old revision/entry expectations refuse, including after retraction and attempted stale low-level publication. / 验证 disable/unload、失败 reload、tab/跨版本替换及撤回后的 stale publication。
- Concurrent state readers observe complete generations, with declaration/bytes/SRI/scope aligned; a file changed after admission does not change bytes until a successful republish. / 并发 reader 不见混合 generation，准入后改文件不改变已发布 bytes。
- Existing built-in Terminal source/projection/registry/loader tests remain green. No fixture is relabeled as Gateway/browser evidence. / Terminal 既有测试保持绿色，不把 fixture 冒称 Gateway/browser 证据。

Run `python qa/browser_live/check_memory.py` from the worktree root before every Cargo invocation; run Cargo serially from the owning package directory. Record real failures, RED/GREEN logs, skipped checks and review outcomes in the tracked run ledger. Update the audit/reference status only after runtime proof. Keep this worktree/branch local with conventional commits; no merge/push.

每次 Cargo 前从 worktree root 跑 memory guard，Cargo 从 owning package directory 串行运行。tracked ledger 保留实际失败、RED/GREEN、skip 与 review；有 runtime 证据后才更新 audit/reference 的完成状态。使用 conventional commits，保留本地 worktree/branch，不 merge/push。

## Planning clarifications from builder check / Builder 检查补充裁定

The following answers settle the builder's eight material questions using current source or explicit assistant design calls; no new user preference is inferred. / 以下据当前源码与显式设计裁定回答 builder 的八个问题，不把 assistant 决定冒称用户偏好。

1. **Identity and scope.** `registry/plugin_registry/mod.rs::PluginRegistry` stores `HashMap<String, PluginRecord>` keyed by plugin ID; `register_plugin` replaces the prior row for that ID. There is one live record per ID, not simultaneous installations of that ID across scopes. Preserve this rule. Different project-scoped plugins can coexist under different IDs. A root/scope/lifecycle-identity change for the same ID replaces its publication, receives the next lifecycle revision and withdraws the prior scope's bytes. Do not widen the registry key or implement multiple installations as part of #7. / registry 以 plugin ID 唯一建模，重复 ID 替换旧 row，不支持同 ID 多 scope 同时安装。不同 ID 可分别属于不同 project；同 ID root/scope/lifecycle identity 变化是 replacement，必须新 revision 并撤回旧 scope bytes；#7 不加宽 registry key。
2. **Context construction.** #7's scoped facade is crate-internal and accepts a canonical `VisibilityCtx` from trusted in-process code; it is not a wire-constructed claim. A known project root uses the existing canonicalization derivation with `Some(root)`; absence uses `VisibilityCtx::default()`. Unknown internal project roots match no retained project scope. The later Gateway producer must resolve a server-bound project against the existing `ProjectStore` and its caller authorization, then use its canonical `Project.workspace_path`; a client root or bare project ID is insufficient. Existing `projects::authz::project_for` is a candidate derivation, but its unrestricted `actor: None` arm must never be used to compensate for missing Panel authentication. Missing/unresolvable/stale **claimed** project context must refuse rather than silently falling back to Global; a caller with genuinely no project gets Global-only. Implementing that authenticated constructor is #2, not #7. / #7 scoped facade 只限 crate 内部，可信代码提供 canonical context，不能从 wire 造 claim。已知 root 使用既有 Some(root) canonicalization，真正无 project 使用 default；内部未知 root 不匹配任何 project scope。后续 Gateway 必须用 server-bound project 经既有 ProjectStore/caller authorization 取得 canonical workspace_path，不能仅信 client root/ID，也不能把 authz 的 actor=None unrestricted 分支用于缺失认证。声称但无法解析或已失效的 project 必须拒绝，不能悄悄降为 Global；该认证 constructor 属于 #2。
3. **Failure categories.** Entry I/O, containment, integrity, size, budget, declaration and duplicate failures omit records and still commit the next complete generation (possibly empty). Revision-allocation failure is generation-wide: atomically mark the state unavailable, clear publishable records and refuse byte reads; do not retain stale bytes. The compatibility snapshot wrapper returns no declarations in that state, while the internal read exposes the refusal. Do not wrap the counter or invent another one. Process termination/allocation failure is not an in-process recovery promise. / entry 级错误省略记录，仍提交可能为空的完整新 generation；revision 分配失败整体进入 unavailable，清空可发布 records 并拒绝 bytes，compat snapshot 无 declarations，内部 read 显式 refusal；不回绕或另造 counter，不承诺进程终止/OOM 自愈。
4. **Mutation visibility.** No external/direct mutation facade is retained. Store construction, `publish`, `retract` and replacement are private to the Core publication builder/owning module, which is reached in production only through lifecycle projection. Unit tests in that module may exercise lower-level validation, but the acceptance tests must also exercise real manager lifecycle transitions. A test-only reinsert cannot change a manager's committed state. / 不保留外部/direct mutation facade；构造、publish/retract/replace 只在 Core publication builder/owner module 内可用，生产仅由 lifecycle projection 到达。模块单测可测低层验证，验收必须另测真实 manager lifecycle，测试重插不能改已提交 manager state。
5. **Budget accounting.** Count every admitted record's byte length independently, even when byte contents happen to be identical. No cross-record deduplication or shared-content identity is introduced. Sort by the canonical logical key before admission, with a stable deterministic tie rule that rejects duplicate keys as a group. / 每 record 按自己的 bytes 长度计费，相同内容也独立计算；不加内容去重或共享 identity，按 canonical key 排序，重复 key 整组拒绝。
6. **Revision trigger.** Every existing invocation of lifecycle `after_transition` that reaches Panel publication commits a new revision, even if the Panel set is unchanged/empty or the transition concerns another capability. This preserves the existing projection trigger. No content-change detector, polling trigger or extra writer is added. / 每次既有 after_transition 到达 Panel publication 就提交新 revision，集合未变/为空或仅其他能力变化也一样，保留既有 trigger，不加 content-change 检测、polling 或第二 writer。
7. **Internal API compatibility.** The old Gateway store symbols are `pub(crate)` and the route is unmounted; no public API compatibility is promised for them. Update internal callers/tests and remove obsolete exports freely. Keep a thin adapter only if a real internal consumer needs it; do not leave a second store, boolean admission constructor or writable compatibility facade. Shared protocol shapes and the existing zero-argument snapshot method remain compatible. / 旧 Gateway symbols 是 crate 内部且 route 未挂载，可更新调用/测试并移除无用 exports，不承诺这些内部 API 兼容；只有真实 consumer 需要时才保留薄 adapter，不保留第二 store、boolean admission constructor 或可写 compatibility facade；wire shape 与零参数 snapshot API 保持兼容。

Existing lifecycle projection tests currently manufacture metadata-only records with nonexistent Panel bytes. Update them to create isolated real files and matching SRI once admission requires bytes; retain pure declaration tests as pure tests. Do not bypass byte admission to keep the old fixture passing. / 既有 lifecycle projection 测试只有 metadata、没有实际 Panel bytes，实现时须改为隔离真实文件与匹配 SRI；pure declaration 测试仍保持 pure，不为旧 fixture 绕过 bytes admission。

## Builder latitude and non-goals / Builder 可决定内容与非目标

The builder may settle exact Rust names, module splitting, buffer representation and error enums while preserving the single-owner contract and minimal dependency footprint. Proposed byte limits, load-gate trust semantics and Global-only contextless discovery are explicit design calls for approval, not hidden defaults.

builder 可决定 Rust 命名、module 拆分、buffer 表示和 error enums，但须维持单一 owner 与轻量依赖。建议 bytes 限制、load-gate trust 语义与无 context discovery 只见 Global 是显式待批准决定，不是隐含默认。

No HTTP endpoint/auth proof, credential injection, install/update/uninstall, marketplace/signature service, external fetch/WASM, multi-file dependency traversal, loader replacement, automatic selection/mount, default-release change, background refresh/live notification or Zahir runtime mount is part of #7. / #7 不包含上述 HTTP/auth、credential、安装生态、多文件与自动挂载等功能。
