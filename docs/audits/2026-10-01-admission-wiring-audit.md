# 准入层断线审计报告（线 2 / T7）

- 日期：2026-09-30
- 范围（只读，未写产品代码）：`src/tools/scoped/dispatch.rs`、`src/tool_output/hygiene.rs`、`src/tool_output/structured/`、`src/context/budget/cheap_passes/tool_result_pruning.rs`、`src/tools/result_processing.rs`，及链路两端的 `src/tool_output/ingress.rs`、`src/tool_output/compressor.rs`、`src/harness/agent/preflight.rs`、`src/harness/agent/turn_budget.rs`
- 方法：每个导出符号先数生产者/消费者（区分生产与 `#[cfg(test)]` 调用），再判定断线；对照 FEATURE_LOCATOR §3.14 的「刻意不做 / DEFER / 覆盖缺口」清单排除非断线
- 总体结论：**主链路每一跳都有且仅有一个生产消费者，接线健康度高**。未发现 critical / important 级断线；以下为 2 条 CONNECT（1 条边界）、1 条 CUT、3 条 DECIDE，全部低/中低风险

链路全景（普查后确认的唯一生产路径）：

```
registry 声明 (runtime.rs:370 max_result_tokens_for)
  → dispatch.rs:1587 resolve_result_budget（ceiling: start/mod.rs:3585 安装 / 3619 decline）
  → dispatch.rs:1617 run_ingress（<128KiB 内联 / ≥128KiB spawn_blocking）
       → ingress.rs:96 clean_for_ingress_of
            → compressor.rs:141 compress_result_value（仅 DevTools 族）
            → hygiene.rs:123 clean_result_value → structured/mod.rs:300 reduce_within
  → dispatch.rs:1637 apply_result_budget（offload → recovery_footer_for → ctx_search 可回收）
  → turn_budget.rs:283 record（Layer-3 spill，spill_replacement）
  → preflight.rs:324 ToolResultPruningStage（stale 通道，reduce_within(cleaned, Some(200))）
```

---

## CONNECT（2 条）

### C1. [低] `ToolResultPruningStage.min_tokens_to_prune` pub 旋钮没有任何生产者

- **证据**：`src/context/budget/cheap_passes/tool_result_pruning.rs:59-71`（pub 字段，默认 `200`）；全仓库唯一构造点 `src/harness/agent/preflight.rs:324-333` 使用 `ToolResultPruningStage::default()`；rg 全仓库无第二个赋值点。对比：同文件 `FileOpSupersedeStage::with_min_pressure_ratio` 有 config 生产者（`prevent_pressure_floor`），预算各处旋钮（`KNOB_REFERENCE_BUDGET_TOKENS=8000` tool_output/mod.rs:27、registry 声明、ceiling）都有显式来源。
- **为什么是断线**：按 R9「All configurability exposed as tools」，这是一个存在但无法调的面；pub 字段 + Default 的形状暗示曾打算暴露。
- **建议修法**：二选一——(a) 降为模块内 `const`（承认不可调）；(b) 经 preflight/config 暴露（如 `min_tokens_to_prune: Option<usize>`）。改动均为 1 文件级。
- **风险**：低。行为不变，纯可及性问题。

### C2. [低·边界，需主会话裁决] ingress 收缩遥测只接到 `tracing::debug`

- **证据**：`IngressOutcome.reductions` / `compressed` 的唯一生产消费者是 `src/tools/scoped/dispatch.rs:1624-1636` 的 `tracing::debug!` 循环；`src/tool_output/ingress.rs:53-56` 字段注释自陈 "(for tracing)"。
- **为什么是断线**：基础设施（按字段 Reduction 遥测）存在，但没有进入任何诊断面（`context.breakdown`、`ToolResultPersist` hook 同级、metrics）——不进 debug 日志即不可见。**但**字段文档已声明 tracing-only 意图，若主会话认定这是刻意设计，应移入「确认非断线」。
- **建议修法**：若认为值得可见：把 reductions 摘要挂到已有的 `ToolResultPersist` hook payload（dispatch.rs:1685-1698 已在 persisted 路径发 hook，顺带捎带零成本），或进 `context.breakdown`。
- **风险**：低。遥测面扩展不影响行为。

## CUT（1 条）

### X1. [低] `compress_tool_output` 文档自陈 test-only，但无 `#[cfg(test)]` 门禁、无生产消费者

- **证据**：`src/tool_output/compressor.rs:202`（`pub(crate)`，doc 明写 "test-only"）；rg 确认模块外零调用者，仅 compressor.rs 内部测试使用。对照 `structured::reduce`/`classify`（structured/mod.rs:282-285、399-403）做了正确的 `#[cfg(test)]` + 文档标注。
- **为什么是断线**：一条「死但活着的」公共 API——它过了 clippy 的死代码检查（因为有测试引用），形状上像生产入口，未来可能被误接。
- **建议修法**：加 `#[cfg(test)]`（及其测试的 import 调整），与 structured 的先例对齐。
- **风险**：低。纯整洁性，无行为变化。

## DECIDE（3 条）

### D1. [中] stale 通道把「剪枝门槛」与「structured 目标预算」绑在同一常量 200

- **证据**：`tool_result_pruning.rs:131` `reduce_within(cleaned, Some(200))` 与 `:138`（`hint < 200` 占位门）、`:145`（`reduction.render()` 双 token 门）共用 `min_tokens_to_prune`。且 200-token 预算下 `Profile::FLOOR`（structured/mod.rs:223-235）仍允许每行最多 80 字符 × 40 行等下限产物——名义 200-token 预算、实际下限产物可能换不掉 200 token 的占位（有占位/双门兜底，不会劣化，但「换」可能白做功）。另：200-token 预算使 `crush` 对 Log|Search 生效（structured/mod.rs:380-396，spec §2b 的 budgeted 路径），应确认这是期望而非误触发。
- **语义模糊点**：`min_tokens_to_prune` 到底是「值得动手的旧结果下限」还是「structured 应压缩到的目标」？两个语义被同一常量锁死，调一个必然动另一个。
- **建议**：主会话裁决常量语义并拆成两个名字（如 `prune_threshold` / `target_budget`），或书面确认绑定是有意的。
- **风险**：中低。当前行为安全（双门兜底），但任何未来调参都会踩中语义双关。

### D2. [低] stale 的「新鲜尾部」按消息条数而非工具新近度计量

- **证据**：`tool_result_pruning.rs:87-94`：`messages.len() <= fresh_tail_count` 直接返回 0；`cut_end = messages.len() - fresh_tail_count`，剪枝窗口是位置切片。
- **语义模糊点**：若 fresh_tail 内堆了大量非 tool 消息（用户长文、系统注入），最近的工具结果会落入剪枝窗口被 digest 替换——「stale」的定义是上下文位置还是工具调用新近度，代码取前者但没有任何注释或测试固定这个语义。
- **建议**：确认位置语义是有意的（hermes 移植惯例？）；若是，加一行注释固化；若工具新近度才是意图，需按 tool_result 索引计量。
- **风险**：低。窗口通常足够大，实际触发罕见。

### D3. [低] 剪枝把历史 `Json` 内容块重建为 `Text` 块

- **证据**：`tool_result_pruning.rs:154-172`：命中块 `ContentBlock::Json` → 替换为 `ContentBlock::Text { text: reduction.render(), cache_control: None }`；image 块保留。Anthropic 线上等价已验证（`providers/anthropic/proto_impl.rs:298-301` 两分支同走 `as_model_text`；当前所有构造点 `cache_control` 均 `None`，prompt.rs:219/406）。
- **语义模糊点**：线上无差异**仅在 cache_control 打点策略不变的前提下成立**；若未来给 tool result 块打 `cache_control`，Json→Text 的块类替换是否仍等价需重验。其他 provider 序列化路径未逐一验证。
- **建议**：在 pruning 的替换处加一行注释「wire-equivalent under as_model_text flattening; re-verify if cache_control stamping lands」。
- **风险**：低。当前无行为差异，纯防御性记录。

## 确认非断线（对照刻意不做 / 覆盖缺口清单排除）

以下条目审计中浮现、看似断线，但对照 FEATURE_LOCATOR §3.14 第三轮遗留清单、代码注释或测试后确认为**刻意设计或非目标**，不列入断线：

1. **resolve_result_budget 旧 name table 已删**（result_processing.rs:158-180）：声明是唯一来源，且 `definitions.rs:4173-4233` 有普查测试 `result_budgets_come_from_declarations_and_default_everywhere` 逐登记名断言声明一致——刻意，且有测试兜底。
2. **`clears_tool_results_server_side` 时整条 Pruning stage 被替换**（preflight.rs:318-327）：注释自陈 "Decided once per run … a documented gap"——刻意接受的 gap，不是断线。
3. **错误通道绕过 apply_layer_two**（dispatch.rs:1789-1792 注释）：错误永不持久化、只经 `clean_error_body` 4000 字符 cap（chars→tokens 换算 bug 已修，注释 :1851-1856）——刻意。
4. **spawn_blocking ≥128KiB 分支无测试**：§3.14 自陈覆盖缺口（rg 证实 `INGRESS_BLOCKING_THRESHOLD` 仅 3 命中：定义/文档/使用）——测试缺口，非断线。
5. **Layer-3 spill 不豁免 reads**（turn_budget.rs:281-283 注释：spilled read = persist+indexed = re-read 非 loss）——刻意。
6. **`tools_invoke` RPC 直连路径不走 Layer 2**（gateway/handlers/tools_invoke.rs:287 注释）：结果回 RPC 调用方、不进模型上下文——非目标。
7. **`REGISTRY_SCHEMA_BASELINE`**（definitions.rs:3571，在 `mod tests` :1566 内）：schema 字节基线测试，与 result budget 无关——非断线。
8. **持久化结果在剪枝中被跳过**（tool_result_pruning.rs:101-115 行扫描 `extract_persisted_ref`，result_store.rs:854-857）：曾有 `starts_with` 漏 hygiene 路径的教训，现按行扫描且有测试——刻意且已修过一轮。
9. **offload/recovery 基础设施生产接线确认**：`recovery_tools()` 默认 `RecoveryTools::ALL`（tools/service.rs:294-303，trait 文档明确「Every production DECORATOR must forward it」），`allowlist_tool_service.rs:135` 有 narrowing 实现 + 测试（:263-293）——不存在「基础设施永不被安装」的暗断线。
10. **`is_read_family` 谓词边界**：仅 `file_read` + `session_decompress`（result_processing.rs:221-227）；测试断言旧拼法 "read_file"/"Read" 已退役、`ctx_search`/`bash` 明确非 read 族（:2287-2296）——边界有测试钉死。
11. **ceiling「decline」与「未安装」两态歧义**：CapabilitySlot outcome 可区分（result_processing.rs:57-137），boot 路径 start/mod.rs:3585/3599/3619——已闭。
12. **compaction_cache（T5/T6 现状）**：`SummaryQuality` 跨 run carry-over 在 `context/compact/`，rg 确认不调 `structured`/`crush`——按其自身设计工作，不在本审计判定范围。
13. **`hoist_inline_images` 双调用**（browser screenshot.rs:267 / exec.rs:2475 工具自 hoist + dispatch.rs:1557 兜底 hoist）：幂等 no-op 兜底——刻意。

## 建议接线优先级

| # | 候选接线 | 一句话理由 | 风险 |
|---|---------|-----------|------|
| 1 | C1：`min_tokens_to_prune` 降为 const 或经 config 暴露 | pub 旋钮无生产者是 R9 视角下唯一实打实的「存在但不可调」，修复成本一行级 | 低 |
| 2 | X1：`compress_tool_output` 加 `#[cfg(test)]` | 与 structured 先例对齐，消除「死但活着」的公共 API 误接面 | 低 |
| 3 | C2：ingress 遥测挂 `ToolResultPersist` hook payload | 数据已在手上（IngressOutcome.reductions），捎带零成本，先决条件是主会话裁决 tracing-only 不是刻意 | 低 |

D1–D3 均为「裁决 + 注释固化」级，不阻断任何接线。

---

## 本审计没有覆盖的区域（State the Negative）

- **运行时验证**：全程静态阅读 + rg 普查，未跑 cargo 编译与任何测试（任务约束：零编译需求）。
- **MCP 工具路径**：仅确认 `split_external_fence` 被 hygiene/compressor 消费（§3.14 第二轮），MCP 工具声明如何进 registry 声明链未逐行审计。
- **image hoist 下游**：`_media` harvest 之后的网关/附件管线未跟进。
- **非 Anthropic provider 的序列化路径**：D3 的等价性论证只覆盖了 anthropic proto_impl.rs:298-301。
- **Layer-3 spill 的完整生命周期**：turn_budget record 的计数来源（`cumulative`）与 ContextIndexWiring 只在 offload 接线处确认存在，未审计 spill 后的检索质量。
- **structured/crush/diff 各 reducer 内部正确性**：审计目标是「接线」而非 reducer 语义；crush 仅 Log|Search 的路由已确认接线，reducer 输出质量未评估。
- **error path 的 `sanitize_tool_error` 全部分支**：确认了 cap 与刻意绕过注释，未逐分支验证 PII 清洗矩阵。

---

## 修复记录（2026-10-02）

分支 `fix/admission-clippy`（基 main @ fa9e4c19e）。两个提交：

| 提交 | 说明 | 文件数 |
|------|------|--------|
| `8e06f8ad9` | `context: fix admission audit findings C1+C2+D1+D2+D3 and drop write-only cache quality tag` | 6 |
| `5af3580f9` | `clippy: zero out -D warnings on alephcore --all-targets`（纯 lint，无行为变化） | 23 |

逐条映射：

| 审计项 | 处置 |
|--------|------|
| **C1+D1** | `tool_result_pruning.rs` 删掉 pub 字段 `min_tokens_to_prune`，拆成两个带 doc 的模块级 const：`PRUNE_THRESHOLD_TOKENS`（"值得动手剪的旧结果下限"，用于占位 hint 门与双 token 门）与 `STRUCTURED_TARGET_BUDGET_TOKENS`（"structured 应压缩到的目标预算"，用于 `reduce_within`；doc 注明 200 预算使 crush 对 Log\|Search 生效是 spec §2b budgeted 路径的**期望行为**）。两者值均 200，行为不变。`ToolResultPruningStage` 随之成为单元结构体，全部 `::default()` 调用点（preflight.rs、13 处测试）改为裸构造（`5af3580f9` 内）。 |
| **C2** | 采纳「挂 hook payload」方案：`HookContext` 新增 `pub ingress_reductions: Option<String>`（derive Default ⇒ 旧构造点/反序列化点向后兼容，缺的字段为 None）；dispatch.rs 的 ToolResultPersist 触发点在 persisted 路径有 shrink 摘要时 `.with_ingress_reductions(...)` 捎带，摘要为 JSON（`{"compressed":bool,"reductions":[{field,method,tokens_before,tokens_after}]}`，`method` 为 `ReductionMethod` 的 Debug 名），双 gate 下（未过 budget 门 / 无 reductions）不发该字段。tracing::debug 保留。新增 3 个测试（summary 构造、absent gate、payload JSON 透传）。 |
| **X1** | **基线已修，无需动作**：`compressor.rs:201` 的 `#[cfg(test)]` 由上游 `086450ee1 audit: review/severed-wire fixes` 加入，模块外零调用者。 |
| **D2** | 注释固化：`fresh_tail` 取「消息尾部最近的 N 条工具结果」的位置语义是刻意的（hermes 移植惯例）；注释说明工具结果落入剪枝窗口的条件（按 token 超限触发、窗口按预算和条数取）。 |
| **D3** | 注释固化：Json 块重建为 Text 块在 `as_model_text` 扁平化下 wire-equivalent（`providers/anthropic/proto_impl.rs:298-301`）；若未来 cache_control 打戳落在 tool-result 块上需重新验证。 |
| **quality 字段** | **选 (a) 删除**。理由：`CompactionCache` 无任何 serde/持久化（进程内 static `COMPACTION_CARRYOVER`，state.db 无 schema 牵连）；`Full` 闸门在 `store_cache` 内**先于构造**以参数判定，字段从未被生产读回（clippy `never read` 实锤），它只是写时打标。删字段、保留 `store_cache(quality)` 签名与 `SummarizerOutcome.quality`；测试改锚行为（Degraded ⇒ 本地有 + carry-over 无；Full ⇒ carry-over `expect` + 正文断言），守卫强度不减。 |

验证（2026-10-02 实测）：

- `cargo clippy -p alephcore --all-targets -- -D warnings` → **exit 0**（含基线 E0603 修复：`harness_bridge/mod.rs` 恢复 `pub use context_blocks::compute_runtime_state_blocks`——模块私有，集成测试 `tests/runtime_state_e2e.rs` 无公开路径可达，与其 doc「供单测练习」的意图一致）。
- 窄域测试全绿：`context::budget` 99 / `tools::scoped` 205 / `orchestrator::` 269 / `tool_output` 169 / `context::compact` 168 / `extension::` 722 通过；`extension::` 另有 5 个失败为 **Windows 环境既有失败**（POSIX 路径假设 `/usr/bin/python3`、Unix `true`、路径分隔符、worktree plugins 扫描），全部位于未触碰文件，与本修复无关。
