# Windows 语义交互补齐 + 验证点亮 — 设计 Spec

- **日期**: 2026-07-04
- **状态**: 已批准，待实现计划
- **范围代号**: B1 + A3 + D1 + A1（A2 已判定误报撤销）
- **前序**: [2026-07-02-computer-use-semantic-interaction-design.md](2026-07-02-computer-use-semantic-interaction-design.md)（对标 orca 的语义交互升级，macOS 侧落地）
- **关联红线/原则**: R1（大脑-四肢分离）、R10（薄 harness）、P4（依赖倒置）、P6（KISS/YAGNI）、P7（防御性设计）

---

## 1. 背景与问题

Aleph 的 desktop「computer use」子系统已相当成熟（FEATURE_LOCATOR §7 全 ✅）。2026-07-02 对标 orca 做过语义交互升级：新增 `set_value` / `ax_action` 两个语义动词、`AxLocator` 无状态定位、`AxVerification` 读回验证、act→observe 融合。**但该升级只在 macOS（Swift helper）侧落地。**

审计发现的真实缺口：

- **B1（主）**: Windows `WindowsAccessibility` 只实现 3 个只读 AX 方法（`query_focused`/`query_tree`/`query_by_role`）。`set_value` / `perform_action` 继承 `AccessibilityCapability` trait 的默认实现 → 返回 `NotImplemented`。因此 `desktop` 工具的 `set_value` / `ax_action` 两个动词在 Windows **直接失败**（"AX capability not available"）。这是 Windows 上最大的语义交互落差。
- **A3**: `desktop/windows/src/ax.rs:333` `node_of()` 里 `value: None`（"VARIANT value extraction deferred"）。导致 Windows 上 `AxElement.value` 恒空 → 弱化 `set_value` 读回验证与 `observe.rs` 的 `focused_element.value`。
- **D1**: 逐动作验证信号。协议层 `AxVerification{state, reason, actual_preview}` + `AxActionResult{performed, path, matched, verification}` **已存在且完整**，`native.rs::ax_action_output` 已诚实呈现 verified/unverified（其注释即 "mirrors orca's action-path metadata"）。**验证链已端到端连好，唯一未接的是 Windows 生产者。**
- **A1**: 文档漂移三处（详见 §7）。其中 `mod.rs:553` `ax_action ... macOS only today` 在本轮实现后**必须**校正。

### 1.1 判定撤销的项：A2（会话防休眠）

原列入范围的「computer-use 会话期间不防休眠」经核查为**误报**：`src/harness/agent/think.rs:406-417` 的 `run_turn_internal` 已在**整个 turn（Think→Act，含工具执行）**期间持有 `power.inhibit_sleep("Aleph agent loop")` 守卫（RAII，作用域退出即释放）。computer-use 工具调用发生在 Act 阶段、turn 之内，**已被覆盖**。在 `DesktopTool` 再加抑制＝重复且无谓触碰 R10 预算区。**A2 撤销，不做。**

---

## 2. 核心洞察

本轮**不碰协议层、不碰 core 分发逻辑**。唯一实现缺口在 `desktop/windows/src/ax.rs`。补齐 UIA 写路径后，D1（验证）与 A3（值读回）随之自动点亮。

**全部改动落在 Windows 四肢 crate（`aleph-desktop-windows`）内，零 `src/` 改动（除 §7 的文档字符串校正），严守 R1「`src` 严禁直接调平台 API」。**

现有已连好、本轮复用不改的链路：

```
LLM → desktop{action:set_value,...}
  → mod.rs        check_hard_block → classify_approval(DesktopType) → requires_lock → session_lock
  → native.rs     locator_from_args(args) → platform.ax().set_value(SetValueParams{locator,value})
  → [平台]        <== 本轮唯一改动点：Windows 实现此方法 ==>
  → AxActionResult
  → native.rs     ax_action_output(r)  → DesktopOutput（verified→"Value set and verified."；unverified→hint）
```

---

## 3. 架构与分层

- **协议契约**（`shared/protocol/src/desktop_bridge/methods/ax.rs`）：`AxLocator`、`SetValueParams`、`PerformActionParams`、`AxVerification`、`AxActionResult`、`AxElement` — **完整，不改**。
- **能力契约**（`desktop/shared/src/traits/ax.rs`）：`AccessibilityCapability::{set_value, perform_action}` 默认 `NotImplemented` — **不改**（Linux 继续自动缺位）。
- **core 分发**（`src/builtin_tools/desktop/native.rs`）：`set_value`/`ax_action` 动词分发、`locator_from_args`、`ax_action_output` — **不改**。
- **Windows 四肢**（`desktop/windows/src/ax.rs`）：**本轮全部实现落点。**

线程模型：沿用现有 `run_blocking`（`tokio::task::spawn_blocking`）+ `ComGuard`（MTA COM apartment RAII）+ 每调用新建 `IUIAutomation`（COM 指针不缓存、不过 async/Send 边界）。零架构偏离。

---

## 4. 组件设计（全部在 `desktop/windows/src/ax.rs` 的 `imp` 模块）

### 4.1 Locator resolver（新）

复刻 macOS Swift helper 的**无状态定位**语义（`AxLocator` doc 一字对齐）：

1. 用现有 `resolve_root_hwnd(locator.pid)` + `ControlViewWalker` walk 目标进程/前台窗口的 UIA 树，收集候选 `(IUIAutomationElement, AxElement 摘要)`。
2. 若 `locator.role` 存在：按 `control_type_to_ax_role` 映射后的 role 字符串**过滤**（复用同一映射表，与 `query_tree`/`desktop_som` 结果一致）。
3. **title 打分**（若 `locator.title` 存在）：exact 匹配 > contains 匹配，均大小写不敏感；exact 命中优先级最高。
4. **center tiebreak**（若 `locator.center` 存在）：在同分候选里取 bounding-rect 中心与 `center` 欧氏距离最近者。UIA bounds 已是物理像素，与 Windows click/screenshot 坐标同空间，直接比较不重缩放。
5. 无候选匹配 → 返回 `None`（上层转 `performed:false` + hint）。

打分与排序逻辑**抽为纯函数**（输入候选摘要 + locator，输出最佳索引），可 host-runnable 单测，不依赖 COM。

### 4.2 `set_value`

**关键契约约束**：`native.rs::ax_action_output` 把**任何** `verification.state=="unverified"` 渲染成固定文案「Value written but read-back did not match (…)」。因此 `unverified` **只能**用于「确实写了、但读回不符」这一种情形（`value_mismatch`）。「定位不到 / 不可写 / 不支持」这些「根本没写」的情形一律走 `Err(...)` —— `native.rs` 的 set_value `Err` 分支已 `recovery::with_hint(format!("set_value failed: {e}"))`，文案正确且带回退提示。**唯有真正发生了写入才返回 `Ok(AxActionResult)`。**

```
1. resolve_locator(locator)
   - 无匹配 → Err(NotAvailable("no element matched role/title; try `ax_snapshot`"))
2. 取 IUIAutomationValuePattern
   - 无该 pattern 或 IsReadOnly → Err(NotAvailable(
       "element does not support a settable value; fall back to click + type_text"))
3. ValuePattern.SetValue(value)
   - 调用失败 → Err(PlatformError(...))
4. 读回 ValuePattern.CurrentValue（失败再试 value_of 的 Legacy 路径）
5. 构造 AxVerification：
   - 读回 == value → state:"verified"
   - 读回 != value → state:"unverified", reason:"value_mismatch",
                     actual_preview:Some(读回值前 200 字符)
6. 返回 Ok(AxActionResult{performed:true, path:"accessibility",
                          matched:Some(el 摘要), verification:Some(...)})
```

`matched` 的 `AxElement` 摘要（childless）通过 `node_of` + `value_of`（§4.4）填充，给模型可见性。**这一步即 D1 在 Windows 的落地。**

### 4.3 `perform_action`

AX 动作名 → UIA pattern 映射（仅覆盖 DESCRIPTION 已宣传的动作，KISS）：

| AX action | UIA 回退链 |
|-----------|-----------|
| `AXPress` / `AXConfirm` | `Invoke.Invoke()` → `Toggle.Toggle()` → `SelectionItem.Select()` → `LegacyIAccessible.DoDefaultAction()` |
| `AXShowMenu` | `ExpandCollapse.Expand()`（不支持则 `Err` 建议右键 `mouse_button`） |
| 其它 | `Err(NotImplemented("ax.perform_action:<name>"))`（模型可读，native.rs 走 `recovery::with_hint`） |

`perform_action` 同样遵守 §4.2 的 Err-vs-Ok 纪律（`ax_action_output` 对 `perform_action` 结果 `verification:None`，故 `performed:false` 不会产出任何 hint 文案）：

- resolve_locator 无匹配 → `Err(NotAvailable("no element matched..."))`
- 回退链**全部** pattern 都不支持 → `Err(NotAvailable("element exposes no actionable pattern; try click at its center"))`
- 回退链按序尝试，**第一个「元素支持且调用成功」**即 `Ok(AxActionResult{performed:true, path:"accessibility", matched:Some(el 摘要), verification:None})`（与 macOS `perform_action` 一致——无 verification 字段）。

映射表抽为纯数据/纯函数，可 host 单测（断言 action 名 → 期望 pattern 序列）。

### 4.4 值读取（A3）

新 helper `value_of(el: &IUIAutomationElement) -> Option<String>`：`IUIAutomationValuePattern::CurrentValue`（失败再试 `LegacyIAccessiblePattern::CurrentValue`）；空串归一为 `None`。

**按需调用，非全树**：仅在 (a) `resolve_locator` 命中的 `matched` 元素、(b) `query_focused` 返回的元素 上调用。**不进 `walk()` 的 4000 节点全树遍历**（每节点一次 COM 调用代价高，会拖慢 `desktop_som`/`ax_snapshot`）。

落地方式：`query_focused` 内对命中元素补 `value_of`；`node_of` 保持 `value:None`（全树 walk 用它，维持快）；`set_value`/`resolve_locator` 的 `matched` 单独补值。这样 `observe.rs` 的 `focused_element.value` 在 Windows 也有值，`set_value` 读回验证可用。

---

## 5. AX 角色/动作词汇（保持 macOS 形，零消费者改动）

- **role**: 沿用现有 `control_type_to_ax_role`（UIA `ControlType` → `"AX*"`）。locator role 过滤用同一映射 → 与 `INTERACTABLE_ROLES` allowlist、`desktop_som`、`ax_snapshot` 结果自洽。
- **action name**: 沿用 `"AX*"` 命名（DESCRIPTION 已教模型用 `AXPress`/`AXShowMenu`）。Windows 侧把这些名字映射到 UIA pattern，模型无感知平台差异。

---

## 6. 错误处理（P7，全部不 panic）

**统一纪律**：只有「动作真正发生」才返回 `Ok(AxActionResult)`；一切「没做成」走 `Err(...)`（`native.rs` 的 set_value/ax_action `Err` 分支已 `recovery::with_hint`，文案正确）。`unverified` 专用于 set_value「写了但读回不符」。

| 情形 | 返回 |
|------|------|
| 无前台/目标窗口 | `Err(NotAvailable)`（现有 `resolve_root_hwnd` 已如此） |
| locator 无匹配 | `Err(NotAvailable("no element matched role/title; try `ax_snapshot`"))` |
| ValuePattern 缺失/只读 | `Err(NotAvailable("...fall back to click + type_text"))` |
| set_value 写了但读回不符 | `Ok(...verification: unverified, reason:"value_mismatch")` |
| perform_action 无可用 pattern | `Err(NotAvailable("no actionable pattern; try click at center"))` |
| 未映射的 AX 动作 | `Err(NotImplemented("ax.perform_action:<name>"))` |
| COM/UIA 调用失败 | `Err(PlatformError)`（不 panic） |

上层 `native.rs::ax_action_output` 与 `recovery::with_hint` 已有，本轮复用。

---

## 7. A1 文档校正（3 处）

1. `src/builtin_tools/desktop/mod.rs:553` — `ax_action: ... macOS only today; other platforms report the capability as unavailable.` → 改为「macOS + Windows (UIA)；Linux 报告 capability 不可用，回退 click」。同时 `set_value`（:552）措辞确认对 Windows 适用（role `"AXTextField"` 经 UIA 映射同样命中）。
2. `src/builtin_tools/desktop/types.rs:30-33` — `action` 字段 doc-comment 补全动词表（与 DESCRIPTION 一致，含 `move_window`/`resize_window`/`restart_app`/`key_button`/`paste`/`set_value`/`ax_action`/`wait_visual`/`display_list`/`batch`/`script`）。
3. `desktop/shared/src/traits/ax.rs:3-5,18-20` — 去掉/校正「Platform implementations that support the macOS Accessibility API」「On non-macOS platforms the `DesktopPlatform` default returns `None` from `ax()`」——现已对 Windows（UIA 返回 `Some`）为假。

> 注：文档校正 #1/#2 触碰 `src/`，但仅为**字符串常量/注释**，非平台 API 调用，不违 R1。

---

## 8. 测试策略

### 8.1 纯逻辑单测（host-runnable，无需 Windows target）
放 `desktop/windows/src/ax.rs` 的 `#[cfg(test)]`，复用现有 host-test 模式（现有 role-mapping 测试已证可行）：

- **locator 打分排序**：给定候选摘要集 + `AxLocator`，断言 exact-title > contains-title > center-nearest 的选择顺序；无匹配返回 `None`。
- **AX-action → UIA pattern 映射表**：断言 `AXPress`/`AXConfirm` → `[Invoke, Toggle, SelectionItem, Legacy]`；`AXShowMenu` → `[ExpandCollapse]`；未知 → `NotImplemented`。
- **role 过滤**：复用 `control_type_to_ax_role`，断言过滤命中一致。

### 8.2 手动 E2E 清单（本机 live Windows，实现后交付执行）
真 UIA `set_value`/`perform_action` 只能在 live 桌面验证（`imp` 是 `cfg(windows)` 且需运行中的 app）：

1. **记事本 set_value**: `desktop{action:set_value, role:"AXTextField", text:"hello 世界"}` → 断言 `verification.state == "verified"`，记事本可见文本。
2. **计算器 AXPress**: `desktop_ax_snapshot` 定位数字键 → `desktop{action:ax_action, ax_action_name:"AXPress", element_title:"5"}` → 断言 `performed:true`，显示更新。
3. **浏览器地址栏 set_value**: 定位地址栏 → set_value URL → 断言 verified。
4. **不支持路径**: 对静态文本 set_value → 断言 `unverified reason:"not_settable"` + 回退提示。
5. **observe 值读回（A3）**: 任一 focus 变更动作带 `observe:"state"` → 断言 `post_state.focused_element.value` 非空。

### 8.3 cargo 纪律
- 至多一次 `cargo check -p aleph-desktop-windows`（本机 Windows target 才编译 `imp` 真实路径）。
- **不跑全量测试**（`alephcore` 构建吃内存，OOM 风险）。
- 全局「80% 覆盖」对 live COM 路径不适用 — 诚实标注：纯逻辑覆盖 + 手动 E2E。

### 8.4 构建前置检查
`aleph-desktop-windows` 的 `windows` crate 依赖需启用 UIA pattern 接口（`Win32_UI_Accessibility` 下的 `IUIAutomationValuePattern` / `IUIAutomationInvokePattern` / `IUIAutomationTogglePattern` / `IUIAutomationSelectionItemPattern` / `IUIAutomationExpandCollapsePattern` / `IUIAutomationLegacyIAccessiblePattern`）。实现首步核对 Cargo.toml `features`，缺则补。

---

## 9. 明确不做（YAGNI / 外科手术式）

- ❌ 不给 `click`/`type_text` 加 verification 字段（合成动作恒 `unverified`，`observe:"state"` 已能读回焦点值；超本轮范围、增 R10 面）。
- ❌ 不改协议层（已完整）、不动 `native.rs`/`mod.rs` 分发逻辑（已连好）。
- ❌ 不做 `native.rs`（1366 行）拆分、`types.rs` 字段去重（属独立一轮「架构重构 C」）。
- ❌ 不给 Windows 补 AppleScript/JXA、camera/STT 等无关能力。

---

## 10. 成功标准

1. `desktop{action:set_value,...}` 在 Windows 返回 `AxActionResult`，读回一致时 `verification.state=="verified"`。
2. `desktop{action:ax_action, ax_action_name:"AXPress"/"AXShowMenu",...}` 在 Windows `performed:true`（元素支持对应 pattern 时）。
3. Windows `AxElement.value`（focused + set_value matched）非空可读。
4. `mod.rs`/`types.rs`/`traits/ax.rs` 三处文档不再声称 AX 写为 macOS-only。
5. 纯逻辑单测通过；手动 E2E 清单 §8.2 在本机跑通。
6. `cargo check -p aleph-desktop-windows` 通过。
7. 零 `src/` 平台 API 调用（R1）；`desktop/shared` 与 `src/harness` 零新增（R10）。
