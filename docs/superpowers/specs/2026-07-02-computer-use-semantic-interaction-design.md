# Computer Use 语义交互升级 — 设计文档 (2026-07-02)

> Design spec for the computer-use (desktop control) refactor: semantic AX action
> path with write verification, act→observe fusion, blocked-app safety guard, and
> structured error-recovery hints. Reference projects: orca, UI-TARS-desktop.

## 1. 背景与差距分析

Aleph 的 computer use 栈（`src/builtin_tools/desktop/` + `desktop/shared/` +
三平台原生实现）已达 UI-TARS-desktop parity：Pythonic action DSL 解析
（`action_script.rs`）、归一化坐标（`coord_resolve.rs`）、截图回注
（`harness/agent/prompt.rs` hoisted image blocks）、`finished`/`call_user`
loop-control、batch、safety hard-block、审批、会话锁、Escape 中止、vision
bridge、SOM、gui_locate、AX 查询（macOS + Windows UIA）。

对照 orca（`/Volumes/TBU4/Github/orca`）逐项审计后，确认四个真实缺口：

| # | 缺口 | orca 的做法 | Aleph 现状 |
|---|------|-------------|-----------|
| G1 | **语义 AX 动作路径 + 写后验证** | `setValue` 走 accessibility 路径（`AXUIElementSetAttributeValue`）直接设值并读回验证，返回 `verification: verified/unverified`；`performSecondaryAction` 触发原生 AX action（AXPress 等）；synthetic 输入标记 `path: synthetic` 提示模型必须再观察 | 所有输入均为 synthetic 盲打（enigo 合成事件），无 path/verification 元数据；表单填写只能 click+type_text，长文本、非 ASCII、已有内容清除都不可靠 |
| G2 | **act→observe 合一** | 每个动作返回动作后的新快照；UI-TARS 则每动作后自动截图注入 | 模型每个动作后要多一轮 `screenshot`/`ax_snapshot` 调用才能确认效果 |
| G3 | **敏感 app 硬阻断** | 8 个密码管理器 bundle id 硬编码黑名单，任何操作直接 `app_blocked` | `safety.rs` 只挡内容 payload（RCE/rm -rf/fork bomb/危险键），不挡目标 app——前台是 1Password 时 click/type 畅通无阻 |
| G4 | **结构化错误恢复提示** | 每类错误码附「下一步怎么办」（re-run get-app-state、indexes go stale、do NOT retry unchanged） | 失败只回 message 字符串，模型常原样重试（违 A2「压缩错误进 context 自愈」精神） |

已排除的伪缺口：Windows AX（已连线，UIA→AX role 映射）、`open_path`
（已在 `system_tool` 暴露）、Linux AX（AT-SPI 需新增 zbus 级依赖，违 R3，
显式推迟）、orca 式 app-state 统一快照重构（破坏现有 32-action 面，违 P6）。

## 2. 方案选型

- **方案 1 — Rust-only 打磨**：G2+G3+G4 + ax_snapshot 截断元数据。低风险，
  但不触及可靠性核心 G1。
- **方案 2 — 语义交互升级（选定）**：方案 1 + G1。能力契约扩展贯穿
  protocol → Swift bridge → trait → tool 四层，macOS 先行，Windows/Linux 经
  trait default `NotImplemented` 自动缺位（复用 PIM 四域既有模式）。
- **方案 3 — orca 式 app-state 大重构**：element-index 交互 + Swift 快照缓存。
  改动面最大且推翻现有 action 面，违 P6/三次法则，不选。

## 3. 设计

### 3.1 G1 · 语义 AX 动作路径（set_value / ax_action）

**Protocol 层**（`shared/protocol/src/desktop_bridge/methods/ax.rs`）：

```rust
pub const METHOD_SET_VALUE: &str = "ax.set_value";
pub const METHOD_PERFORM_ACTION: &str = "ax.perform_action";

/// Stateless element locator — the bridge re-walks the AX tree per call and
/// picks the best match. No element handles cross the IPC boundary.
pub struct AxLocator {
    pub pid: Option<i32>,        // None = frontmost app
    pub role: Option<String>,    // e.g. "AXTextField"
    pub title: Option<String>,   // fuzzy: exact > contains (case-insensitive)
    pub center: Option<[f64; 2]>,// nearest-center tiebreak (global px)
}
pub struct SetValueParams { pub locator: AxLocator, pub value: String }
pub struct PerformActionParams { pub locator: AxLocator, pub action: String } // e.g. "AXPress"

pub struct AxActionResult {
    pub performed: bool,
    pub path: String,                    // "accessibility"
    pub matched: Option<AxElement>,      // the element acted on (sans children)
    pub verification: AxVerification,
}
pub struct AxVerification {
    pub state: String,                   // "verified" | "unverified"
    pub reason: Option<String>,          // "value_mismatch" | "value_unreadable" | …
    pub actual_preview: Option<String>,  // first 200 chars of read-back value
}
```

定位为**无状态 locator**而非 orca 的快照缓存 + element index：bridge 每次调用
现场 re-walk（`ax.query_tree` 已有同款遍历），按 role 过滤 → title 匹配
（精确 > 包含）→ center 最近邻打分取最优。杜绝跨 IPC 的元素句柄生命周期
问题，符合现有 bridge 无状态设计。多候选歧义时返回 error + 候选摘要
（模型可加 center 消歧）。

**Swift 层**（`desktop/macos/bridge/Sources/AlephBridge`）：
- `ax.set_value`：定位 → `AXUIElementSetAttributeValue(kAXValueAttribute)` →
  读回 `kAXValueAttribute` 与期望比较 → `verification.state`。
- `ax.perform_action`：定位 → `AXUIElementPerformAction(action)`。动作名单
  透传（AXPress/AXConfirm/AXCancel/AXRaise/AXShowMenu…），未支持的动作由
  AX API 自然报错。
- 两方法登记进 handshake `supported_methods`；旧 helper 收到新方法回
  -32601，Rust 侧 lift 为 `DesktopError`，工具层给恢复提示（G4）。

**Trait 层**（`desktop/shared/src/traits/ax.rs`）：
`AccessibilityCapability` 新增 `set_value` / `perform_action`，默认体返回
`NotImplemented`——Windows（UIA ValuePattern/Invoke 留待后续）与未来平台
自动缺位；macOS override 代理 bridge。

**Tool 层**（`src/builtin_tools/desktop/`）：
- 新 action `set_value`：`{action:"set_value", pid?, role?, element_title?, x?, y?, text}`
  （x/y 作 center 消歧，接受 coord_space 归一化）。审批类型 `DesktopType`，
  `text` 过 `safety::check_typed_text` 硬阻断。输出 data 携带
  `{path, matched:{role,title}, verification}`。
- 新 action `ax_action`：`{action:"ax_action", ax_action_name:"AXPress", pid?, role?, element_title?, x?, y?}`。
  审批类型 `DesktopClick`。
- 平台无 AX（Linux）或 helper 过旧 → 失败 message 附「fall back to
  click/type_text」提示。
- DESCRIPTION 更新：教模型「优先 set_value 填表单（verified 写入），
  type_text 是 unverified 的 synthetic 兜底」。

### 3.2 G2 · act→observe 合一（`observe` 参数）

`DesktopArgs`/`DesktopBatchAction` 新增 `observe: Option<String>`
（`"state"` | `"screenshot"`；缺省 None = 现行为 byte-identical）：

- 仅对**变更类 leaf action 成功后**生效（classify_approval 为 Some 的集合，
  减 batch）；读类 action 忽略。
- `"state"`：sleep 300ms（UI-TARS loopInterval parity）→ 收集
  `{frontmost_app, focused_window_title, focused_element:{role,title,value?}}`
  （frontmost 经 `SystemCapability::list_running_apps` 的 `is_active`，
  focused 经 `ax.query_focused`，AX 缺位平台自动省略该字段）→ 挂到
  `data.post_state`。纯文本，token 开销极小。
- `"screenshot"`：`"state"` + 复用既有 wait_visual 短轮询（2 次采样判稳，
  上限 ~1.5s）后走既有 screenshot 预算管线（auto-JPEG/降采样），图随
  result_processing 既有 hoist 管线注入。
- batch：sub-action 继承 batch 级 `observe`（同 coord_space 继承模式）；
  典型用法「最后一个动作带 observe」由模型自行决定。
- DESCRIPTION 更新：教模型「动作后想确认效果就带 observe，省一轮往返」。

### 3.3 G3 · 敏感 app 硬阻断

`safety.rs` 新增 `BLOCKED_FRONTMOST_APPS`（orca 清单为基础：1Password、
Bitwarden、Dashlane、LastPass、NordPass、Proton Pass × bundle-id 前缀 +
名称兜底匹配，覆盖三平台命名差异）：

- **前台守卫**：mutating action（classify_approval Some，含 batch 的每个
  sub-action 经递归自然覆盖）执行前查 frontmost app（`list_running_apps`
  找 `is_active`），命中黑名单 → 拒绝：`"Refused: '<app>' is a password
  manager — computer use is blocked in it for credential safety."`。
  frontmost 查询失败时**放行**（fail-open：这是纵深防御的附加层，不因
  查询抖动阻断正常操作；审批策略与内容硬阻断仍在）。
- **目标守卫**：`launch_app`/`quit_app`/`restart_app` 的 bundle_id/名称
  命中黑名单 → 同样拒绝（不帮模型把密码管理器带到前台）；`focus_window`
  经 `window_list` 反查 `window_id` 的 owner 名称后同判（查不到即放行，
  与前台守卫同一 fail-open 语义）。
- 位置：`check_hard_block` 同层（approval 之下的无条件层），但因需
  async 平台查询，作为 `call()` 中紧邻 hard_block 的独立 pre-flight。

### 3.4 G4 · 结构化错误恢复提示

新增 `src/builtin_tools/desktop/recovery.rs`（~80 行）：失败类别 → 一句
「下一步怎么办」，追加到失败 `DesktopOutput.message` 尾部（`" Hint: …"`），
不加新字段（信封零破坏）：

| 失败类别 | 提示 |
|---|---|
| window/element not found | Re-run desktop_ax_snapshot / window_list — indexes and geometry go stale after navigation or focus change. Do not retry unchanged. |
| AX capability unavailable | Platform has no accessibility tree — use screenshot + click coordinates instead. |
| permission denied | Surface the PermissionGuide steps to the user (desktop_check_permissions). |
| bridge disabled/backoff | Desktop helper is restarting — wait a moment, or fall back to screenshot-based actions. |
| set_value unverified / ambiguous locator | Re-observe with desktop_ax_snapshot, then disambiguate with x/y center. |
| coordinate out of bounds | Take a fresh screenshot and use its coordinate_space block to rescale. |

应用点：`native.rs` 错误路径、`ax.rs`、`gui_locate.rs`、新 set_value/ax_action。
分类基于 `DesktopError` variant + message 关键词的**机器生成文本**匹配
（不违 P8——这是对结构化错误的路由，不是对自然语言的语义判断）。

### 3.5 打磨 · ax_snapshot 截断与聚焦元数据

`desktop_ax_snapshot`（`ax.rs`）输出补齐 orca 式自描述：
`truncated: bool`、`total_seen` / `returned` 计数、`focused_id`（当前聚焦
元素在快照中的序号，若在）。模型据此知道「树被裁过、焦点在哪」。

## 4. 数据流（G1 主链）

```
LLM {action:"set_value", role:"AXTextField", element_title:"Email", text:"a@b.c"}
  → DesktopTool::call: hard_block(text) → blocked-app pre-flight → approval(DesktopType)
  → session lock → coord normalize (若带 x/y)
  → platform.ax().set_value(locator, value)          [trait]
  → SwiftBridge::call("ax.set_value", …, 60s timeout) [IPC]
  → AlephBridge: walk AX tree → best-match → AXUIElementSetAttributeValue
      → read back → AxActionResult{verification}
  → DesktopOutput{success, data:{path:"accessibility", matched, verification}}
  → (可选 observe:"state" → post_state 附带)
```

## 5. 错误处理

- 定位失败/歧义：`success:false` + 候选摘要 + G4 提示；不猜。
- 只读元素 set_value：AX API 报错原样透传 + 提示 fall back type_text。
- 旧 helper（-32601 method not found）：提示「helper 需随 server 升级；
  暂用 click/type_text」。
- 验证 mismatch：`performed:true` 但 `verification.state:"unverified"` +
  `reason` + `actual_preview` — 动作已发生，如实报告（不把成功改成失败）。
- blocked-app 查询失败：fail-open（见 3.3）；set_value 的 AX 走 bridge
  超时（60s 默认）已有钳制。

## 6. 测试

- `safety.rs`：黑名单匹配（bundle-id 前缀/名称、大小写）单测。
- `recovery.rs`：分类→提示映射单测。
- `types.rs`/`mod.rs`：`observe`/`set_value` 参数 plumbing、审批分类
  （set_value→DesktopType、ax_action→DesktopClick）、hard_block 覆盖
  set_value 文本——扩展现有 `tests.rs` 模式。
- locator 打分（若纯函数部分落在 Rust 侧共享）：单测；Swift 侧匹配逻辑
  由 `just bridge-test` 单测覆盖（Swift 单测已有基建）。
- 协议 golden fixture：`just bridge-schema` 再生成。
- e2e（`bridge_e2e`，`--ignored`）：set_value 对 TextEdit 往返——列出但
  默认不跑（需 TCC 权限，属手动 QA）。

## 7. 非目标（显式推迟）

- Linux AX（AT-SPI/zbus 新依赖，违 R3）。
- Windows UIA ValuePattern/Invoke 的 set_value/perform_action 实现
  （trait default 缺位即可；无法本机编译验证，留待 Windows 会话）。
- orca 式快照缓存与 element-index 寻址（无状态 locator 已覆盖需求）。
- 窗口局部坐标系（现行全局 px + normalized 双模已自洽）。
- 审批「记住临时授权」增强（独立议题）。

## 8. 红线自检

- R1：所有平台 API 仍在 `desktop/*`（Swift bridge + trait），`src` 只碰契约。✅
- R7/R9/R10：无新增 harness 逻辑；verification/post_state/hint 都是
  「给模型更多可见性」，决策权全留 LLM；G4 是 A2 采纳条款的落地。✅
- R3：零新第三方依赖。✅
- P6：locator 无状态设计避免快照缓存机件；observe 缺省 None 保持
  byte-identical。✅
