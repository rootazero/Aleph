# `RightPaneSlot` Trait 接口规范(阶段 1 spec · 2026-10-15)

> **本文是 spec / 调研交付,不是实施代码**。本文件定义 `RightPaneSlot`
> trait 的接口形状、生命周期、状态机与可测试判据,作为阶段 1 实施时
> 的契约基线。配套工程报告:[2026-10-15-rightside-optimization-plan.md](2026-10-15-rightside-optimization-plan.md)
> §3.1.2、§4 阶段 1、§5 风险与红线。
>
> **接口形态基线**:trait 形态(非 enum)、注册式、可空可见性(由 `SlotCtx`
> 派生)、i18n 走 `&'static str` 键。所有决策点已与用户拍板。

---

## 0. 范围与非目标

### 0.1 在范围内(本 spec 覆盖)

- `RightPaneSlot` trait 必填方法集合与签名形状
- `RightPaneRegistry` 注册与查询契约
- `RightPane` 容器(宿主)的最小行为集(宽度/拖拽/折叠/键盘)
- `SlotCtx` 注入上下文(可见性判定、订阅桥接、deep link 锚点)
- 5 个具名 slot 的接口实现要求(SubagentTree / Artifacts / Capabilities / Browser / Approvals)
- 状态机:开/关/拖拽中/折叠/默认 Tab 选择
- 守卫:空可见集合、id 冲突、未注册时 fallback
- 单测/视觉回归/性能预算

### 0.2 不在范围内(本 spec **不**定义)

- 具体 CSS 变量名、像素值、动画曲线(留给阶段 2)
- 实际后端 `RightPaneSlotDescriptor` JSON 字段(留给阶段 2 与 Capability 集成)
- 受控浏览器嵌入的 wire 协议(留待阶段 6,本期仅占位)
- TUI 端右列(留待阶段 7)
- 移动端(Panel iOS)行为(留待后续独立 spec)

### 0.3 与既有约定的关系

- `Workspace Pane` 旧行为(`WorkspaceBody` 枚举 + absolute overlay)在
  阶段 1 **并跑**:旧 5 处联动代码保留,新 trait 注册结果与旧
  `available_with` 求**并集**;阶段 2 切换为单一新接口
- `WorkspaceState`(`state/layout.rs`)的字段(`unseen_artifacts` 等)不
  在本 spec 内改;新接口通过 `SlotCtx` 读取,内部实现可继续用现有
  共享态
- i18n 走 `td_string!(locale, …)` 模式(见 FEATURE_LOCATOR §6.1
  "i18n" 段),不引入新宏

---

## 1. `RightPaneSlot` Trait

### 1.1 方法集合(全 7 个)

| # | 方法 | 必填 | 返回 | 职责 |
|---|---|---|---|---|
| 1 | `id() -> &'static str` | ✅ | stable id | 注册表与 deep link 唯一锚点;**不可变**(编译期常量),运行时不再分配 |
| 2 | `label() -> &'static str` | ✅ | i18n key | **不**返回用户可见字符串,返回 i18n 资源键(同 `td_string!` 约定) |
| 3 | `icon() -> Option<&'static str>` | ❌ | i18n key 或 None | Tab 图标的 i18n 资源键;`None` ⇒ 不渲染图标 |
| 4 | `order() -> u16` | ✅ | 排序权重 | Tab 顺序;**不**要求全局唯一(同 order 时按 `id` 字典序) |
| 5 | `visible(&SlotCtx) -> bool` | ✅ | 派生可见性 | 由 ctx 决定本次渲染是否出现;**纯函数**,无副作用 |
| 6 | `default_open() -> bool` | ✅ | 是否默认开启 | `WorkspaceState::default_open` 初值,用户可覆写 |
| 7 | `render(&SlotCtx) -> impl IntoView` | ✅ | Leptos view | 实际渲染;内部按需订阅事件流(走 `SlotCtx::subscribe`) |

### 1.2 `id()` 约束(硬规则)

- 类型:`&'static str`,编译期字面量
- 命名:全小写蛇形,前缀 `rps.`(right pane slot)以避免与现有命名冲突
- 唯一性:全局注册表中**禁止**重复;`register` 时同 id 已有者 fail-closed(`Err(DuplicateSlotId)`)
- 反例:`rps.subagent_tree`(✅)、`subagent`(❌,无前缀,撞 `subagent_tree/mod.rs` 命名)

### 1.3 `label()` 约束

- 类型:`&'static str`,但**是 i18n 资源键**,不是字面用户文案
- 必须存在于 `interfaces/webchat/src/i18n/<locale>.rs` 的 slot 命名空间下
- 命名建议:`slot.<id>` 形式(例:`slot.rps.subagent_tree`);缺键时 UI 走 `key_not_found` 兜底
- 反例:返回 `"Subagent Tree"`(❌,硬编码英文,撞 §6.1 i18n 教训)

### 1.4 `visible(&SlotCtx)` 状态机

- 输入:`SlotCtx` 不可变引用
- 输出:`bool` —— **本 slot 这次是否该渲染**
- 触发重新计算的时机:`SlotCtx::visible_version` 变化时
- `SlotCtx::visible_version` 的写者(根 Container):
  - `WorkspaceState::mode` 变化(Team/Personal)
  - `is_connected` 变化
  - `active_run_id` 变化
  - 用户折叠/展开右列
  - 用户切换 tab
- **纯函数**:`visible(ctx) == visible(ctx')` 当 `ctx.visible_version == ctx'.visible_version`
- 测试判据:同一 ctx 调两次必须返回同值;ctx 变 → 必须重新求值

### 1.5 `default_open()` 约束

- 首次加载时,`WorkspaceState::default_open` 初值取**所有 `default_open() == true` 的 slot id**
- 用户操作(关闭/钉选)覆盖初值;用户态在 `WorkspaceState::user_opened: BTreeSet<&'static str>` / `user_closed: BTreeSet<&'static str>` 持久化
- 三态机:`user_opened` 优先 → `user_closed` 优先 → `default_open()`

### 1.6 `render(&SlotCtx)` 契约

- **不**保证在 `visible() == false` 时不被调用(实现可以同时优化两者)
- 内部订阅事件流必须经 `SlotCtx::subscribe(topic) -> Receiver<T>`,**不**直连 `DashboardState`
- 内部不允许捕获 `Signal` 持有时间超过自身 mount;unmount 由 Leptos 生命周期保证
- 渲染必须是无副作用的(纯函数 over ctx);副作用(订阅、订阅清理)由 Leptos effect 托管
- 错误处理:`render` 内部 panic 必须由右列 catch 并显示空状态卡,**不**冒泡到 main chat

### 1.7 关键不变量(违反即 bug)

1. `id()` 在整个程序生命周期内**不变**(否则 `user_opened` 持久化语义破)
2. `visible` 必须是纯函数(否则依赖追踪坏)
3. `render` **不**直接调 `stream.chat` / LLM(R7 反对)
4. 任何实现**不**引入新三方库(R3 反对)
5. 实现**不**写跨 turn 全局可变状态到 `WorkspaceState` 以外的容器(避免"右列"成为第二份事实源)

---

## 2. `RightPaneRegistry`

### 2.1 单一真源

- 位置:`interfaces/webchat/src/components/right_pane/registry.rs`
- 类型:`pub struct RightPaneRegistry { entries: BTreeMap<&'static str, Arc<dyn RightPaneSlot>>, default_open: BTreeSet<&'static str> }`
- 唯一构造点:启动期 `app.rs::start` 中一次性 `RightPaneRegistry::new()` + `register(...)` × N
- 注册顺序:由 `order()` 升序(同 order 按 `id` 字典序)——`RightPane` 渲染时按此序遍历

### 2.2 `register` 契约

```text
pub fn register(
    &mut self,
    slot: Arc<dyn RightPaneSlot>,
) -> Result<(), RegistryError>;
```

- **重复 id ⇒ `Err(DuplicateSlotId(id))`**,不静默替换
- 启动期:第一个 `Err` 必须使进程启动失败(`expect("right pane registry")`)
- 测试期:必须能被捕获并断言

### 2.3 `RightPaneRegistry` 行为(纯函数)

| 方法 | 签名 | 职责 |
|---|---|---|
| `iter()` | `-> impl Iterator<Item = (&'static str, Arc<dyn RightPaneSlot>)>` | 按 `order` + `id` 排序遍历 |
| `get(id)` | `-> Option<Arc<dyn RightPaneSlot>>` | O(log n) 查 |
| `default_open_ids()` | `-> &BTreeSet<&'static str>` | 初值,给 `WorkspaceState` 注入 |
| `len()` | `-> usize` | 调试用 |

### 2.4 与 `WorkspaceState` 的边界

- `WorkspaceState::default_open: BTreeSet<&'static str>` 由 registry 注入,**不**是 registry 字段
- registry 是只读(构造后),`WorkspaceState` 持有运行时态
- 持久化键:slot id(字符串)→ 用户的"开/关/钉"决策;`WorkspaceState::persisted_state` 独立于 registry 序列化

---

## 3. `SlotCtx`

### 3.1 必填字段

| 字段 | 类型 | 用途 |
|---|---|---|
| `locale: Locale` | 复制 | i18n |
| `mode: LayoutMode` | 复制 | Team/Personal 决定可见 slot 集合 |
| `is_connected: bool` | 复制 | 断线/重连 |
| `active_run_id: Option<RunId>` | 复制 | 当前活跃 run,subagent tree 派发用 |
| `visible_version: u64` | 复制 | `visible` 缓存键 |
| `user_opened: BTreeSet<&'static str>` | 引用 | 用户已开的 slot id 集合 |
| `user_closed: BTreeSet<&'static str>` | 引用 | 用户已关的 slot id 集合 |
| `topic_bus: &dyn TopicBus` | trait obj | `subscribe(topic) -> Receiver<T>` |
| `services: &ServiceHub` | 引用 | subagent / artifact / capability 后端句柄(只读) |

### 3.2 派生函数

```text
fn effective_open(&self, slot: &dyn RightPaneSlot) -> bool {
    if self.user_opened.contains(slot.id()) { return true; }
    if self.user_closed.contains(slot.id()) { return false; }
    slot.default_open()
}
```

- 唯一决策点(防止 R7 反模式:**不**用 LLM 决断)
- 测试:100% 覆盖三条分支 + 组合(同 id 在两集合中时 `user_opened` 胜)

### 3.3 `subscribe` 边界

- `topic_bus.subscribe(topic) -> Receiver<T>` 必须**只**接受 registry 内部已登记的 topic
- 越界 topic 立即 `Err(UnknownTopic(topic))`
- 反例:slot 偷偷订阅 `stream.chat.*` ⇒ fail-closed,启动期 panic

---

## 4. `RightPane` 容器(宿主)

### 4.1 渲染位置

- 顶层 shell 三段:`[ModeSidebar | Main | RightPane]`
- `<RightPane>` 是一个新组件,接受 `Arc<RightPaneRegistry>` + `Signal<WorkspaceState>`
- 旧 `WorkspacePanel` 的 absolute 行为**保留 1 个 release**(标 `data-legacy="absolute"`),作为回退开关;默认走新 flex 路径

### 4.2 必填行为

| 行为 | 实现要点 | 测试判据 |
|---|---|---|
| 宽度状态 | `clamp(min=280px, max=max(280px, 视口宽 80%), fallback=280px)` | 边界 280 / `max(280, 80%)` / 视口异常 / 超界输入 |
| 拖拽 | pointer capture + `set_property(--aleph-workspace-w)` 文档根 | 拖动期间 `SecondTick` 不订阅;抬起持久化 |
| 折叠/展开 | 快捷键 ⌘[ / ⌘]`(PC)/ `Cmd+Shift+R`(TUI 端为 `'`)| 折叠后下次展开恢复 `active_tab` |
| Tab 切换 | 点击 / 方向键 / `Tab` 键可达 | `active_tab` 变化触发 `visible_version` bump |
| 空态 | 所有 slot `visible() == false` ⇒ 显示"右栏为空"占位卡 | 强制断言占位卡存在,防止静默空白 |
| 错误捕获 | `render` panic ⇒ 捕获后该 slot 显示错误卡(不冒泡) | 注入一个 panic slot,验证只影响该 slot |

### 4.3 与 `WorkspaceState` 的接口

- 读取:`right_pane_width` / `active_tab` / `user_opened` / `user_closed` / `collapsed`
- 写入:`toggle_tab(id)` / `set_width(px)` / `collapse()` / `expand()` / `pin(id)` / `unpin(id)`
- **不**写:`default_open` (初始化时一次性注入)
- **不**新增 `WorkspaceState` 字段(R10 反对;复用现有信号)

### 4.4 Reducers 与 effect 边界

- Tab 切换:reducer 内事务,`active_tab = id; user_opened.insert(id); visible_version += 1`
- 拖拽:pointer move 只写 `set_width`,抬起才持久化到 `persisted_state`
- 持久化:同 `WorkspacePanel` 既有 `layout.rs` 路径,无新 store

---

## 5. 5 个具名 slot 的接口实现要求

### 5.1 `SubagentTreeSlot`(`rps.subagent_tree`)

- `default_open() = true`(右列默认打开)
- `order() = 100`
- `visible() = true`(无条件)
- `render`:把现有 `subagent_tree/mod.rs:60-152` 的递归渲染搬入;订阅 `subagent.tree` topic
- 数据流:`subagent.tree` RPC 拿一次初始值,后续 `TREE_TOPIC` 增量
- **`/dashboard/subagents` deep link**:打开时若右列未开则 `RightPane::expand_with("rps.subagent_tree")`
- 关键不变量:不持有"已订阅的 slot id"缓存,unmount 时 unsub 走 Leptos 生命周期

### 5.2 `ArtifactsSlot`(`rps.artifacts`)

- `default_open() = true`(与 subagent tree 共开;宽度够时双 tab 可见)
- `order() = 200`
- `visible() = true`
- `render`:把现有 `components/artifacts/mod.rs:145-330` 搬入
- 数据流:`artifacts.changed` topic + WorkspaceState::unseen_artifacts 计数
- 与 `unseen_artifacts`:在右列 tab 标题显示红点(沿用既有 `unseen_*` 模式)

### 5.3 `CapabilitiesSlot`(`rps.capabilities`)

- `default_open() = false`(用户首次打开时拉后端)
- `order() = 300`
- `visible() = self.last_revision_loaded.is_some() || self.fetch_attempted`(首次尝试后才显示)
- `render`:只读卡片 + 状态徽章 + 折叠态只显示一个 dot
- 数据流:启动时**不**自动 fetch;用户点击 tab 时 `CapabilityService::health()` 拉一次,后续 `subscribe()` 走 `CapabilityChangeStream`
- 后端:`src/bin/aleph-server/commands/start/mod.rs` 暴露 `cmd.health` RPC(阶段 4 落地)

### 5.4 `BrowserSlot`(`rps.browser`)

- `default_open() = false`
- `order() = 400`
- `visible() = self.binding.is_some()`(没 bind 的 browser 不显示)
- `render`:本期**仍**走 `LaunchSection.open` → `_blank`(零行为变化);占位 + TODO 注释
- 关键不变量:**不**引入 webview tag / iframe / CDP(阶段 6 之前是占位)

### 5.5 `ApprovalsSlot`(`rps.approvals`)

- `default_open() = false`
- `order() = 500`
- `visible() = self.has_pending()`(没有 pending 不显示)
- `render`:把 `ApprovalCard` 复制到右列(共享组件,见 `approval_card.rs:60` 起)
- 数据流:`stream.approval.requested` topic,过滤当前 session_id
- 与聊天内 `ApprovalCard` 共存:钉到右列后**不**删除聊天内卡片(双显,各走各的生命周期)

---

## 6. 命名约定(避免再添 4 个相似文件名)

### 6.1 slot 命名

- `SubagentTreeSlot` = 右列 slot,数据源 `subagent.tree` RPC + `TREE_TOPIC`
- **不**用 `AgentPanel` / `agents_panel` / `agents_sidebar` / `agents_overlay` 这 4 个旧名(语义不同,见工程报告 §1.3)
- 文件:`slots/subagent_tree.rs` 严格区分旧名

### 6.2 文件命名

- 旧 4 个文件名**不**改(改动 import 太多);新代码严守
- `RightPaneSlot` 实现文件:`slots/<id_suffix>.rs`,与 `id` 一一对应
- `slots/mod.rs` 集中 `pub use` + 顶部 1 段语义注释锁定

### 6.3 `deny(unreachable_pub)` 防御

- 整个 `right_pane/` 模块加 `#[deny(unreachable_pub)]`
- 防止下次再创建"长得像但语义不同"的新名字

---

## 7. 状态机(`RightPane`)

### 7.1 顶层状态

```text
RightPaneState {
    collapsed: bool,        // false 默认
    width_px: u32,          // 展开时为 280..=max(280, 视口宽 80%)
    active_tab: Option<&'static str>, // None = 未选中(右列开但无 tab)
    dragging: bool,         // 拖拽中
}
```

### 7.2 状态转移(纯函数)

| from | event | to | 副作用 |
|---|---|---|---|
| 任何 | `expand()` | `collapsed = false` | 写 persisted_state |
| 任何 | `collapse()` | `collapsed = true; active_tab = None` | 写 persisted_state |
| `collapsed` | 用户点 tab | `collapsed = false; active_tab = id` | 写 user_opened |
| `!collapsed` | 用户点已激活 tab | `collapsed = true`(再点折叠) | 写 persisted_state |
| `!collapsed, !dragging` | pointer down on resizer | `dragging = true` | none |
| `dragging` | pointer move | `width_px = clamp(新值)` | 写 CSS var,不写 persisted_state |
| `dragging` | pointer up | `dragging = false; width_px = clamp(最终值)` | 写 persisted_state |

### 7.3 边界条件

- `width_px` 不编码折叠态；展开时遵守 `[280px, max(280px, viewport × 80%)]`，折叠由 `collapsed` 单独表示
- `persisted_state` 反序列化失败 ⇒ fallback 到 default,日志 warn
- 启动期 `registry` 0 个 slot ⇒ `RightPane` 不渲染(空 `<aside>` 占位)

---

## 8. 测试矩阵(可执行判据)

### 8.1 单元测试(必过,合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| UT-1 | `registry_register_unique_ids_succeeds` | 5 个不同 id 注册成功,iter 顺序按 order + id 字典序 |
| UT-2 | `registry_register_duplicate_id_fails_closed` | 同 id 二次注册返回 `Err(DuplicateSlotId)`,**不**覆盖 |
| UT-3 | `slot_visible_is_pure_function` | 同 ctx 调两次结果相等;ctx 改 → 重新求值(用 prop 测试) |
| UT-4 | `slot_id_is_static` | 编译期保证 `id() -> &'static str`(返回引用不可变) |
| UT-5 | `effective_open_three_way_decision` | `user_opened` ∪ `user_closed` ∪ `default_open` 三态机 |
| UT-6 | `slot_render_does_not_panic_on_empty_ctx` | `render(&SlotCtx::default())` 不 panic(空卡可接受) |
| UT-7 | `width_clamp_boundaries` | 280 / `max(280, 80%)` / 视口异常 / 超界输入 |
| UT-8 | `persisted_state_roundtrip` | 序列化 → 反序列化后 `user_opened` / `user_closed` / `active_tab` 一致 |
| UT-9 | `subscribe_unknown_topic_fails_closed` | `topic_bus.subscribe("never.allowed")` 返回 `Err` |
| UT-10 | `topic_bus_subscribe_whitelist_contains_all_five_slots` | 5 个 slot 各自订阅的 topic 都在白名单 |

### 8.2 视觉回归(必过,合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| VS-1 | `right_pane_default_state_personal` | Personal 模式右列默认开,subagent_tree + artifacts 双 tab |
| VS-2 | `right_pane_default_state_team` | Team 模式同上,team 任务条在右列上方 |
| VS-3 | `right_pane_empty_state` | 所有 slot `visible == false` 时占位卡 |
| VS-4 | `right_pane_dragging_visual` | 拖拽中宽度变化、cursor 变 resize、双向 |
| VS-5 | `right_pane_collapsed_visual` | 折叠后 main 区吃回宽度,无残影 |
| VS-6 | `right_pane_320px_boundary` | 视口 320px 宽时右列宽度 0(自动折叠) |
| VS-7 | `right_pane_640px_boundary` | 视口 640px 宽时双 tab 显示完整 |
| VS-8 | `right_pane_1024px_boundary` | 视口 1024px 宽时拖拽手柄可见 |

### 8.3 端到端 smoke(必过,合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| E2E-1 | `open_close_via_shortcut` | ⌘[ / ⌘]` 切换右列 |
| E2E-2 | `switch_tab` | 点击 4 个 tab,active_tab 切换正确 |
| E2E-3 | `drag_to_boundary` | 拖到 280px / 80% 上限 |
| E2E-4 | `pin_attachment` | 钉一个附件,跨 turn 仍在 |
| E2E-5 | `approval_card_pin` | 审批卡片钉到右列,resolve 后两处同时消失 |
| E2E-6 | `subagent_tree_deep_link` | `/dashboard/subagents` 打开时右列未开则自动开 + 切到 subagent |

### 8.4 性能预算(可接受判据)

| 项 | 上限 | 实测 |
|---|---|---|
| 切换 Tab | < 50ms | (留待阶段 2 测) |
| 打开右列 | < 100ms | (留待阶段 2 测) |
| 缩略图 hover 生成 | < 200ms | (留待阶段 5 测) |
| `subagent.tree` 事件回放 | < 300ms @ 节点数 ≤ 200 | (留待阶段 3 测) |

---

## 9. 红线与不做清单(重复 + 收紧)

### 9.1 红线(R1–R10)复述

- R1 — Rust Core 不渲染右列;trait 是 Leptos 概念
- R2 — 复杂业务 UI 留在 Leptos;Rust 端仅暴露 `RightPaneSlotDescriptor`(阶段 2)
- R3 — 不引入新三方库;`use_drag` / `use_resize` 走现有 `web_sys` + `leptos_use`
- R4 — `interfaces/cli` / `interfaces/tui` 不引入右列抽象的**业务**概念
- R5 — Pin/通知不绕过 `GlobalBus`
- R6 — 移动端(Phone/Tablet)本期不同步;在文档里标注"桌面端优先"
- R7 — `visible` / `effective_open` **不**调 LLM;rule 决定
- R8 — `Pin` / `Open in new window` 等操作暴露为 `WorkspaceAction` 工具(阶段 5 落地)
- R9 — "哪个 tab 该在前面"的智能行为移到 prompt,**不**写代码
- R10 — 本次新增 ≤ 7 文件(`right_pane/{mod,slot,registry,pane}.rs` + `slots/{mod,subagent_tree,artifacts,capabilities,browser,approvals}.rs`),**不**污染 harness

### 9.2 已知不做

- ❌ 受控浏览器嵌入(阶段 6 spec-only)
- ❌ TUI 右列(阶段 7,需独立审批)
- ❌ 移动端(独立 spec)
- ❌ 合并 `ApprovalCard` 与 `AskUserCard`(语义不同,见 §5.5)
- ❌ 在右列里直接调 `stream.chat`(R9 反对)
- ❌ 用 LLM 决断 `default_open` 或 `visible`(R7 反对)
- ❌ 引入 webview tag / iframe(阶段 6 之前)
- ❌ 改 `WorkspaceState` 既有字段(只读)
- ❌ 改 `WorkspacePanel` 的 resizer / z-index / 动画细节(阶段 2 一次到位)

---

## 10. 验收标准(阶段 1,合入门槛)

阶段 1 启动条件:**用户批准本 spec**(本季度内)。阶段 1 完成的合入门槛:

1. ✅ 所有 8 个文件落地(§9.1 R10 文件清单),`cargo build -p aleph-panel` 通过
2. ✅ `RightPaneRegistry` + 5 个 slot 的注册代码就位
3. ✅ 旧 `WorkspaceBody` 枚举 / `available_with` / 5 处联动代码仍编译通过(并跑期)
4. ✅ 10 条单元测试全绿(UT-1 ~ UT-10)
5. ✅ 8 条视觉回归脚本通过(VS-1 ~ VS-8)
6. ✅ `RightPane` 容器**未**触发实际渲染(本阶段仅注册,阶段 2 切换 host)
7. ✅ 文档:R10 文件清单 + 此 spec 在工程报告 §4 阶段 1 的"启动条件"回写
8. ✅ 红线守住:不引入新三方库、不改 Rust Core、不调 LLM、命名严格区分

---

## 11. 附录 — 与工程报告章节的对应

| 本 spec 章节 | 工程报告章节 | 关系 |
|---|---|---|
| §1 | 工程报告 §3.1.2 | 接口形状,本 spec 是细化 |
| §2 | 工程报告 §4 阶段 1 | 注册器落地 |
| §3 | 工程报告 §3.1.2 | SlotCtx 注入 |
| §4 | 工程报告 §4 阶段 2 | 容器行为(本阶段仅注册) |
| §5 | 工程报告 §4 阶段 3–6 | 5 个 slot 的接口要求 |
| §6 | 工程报告 §1.3 ⚠ 命名陷阱 | 命名约定 |
| §7 | 工程报告 §5.1 战略风险 | 状态机 + 边界 |
| §8 | 工程报告 §6 验证方法 | 测试矩阵 |
| §9 | 工程报告 §5.3 红线 | 重复 + 收紧 |
| §10 | 工程报告 §4 阶段 1 验收 | 合入门槛 |

---

*Last updated: 2026-10-15 · 阶段 1 spec · 与工程报告配套,本季度可独立 ship*
