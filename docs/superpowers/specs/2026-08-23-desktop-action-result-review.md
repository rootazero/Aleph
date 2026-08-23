# ⓐ 评审稿：ActionResult `effect`/`route`/`escalation` 闭集枚举移植评审

> **来源**：FEATURE_LOCATOR §7.3「动作可验证性（2026-08-21）」轮 ⓐ，当时裁决「高爆炸半径、需评审、列为建议下一轮」。本文即那份评审。
> **状态**：✅ Stage 1 已落地（2026-08-23）——四值闭集采纳 cua-driver 原生词表（`confirmed`/`partial`/`unverifiable`/`suspected_noop`，其 `refused` 与 Aleph 的 `success:false` 正交故不移植）；Stage 2（逐动词证据升级）未动工。前置条件 (a) 已闭合：cua-driver 契约原文（`libs/cua-driver/docs/action-result-contract.md`）经网络复核，关键收获是 `suspected_noop` 的「怀疑而非证明」纪律与「OS 受理只是诊断、不晋升 confirmed」不变量，均已采纳。
> **参考**：cua-driver `ActionResult`（`effect`/`route`/`escalation` 三个闭集枚举维度）。

## 1. 参考模型与不确定性声明

cua-driver 的论点（上轮扫描结论）：`success: bool` 不足以表达 GUI 动作「**投递了但没生效**」——事件被 OS 受理 ≠ UI 状态改变。它用三个闭集枚举替换裸布尔：

- `effect`：动作是否实际生效（区别于「投递成功」）
- `route`：事件走的哪条投递路径
- `escalation`：何时/如何升级给人

⚠️ **不确定性（AGENTS.md §4）**：精确的变体集**未在本评审中核实**——上轮扫描的原始日志（`cua-cs.log`/`ocu-docs.log`）当前为空文件，本地无参考项目 checkout。本文的设计基于上轮记录的三个维度名与语义主张；**定稿枚举拼写前必须对照 cua-driver 源码复核**，下文的取值是 Aleph 侧的自足设计，不是转录。

## 2. 现状契约盘点（实测）

### 2.1 输出契约的形状

`DesktopOutput`（`src/builtin_tools/desktop/types.rs:647`）：

```rust
pub struct DesktopOutput {
    pub success: bool,
    pub data: Option<Value>,      // 每动词自由形状
    pub message: Option<String>,
}
```

**关键结构性事实：desktop 工具的输出没有 JSON Schema。** `AlephTool::Output` 的约束只有 `Serialize`（`src/tools/traits.rs:73`）——schema 自动生成只覆盖 **Args**（输入）。输出契约的唯一成文真源是 `DESCRIPTION` 散文（实测 **12,594 字节**；上轮记录的「17KB schema」数字不准，应为 DESCRIPTION 散文体量——**量错不改结论**，爆炸半径的构成见 §4）。这意味着：

- 加字段对 wire 格式**向后兼容**（模型读 JSON，多一个键不破坏任何人）；
- 但「契约」是散文承诺，一旦写进 DESCRIPTION，每个新动词都必须兑现（见 §6 trade-off）。

### 2.2 每变更动词的「生效证据」矩阵（今天）

| 动词 | route（`delivery`） | 事后生效证据 | `success:true` 的实际语义上限 |
|---|---|---|---|
| click / double_click / hover / mouse_button | ✓ | 无 | OS 受理了事件 |
| drag | ✓ | 无 | OS 受理 |
| scroll | ✓ + `at` 落点 | 无 | OS 受理 + 报告滚在哪 |
| key_combo / key_button | ✓ | 无 | OS 受理 |
| type_text | ✓ | **事前**闸（focus_gate 拒无焦点/密码框） | 通过前置校验 + OS 受理 |
| paste | ✓ | `clipboard_restored`（半个：剪贴板恢复状态） | OS 受理 |
| **set_value** | ✓ | **readback `verification.state: verified/unverified`** | **有真事后证据** |
| ax_action | ✓ | AXPerformAction 返回值 | API 受理（对 `enabled:false` 元素是静默 no-op） |
| launch_app / quit_app | — | 无 | OS 受理 |
| **restart_app** | — | **`verified: true/false/null` 三态轮询** | **有真事后证据** |
| focus_window / move_window / resize_window | — | 无 | OS 受理 |
| batch | 逐子项 | 逐子项 | 逐子项 |

辅助通道（非判定）：`observe:"state"/"screenshot"` 的 `post_state`/`post_screenshot`（additive，模型自读）；`verify_state` 只读 arm（模型驱动的后置断言）。

**结论**：`route` 维度 Aleph 已有一半（`delivery`，全动词均匀）；`effect` 维度只有 2.5 个动词有真证据，其余全是「OS 受理」。cua-driver 的批评在 Aleph 身上成立。

## 3. `escalation` 维度的适配性判断（建议不移植）

`escalation` 是「告诉模型何时/如何升级给人」的枚举。这与 Aleph 的既有结构冲突：

- **R10**（智能在 prompt、零 middleware tax）：升级判断是认知，归模型与人类，不该由工具结果里的枚举越俎代庖；
- Aleph 已有承担该轴的专职机制：exec tier 审批卡（World B）、`clarification::ask`、plan_gate、`call_user` loop-control 动词。第三个「升级」语义源是冗余通道，且三者必然漂移。

**建议：本次评审范围收敛为 `effect` 一个维度**（`route` 已有），`escalation` 明确不移植并记入「刻意不做」。

## 4. 爆炸半径清单（若做）

1. **类型**：`types.rs` 新增 `ActionEffect` 闭集枚举 + `DesktopOutput.data` 各动词填充（或提为 `DesktopOutput` 顶层字段——见 §5 方案分歧）。
2. **构造点**：`native.rs` 大 match 的 ~12 个变更臂 + `ax.rs` 的 `set_value`/`ax_action` 两臂 + `mod.rs::execute_batch` 的手工 JSON 包装（子项形状要与叶子一致）。合计约 **15 个产出点**。
3. **文档真源**：`DESCRIPTION`（12.6KB）新增 `effect` 语义段——每动词一句。
4. **测试面**：`desktop/tests.rs`（60 测试）+ `native.rs` 内联（40 测试）中断言输出形状的用例；需新增 census 测试钉住「每个变更动词都填 `effect` 闭集值」（同 `curated_default_covers_every_action_type` 的守卫模式）。
5. **下游消费者**（实测全部宽松）：`execute_batch` 只读 `success`；`result_processing::hoist_inline_images` 只找图片键；qa 无 desktop harness（ⓔ 仍欠账）；无接口层解析 `data`。→ **无硬破坏面**，风险集中在「承诺出去的值是否诚实」。
6. **平台 limb**：若想让 `click` 等动词报真 `effect`，需要事后证据（AX 重查 / frontmost 对比），三平台能力不齐——与 ⓑ limb 侧半同一个验证困境（只有 UIA 可本机编验证）。

## 5. 方案与 trade-offs

### 方案 A：全量移植（effect + route + escalation 三维度，每动词全量化）

- 得：与参考项目契约对齐，信息最完整。
- 失：**`escalation` 撞 R10**（§3）；`effect` 对大多数动词今天只能报「不知道」，全量化等于先承诺后补证据；改动面最大。
- 让什么变难：将来任何 `effect` 取值升级都要过「是否改契约」评审；三平台 limb 不齐时枚举长期停在低信息值。

### 方案 B（推荐）：只加 `effect` 单维度、闭集、只用今天拿得到的证据，分两阶段

**Stage 1（契约落地，零新平台代码）**：`data.effect` 闭集四值——

| 值 | 语义 | 今天的填充者 |
|---|---|---|
| `verified` | 有事后证据确认生效 | set_value（readback）、restart_app（轮询） |
| `os_accepted` | OS/API 受理，无事态证据 | click/drag/scroll/key_*/type_text/paste/launch/quit/focus/move/resize/ax_action |
| `no_op` | 可证的无操作 | （初期可为空集；预留诚实出口，如未来检测 scroll 触边界、ax_action 打 disabled） |
| `unknown` | 平台无法判定 | 无 AX 层 / 无枚举能力平台的降级 |

`success: bool` **保留不动**（向后兼容 + 它仍有「投递层失败」的独立语义：审批拒、锁冲突、escape、hard-block）。`delivery` 维持现名不更名为 `route`（重命名是纯噪音，DESCRIPTION 里注明对应关系即可）。

**Stage 2（可选、逐动词、独立 PR）**：给证据便宜的动词升级为真 `verified`——click → 投递后 AX hit-test / 坐标下元素重查；focus_window → 重读 frontmost 对比；move/resize_window → 重读 frame 对比。每个动词单独可评审、可回退，不阻塞 Stage 1。

- 得：契约诚实（枚举值全部被今天的代码真实填充，不承诺没有的东西）；爆炸半径收敛到类型 + 15 个填充点 + DESCRIPTION + census 测试；R10 无冲突。
- 失 / 让什么变难：
  - **`effect` 一进 DESCRIPTION 就是永久承诺**——此后每个新动词都必须填它，枚举成为兼容面（可加值、不可改名）；
  - **告警疲劳风险**：初期 `os_accepted` 占比 ~80%，模型可能学会无视该字段——缓解靠 DESCRIPTION 写清「`os_accepted` ≠ 生效，要确证请用 verify_state / observe」，把字段定位成「证据分级」而非「判定」；
  - 与 `observe`/`verify_state` 的分工必须在 DESCRIPTION 一刀划清，否则成为第三个冗余观察通道（熵增）。

### 方案 C：维持现状，继续逐动词机会主义补证据

即 2026-08-21/23 两轮实际在走的路（restart verified、scroll at、observe 融合）。

- 得：零契约变更。
- 失：`success:true` 的语义上限继续停留在「OS 受理」，模型无统一入口区分证据等级；每个动词的证据字段各自命名（`verified`/`at`/`clipboard_restored`），模型要逐个记。

## 6. 建议

**采纳方案 B，Stage 1 独立一轮，Stage 2 按需逐动词。** 前置条件两条：

1. 对照 cua-driver 源码复核其枚举变体集（§1 的不确定性），确认 Aleph 四值闭集没有漏掉对方踩过的坑（尤其是否需要一个独立的 `refused`/`blocked` 维度——本文判断不需要：Aleph 的拒绝走 `success:false + message`，与生效证据正交）；
2. ⓒ（审批 fail-dead）裁决先行——若 ⓒ 方向 A 落地改了 desktop 工具的拒绝路径，`effect` 与拒绝语义的边界要在同一份 DESCRIPTION 修订里一次写清，避免两份半契约。

**验收标准（若动工）**：

- 每个变更动词的输出含 `effect` 闭集值，census 测试钉住全覆盖；
- DESCRIPTION 含 `effect` 语义段 + 与 `observe`/`verify_state` 的分工一段；
- `set_value`/`restart_app` 的现有证据改挂到 `effect: verified` 名下，旧字段保留一个版本周期或文档注明别名关系；
- 全量受影响测试面绿（desktop tests.rs + native.rs 内联 + batch 路径）。

## 8. 落地记录（Stage 1，2026-08-23）

- `types.rs`：`ActionEffect` 闭集枚举（四值，snake_case wire）+ 纯函数分类器 `effect_for`；**`SuspectedNoop` 今天无生产者**，按 cua-driver 的诚实出口纪律预留在闭集里（契约值永不加在活契约之下）。
- `mod.rs`：注入点是 `call()` step 7.4 的**单点**（`output.success && classify_approval().is_some()`），不是评审预估的 15 个填充点——分类器从动词**已有**的证据字段派生等级，零动词逐个改；`execute_batch` 的顶层 fold：全成→`confirmed` 当且仅当每个子项都 `confirmed`（链条只及最弱一环），早停且有交付→`partial` + `delivered_count`，首项即败→无等级（那是 `success:false` 已经陈述的事实）。
- DESCRIPTION 新增 effect 契约段（证据分级 ≠ 判定、与 observe/verify_state 分工、refusal 无等级）。
- 测试：`tests.rs::effect_grading`（分类器单测 3 + 闸口 census 1 + mock 集成 6）。
- **刻意未做**：cua-driver 的 `route`/`delivery.mode`/`evidence[]`/`escalation` 四个维度（`delivery: targeted|global` 已在、更名是纯噪音；`evidence[]` 的证据已隐含在既有字段里；`escalation` 撞 R10）；Stage 2 逐动词证据升级；ⓒ 未裁决，拒绝路径与 effect 的边界按「拒绝不带等级」划定，与 ⓒ 的任何出路兼容。
- **真机 e2e（2026-08-23，Ubuntu/XFCE/X11，`examples/desktop_effect_e2e.rs`，17 项全绿）**：真 AT-SPI2 总线上 set_value 的 read-back 给了真 `confirmed`；click 走 global 轨后 type_text 过了真焦点闸；verify_state 在真树上 satisfied。两个环境发现：① mousepad 被脏杀后的**会话恢复提示**是独立状态——此时整棵树只有两个 AXButton、无文本控件（线束用 `ax_action` AXPress「不恢复」自愈）；② XFCE 默认 `org.gnome.desktop.interface toolkit-accessibility=false`，GTK 应用只暴露骨架树——本次已翻为 true（可逆：`gsettings set ... false`）。该线束可作为 ⓔ `qa/desktop/` 的种子。

- 未核实 cua-driver 精确变体集（本地无源，日志空）——动工前的硬性前置；
- 未评估 `data` 顶层化（把 `effect` 提成 `DesktopOutput` 结构字段而非 `data` 内键）——结构字段更严但改动序列化形状，`data` 内键与现有 `delivery` 同居更一致；留给实施轮定；
- 未触及 ⓑ（AX limb 侧半）、element_token——各自仍是独立建议项；
- Stage 2 的 click AX hit-test 在三平台的可行性未验证（AT-SPI2 的 hit-test 能力存疑）。

## 7. 评审遗留（动工前已解决的不在列）
