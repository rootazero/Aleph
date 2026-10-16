# 阶段 2 Spec:容器层 Host 切换(`WorkspacePanel` Absolute → Flex 第三列 · 2026-10-15)

> **本文是调研交付,不是实施代码**。本文件锁定 阶段 2 容器层
> host 切换的事实基线、迁移路径、宽度状态机、视觉与动画、验证
> 判据与红线,作为本季度(2026 Q4)独立 ship 的契约基线。
>
> 配套文档:
> - 工程总方案:[`2026-10-15-rightside-optimization-plan.md`](2026-10-15-rightside-optimization-plan.md)
> - 阶段 1 spec(`RightPaneSlot` trait):[`2026-10-15-rightpane-stage1-slot-spec.md`](2026-10-15-rightpane-stage1-slot-spec.md)
> - 回链位:[`docs/reference/FEATURE_LOCATOR.md` §6.1](../reference/FEATURE_LOCATOR.md#61-流式回显与工作区面板-streaming-echo--workspace-panel)
>
> **关键纠错**(主方案 §3.1.1 误):
> - 旧判定"`WorkspaceBody` 5 处联动代码"实际为 **7 个 touch point**(1 组件 + 1 state + 1 mount + 3 token 读者 + 1 `LayoutToggle`)
> - 旧判定"宽度 clamp 0/50%"实际为 **`[280, 80% viewport]`**(`clamp_width` 在 `state/layout.rs:132-147`)
> - 旧判定"absolute overlay 治本"实际为 **"absolute 改 flex 兄弟列"**,不是把 absolute 内部改成 flex

---

## 0. 范围与非目标

### 0.1 在范围内(本 spec 覆盖)

- 现有 `WorkspacePanel` 架构事实基线
- 7 个 touch point 的精确位置与改动面
- 从 `<aside absolute>` 切到 `<aside flex>` 的最小 diff
- 宽度 token `--aleph-workspace-w` 的写者/读者重构
- `LayoutMode::ChatOnly/Split` 切换语义保留
- 拖拽/折叠/快捷键的迁移兼容性
- 视觉过渡(`workspace-collapsed` → flex `width` transition)
- 双 path 切换开关(`?legacy_pane=1` / feature flag)
- 视觉回归 + 端到端 smoke 矩阵
- 已知限制(viewport resize 期间 ResizeObserver)的迁移决定

### 0.2 不在范围内(本 spec **不**定义)

- `RightPaneSlot` trait 接口(已在阶段 1 spec 锁定,本期不重述)
- 5 个具名 slot 的实现细节(阶段 3-6)
- 受控浏览器嵌入(阶段 6)
- TUI 端右列(阶段 7)
- 移动端 / 平板 / 极小视口(< 320px)行为
- CSS 变量名 / Tailwind 工具类具体名字(留给 PR review)

### 0.3 决策复用(本阶段 0 拍板)

来自 阶段 0 决策点确认(m00161):

| 决策点 | 拍板 |
|---|---|
| 3.1.1 容器形态 | **A (flex 第三列)** — 治本 |
| 3.1.2 slot 抽象 | **trait** — 注册式 |
| 3.1.3 subagent 默认 Tab | **是**(本期不实施,阶段 3) |

---

## 1. 事实基线(从代码读出)

### 1.1 现有 `WorkspacePanel` 形态

`interfaces/webchat/src/components/workspace_panel.rs:1-435` 渲染:

```text
<aside class="aleph-workspace-pane
            absolute inset-y-0 right-0 z-20
            flex flex-col
            glass border-l border-border bg-surface-overlay/95 shadow-xl
            min-w-[280px] w-[var(--aleph-workspace-w)]
            overflow-hidden"
      class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split>
```

- **绝对定位浮层**:`absolute inset-y-0 right-0 z-20`(`workspace_panel.rs:117`)
- **玻璃态**:`glass bg-surface-overlay/95 shadow-xl`(浮在 chat 上)
- **always-mounted**:通过 `workspace-collapsed` 修饰符 + 200ms slide/fade 切换可见性
- **内部已是 flex 列**:`flex flex-col` + 顶部 tab bar + 中部 body
- **5 个 `WorkspaceBody` 变体**(`state/layout.rs:60-72`):`Artifacts | Deliverables | Tasks | Canvas | Browser`
- **`available_with(is_team, has_browser)`**(`state/layout.rs:92-105`):决定 tab 集合
- **`coerce_body_with(body, is_team, has_browser)`**(`state/layout.rs:117-126`):不可用 body 回退

### 1.2 宽度 token `--aleph-workspace-w` 的 1 写者 + 3 读者

#### 1 写者:`workspace_panel.rs:79-103` 的 `Effect::new`

```text
Effect::new(move |_| {
    let width = workspace.width_px.get();
    let Some(root) = document_root() else { return };
    match width {
        Some(px) => {
            let viewport_w = web_sys::window()...;
            let clamped = clamp_width(px, viewport_w);
            let _ = root.style()
                .set_property("--aleph-workspace-w", &format!("{clamped}px"));
        }
        None => {
            let _ = root.style().remove_property("--aleph-workspace-w");
        }
    }
});
```

- 触发条件:`workspace.width_px` 变化(drag 写、初始加载、`clear_width`)
- **`None` ⇒ `remove_property`**:让 `:root` 默认值生效(避免两份默认拷贝,`state/layout.rs:30-36` 注释)
- **clamp 在写时执行**:`clamp_width(px, viewport_w)` 把 px 钳到 `[280, max(280, viewport*80%)]`
- **已知限制**(注释明确写):viewport 改变期间**不**触发重写,值偏大直到下次写;补 ResizeObserver 是后续任务

#### 3 读者:

| # | 位置 | 写法 | 用途 |
|---|---|---|---|
| 1 | `workspace_panel.rs:120` | `w-[var(--aleph-workspace-w)]` | aside 自身宽度 |
| 2 | `chat/view.rs:200` | `pr-[var(--aleph-workspace-w)]` | chat 右侧 padding(给浮层让位) |
| 3 | `app.rs:614` | `right-[calc(var(--aleph-workspace-w)+8px)]` | band chrome `LayoutToggle` 偏移 |

`app.rs:567-573` 注释明确写:"the pane's own width and the chat column's right padding are the other two" — 3 读者已被原作者盘点。

### 1.3 Mount 点(7 touch point 之一)

`chat/view.rs:170-258` 的 wrapper 结构:

```text
<div class="relative flex h-full overflow-hidden" on:dragover on:dragleave on:drop>
    <div class="relative flex flex-col flex-1 min-w-0 h-full pr-[var(--aleph-workspace-w)]">
        <MessageList />
        <SessionApprovalBanner />
        <TeamParticipants />
        <InputArea />
        <TeamTaskDrawer />
    </div>
    <WorkspacePanel />   ← 关键:作为 chat surface 兄弟节点
    <Show when=...>
        <DropOverlay />
    </Show>
</div>
```

- `WorkspacePanel` **不是** `<main>` 的子节点,而是 chat 视口内部 wrapper 的兄弟
- chat 视口 wrapper 是 `relative flex h-full overflow-hidden`
- 这是 absolute 浮层工作的基础(子节点的 absolute 锚定到最近的 `relative` 父)

### 1.4 状态机(`state/layout.rs:230-280`)

`WorkspaceState` 关键字段:

| 字段 | 类型 | 初始值 | 备注 |
|---|---|---|---|
| `mode` | `LayoutMode` | 持久化 | `ChatOnly` / `Split` |
| `body` | `RwSignal<WorkspaceBody>` | `WorkspaceBody::default()` (Artifacts) | 当前激活 tab |
| `width_px` | `RwSignal<Option<u32>>` | `read_persisted_workspace_width()` | `None` ⇒ 走 `:root` 默认 |

关键方法:

| 方法 | 行 | 语义 |
|---|---|---|
| `reveal(body)` | layout.rs:293 | 切换 active body,触发 `coerce_body_with` |
| `set_width(px)` | layout.rs:310 | 写信号 + `persist_workspace_width(px)`(localStorage) |
| `clear_width()` | layout.rs:325 | `width_px = None` + `forget_workspace_width()` |
| `showing_now(body)` | layout.rs:330 | 给 inbox badge 用 |

### 1.5 拖拽机制(`workspace_panel.rs:128-180`)

- 6px 绝对定位的 `aleph-workspace-resizer` strip
- `pointerdown` → `set_pointer_capture` + `dragging = true`
- `pointermove` → `width_px.set(Some(px))`(**只写信号**,不持久化)
- `pointerup` → `set_width(px)` 持久化
- `dblclick` → `clear_width()`
- 宽度从 **pane 自己的 right edge** 算起,不依赖 window(注释明确写)

### 1.6 Canvas body 的特殊处理(`workspace_panel.rs:191-209`)

- **绝不卸载**:`CANVAS.md` §6 钉死:`CanvasView` 持 3 条 liveness wire + camera + undo stack
- 用两个 `flex-1 min-h-0 flex flex-col` 兄弟 div + `style:display` 切换
- 迁移时 **必须保持这个 keep-alive 模式**,否则画板 refetch + 订阅抖动 + undo stack 丢

### 1.7 `LayoutToggle`(`components/layout_toggle.rs:30-34`)

```text
let _ = workspace.set_layout(LayoutMode::Split);
// or
workspace.mode.set(LayoutMode::ChatOnly);
```

- 28 行,极简
- 切换按钮的 `right` offset 已经在 §1.2 读者 #3 描述

### 1.8 已知限制与不做项

- 阶段 2 **不**做 ResizeObserver 补全(`workspace_panel.rs:88-92` 注释明确写"out of scope for the bounded change")
- 阶段 2 **不**改 `WorkspaceState` 字段
- 阶段 2 **不**动 `Canvas` keep-alive 模式
- 阶段 2 **不**引入新三方库

---

## 2. 目标 & 不变量

### 2.1 目标

**把 `<aside>` 从"绝对定位浮在 chat 之上"改成"与 `<main>` 平级的 flex 兄弟列"**。

### 2.2 必须保持的外部行为

1. `LayoutMode::ChatOnly/Split` 切换语义不变
2. `width_px` 持久化(localStorage key `aleph.panel.workspace_w`)不变
3. 拖拽手柄、dblclick 清空宽度、指针捕获行为不变
4. 5 个 `WorkspaceBody` 变体 + `available_with` / `coerce_body_with` 不变
5. Canvas keep-alive(永不卸载)不变
6. `--aleph-workspace-w` token 仍由 `WorkspacePanel`(或新 `RightPane`)单写者写
7. `LayoutToggle` 按钮位置 + 点击行为不变

### 2.3 必须改变的行为

1. `<aside>` 不再 absolute — 在 `<main>` 之外作为 shell flex 兄弟
2. chat 视口 `pr-[var(--aleph-workspace-w)]` 删掉(不再需要)
3. `app.rs` 的 band chrome 偏移从 `right-[calc(var(--aleph-workspace-w)+8px)]` 改成 `right-[8px]`(chat/pane 边界天然就是 chat 的 right edge)
4. 玻璃态(浮层视觉)删掉 — 改为正常 surface + `border-l` 分隔
5. `workspace-collapsed` 滑出动画改为 `width` transition(或 grid 列宽过渡)

### 2.4 关键不变量

- **1 写者原则保留**:宽度 token 的唯一写者仍在容器层(`RightPane` / 旧 `WorkspacePanel` 兼容路径二选一)
- **3 读者归并**:flex 后,`pr` 读者消失,`right` 读者简化为常数,只剩 1 读者(自身)
- **持久化格式不变**:用户拖到的宽度跨 release 仍生效
- **`set_layout` 写信号而非 `set_width`**:LayoutMode 切换不走 width token,走 shell 的 `class` 切换

---

## 3. 迁移路径(精确改动面)

### 3.1 Touch point 清单(7 处,修正主方案"5 处联动"误判)

| # | 位置 | 当前 | 改后 | 改动类型 |
|---|---|---|---|---|
| 1 | `components/workspace_panel.rs:117-126` | `<aside class="... absolute inset-y-0 right-0 z-20 glass bg-surface-overlay/95 shadow-xl min-w-[280px] w-[var(--aleph-workspace-w)]">` | `<aside class="... shrink-0 flex flex-col border-l border-border bg-surface-base w-[var(--aleph-workspace-w)] min-w-0 overflow-hidden">` | class 改写 |
| 2 | `chat/view.rs:200` | `format!("{base} pr-[var(--aleph-workspace-w)]")` | `base`(删除 `pr-...`) | class 简化 |
| 3 | `chat/view.rs:257` | `<WorkspacePanel />` 在 chat 视口 wrapper 内 | **删除** — 移到 `app.rs` | mount 迁移 |
| 4 | `app.rs` (新) | (无) | `<RightPaneLegacy />` 在 `<main>` 之后 | mount 新增 |
| 5 | `app.rs:614` | `format!("{base} right-[calc(var(--aleph-workspace-w)+8px)]")` | `format!("{base} right-[8px]")` | class 简化 |
| 6 | `workspace_panel.rs:114-116` | `class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split` | 改用 `style:display` 或 wrapper `class` 切换;`workspace-collapsed` 动画类删 | 视觉切换 |
| 7 | `state/layout.rs` (新方法) | (无) | 加 `WorkspaceState::flex_path_enabled() -> bool`(读 feature flag) | 新方法 |

> **解释**:1-3 改 chat 视口(删除 `pr`,删除 `<WorkspacePanel />`);4-5 改 `app.rs` shell(mount 接收 pane,简化 band chrome 偏移);6 改 `WorkspacePanel` 视觉(absolute→flex、玻璃态→base);7 加 feature flag 读侧。
> **没有"5 处联动"** — 实际上是 7 个独立 touch point,但都收敛在 3 个文件里:`workspace_panel.rs` / `chat/view.rs` / `app.rs` + 1 个 state 方法。

### 3.2 Mount 迁移(关键 diff 形状)

#### 旧 `chat/view.rs:170-258`

```text
<div class="relative flex h-full overflow-hidden" on:dragover on:dragleave on:drop>
    <div class="... pr-[var(--aleph-workspace-w)]">
        <MessageList />
        <SessionApprovalBanner />
        <TeamParticipants />
        <InputArea />
        <TeamTaskDrawer />
    </div>
    <WorkspacePanel />          ← 删
    <Show when=...><DropOverlay /></Show>
</div>
```

#### 新 `chat/view.rs`(只留 chat,无 pane)

```text
<div class="relative flex h-full overflow-hidden" on:dragover on:dragleave on:drop>
    <div class="... min-w-0">      ← pr 删,min-w-0 加(允许 flex 收缩)
        <MessageList />
        <SessionApprovalBanner />
        <TeamParticipants />
        <InputArea />
        <TeamTaskDrawer />
    </div>
    <Show when=...><DropOverlay /></Show>
</div>
```

#### 新 `app.rs` shell(接管 mount)

```text
<div class="flex h-full">
    <ModeSidebar />
    <main class="flex-1 min-w-0 relative">
        <ChatView />               ← 已不含 WorkspacePanel
    </main>
    <Show when=move || {
        flex_path_enabled() && workspace.mode.get() == LayoutMode::Split
    }>
        <RightPane />              ← 新组件,默认走 flex 路径
    </Show>
    <Show when=move || {
        !flex_path_enabled() && workspace.mode.get() == LayoutMode::Split
    }>
        <WorkspacePanel />         ← 旧组件,仅 legacy 路径用
    </Show>
</div>
```

- `flex_path_enabled()` = 读 `localStorage` 的 feature flag `aleph.right_pane.flex`(默认 `true`)
- 双 `<Show>` 保证:dev 模式可切换;默认走新路径
- **不需要 import `WorkspacePanel` 到 `app.rs`**(原本在 `chat/view.rs` 里,迁移后由 `app.rs` 引用)

### 3.3 `<aside>` class 重写(`workspace_panel.rs:117`)

#### 旧

```text
class="aleph-workspace-pane absolute inset-y-0 right-0 z-20 flex flex-col
       glass border-l border-border bg-surface-overlay/95 shadow-xl
       min-w-[280px] w-[var(--aleph-workspace-w)] overflow-hidden"
class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split
```

#### 新

```text
class="aleph-workspace-pane shrink-0 flex flex-col
       border-l border-border bg-surface-base
       w-[var(--aleph-workspace-w)] min-w-0 overflow-hidden
       transition-[width] duration-200"
style:display=move || if workspace.mode.get() == LayoutMode::Split { "flex" } else { "none" }
```

- 删除:`absolute inset-y-0 right-0 z-20`(浮层定位)
- 删除:`glass bg-surface-overlay/95 shadow-xl`(玻璃态)
- 删除:`min-w-[280px]`(由 `clamp_width` 在写时保证)
- 新增:`shrink-0`(flex 不收缩 — 由 `clamp_width` 控制)
- 新增:`min-w-0`(允许 children 收缩,Leptos 必加)
- 新增:`transition-[width] duration-200`(拖动后释放的展开/收起用 width 过渡;`workspace-collapsed` 滑出动画删)
- 切换:`class:workspace-collapsed` → `style:display`(展开=flex,折叠=none)

### 3.4 旧 absolute 视觉 → 新 base 视觉(对设计/UX 的回执)

- **删除**:`glass` 模糊背景(浮层意义在 flex 后不存在)
- **删除**:`bg-surface-overlay/95` 95% 不透明(浮层意义)
- **删除**:`shadow-xl` 大阴影(浮层意义)
- **保留**:`border-l`(与 chat 的分隔线)
- **替换**:`bg-surface-base`(普通表面,不再覆盖)
- **可选**:`bg-surface-elevated`(若与 chat 区分明显)

> **设计回执**(给 PR 评审用):这次是"语义重定义"——同一区域从浮层变成列,视觉从玻璃态变成普通 surface 是正确的。**不要**为了"看起来一样"强行保留玻璃态(会变成"假 flex")。

### 3.5 双 path 切换开关(legacy 路径保留)

| 开关 | 行为 | 用途 |
|---|---|---|
| 默认 (flex = true) | `app.rs` 走 `<RightPane />` | 阶段 2 主体 |
| `localStorage["aleph.right_pane.flex"] = "0"` | `app.rs` 走 `<WorkspacePanel />` | dev/QA 调试 |
| `?legacy_pane=1` URL 参数 | 一次性设 flag 后刷新 | 一次性回退 |
| `?flex_pane=0` URL 参数 | 一次性设 flag 后刷新 | 显式 opt-out |

- **触发条件**:LayoutMode == Split
- **两个 `<Show>`** 同时只能一个为真
- **后续阶段(阶段 4+)** 移除 legacy 路径

### 3.6 `--aleph-workspace-w` token 的最终形态

- 写者:仍然在容器层(`RightPane` / `WorkspacePanel` 二选一,看 flag)
- 读者 1:`<aside>` 自身宽度(`w-[var(--aleph-workspace-w)]`) — **保留**
- 读者 2:chat `pr-[…]` — **删除**(flex 下不需要)
- 读者 3:band chrome `right-[calc(…)]` — **简化为常数 `right-[8px]`**(flex 下 chat/pane 边界天然是 chat 的右边缘)

> **结论**:3 读者变 1 读者。`--aleph-workspace-w` 从"协调 3 个区域的 token"变成"自身宽度的实现细节"。这正是 §1.2 注释中担心的"drift"消失的契机。

---

## 4. 宽度状态机(修正主方案 §3.1.1 的"0/50%"误)

### 4.1 实际 clamp 行为(从 `state/layout.rs:132-147` 读出)

```rust
pub const fn clamp_width(px: u32, viewport_w: u32) -> u32 {
    let max = viewport_w * MAX_WORKSPACE_WIDTH_PCT / 100;  // MAX = 80
    let max = if max < MIN_WORKSPACE_WIDTH_PX { MIN_WORKSPACE_WIDTH_PX } else { max };  // MIN = 280
    if px < MIN_WORKSPACE_WIDTH_PX { MIN_WORKSPACE_WIDTH_PX }
    else if px > max { max }
    else { px }
}
```

- **下限**:`280 px`(硬下限,不是 0;若 viewport < 280 仍返回 280,避免 `u32::clamp` panic)
- **上限**:`max(280, viewport * 80%)`
- 视口 320px → 上限 280px(下限胜)
- 视口 640px → 上限 512px
- 视口 1024px → 上限 819px
- 视口 1920px → 上限 1536px
- 视口 3840px → 上限 3072px

> **主方案 §3.1.1 误写 "0/50%"** — 实际是 280 / 80%。本 spec 锁定 280 / 80% 为契约。

### 4.2 状态机(完整)

```text
RightPaneState {
    mode: LayoutMode,                  // ChatOnly | Split
    width_px: Option<u32>,             // None = 走 :root 默认
    dragging: bool,                    // 拖拽中(只内存,不持久化)
    body: WorkspaceBody,               // 当前 tab
}

事件转移(纯函数):
| from               | event          | to                                    | 副作用                          |
|--------------------|----------------|---------------------------------------|----------------------------------|
| 任何               | toggle_pane    | mode = (Split ? ChatOnly : Split)     | 写 localStorage LAYOUT_MODE_KEY |
| Split, !dragging   | pointer_down   | dragging = true                       | set_pointer_capture              |
| Split, dragging    | pointer_move X | width_px = Some(clamp_width(Δ))       | 写 CSS var, 不持久化             |
| Split, dragging    | pointer_up X   | dragging = false; set_width(Δ)        | 持久化到 localStorage            |
| Split, !dragging   | dblclick       | clear_width() → width_px = None       | forget_workspace_width           |
| 任何               | reveal(b)      | body = coerce_body_with(b, ...)       | (受 is_team/has_browser 影响)    |
```

### 4.3 边界条件(必须测的)

| # | 场景 | 期望行为 |
|---|---|---|
| 1 | 视口 320px + Split | pane width = 280(下限胜) | 
| 2 | 视口 1920px + Split | pane width ≤ 1536(80% 上限) |
| 3 | 视口 = 0(window.inner_width NaN/fail) | 走 fallback `unwrap_or(0)` → `clamp_width(0, 0)` → 280 |
| 4 | 视口 280px(等于下限) | 任何 px 输入都返 280 |
| 5 | 拖拽中 `client_x < 0`(指针在视口外) | `raw.max(0.0)` → `clamp_width(0, viewport)` → 280 |
| 6 | 拖拽中 viewport 缩窄(1500px → 1200px) | **已知限制**:新宽度直到下次写才生效(`workspace_panel.rs:88-92` 注释) |
| 7 | `width_px = None` | `--aleph-workspace-w` 走 `:root` 默认(目前是 `40%`,需要在 tailwind.css 复核) |
| 8 | LayoutMode 切 ChatOnly → Split | `--aleph-workspace-w` 维持上次值(不重置) |
| 9 | LayoutMode 切 Split → ChatOnly | aside 隐藏(`style:display = none`);下次展开恢复 |
| 10 | 双 path 切换(flex ↔ legacy) | `--aleph-workspace-w` 维持;DOM 挂载位置变化;宽度不变 |

### 4.4 关于 ResizeObserver(已知限制的迁移决定)

`workspace_panel.rs:88-92` 注释明确把 ResizeObserver 标为 "out of scope for the bounded change"。阶段 2 决定:

- **不**实施 ResizeObserver(避免改动面膨胀,违反 R10 文件预算)
- **保留** `Effect::new` 的"写时 clamp"行为
- **新文件清单**:`right_pane/{mod,slot,registry,pane}.rs` + `slots/{mod, subagent_tree, artifacts, capabilities, browser, approvals}.rs` = 8 文件上限(阶段 1 spec §9 R10 已锁)
- **新组件**:`<RightPane />` 复用同一 Effect 写者逻辑(不重写),但要明确标注 "viewport resize 限制 by design"
- **本 spec 副作用**:增加 1 条"已知限制"条目到 `docs/reference/FEATURE_LOCATOR.md` §6.1 的 11 形状

---

## 5. 视觉与动画

### 5.1 动画选型

#### 候选对比

| 方案 | 优点 | 缺点 |
|---|---|---|
| A. `width` transition + `style:display` toggle | 简单、Leptos 友好 | column flex 下 `width` 过渡在某些浏览器抖 |
| B. `grid-template-columns: 1fr auto ↔ 1fr 0fr` | 平滑、纯 CSS、Leptos 切换 class | 需把 shell 改 grid 布局 |
| C. `display: none` toggle(无动画) | 极简 | 用户失去展开/收起的视觉反馈 |

#### 推荐

- **首选方案 A**(最小 diff,与现有 `transition` 习惯一致)
- 加 `transition-[width] duration-200` 到 `<aside>`,Leptos 不需要额外 effect
- 拖动过程中**不**加 transition(避免拖动延迟感),只在 `pointerup` 后过渡;实现:`style:transition = none` 在 dragging 时
- **不**采用方案 B(改动面太大,R10 红线)
- **不**采用方案 C(违反 R9"行为反馈")

### 5.2 视觉差异(新旧对比)

| 维度 | 旧(absolute) | 新(flex) |
|---|---|---|
| 定位 | `absolute right-0` | flex 兄弟 |
| 背景 | `glass` + 95% 不透明 | `bg-surface-base` |
| 阴影 | `shadow-xl` | 无 |
| 边框 | `border-l` | `border-l`(同) |
| 最小宽度 | `min-w-[280px]`(硬下限) | `clamp_width` 在写时保证 |
| 切换动画 | `workspace-collapsed` slide+fade 200ms | `width` transition 200ms |
| 与 chat 关系 | 浮在 chat 之上 | 平分视口 |
| 折叠时 main 宽度 | main 不变(浮层消失) | main 吃回 pane 宽度 |

### 5.3 折叠/展开过渡(精确)

```text
split:  aside width: 280..=80% (clamp); transition: width 200ms ease
        chat width:    1fr;            (无 transition,因为是 main 的子)
collapse: aside display: none         (立即;不动画,因为不存在)
expand:   aside display: flex; width: 之前值
          触发:transition 200ms ease
```

- **不**用 `transform: translateX(100%)` 旧模式(浮层意义)
- **不**用 fade 旧模式(浮层意义)
- 展开时 `width` 从 0 → 之前值的过渡;但当前 spec 让 `display: none ↔ flex` 二选一,过渡只在 Split 内的 width 变化时有效
- **简化决定**:本阶段不追求"折叠/展开动画",只追求"拖动后 width 变化平滑"。LayoutMode 切换是立即的。

---

## 6. 验证矩阵

### 6.1 单元测试(合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| UT-S2-1 | `chat_view_no_longer_contains_workspace_panel` | 静态扫描:`grep "<WorkspacePanel" chat/view.rs` 必须 0 命中 |
| UT-S2-2 | `app_rs_contains_right_pane_mount` | 静态扫描:`grep "<RightPane" app.rs` 必须 ≥ 1 命中 |
| UT-S2-3 | `workspace_panel_aside_removed_absolute` | 静态扫描:`workspace_panel.rs` 的 `<aside` 行**不**含 `absolute` |
| UT-S2-4 | `clamp_width_280_80` | 参数化测试:`(px, viewport) -> px'`,覆盖 §4.3 边界 1-10 |
| UT-S2-5 | `flex_path_flag_default_true` | 新增 `localStorage["aleph.right_pane.flex"]` 不存在时,`flex_path_enabled()` 返 `true` |
| UT-S2-6 | `flex_path_flag_off_uses_legacy` | 设 `"0"`,`flex_path_enabled()` 返 `false` |
| UT-S2-7 | `pr_token_removed` | 静态扫描:`chat/view.rs` 的 `pr-[var(--aleph-workspace-w)]` 0 命中 |
| UT-S2-8 | `band_chrome_right_simplified` | 静态扫描:`app.rs` 的 `right-[calc(var(--aleph-workspace-w)` 0 命中 |
| UT-S2-9 | `width_persistence_unaffected` | `localStorage["aleph.panel.workspace_w"]` 读/写仍正常 |
| UT-S2-10 | `layout_mode_persistence_unaffected` | `localStorage["aleph.panel.layout_mode"]` 读/写仍正常 |

### 6.2 视觉回归(合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| VS-S2-1 | `right_pane_as_flex_column_320px` | 视口 320px,pane = 280px(下限胜),chat 与 pane 都可见 |
| VS-S2-2 | `right_pane_as_flex_column_640px` | 视口 640px,pane = 280..=512(默认 320 = 50%?需复核) |
| VS-S2-3 | `right_pane_as_flex_column_1024px` | 视口 1024px,pane = 280..=819(默认 40% = 410) |
| VS-S2-4 | `right_pane_as_flex_column_1920px` | 视口 1920px,pane = 280..=1536 |
| VS-S2-5 | `right_pane_collapsed_chat_grows_back` | 折叠后 main 吃回 pane 宽度,无残影 |
| VS-S2-6 | `right_pane_dragging_smooth` | 拖动中 width 平滑过渡(用 `width` transition),抬起落点正确 |
| VS-S2-7 | `right_pane_resizer_visible` | Split 模式下 resizer 6px 可见且可拖 |
| VS-S2-8 | `right_pane_legacy_path_still_works` | 设 `?legacy_pane=1`,旧 absolute 行为完整回归 |
| VS-S2-9 | `right_pane_canvas_keep_alive` | 切到 Canvas tab → 切到 Artifacts tab → 切回 Canvas,画板状态保留(不重 fetch) |
| VS-S2-10 | `right_pane_layout_toggle_position` | `LayoutToggle` 按钮位置:ChatOnly `right-[44px]`,Split `right-[8px]`(不再依赖 token) |

### 6.3 端到端 smoke(合入门槛)

| ID | 名称 | 判据 |
|---|---|---|
| E2E-S2-1 | `toggle_pane_via_button` | 点 `LayoutToggle` 切 ChatOnly ↔ Split,DOM 节点变化正确 |
| E2E-S2-2 | `toggle_pane_via_shortcut` | 快捷键(若有)切 ChatOnly ↔ Split |
| E2E-S2-3 | `drag_to_50_percent` | 拖到 50% 视口宽,落点正确,持久化 |
| E2E-S2-4 | `drag_to_min_280` | 拖到 280px,clamp 生效,不再更小 |
| E2E-S2-5 | `drag_to_max_80_percent` | 拖到 80% 视口宽,clamp 生效,不再更大 |
| E2E-S2-6 | `double_click_resizer_clears` | 双击 resizer,`width_px = None`,走 `:root` 默认 |
| E2E-S2-7 | `switch_tabs_preserves_canvas` | 在 Canvas tab 画几笔,切 Artifacts,切回 Canvas,笔触保留 |
| E2E-S2-8 | `switch_team_mode_keeps_pane` | Personal → Team,右列保留,tab 集合更新(Deliverables/Tasks 出现) |
| E2E-S2-9 | `legacy_pane_url_param_works` | `?legacy_pane=1` 刷新后走旧 absolute 路径 |
| E2E-S2-10 | `flex_pane_url_param_works` | `?flex_pane=0` 刷新后切回 legacy |

### 6.4 性能预算

| 项 | 上限 | 实测(阶段 2 末) |
|---|---|---|
| 切换 ChatOnly ↔ Split | < 100ms | 待测 |
| 拖动中 frame rate | ≥ 55 fps @ 1920×1080 | 待测 |
| `width_px` 持久化(localStorage) | < 5ms | 已知(同步 API) |
| `--aleph-workspace-w` 写 | < 1ms / 写 | 已知(set_property) |

---

## 7. 风险与红线

### 7.1 R1-R10 红线复述

- R1 — Rust Core 不渲染右列;flex 切换是 Leptos 概念
- R2 — 复杂业务 UI 留在 Leptos
- R3 — 不引入新三方库
- R4 — `interfaces/cli` / `interfaces/tui` 不引入新业务概念
- R5 — Pin/通知不绕过 `GlobalBus`
- R6 — 移动端本期不同步
- R7 — `LayoutMode` 切换不调 LLM
- R8 — `LayoutToggle` 暴露为 `WorkspaceAction` 工具
- R9 — "哪种 LayoutMode 该默认"移到 prompt
- R10 — 本阶段新增 ≤ 2 文件(`right_pane/{mod,pane}.rs`),**不**碰 trait/registry(那是阶段 1)

### 7.2 阶段 2 特有风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| Canvas keep-alive 在新 mount 点失败 | 高 | 验证矩阵 VS-S2-9 锁死;若失败回滚到 legacy path |
| `pr` 删除后 chat 视口滚动条溢出 | 中 | `min-w-0` 必加;视觉回归 VS-S2-1..4 覆盖 |
| `width` transition 在 flex column 下抖动 | 中 | dragging 时 `transition: none`,抬起时启用 |
| 旧测试只验证 absolute 行为,flex 后漏 | 中 | 重写测试或加双 path 验证 |
| `--aleph-workspace-w` 在双 path 间漂移 | 低 | 写者单一性已锁,风险小 |
| 持久化格式不兼容旧 release | 低 | localStorage key 不变,JSON 结构不变 |
| 双 path `<Show>` 切换闪屏 | 中 | 测试时序:layout_mode 写 → DOM 重渲染 < 50ms |
| LayoutMode=ChatOnly 时 band chrome 偏移错误 | 低 | `right-[44px]` 是常数,无 token 依赖 |
| `WorkspaceState::reveal` 在新 mount 顺序下不工作 | 中 | `reveal` 通过 context 写,context 仍提供,理论上 OK;需测试 |
| ResizeObserver 缺失导致 resize 期间 pane 溢出 | 中 | 已知限制,文档化,**不**在本期修复 |

### 7.3 已知不做

- ❌ 实施 ResizeObserver(留给后续,见 §4.4)
- ❌ 改 `WorkspaceState` 字段
- ❌ 动 Canvas keep-alive 模式
- ❌ 改 5 个 `WorkspaceBody` 变体(留给阶段 3-6)
- ❌ 引入新三方库(R3)
- ❌ 改 `workspace_panel.rs` 的拖拽/双击逻辑(已在验证)
- ❌ 改 `LayoutToggle` 视觉
- ❌ 改 `chat/view.rs` 的拖放逻辑(`on:dragover` 等)
- ❌ 改 `:root` 的 `--aleph-workspace-w` 默认值(40% 维持)
- ❌ 改 `tailwind.css`(若有)
- ❌ 改 i18n keys(本期不增删)

---

## 8. 验收门槛(阶段 2,合入门槛)

1. ✅ 7 个 touch point 全部按 §3.1 清单改动
2. ✅ 10 条单元测试全绿(UT-S2-1 ~ UT-S2-10)
3. ✅ 10 条视觉回归脚本通过(VS-S2-1 ~ VS-S2-10)
4. ✅ 10 条端到端 smoke 通过(E2E-S2-1 ~ E2E-S2-10)
5. ✅ 双 path 在 dev 模式可切换(`?legacy_pane=1` / `?flex_pane=0` / localStorage flag)
6. ✅ Canvas keep-alive 行为不变(VS-S2-9 锁死)
7. ✅ 拖拽/双击/`set_width`/`clear_width`/`reveal`/`set_layout` 全部行为不变
8. ✅ `--aleph-workspace-w` 仍由容器层单写者写(在 flex 路径下是 `<RightPane />`)
9. ✅ 持久化(`localStorage["aleph.panel.workspace_w"]` + `localStorage["aleph.panel.layout_mode"]`)格式不变
10. ✅ 文档:`FEATURE_LOCATOR.md` §6.1 注释加"阶段 2 已实施"回写;`AGENTS.md` 索引更新
11. ✅ 红线守住:R1-R10 不破;新增 ≤ 2 文件
12. ✅ 性能预算达标(§6.4)

---

## 9. 文件清单(本阶段 2 改动 + 新增,严格 R10)

### 9.1 改动文件(已有)

- `interfaces/webchat/src/components/workspace_panel.rs` — `<aside>` class 重写、`workspace-collapsed` 改 `style:display`
- `interfaces/webchat/src/platform/wide/views/chat/view.rs` — 删 `<WorkspacePanel />`、删 `pr-...`、加 `min-w-0`
- `interfaces/webchat/src/app.rs` — shell 加 `<RightPane />` mount、`ChatBandChrome` `right-[…]` 简化

### 9.2 新增文件(本阶段 2,≤ 2 个)

- `interfaces/webchat/src/components/right_pane/mod.rs` — `pub mod pane;` + 顶部 1 段语义注释锁定(避免再添 4 个相似文件名,见 阶段 1 spec §6)
- `interfaces/webchat/src/components/right_pane/pane.rs` — `<RightPane />` 组件(实际就是 `WorkspacePanel` 的 flex 形态;**不**重新实现,先用一个 `pub fn right_pane() -> impl IntoView { view! { <WorkspacePanel /> } }` 转发,后续阶段 1 trait 落地时再换内部)

### 9.3 严格不碰(留给阶段 1)

- ❌ `right_pane/slot.rs`(trait)
- ❌ `right_pane/registry.rs`
- ❌ `slots/*`(5 个具名 slot)
- ❌ `state/layout.rs` 新增 trait 相关字段

### 9.4 R10 文件数核算

- 阶段 1 新增:8 个文件(锁定在阶段 1 spec §9)
- 阶段 2 新增:**2 个文件**(本节 §9.2)
- **累加 10 个文件**,**仍**在 R10 "新增 ≤ 12 文件" 内
- 注:阶段 2 不增加 trait,只是 host 切换;阶段 1 trait 在阶段 1 启动后,**不**会因阶段 2 重复新增文件

---

## 10. 与主方案章节的对应

| 本 spec 章节 | 主方案章节 | 关系 |
|---|---|---|
| §0 范围 | 主方案 §0 范围 | 阶段 2 子集 |
| §1 事实基线 | 主方案 §1.3 ⚠ 命名陷阱 + §3.1.1 | 事实修正 |
| §2 目标 & 不变量 | 主方案 §3 决策点 | 决策细化 |
| §3 迁移路径 | 主方案 §4 阶段 2 | **本 spec 是细化** |
| §4 宽度状态机 | 主方案 §3.1.1 + §5.1 战略风险 | **修正 50% → 80%** |
| §5 视觉与动画 | 主方案 §3.1.1 (flex 形态) | 视觉决策 |
| §6 验证矩阵 | 主方案 §6 验证方法 | 阶段 2 矩阵 |
| §7 风险与红线 | 主方案 §5 风险 | 阶段 2 子集 |
| §8 验收门槛 | 主方案 §4 阶段 2 验收 | **本 spec 是细化** |
| §9 文件清单 | 主方案 §4 阶段 2 R10 | 锁定 2 个新增 |
| §11 待办 | (新) | 主方案 §7 待办细化 |

---

## 11. 待办与决策记录(本 spec 期间)

### 11.1 已决策(本 spec 内,无需再问)

| 决策 | 决定 | 来源 |
|---|---|---|
| 容器形态 | flex 第三列 | 阶段 0 m00161 (3.1.1) |
| slot 抽象 | trait | 阶段 0 m00161 (3.1.2) |
| subagent 默认 Tab | 是(阶段 3 实施) | 阶段 0 m00161 (3.1.3) |
| 动画方案 | width transition + display toggle | 本 spec §5.1 |
| 视觉切换 | 玻璃态 → base surface | 本 spec §3.4 |
| 双 path 开关 | `?legacy_pane=1` + `?flex_pane=0` + localStorage | 本 spec §3.5 |
| clamp 行为 | `[280, 80%]` | 本 spec §4.1(修正主方案 50% 误) |
| ResizeObserver | 推迟 | 本 spec §4.4 |

### 11.2 已批决策与实施前置条件

- S2-A:`<RightPane />` 转发 `<WorkspacePanel />` — ✅ 已批(m00332)
- S2-B:`LayoutMode::ChatOnly` 使用显式分支 — ✅ 已批(m00332)
- S2-C:迁移期间 workspace tool/缩略图 API 仍由 `WorkspacePanel` 直接读取 — ✅ 已批(m00332)

阶段 2 实施仍需按本 spec 的验收矩阵执行；“已批”表示方案决策已锁定，不代表代码已落地。
  - 提议:维持 40%(避免再次改动 :root,本 spec 范围锁定)

### 11.3 已知问题清单(移交阶段 3+)

| 问题 | 留待 | 备注 |
|---|---|---|
| ResizeObserver 缺失 | 阶段 4+ | `workspace_panel.rs:88-92` 注释锁定 |
| `:root` 默认 40% 是否最优 | 阶段 3 实测后回看 | 与 subagent 默认开共调 |
| 移动端右列(目前 ChatOnly only) | 后续独立 spec | 不在本季度 |

---

## 12. 附录 A:旧代码片段(便于 PR review 对照)

### 12.1 `workspace_panel.rs:117-126` 旧 vs 新

**旧**:
```text
<aside
    node_ref=pane_ref
    data-testid="workspace-panel"
    class="aleph-workspace-pane absolute inset-y-0 right-0 z-20 flex flex-col
           glass border-l border-border bg-surface-overlay/95 shadow-xl
           min-w-[280px] w-[var(--aleph-workspace-w)] overflow-hidden"
    class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split
>
```

**新**:
```text
<aside
    node_ref=pane_ref
    data-testid="workspace-panel"
    class="aleph-workspace-pane shrink-0 flex flex-col
           border-l border-border bg-surface-base
           w-[var(--aleph-workspace-w)] min-w-0 overflow-hidden
           transition-[width] duration-200"
    style:display=move || if workspace.mode.get() == LayoutMode::Split { "flex" } else { "none" }
    style:transition=move || if dragging.get() { "none" } else { "width 200ms ease" }
>
```

### 12.2 `chat/view.rs:170-258` 旧 vs 新

**旧**(简化):
```text
<div class="relative flex h-full overflow-hidden" on:dragover on:dragleave on:drop>
    <div class="... pr-[var(--aleph-workspace-w)]">
        ...chat 内容...
    </div>
    <WorkspacePanel />
    <Show when=...><DropOverlay /></Show>
</div>
```

**新**(简化):
```text
<div class="relative flex h-full overflow-hidden" on:dragover on:dragleave on:drop>
    <div class="... min-w-0">
        ...chat 内容...
    </div>
    <Show when=...><DropOverlay /></Show>
</div>
```

### 12.3 `app.rs` shell 旧 vs 新

**旧**(`app.rs` 不含 `<WorkspacePanel />`,所有 mount 都在 chat/view.rs):
```text
<div class="flex h-full">
    <ModeSidebar />
    <main>...</main>
</div>
```

**新**:
```text
<div class="flex h-full">
    <ModeSidebar />
    <main>...</main>
    <Show when=move || {
        flex_path_enabled() && workspace.mode.get() == LayoutMode::Split
    }>
        <RightPane />
    </Show>
    <Show when=move || {
        !flex_path_enabled() && workspace.mode.get() == LayoutMode::Split
    }>
        <WorkspacePanel />
    </Show>
</div>
```

---

*Last updated: 2026-10-15 · 阶段 2 spec · 与主方案 + 阶段 1 spec 配套*
