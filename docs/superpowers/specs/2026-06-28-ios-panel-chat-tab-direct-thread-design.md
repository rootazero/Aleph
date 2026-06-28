# iOS Panel — Chat tab 直达聊天屏（历史移入按钮）

**Date:** 2026-06-28
**Scope:** `interfaces/webchat/src/platform/phone/chat/` — 纯表现层重构，零 core / 零 server。
**Status:** Design approved, pending implementation plan.

## 1. 背景与问题

当前 iOS Panel 的 Chat tab 落地页是 **`PhoneChatList`**：一行 “+ New chat” 按钮 + 会话历史列表。用户必须先在列表里点一下（新建或选会话）才能进入 **`PhoneChatThread`** 对话屏。

期望：点 Chat tab **直接进入聊天屏**（新聊天状态），**历史记录改为通过按钮进入**。这与 ChatGPT / Claude App 的手机形态一致。

本质是把现有两块屏（list / thread）**角色互换**，并按下方状态规则重新连线，而非新建组件。

## 2. 行为模型（核心）

### 2.1 路由表

| 路由 | 渲染组件 | 角色 |
|------|----------|------|
| `/`（以及 `/chat`） | `PhoneChatThread` | Chat tab 落地页 = 聊天屏 |
| `/chat/history` | `PhoneChatHistory`（原 `PhoneChatList`） | 会话历史列表（按钮进入） |

`PanelMode::from_path` 对 `/`、`/chat`、`/chat/history` 均回落到 `PanelMode::Chat`（无其它 mode 前缀匹配），故 Chat tab 在三者下都保持高亮。现有外部 deep-link（`command_palette.rs`、`nav_menu.rs` 的 `PanelMode::Chat => "/chat"`、`extensions/category_nav.rs`）navigate 到 `/chat` 时仍落到聊天屏，无破坏。

### 2.2 状态规则（用户两次确认的最终版）

- **冷启动**（彻底退出 App 后重新打开）→ `ChatState` 天然为空（无 localStorage 持久化；`restore_from`/snapshot 仅为 app 内会话切换缓存）→ 聊天屏渲染 welcome hero = **新聊天状态**。
- **切到别的 tab 再回到 Chat tab** → tab 切换由 `MainContent` 的 `display:contents/none` 实现、容器不卸载、`ChatState`（app 根 provide）常驻 → **保留当前对话**。**不需要任何“点 tab 清空”机制。**
- **✎ 新建按钮**（聊天屏右上）→ `chat.clear_session()`（清 messages / session_key / phase 等，保留 `agent_id`）→ 回到 hero。被清掉的对话若已发过消息，已在服务端持久化，可从历史找回。
- **🕘 历史按钮**（聊天屏左上）→ navigate `/chat/history`。
  - 点列表中某会话 → 载入该 session 进 `ChatState` + navigate `/` → 聊天屏显示该会话。
  - 点返回（不选）→ navigate `/` → 聊天屏保留进入历史前的当前对话原样。

### 2.3 状态图

```
冷启动 ─────────────► [/  空 ChatState]  →  hero（新聊天）
                          │  ▲
        切 tab 往返(显隐) │  │ 容器常驻 → 对话保留
                          ▼  │
                     [/  有对话]
                       │   │   └── ✎ 新建 → clear_session() → 回 hero
                       │   └────── 🕘 历史 → /chat/history
                       │                         │
                       │            选会话: 载入 + navigate "/"
                       │            返回:   navigate "/"（不清空）
                       ▼
                  发送消息 → 服务端持久化（历史可见）
```

## 3. 组件改动（三处文件，均在 `platform/phone/chat/`）

### 3.1 `mod.rs`（`PhoneChat` 路由器）
- 路由判断从 `pathname == "/chat" → Thread / else → List` 改为：
  `pathname == "/chat/history" → PhoneChatHistory`，**否则** → `PhoneChatThread`。
- `run.*` 订阅 / `stream.*` topic 订阅 / `on_cleanup` 逻辑**不动**（聊天屏与历史屏共用这一层订阅，挂载点不变）。

### 3.2 `thread.rs`（`PhoneChatThread` — 聊天屏，落地页）
- **移除** `‹ Chat` 返回按钮（聊天屏现在是 tab 根，无上级可返回）。
- 顶栏改为三段式：
  - 左：**🕘 历史** 图标按钮 → `navigate("/chat/history")`。
  - 中：标题文字 **"Aleph"**（静态；动态会话 topic 留作后续，见 §5）。
  - 右：**✎ 新建** 图标按钮 → `chat.clear_session()`（已在 `/`，无需 navigate）。
- 下方 `MessageList` + `PhoneComposer` + `PhoneTabBar` 全部复用，结构不变。
- 仍保持 `fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col` 自管滚动布局。

### 3.3 `list.rs` → `PhoneChatHistory`（历史屏）
- 组件/文件语义从“landing list”变为“pushed history”：
  - 改用 `PhoneShell` 包裹并传 `title="History"`、`back="/"`、`back_label="Chat"`（带 `‹ Chat` 返回）。
  - **移除顶部 “+ New chat” 行**（新建已由聊天屏 ✎ 承担，避免双入口）；连带移除 `on_new` 处理器。
  - `on_select` 末尾 navigate 目标从 `"/chat"` 改为 `"/"`（选中后回聊天屏显示）。
- 保留：`SessionRow` / `sort_sessions_desc` / connect-gated 加载 Effect / Loading·Connecting·Error·Retry·空态 / 现有单测。
- 命名：`PhoneChatList` → `PhoneChatHistory`（同步更新 `mod.rs` 的 `pub use` 与引用）；保留为同一文件 `list.rs` 或重命名 `history.rs` 由实现阶段定（倾向重命名以名实相符）。

## 4. 约束与不变量

- **R4（Interface 纯 I/O）**：全部改动只是表现层路由与渲染；数据仍走既有 `sessions.list` / `ChatApi` / `ChatState`，无业务逻辑下沉。
- **零 core / 零 server / 零依赖**：不动 `ChatState`、`MessageList`、`PhoneComposer`、`clear_session` 等。
- **桌面 (wide) 不变**：`ChatView` 与 wide 路由字节不变。
- **其它 phone tab 不变**：Memory / Agents / Settings / More 等不受影响。
- **单订阅不变量**：每 form factor 仅一个 `PhoneChat` 挂载，`run.*` 订阅仍由 `mod.rs` 唯一持有。
- **PhoneShell dynamic-child footgun**：历史屏若 static + dynamic 兄弟混排，需包进单个 `<div>`（沿用既有写法，已是如此）。

## 5. 明确不做（YAGNI / 后续）

- **动态标题**：聊天屏顶栏标题暂固定 "Aleph"；显示当前会话 topic 需把 topic 引入 `ChatState`，留作独立后续。
- **历史屏内新建入口**：不在历史列表里保留“+ New chat”行。
- **草稿保留**：进入历史再返回不清空“当前对话”，但 composer 里**未发送的草稿文本**不在保留范围内（`clear_session` 才清，返回不触发 clear，故草稿实际也会保留；不为此额外加机制）。
- **会话删除 / 重命名 / 搜索**：不在本次范围。

## 6. 验收标准（运行时 QA，iOS sim，权威门）

1. 冷启动进入 App → Chat tab 直接是聊天屏 + welcome hero（不是列表）。
2. 输入并发送 → 流式回复正常；停止按钮可用。
3. 切到 Memory tab 再切回 Chat → **当前对话仍在**（不被重置）。
4. 点右上 ✎ → 回到 hero（新聊天）；之前对话可在历史中找到。
5. 点左上 🕘 → 进历史列表；点某会话 → 回聊天屏并显示该会话；点 `‹ Chat` 返回 → 回到先前对话原样。
6. 全程**无桌面式左右分屏**，符合手机下钻法则。
7. `just wasm` 编译通过（渲染层 Leptos 类型门兜底）。

## 7. 影响文件清单

- `interfaces/webchat/src/platform/phone/chat/mod.rs`（路由判断 + `pub use` 改名）
- `interfaces/webchat/src/platform/phone/chat/thread.rs`（顶栏三段式：历史/标题/新建，去返回键）
- `interfaces/webchat/src/platform/phone/chat/list.rs`（→ 历史屏：PhoneShell 带返回、去 “+ New chat”、`on_select` navigate `/`；可能重命名 `history.rs`）
- 预重建 `dist/`（panel 编译期嵌入二进制，需 `just wasm` + 重编 server 才在运行时生效）
