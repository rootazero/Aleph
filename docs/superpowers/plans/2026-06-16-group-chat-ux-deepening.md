# 群聊体验深耕 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 群聊气泡显示头像+display name(身份在气泡外)、＠ 从花名册自动补全、群聊像单聊一样进侧栏历史可随时接续。

**Architecture:** 一个共享前端 helper `agent_identity`(id→{name,avatar,color(哈希)})统一三处呈现;阶段一纯前端(气泡 Layout A + ＠ 调色板 + 新建按钮图标);阶段二加两个轻量后端 RPC(`teams.chat.history` 回放消息、`agents.teams` 摘要补头像簇/最近一条)+ 侧栏 Layout C(群聊可折叠+单一滚动+点进接续)。

**Tech Stack:** Rust core (`alephcore`) gateway handlers + teams 模块;Leptos/WASM 面板 (`aleph-panel`);SQLite `team_messages`;JSON-RPC over WS。

**Source spec:** `docs/superpowers/specs/2026-06-16-group-chat-ux-deepening-design.md`

---

## 执行约定 (Execution Conventions — READ FIRST)

- **测试边界**: 纯函数(颜色哈希、身份解析、＠ 匹配、气泡合并判定、history DTO 映射、后端 handler 逻辑)写 `#[cfg(test)]` 单测并真跑;Leptos `view!` 渲染层**无法单测**,以 `cargo build --target wasm32` 编译通过 + 人工 E2E 验收。
- **cargo 节制**: 每个 Task **批量跑一次** 验证(build/test 合并),不要每个 step 都跑。这是对本项目"极度节制 cargo 调用"约定的刻意适配。
- **面板 host 单测**: 纯函数测试用 `cargo test -p aleph-panel <filter>`(host 目标,沿用 `team_events.rs` 已有 `#[cfg(test)]` 先例)。若该 crate 在本机不走 host 测试,改用 `cargo build -p aleph-panel --target wasm32-unknown-unknown` 编译验证 + 人工 E2E,并在提交说明里注明。
- **提交规范**: `<scope>: <description>`(英文),无 Co-Authored-By 署名(项目全局禁用)。每个 Task 末尾提交一次。
- **部署刷新链**(阶段二末或需要看效果时): `just wasm` → `cargo build --release -p alephcore --bin aleph-server` → 替换运行中的 binary 让 supervisor relaunch(见 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」)。单跑 `just wasm` 不够。
- **不要碰** `src/harness/`(R10)。面板不自算成员归属,只调 `agents.teams`(R4)。

---

# 阶段一 — 气泡身份 + ＠ 补全 + 新建按钮(纯前端,零后端)

## Task 1: 共享身份 helper `agent_identity`

**Files:**
- Create: `interfaces/webchat/src/views/chat/agent_identity.rs`
- Modify: `interfaces/webchat/src/views/chat/mod.rs`(加 `mod agent_identity;` 声明)

- [ ] **Step 1: 写新模块(含失败前的测试)**

Create `interfaces/webchat/src/views/chat/agent_identity.rs`:

```rust
//! Shared agent visual identity for chat surfaces (bubbles, sidebar clusters,
//! @-mention palette). Resolves an agent_id to a display name, an avatar glyph
//! (emoji or monogram fallback), and a stable color hashed from the id.
//!
//! Pure functions only — host-testable, no Leptos signals or DOM.

use crate::api::agents::AgentSummary;
use std::collections::HashMap;

/// 6-slot palette shared with the roster rail. Slot chosen by id hash so a
/// given agent keeps its color regardless of roster order.
const PALETTE: [&str; 6] = ["#7c9cff", "#4ec9b0", "#e0a458", "#c586c0", "#4fc1ff", "#d16969"];

/// Stable color for an agent, hashed from its id (FNV-1a 32-bit). Deterministic
/// across sessions, independent of roster membership/order.
#[must_use]
pub fn agent_color_for_id(agent_id: &str) -> &'static str {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in agent_id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

/// Resolved visual identity for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentityView {
    pub name: String,
    /// Avatar glyph: the agent's emoji, or a 1-char monogram fallback.
    pub avatar: String,
    pub color: &'static str,
}

/// Resolve identity from an id→summary map (built from `agents.list`). Falls
/// back gracefully: name→id, emoji→monogram(first char of name/id), color always.
#[must_use]
pub fn agent_identity(agent_id: &str, agents: &HashMap<String, AgentSummary>) -> AgentIdentityView {
    let summary = agents.get(agent_id);
    let name = summary
        .and_then(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| agent_id.to_string());
    let avatar = summary
        .and_then(|s| s.emoji.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| monogram(&name));
    AgentIdentityView { name, avatar, color: agent_color_for_id(agent_id) }
}

/// First character of `source`, uppercased, as a monogram avatar. Empty → "?".
fn monogram(source: &str) -> String {
    source
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(id: &str, name: Option<&str>, emoji: Option<&str>) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: name.map(String::from),
            emoji: emoji.map(String::from),
            description: None,
            model: None,
            is_default: false,
        }
    }

    #[test]
    fn color_is_stable_per_id_and_in_palette() {
        assert_eq!(agent_color_for_id("risk_analyst"), agent_color_for_id("risk_analyst"));
        assert!(PALETTE.contains(&agent_color_for_id("anything")));
    }

    #[test]
    fn resolves_name_and_emoji_when_present() {
        let mut m = HashMap::new();
        m.insert("a".to_string(), sum("a", Some("风险分析师"), Some("🛡️")));
        let id = agent_identity("a", &m);
        assert_eq!(id.name, "风险分析师");
        assert_eq!(id.avatar, "🛡️");
    }

    #[test]
    fn falls_back_to_id_and_monogram_for_unknown_agent() {
        let m = HashMap::new();
        let id = agent_identity("growth_analyst", &m);
        assert_eq!(id.name, "growth_analyst");
        assert_eq!(id.avatar, "G");
    }

    #[test]
    fn monogram_uses_name_first_char_when_no_emoji() {
        let mut m = HashMap::new();
        m.insert("x".to_string(), sum("x", Some("alice"), None));
        assert_eq!(agent_identity("x", &m).avatar, "A");
    }
}
```

- [ ] **Step 2: 注册模块**

In `interfaces/webchat/src/views/chat/mod.rs`, add alongside the other `mod` lines (e.g. near `mod team_events;`):

```rust
mod agent_identity;
```

If sibling modules use `pub mod`, match that; the helper must be reachable as `crate::views::chat::agent_identity::{agent_identity, agent_color_for_id, AgentIdentityView}`. Re-export if the file's convention is to re-export from `mod.rs`.

- [ ] **Step 3: 跑测试(一次)**

Run: `cargo test -p aleph-panel agent_identity`
Expected: 4 tests pass. (若 host 测试不可用 → `cargo build -p aleph-panel --target wasm32-unknown-unknown` 编译通过即可,提交说明注明降级。)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/chat/agent_identity.rs interfaces/webchat/src/views/chat/mod.rs
git commit -m "panel: shared agent_identity helper (name/avatar/hash-color)"
```

---

## Task 2: 群聊气泡 Layout A(头像外置 + 名字在上 + 连续合并)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs`(team 归属分支,Explore 定位 ~477-492)
- Modify: `interfaces/webchat/src/views/chat/team_events.rs`(把 `agent_color(index)` 迁到 `agent_color_for_id`;roster rail 调用点同迁)

**前置阅读(实现者必读)**: 当前 `messages.rs` 里 `message.agent_id` 为 `Some` 时渲染彩色名字标签的那段(Explore: ~477-492,形如 `style=format!("color:{color}")`);以及消息列表的迭代位置(需要拿到"上一条已渲染消息的 agent_id"来做合并)。气泡现有容器/Tailwind 类沿用(`bg-surface-*`、圆角等),只重排身份件。

- [ ] **Step 1: 加合并判定纯函数 + 测试**

In `interfaces/webchat/src/views/chat/agent_identity.rs`, append:

```rust
/// Telegram-style grouping: show the avatar + name header only when this
/// message starts a new run of the same agent. `prev` is the agent_id of the
/// previously rendered message (None for the first, or a non-team message).
#[must_use]
pub fn should_show_attribution(prev: Option<&str>, this: Option<&str>) -> bool {
    match this {
        None => false,            // own / single-agent message: never a team header
        Some(id) => prev != Some(id),
    }
}
```

Append to that file's `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn attribution_shows_on_agent_change_and_hides_on_repeat() {
        assert!(should_show_attribution(None, Some("a")));        // first incoming
        assert!(should_show_attribution(Some("b"), Some("a")));   // switched agent
        assert!(!should_show_attribution(Some("a"), Some("a")));  // same agent → merge
        assert!(!should_show_attribution(Some("a"), None));       // own message
    }
```

- [ ] **Step 2: 迁移颜色到 hash,并在气泡分支渲染 Layout A**

In `team_events.rs`, the roster-rail / bubble color must come from `agent_color_for_id(&agent_id)` (use `crate::views::chat::agent_identity::agent_color_for_id`). Keep `agent_color(index)` only if another live caller remains; if the only callers were bubble+roster, delete `agent_color` and its two `#[cfg(test)]` tests (entropy reduction). Grep `agent_color(` before deleting.

In `messages.rs`, replace the team-attribution rendering. The iteration must track the previous message's `agent_id`; pass it to `should_show_attribution`. Render (Tailwind + inline color):

```rust
// inside the message-row view, only when message.agent_id.is_some():
let id_view = crate::views::chat::agent_identity::agent_identity(agent_id, &agents_map);
let show_header = crate::views::chat::agent_identity::should_show_attribution(prev_agent_id, Some(agent_id));
view! {
    <div class="flex gap-2 items-start">
        // avatar column — rendered when starting a new run, else a 30px spacer
        {if show_header {
            view! {
                <div class="w-7 h-7 rounded-full flex items-center justify-center text-sm shrink-0"
                     style=format!("background:{}1f;color:{}", id_view.color, id_view.color)>
                    {id_view.avatar.clone()}
                </div>
            }.into_any()
        } else {
            view! { <div class="w-7 shrink-0"></div> }.into_any()
        }}
        <div class="flex flex-col min-w-0">
            {show_header.then(|| view! {
                <div class="text-[11px] font-semibold mb-0.5 ml-0.5"
                     style=format!("color:{}", id_view.color)>
                    {id_view.name.clone()}
                </div>
            })}
            <div class="chat-bubble">/* existing bubble text content unchanged */</div>
        </div>
    </div>
}
```

`agents_map: HashMap<String, AgentSummary>` must be available where bubbles render. If the messages view already holds the roster or an agents list, build the map there; otherwise thread it from the parent (the team roster `chat.team_members` carries ids; the `agents.list` cache carries name/emoji). Reuse whatever the surrounding component already fetched — do NOT add a new RPC call inside the render loop. `background:{color}1f` appends alpha `0x1f` to the hex for the tinted disc (matches the mockup).

- [ ] **Step 3: 验证(一次)**

Run host test + wasm build together:
```
cargo test -p aleph-panel agent_identity && cargo build -p aleph-panel --target wasm32-unknown-unknown
```
Expected: 5 tests pass; wasm compiles clean.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/chat/messages.rs interfaces/webchat/src/views/chat/team_events.rs interfaces/webchat/src/views/chat/agent_identity.rs
git commit -m "panel: group-chat bubble Layout A (avatar+name outside, consecutive merge)"
```

---

## Task 3: ＠ 自动补全调色板

**Files:**
- Create: `interfaces/webchat/src/views/chat/mention_palette.rs`
- Modify: `interfaces/webchat/src/views/chat/mod.rs`(`mod mention_palette;`)
- Modify: `interfaces/webchat/src/views/chat/composer/mod.rs`(触发 + 插入)

**前置阅读(实现者必读)**: `composer/mod.rs`(Explore: InputArea ~100-143;`SlashPaletteView` import 在 ~17 — 把它当结构范本:如何监听 textarea、如何浮层、如何把选中项写回 textarea)。team 模式判定用 `chat.team_id.get_untracked()`(已在 send 路径出现)。花名册 `chat.team_members`(`TeamMemberView` 带 `agent_id`、`name`)。

- [ ] **Step 1: 加 ＠ 匹配纯函数 + 测试**

Create `interfaces/webchat/src/views/chat/mention_palette.rs` (logic first):

```rust
//! @-mention autocomplete for team chat. Typing '@' opens a roster picker
//! (avatar + name + dim id); selecting inserts the canonical `@<id>` token —
//! the backend (`teams/messages/mentions.rs`) resolves on agent_id, and names
//! are not unique, so the inserted token is always the id.

/// Match a roster candidate against the text typed after '@'. Matches name OR
/// id, case-insensitive. Empty query matches everything.
#[must_use]
pub fn mention_matches(query: &str, name: &str, id: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    name.to_lowercase().contains(&q) || id.to_lowercase().contains(&q)
}

/// Given textarea text and the caret byte offset, return the active '@' query
/// (text from the '@' up to caret) if the caret is inside a mention token, else
/// None. A token is `@` preceded by start-or-whitespace, followed by
/// `[A-Za-z0-9_-]*` with no whitespace before the caret.
#[must_use]
pub fn active_mention_query(text: &str, caret: usize) -> Option<String> {
    let head = text.get(..caret)?;
    let at = head.rfind('@')?;
    // char before '@' must be start or whitespace
    if at > 0 {
        let prev = head[..at].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let token = &head[at + 1..];
    if token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        Some(token.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_name_or_id_case_insensitive() {
        assert!(mention_matches("ris", "风险", "risk_analyst"));
        assert!(mention_matches("风", "风险", "risk_analyst"));
        assert!(mention_matches("", "x", "y"));
        assert!(!mention_matches("zzz", "风险", "risk_analyst"));
    }

    #[test]
    fn detects_active_query_at_caret() {
        assert_eq!(active_mention_query("hi @ris", 7), Some("ris".to_string()));
        assert_eq!(active_mention_query("@all", 4), Some("all".to_string()));
        assert_eq!(active_mention_query("email a@b", 9), None); // '@' not after ws
        assert_eq!(active_mention_query("done @ris now", 13), None); // caret past token
    }
}
```

- [ ] **Step 2: 注册模块 + 跑测试(一次)**

Add `mod mention_palette;` to `views/chat/mod.rs`. Run: `cargo test -p aleph-panel mention_palette` → 2 tests pass.

- [ ] **Step 3: 渲染调色板组件 + 接入 composer**

In `mention_palette.rs` add a `#[component] pub fn MentionPaletteView(...)` mirroring `SlashPaletteView`'s structure. Behavior:
- Props: the textarea node/ref + signals the composer already exposes, the team roster (`chat.team_members`), and the agents map for `agent_identity`.
- On textarea `input`, compute `active_mention_query(text, caret)`; when `Some(q)` and `chat.team_id` is set, show a floating list: a top **「@所有人」** row (inserts `@all `) followed by roster members where `mention_matches(&q, &name, &id)`. Each row: `agent_identity` avatar disc + name + dim `id` (`text-text-tertiary text-[10px]`).
- On select: replace the active `@<query>` span in the textarea with `@<id> ` (canonical id + trailing space) — splice using the `@` offset from Step 1 and the caret; reuse the composer's existing setter that `SlashPaletteView` uses to write back.
- Hide on whitespace/escape/blur/empty-roster.

Mount `MentionPaletteView` inside the composer next to the existing `SlashPaletteView` (only active in team mode). Do NOT modify `mentions.rs`/`message_send.rs`/`targets.rs` — they already resolve `@<id>` and `@all`.

- [ ] **Step 4: 验证(一次)**

```
cargo test -p aleph-panel mention_palette && cargo build -p aleph-panel --target wasm32-unknown-unknown
```
Expected: tests pass; wasm compiles.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/mention_palette.rs interfaces/webchat/src/views/chat/mod.rs interfaces/webchat/src/views/chat/composer/mod.rs
git commit -m "panel: @-mention autocomplete palette (insert canonical @<id>)"
```

---

## Task 4: 新建按钮 → "+" 方形图标

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:646-652`

- [ ] **Step 1: 替换按钮内容与形状**

Replace the new-chat button (currently text `{t_string!(i18n, chat.new)}`) with a square "+" icon button; move the i18n string to `title` + `aria-label`:

```rust
<button
    class="w-9 h-9 shrink-0 flex items-center justify-center rounded-lg bg-primary text-white
           hover:bg-primary/90 transition-colors"
    title=move || t_string!(i18n, chat.new).to_string()
    aria-label=move || t_string!(i18n, chat.new).to_string()
    on:click=on_new_chat
>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <line x1="12" y1="5" x2="12" y2="19" />
        <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
</button>
```

(If `title`/`aria-label` closures don't typecheck in this Leptos version, use plain string bindings — read a neighboring icon button in the file for the exact attribute form.)

- [ ] **Step 2: 验证(一次)**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown`
Expected: compiles clean.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: new-chat button as square + icon"
```

---

# 阶段二 — 后端 RPC + 侧栏 Layout C + 历史接续

## Task 5: 后端 RPC `teams.chat.history`

**Files:**
- Modify: `src/gateway/handlers/teams.rs`(新 handler + 注册,紧邻 `handle_chat_thread` ~833-891)

**前置阅读(实现者必读)**: `handle_chat_thread`(返回 `{items:[...]}` 的脚手架:ctx 怎么拿 store、怎么建响应、方法名怎么注册到 dispatch);`MessageStore::list_team_messages(team_id)`(`src/teams/messages/store.rs` ~119-130,**不按 expires_at 过滤**);`TeamMessage`(`src/teams/messages/types.rs:113-127`:`from_agent`、`content`、`msg_type: MessageType`、`created_at: DateTime<Utc>`)。

- [ ] **Step 1: 写 handler + 单测**

Add a handler that mirrors `handle_chat_thread`'s scaffolding but reads messages. Bubble DTO + mapping (real logic; adapt ctx/store accessor to match `handle_chat_thread`):

```rust
#[derive(serde::Serialize)]
struct ChatHistoryItem {
    from_agent: String,
    content: String,
    msg_type: String,
    created_at: i64, // epoch millis
}

// inside handle_chat_history(team_id): after fetching messages from the store
let items: Vec<ChatHistoryItem> = messages
    .into_iter()
    .map(|m| ChatHistoryItem {
        from_agent: m.from_agent,
        content: m.content,
        msg_type: m.msg_type.as_str().to_string(), // MessageType→str (use existing accessor)
        created_at: m.created_at.timestamp_millis(),
    })
    .collect();
// messages already chronological from the store; if not, sort_by_key(|i| i.created_at)
// respond with { "items": items }
```

Register the method string `teams.chat.history` next to `teams.chat.thread` in the same dispatch/registration site. team not found → return `{ "items": [] }` (not an error).

Add a unit test next to the other teams-handler tests (mirror their construction of an in-memory store; if none exists, test the pure mapping by factoring the `Vec<TeamMessage> -> Vec<ChatHistoryItem>` step into a free fn `map_history(msgs) -> Vec<ChatHistoryItem>` and testing that):

```rust
#[test]
fn maps_team_messages_to_history_items_in_order() {
    // build two TeamMessage values (t0 < t1), call map_history, assert
    // from_agent/content carried through and created_at ascending.
}
```

- [ ] **Step 2: 验证(一次)**

Run: `cargo test -p alephcore --lib chat_history`
Expected: mapping test passes. (`cargo check -p alephcore` if a full test run is too heavy — note in commit.)

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/teams.rs
git commit -m "gateway: teams.chat.history RPC (replay durable team_messages as bubbles)"
```

---

## Task 6: 后端 `agents.teams` 摘要增补(members_preview + last_message)

**Files:**
- Modify: `src/gateway/handlers/teams.rs`(`handle_agent_teams` / 构造 `TeamSummary` 响应处,Explore ~50-126)

**前置阅读(实现者必读)**: `handle_agent_teams`/`handle_list` 怎么构造每个 team 的摘要 JSON;`TeamStore::get_members(team_id)`(roster);`MessageStore::list_team_messages`(取最后一条);agent 的 emoji/name 来源(`AgentManager`/`agents.list` 同源 — 复用,别新查每成员)。

- [ ] **Step 1: 在 agents.teams 每个 team 摘要里附加两字段**

For each team returned by `agents.teams(agent_id)`, augment the JSON object with:
- `members_preview`: up to 4 entries `{ id, name, emoji }` from `get_members(team_id)` (resolve name/emoji from the agent registry; omit emoji if unset).
- `last_message`: the most recent `team_messages` content for that team, truncated to ~60 chars, or `null` if none.

Real shaping logic (adapt accessors to the handler's context):

```rust
let members = store.get_members(&team.id).await?;
let members_preview: Vec<serde_json::Value> = members.iter().take(4).map(|mem| {
    let def = agent_manager.get(&mem.agent_id); // existing lookup
    serde_json::json!({
        "id": mem.agent_id,
        "name": def.and_then(|d| d.name.clone()).unwrap_or_else(|| mem.agent_id.clone()),
        "emoji": def.and_then(|d| d.identity_emoji()), // existing emoji accessor; None ok
    })
}).collect();

let last_message = message_store
    .list_team_messages(&team.id).await
    .ok()
    .and_then(|mut v| v.pop()) // store is chronological → last is newest
    .map(|m| m.content.chars().take(60).collect::<String>());
```

Insert `members_preview` + `last_message` into the existing per-team JSON. Keep `teams.list` (non-agent-scoped) unchanged unless trivially shared. This is best-effort: any per-team store error → `members_preview: []`, `last_message: null`, never fail the whole list.

- [ ] **Step 2: 验证(一次)**

Run: `cargo check -p alephcore`
Expected: compiles. (Shape is exercised end-to-end in Task 9 E2E; a focused unit test is optional if the handler isn't easily unit-constructable.)

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/teams.rs
git commit -m "gateway: agents.teams summary carries members_preview + last_message"
```

---

## Task 7: 前端 API — `TeamChatApi::history` + `TeamSummary` 增补字段

**Files:**
- Modify: `interfaces/webchat/src/api/team_chat.rs`(加 `TeamMessageItem` + `history()`)
- Modify: `interfaces/webchat/src/api/teams.rs`(`TeamSummary` 加两个可选字段 + `MemberPreview`)

- [ ] **Step 1: 加 history DTO + 方法 + 测试**

In `interfaces/webchat/src/api/team_chat.rs`:

```rust
/// One replayed group-chat bubble from `teams.chat.history`.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamMessageItem {
    pub from_agent: String,
    pub content: String,
    pub msg_type: String,
    pub created_at: i64,
}

impl TeamChatApi {
    /// Replay the durable group-chat transcript as bubbles, chronologically.
    pub async fn history(state: &DashboardState, team_id: &str) -> Result<Vec<TeamMessageItem>, String> {
        let result = state
            .rpc_call("teams.chat.history", json!({ "team_id": team_id }))
            .await?;
        let items = result.get("items").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| e.to_string())
    }
}
```

Add to that file's `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn deserializes_history_item() {
        let j = r#"{"from_agent":"risk_analyst","content":"hi","msg_type":"message","created_at":123}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.from_agent, "risk_analyst");
        assert_eq!(it.created_at, 123);
    }
```

- [ ] **Step 2: TeamSummary 加可选字段**

In `interfaces/webchat/src/api/teams.rs`, add a `MemberPreview` type and two `#[serde(default)]` fields to `TeamSummary` (additive — existing `teams.list`/`teams.get` responses without them still parse):

```rust
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MemberPreview {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
}
```

Add inside `struct TeamSummary { ... }`:

```rust
    #[serde(default)]
    pub members_preview: Vec<MemberPreview>,
    #[serde(default)]
    pub last_message: Option<String>,
```

- [ ] **Step 3: 验证(一次)**

```
cargo test -p aleph-panel team_chat && cargo build -p aleph-panel --target wasm32-unknown-unknown
```
Expected: deserialization test passes; wasm compiles (additive fields don't break existing call sites).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/api/team_chat.rs interfaces/webchat/src/api/teams.rs
git commit -m "panel/api: teams.chat.history wrapper + TeamSummary members_preview/last_message"
```

---

## Task 8: 侧栏 Layout C(可折叠群聊节 + 头像簇 + 点进接续)

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`(会话列表区,Explore ~682-953;agent 过滤 ~705)

**前置阅读(实现者必读)**: 当前会话列表的获取/过滤/渲染(`sessions.list` → `SessionEntry`,按 `selected_agent` 过滤 ~705,排序 ~719,行渲染 ~734-948,`on_select_session` ~390-424);进 team 模式怎么设(`chat.team_id`/`chat.team_members` + `subscribe_team_events` — 见 `team_events.rs` 与 team_compose 的进入路径)。

- [ ] **Step 1: 拉取选中 agent 的群聊**

When `selected_agent` changes (reuse the existing `reload_data`/effect that loads `sessions.list`), also call `TeamsApi::agent_teams(&dash, &agent_id)` into a `RwSignal<Vec<TeamSummary>>` named `groups`. On agents map: reuse the `agents.list` result already fetched for the agent dropdown to build the `HashMap<String, AgentSummary>` for `agent_identity` (do not add a new RPC).

- [ ] **Step 2: 渲染单一滚动列表 = 可折叠「群聊」节 + 「单聊」节**

Wrap the existing session-list scroll container so it holds, top-to-bottom in ONE scroll region:

1. A collapsible **群聊** header `群聊 <count> ▾` (a local `RwSignal<bool> collapsed`, default expanded). Hidden entirely when `groups` is empty.
2. When expanded, one row per group — avatar cluster + title + `last_message`:

```rust
view! {
    <div class="flex items-center gap-2 px-1.5 py-1.5 rounded-lg hover:bg-surface-sunken cursor-pointer"
         on:click=move |_| on_open_group(group.id.clone())>
        <div class="flex shrink-0">
            {group.members_preview.iter().take(3).map(|mp| {
                let idv = agent_identity(&mp.id, &agents_map);
                let glyph = mp.emoji.clone().filter(|e| !e.is_empty()).unwrap_or(idv.avatar.clone());
                view! {
                    <div class="w-6 h-6 rounded-full -ml-2 first:ml-0 flex items-center justify-center text-[11px]"
                         style=format!("background:{}1f;color:{};border:2px solid var(--surface)", idv.color, idv.color)>
                        {glyph}
                    </div>
                }
            }).collect_view()}
        </div>
        <div class="flex flex-col min-w-0 flex-1">
            <div class="text-sm font-semibold text-text-primary truncate">{group.name.clone()}</div>
            <div class="text-[11px] text-text-tertiary truncate">
                {group.last_message.clone().unwrap_or_default()}
            </div>
        </div>
    </div>
}
```

3. A **单聊** subheader, then the existing filtered `SessionEntry` rows (unchanged logic from ~734-948). The whole thing scrolls as one region (single `overflow-y-auto`); the group section is natural-height (no inner scrollbar), per Layout C.

Use the project's surface CSS var for the avatar ring border (read a neighboring component for the exact token name; `var(--surface)` is a placeholder — match the codebase).

- [ ] **Step 3: 点群聊行 → 进 team 模式 + 回放历史**

Implement `on_open_group(team_id)`:

```rust
// 1. fetch roster, set team mode
let detail = TeamsApi::get(&dash, &team_id).await?;        // members → chat.team_members (TeamMemberView)
chat.clear_session();
chat.team_id.set(Some(team_id.clone()));
chat.team_members.set(/* map detail.members → TeamMemberView */);
// 2. replay history into the bubble list
let items = TeamChatApi::history(&dash, &team_id).await?;
chat.messages.set(items.into_iter().map(|it| /* build ChatMessage with agent_id: Some(it.from_agent) */).collect());
// 3. ensure team.* subscription is active (subscribe_team_events) if not already
```

Build each `ChatMessage` exactly as `team_events.rs:38-54` does (same field set), with `agent_id: Some(it.from_agent)`, `content: it.content`, `role: "assistant"`, `timestamp: Some(it.created_at)`, `is_final: true`, `text_finalized: true`. Map `detail.members` (`TeamMember{agent_id, role, ...}`) to the panel's `TeamMemberView` shape used by the roster rail (read `state.rs` for its fields; resolve name/status via roster + `agent_identity`). Re-entry is idempotent — guard against double-subscribing `team.*`.

- [ ] **Step 4: 验证(一次)**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown`
Expected: compiles clean. (UI behavior验收在 Task 9。)

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: sidebar group-chat section (collapsible cluster rows + resume history)"
```

---

## Task 9: 集成构建 + 部署 + 人工 E2E

**Files:** 无代码改动(除非 E2E 暴露缺陷)。

- [ ] **Step 1: 全量 WASM + server 重建**

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```
Expected: both succeed. (`just wasm` 重建 `dist/*`;server 重建让 `rust_embed` 把新 dist 烧进 binary。)

- [ ] **Step 2: 替换运行中的 binary 让 supervisor relaunch**

Per CLAUDE.md「Panel ↔ Daemon 资源嵌入链」(dev 或 .app daemon 二选一):
```bash
./target/release/aleph-server stop && cargo run --release -p alephcore --bin aleph-server start
```
(或 .app daemon: 备份旧 binary → cp 新 binary → kill pid,supervisor relaunch。)

- [ ] **Step 3: 人工 E2E 清单(逐条勾)**

- [ ] 侧栏:选中一个参与群聊的 agent → 顶部出现可折叠「群聊」节,行带成员头像簇 + 标题 + 最近一条;折叠/展开正常;无群聊的 agent 整节隐藏。
- [ ] 点群聊行 → 历史气泡回放(头像在外、彩色名字在上、同一 agent 连续合并),进入 team 模式。
- [ ] 在群里发言 → 成员实时回复,气泡带正确头像/名字归属;`team.*.activity` 花名册状态变化。
- [ ] 输入 `@` → 弹出花名册下拉(头像+名字+灰 id),过滤可用;选中插入 `@<id>`;目标成员被点名响应。`@所有人` → 全员响应。
- [ ] 新建按钮显示为方形 "+" 图标,hover 提示文案,点击新建会话正常。
- [ ] 单聊路径完全不受影响(气泡、列表、新建)。

- [ ] **Step 4: Commit(若 E2E 触发修复)**

```bash
git add -A
git commit -m "panel: group-chat UX E2E fixes"
```

---

## Self-Review(写计划者已核对)

- **Spec coverage**: Req1 气泡=Task2;头像来源/哈希色=Task1;＠ 补全=Task3;新建按钮=Task4;Req2 RPC=Task5;agents.teams 增补=Task6;前端 API=Task7;侧栏 Layout C + 接续=Task8;红线(不碰 harness、面板纯 I/O)贯穿。MVP 边界(不进 SessionSnapshot / 不美化 @id→name / 不上传图片)= 按 spec 未列任务,正确。
- **Type consistency**: `agent_identity`/`agent_color_for_id`/`should_show_attribution`/`AgentIdentityView`/`mention_matches`/`active_mention_query`/`TeamMessageItem`/`MemberPreview`/`ChatHistoryItem` 跨任务一致;`AgentSummary`/`TeamSummary`/`TeamMessage` 字段与已读源文件一致。
- **已知降级(非占位,刻意)**: Leptos `view!` 渲染层以 build + 人工 E2E 验收(无单测);若干后端 handler 脚手架(ctx/store 访问、方法注册)要求实现者照 `handle_chat_thread` 现有范式接入 —— 已在每个 Task 的「前置阅读」明确点名,非"TODO 占位"。
