# 群聊对话区回归单聊布局 — Design Spec

> Date: 2026-06-16 · Status: Approved (design) · Scope: Panel (Leptos/WASM) frontend-only

## 1. Background / 现状

团队聊天("群聊")在 `views/chat/view.rs` 当前是三段横向布局:

```
<div class="relative flex h-full">
    <Show when=team_id.is_some()>  <TeamRoster />  </Show>   ← 左:常驻 w-40 成员栏
    <div class="flex flex-col flex-1 min-w-0 h-full"> … MessageList / SessionTabs / InputArea </div>  ← 中:对话区
    <WorkspacePanel />                                        ← 右:交付物/任务,仅 Split 模式开启,默认不占位
</div>
```

- `TeamRoster`(`components/team_roster.rs`,46 行):读 `chat.team_members`(`Vec<TeamMemberView>`),逐行渲染状态点 + leader 标 + 名字。常驻 `w-40 shrink-0 border-r`,**始终挤占对话区宽度**。
- 用户消息(`role="user"`,`agent_id=None`,由 `state.rs::push_user_message` 写入)在 `messages.rs` 已走 `is_user → "flex justify-end"` 的右对齐单聊气泡,**不进** Layout A(Layout A 仅 `message.agent_id.is_some()` 触发)。
- agent 消息走 Layout A(头像盘在气泡外 + 彩名在上,Telegram 式),由 `team_events.rs` 以 `agent_id=Some(..)` 推入。

相关已存在、本次复用的符号:
- `chat.team_members: RwSignal<Vec<TeamMemberView>>`,`TeamMemberView { agent_id: String, name: String, emoji: Option<String>, is_leader: bool, status: MemberStatus }`(`state.rs:229`)。
- `MemberStatus { Working, Done, Error, Idle }`(`state.rs:241`)。状态色见 `team_roster.rs`:Working `#e0a458` / Done `#4ec9b0` / Error `#d16969` / Idle `#6b7280`。
- `agent_identity::agent_color_for_id(&str) -> &'static str`(FNV-1a id 哈希,会话稳定配色)。
- `agent_identity::monogram`(私有,首字大写;本次改 `pub` 复用)。
- 现有头像簇视觉参照:`chat_sidebar.rs:824`(`w-6 h-6 rounded-full` 重叠盘,emoji/monogram,overlap `-ml-2`,≤3 + "+N")。**数据不同源**(sidebar 用 `members_preview` 摘要;聊天区用 `chat.team_members`),故只对齐样式,不强行抽共享组件。

## 2. Goals / 非目标

### Goals
1. **用户消息右对齐、无头像,与单聊完全一致。** —— 现状代码已满足,本条**预计零代码改动**,仅在 E2E 实测确认;若发现个例(如某路径误带 `agent_id`)再外科修。
2. **移除常驻左侧成员栏,改为聊天区右上角"头像簇按钮 → 点击展开浮层"。** 对话区占满宽度 = 单聊布局。折叠态即可一眼看到参与者(贴合"让群聊人物更明显")。

### 非目标(明确不动)
- 后端、任何 RPC、`team_chat` API、composer 发送路由(`composer/mod.rs`)。
- `WorkspacePanel`(交付物/任务):本来就是切换式右栏、默认不占位,保持原样。
- `messages.rs` 的 agent 气泡 Layout A、`team_events.rs` 事件投影。
- 单聊路径任何行为(零回归)。

## 3. Decisions(来自澄清问答)
- **用户头像**:不加。用户消息保持右对齐、无头像(Telegram/微信惯例:"我"在右无头像,agents 在左带头像)。
- **成员入口触发方式**:顶部头像簇按钮 → 点击展开浮层(非抽屉、非 WorkspacePanel tab)。

## 4. Architecture / 范围

纯前端组件级重构,零后端、零 RPC、零新数据。数据全部取自已填充的 `chat.team_members`,配色/头像复用 `agent_identity`。

### 改动清单

| 文件 | 改动 |
|---|---|
| `views/chat/view.rs` | ① 删除左侧 `<Show when=team_id.is_some()><TeamRoster/></Show>` 整块 + `use ...team_roster::TeamRoster;` import。聊天区 `flex flex-col flex-1 min-w-0` 自然占满全宽。② team 模式下,在聊天区顶部叠层右上角挂 `<TeamParticipants/>`(见 §5 放置)。 |
| **新建** `components/team_participants.rs` | 头像簇按钮 + 展开浮层组件(见 §6)。 |
| `components/mod.rs` | 删 `pub mod team_roster;`(line 27),加 `pub mod team_participants;`(按字母序)。 |
| **删除** `components/team_roster.rs` | 移除左栏后成孤儿。**前置校验**:`grep -rn "TeamRoster\b" interfaces/webchat/src` 确认仅 `view.rs` + `mod.rs` 引用后再删。 |
| `views/chat/agent_identity.rs` | `fn monogram` 改 `pub fn monogram`,供新组件复用(避免第三处重复首字逻辑)。 |

## 5. `view.rs` 顶部叠层放置

聊天区现有顶部叠层:

```
<div class="absolute inset-x-0 top-0 z-10"><SessionTabs /></div>
```

`SessionTabs` 仅在 ≥2 个单聊 agent 打开时渲染;群聊态通常为空,故右上角空闲。新增**独立**绝对定位元素(不与 SessionTabs 混排,避免互相挤压):

```
<Show when=move || chat.team_id.get().is_some()>
    <div class="absolute top-2 right-2 z-20"><TeamParticipants /></div>
</Show>
```

**实现注意**:`ChatBandChrome`(全局 `aleph-main-drag-band`,在 `<main>` 顶部)挂了工作区切换按钮,`NotificationCenter` 铃是 window-fixed。二者在交通灯行/窗口右上,与聊天区内部 `top-2 right-2` 处于不同层。实现时**目测验证无重叠**;若与工作区切换按钮/铃相撞,把 `right-2` 增大(左移)或下移 `top`。

## 6. 组件设计:`TeamParticipants`

`#[component] pub fn TeamParticipants() -> impl IntoView`,`expect_context::<ChatState>()`,本地 `let open = RwSignal::new(false);`。

### 折叠态(按钮)
- 横向:重叠头像簇 + 下拉小三角 `▾`。整体是一个 `<button on:click=move|_| open.update(|o| *o = !*o)>`。
- 头像簇:`chat.team_members.get()` 取前 4 个,每个 `w-6 h-6 rounded-full`,重叠 `-ml-2`(首个不重叠),`style="background:{color}1f;color:{color}"`,内容 = `member_glyph(member)`。color = `agent_color_for_id(&member.agent_id)`。
- 溢出:`len > 4` 时追加一个同尺寸盘,内容 `"+{len-4}"`,中性灰底。
- 折叠按钮容器默认样式(可直接用,实现时可微调贴合邻近 chrome):`rounded-full px-1.5 py-1 bg-surface-raised/70 backdrop-blur border border-border/60 hover:bg-surface-raised/90 transition-colors`。

### 展开态(浮层)
`<Show when=move || open.get()>` 内:
1. 透明 backdrop:`<div class="fixed inset-0 z-10" on:click=move|_| open.set(false)/>`(点外部关闭;不抢焦点,合 R5)。
2. 浮层卡片:`<div class="absolute right-0 top-full mt-1 z-20 min-w-[180px] rounded-lg border border-border bg-surface-raised/95 backdrop-blur shadow-lg p-1.5 space-y-0.5">`。
3. 逐行(`chat.team_members.get()`,leader 优先可选——保持 `team_members` 原序即可,leader 已在序内):
   - 状态点 `●`,色 = `status_color(member.status)`(沿用 §1 四色)。
   - 头像盘 `w-6 h-6 rounded-full`(同折叠态单盘样式)。
   - 名字 `truncate`。
   - `member.is_leader` 时附 `<span class="text-[10px] opacity-60">"leader"</span>`(文案走 i18n,若无现成 key 则新增 `chat.team_leader`)。

### 纯函数(host 可测,放本文件)
- `pub fn member_glyph(m: &TeamMemberView) -> String`:`m.emoji` 非空则用,否则 `agent_identity::monogram(&m.name)`。
- `pub fn cluster_overflow(n: usize) -> Option<usize>`:`if n > 4 { Some(n - 4) } else { None }`。
- `fn status_color(s: MemberStatus) -> &'static str`:四分支映射(与 `team_roster.rs` 一致)。

> 渲染走 Leptos 信号;以上纯函数与 `agent_color_for_id` 提供可单测的逻辑核,DOM 由人工 E2E 覆盖。

## 7. 意见1(用户右对齐)处理

`messages.rs` 用户气泡路径已是 `justify-end` + 单聊 chip + 无头像,且 `push_user_message` 恒置 `agent_id=None`。**不改代码**;在 E2E 勾选"用户消息在右、无头像、占满宽度后仍正确"。如实测暴露反例,届时再开最小修复(不预先改)。

## 8. 被否备选(取舍记录)
- **抽屉式滑入/滑出 rail**:打开即恢复挤宽度的列;用户要"弹窗"非抽屉 → 否。
- **塞进 `WorkspacePanel` 第三个 tab(成员/交付物/任务)**:默认收起看不到人,违背"让群聊人物更明显" → 否。

## 9. 测试
- **单元(host)**:`team_participants.rs` 的 `cluster_overflow`(0/4 → None;5 → Some(1))、`member_glyph`(有 emoji 用 emoji;无 emoji 用名字首字大写;空名 → "?")、`status_color` 四分支。复用 `agent_identity` 既有测试。
- **构建**:`cargo build -p aleph-panel --target wasm32-unknown-unknown` 通过(或 `just wasm`)。
- **人工 E2E**:
  1. 进群聊 → 左侧**不再有**成员栏;对话区占满全宽。
  2. 右上角见头像簇(≤4 盘 + 溢出 "+N")。
  3. 点头像簇 → 浮层列 leader+成员 + 实时状态点;点浮层外关闭。
  4. 用户消息在右、无头像;agent 消息在左带头像盘(Layout A 不变)。
  5. 单聊无任何变化(零回归)。

## 10. 风险 / 注意
- **孤儿删除**:删 `team_roster.rs` 前 grep 确认无其他引用(`TeamRoster` 组件 + `agent_color` 是否被别处用;`team_events::agent_color` 是另一函数,勿误删——`team_roster.rs` 内 `use ...team_events::agent_color` 只是消费方)。
- **放置重叠**:见 §5,实测目测验证右上角不撞工作区切换/通知铃。
- **DRY**:`monogram` 改 `pub` 复用,避免第三处首字逻辑;不顺手重构 `messages.rs::team_avatar`(超范围)。
- **部署链**:改完需 `just wasm` → 重编 `aleph-server`(rust_embed 烧 dist)→ 换运行中 daemon binary,否则看不到效果(见 CLAUDE.md)。
