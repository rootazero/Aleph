# Multiagent 群聊广播范式 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `teams.chat.send` 从 leader 单点编排(hub-and-spoke)改成 telegram 式平等广播——用户/agent 消息经 @mention 触发每个被点名 agent 各自独立 run,回复进共享 transcript 并可互相 @ 接话,代码层所有 agent 平等、leader 仅靠 prompt 身份。

**Architecture:** 复用现有 `teams/messages` 总线(store/mentions/router)+ `TeamFanoutEmitter` + Panel attributed bubbles;新增一个**广播编排器**(`src/teams/broadcast/`),核心是一个递归 `dispatch`:解析 @mention → 宽度门控 → 为每个目标用 `CollectingEventEmitter` 包进 `TeamFanoutEmitter` 跑 run → `execute().await` 后取 `final_response` → 存回 transcript → 解析回复 @mention → `chain_depth+1` 递归。`chain_depth` 是**内存递归参数,不落库**(覆盖 spec §12.1)。防风暴三道闸(@门控/深度≤6/宽度≤5)全是纯函数确定性脚手架,不进 `src/harness/`(守 R10)。

**Tech Stack:** Rust, async-trait, tokio, rusqlite, serde_json;现有 `alephcore` crate。

**关键签名(已由源码核实)：**
- `ExecutionAdapter::execute(&self, RunRequest, Arc<AgentInstance>, Arc<dyn EventEmitter+Send+Sync>) -> Result<(), ExecutionError>`(`src/gateway/execution_adapter.rs`)
- `CollectingEventEmitter::new()` / `async fn events(&self) -> Vec<StreamEvent>`(`src/gateway/event_emitter/`)
- `StreamEvent::RunComplete { run_id, seq, summary: RunSummary, total_duration_ms }`,`RunSummary.final_response: Option<String>`
- `TeamFanoutEmitter::new(bus: Arc<GatewayEventBus>, team_id: impl Into<String>, agent_id: impl Into<String>, inner: Option<Arc<dyn EventEmitter+Send+Sync>>)` + `team_event_bus() -> Option<Arc<GatewayEventBus>>`
- `extract_mentions(&str) -> Vec<String>`,`MENTION_ALL = "*all*"`(`src/teams/messages/mentions.rs`)
- `MessageStore::send_message(NewMessage) -> Result<TeamMessage>`,`NewMessage { team_id, from_agent, msg_type, subject, content, recipients, reply_to, attachments }`,`Recipient { agent_id, role: RecipientRole::{To,Cc} }`
- `SessionKey::task(agent_id, task_type, task_id)`(`src/routing/session_key.rs`)
- `GatewayContext::agent_registry() -> &Arc<AgentRegistry>`,`execution_adapter() -> &Arc<dyn ExecutionAdapter>`,`AgentRegistry::get(&str) -> Option<Arc<AgentInstance>>`

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `src/teams/broadcast/targets.rs` | 纯函数:解析 @mention → 目标 agent 集合(@all 展开/leader 兜底/去自@/去@user/宽度截断) | Create |
| `src/teams/broadcast/transcript.rs` | 纯函数:把 `Vec<TeamMessage>` 格式化为 `[from]: content` 注入文本 + token 预算从尾截断 | Create |
| `src/teams/broadcast/member_prompt.rs` | 纯函数:组装成员/leader 的 run 输入(身份 + 名册 + transcript + 接话协议 + team_id) | Create |
| `src/teams/broadcast/mod.rs` | 编排器:`GroupChatBroadcaster`,递归 `dispatch_user` / `dispatch_reply` → run agent + 回流 + chain_depth 守卫 | Create |
| `src/teams/messages/store.rs` | 加 `list_team_messages(team_id, limit)` trait 方法 + SqliteMessageStore impl | Modify |
| `src/teams/mod.rs` | `pub mod broadcast;` 导出 | Modify |
| `src/gateway/handlers/teams.rs` | `handle_chat_send` 改为:存 user 消息 → 调 `GroupChatBroadcaster::dispatch_user` | Modify |

设计单元边界:`targets`/`transcript`/`member_prompt` 是**无 IO 纯函数**(host 可测,不需真 LLM);`mod.rs` 是唯一碰 IO/异步的编排层。`leader_prompt.rs` 保留不动(其 `build` 已被本会话修过 team_id bug,仍被现有路径引用;新广播路径用新的 `member_prompt`)。

**常量**(放 `src/teams/broadcast/mod.rs` 顶部):
```rust
/// 接话链最大深度(防 A↔B 无限互@)。spec §7。
pub const MAX_CHAIN_DEPTH: u32 = 6;
/// 单轮最多同时唤醒的 agent 数(防 @all 在大群一次炸开)。spec §7。
pub const MAX_FANOUT_WIDTH: usize = 5;
/// 群 transcript 注入的 token 预算(超出从尾保留最近)。
pub const TRANSCRIPT_TOKEN_BUDGET: usize = 8000;
/// 保留 handle:agent 不能 @ 回用户(防自环)。openteams RESERVED_USER_HANDLE。
pub const RESERVED_USER_HANDLE: &str = "user";
```

---

## Task 1: store 加 `list_team_messages`（按 team_id 拉全群 transcript）

**Files:**
- Modify: `src/teams/messages/store.rs`(trait `MessageStore` + `impl SqliteMessageStore`)

群 transcript 需要"按 team_id 取全部消息,按 created_at 升序"。现有 trait 只有 `read_thread`(按 thread_id)/`read_inbox`(per-agent),缺这个。

- [ ] **Step 1: 先确认 trait 的所有 impl(避免漏给某个 impl 加方法)**

Run: `grep -rn "impl MessageStore for" src/`
Expected: 列出所有实现者(预期只有 `SqliteMessageStore`;若有 test mock 也要在 Step 3 一并加)。

- [ ] **Step 2: 写失败测试**

在 `src/teams/messages/store.rs` 的 `#[cfg(test)] mod tests` 末尾(若无则新建)加:

```rust
#[tokio::test]
async fn list_team_messages_returns_all_for_team_ordered_by_created_at() {
    let store = SqliteMessageStore::new_in_memory().await;
    // 两条不同 team 的消息 + 同 team 两条
    for (team, body) in [("team-A", "first"), ("team-B", "other"), ("team-A", "second")] {
        store
            .send_message(NewMessage {
                team_id: team.to_string(),
                from_agent: "alice".to_string(),
                msg_type: MessageType::Message,
                subject: String::new(),
                content: body.to_string(),
                recipients: vec![],
                reply_to: None,
                attachments: vec![],
            })
            .await
            .expect("send");
    }
    let got = store.list_team_messages("team-A", 100).await.expect("list");
    let bodies: Vec<&str> = got.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(bodies, vec!["first", "second"], "only team-A, in insertion order");
}

#[tokio::test]
async fn list_team_messages_respects_limit_keeping_most_recent() {
    let store = SqliteMessageStore::new_in_memory().await;
    for body in ["m1", "m2", "m3"] {
        store
            .send_message(NewMessage {
                team_id: "t".to_string(),
                from_agent: "a".to_string(),
                msg_type: MessageType::Message,
                subject: String::new(),
                content: body.to_string(),
                recipients: vec![],
                reply_to: None,
                attachments: vec![],
            })
            .await
            .expect("send");
    }
    let got = store.list_team_messages("t", 2).await.expect("list");
    let bodies: Vec<&str> = got.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(bodies, vec!["m2", "m3"], "limit keeps the most recent N, still ascending");
}
```

- [ ] **Step 3: 加 trait 方法 + impl**

在 `trait MessageStore` 内加方法声明:

```rust
    /// List all non-expired messages for a team ordered by `created_at` ascending.
    /// `limit` keeps the most recent N (still returned oldest-first). Powers the
    /// group-chat transcript injection (telegram-style broadcast).
    async fn list_team_messages(&self, team_id: &str, limit: usize) -> Result<Vec<TeamMessage>>;
```

在 `impl MessageStore for SqliteMessageStore` 内加(SQL 取最近 N 再升序;沿用本文件既有的行→`TeamMessage` 物化 helper,实现时照抄 `read_thread` 的 row-mapping 写法):

```rust
    async fn list_team_messages(&self, team_id: &str, limit: usize) -> Result<Vec<TeamMessage>> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        // 取最近 limit 条(DESC),稍后反转为升序;过滤已过期。
        let mut stmt = conn.prepare(
            "SELECT id, team_id, from_agent, msg_type, subject, content, reply_to, thread_id, created_at, expires_at \
             FROM team_messages \
             WHERE team_id = ?1 AND (expires_at IS NULL OR expires_at > ?2) \
             ORDER BY created_at DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![team_id, now, limit as i64],
            |row| Self::row_to_message(row), // 实现时:复用 read_thread 用的同一行映射;若它是内联闭包,提取成关联 fn row_to_message
        )?;
        let mut msgs: Vec<TeamMessage> = Vec::new();
        for r in rows {
            let mut m = r?;
            // recipients/attachments 子表填充:复用 read_thread 里同样的二次查询 helper
            Self::hydrate_children(&conn, &mut m)?;
            msgs.push(m);
        }
        msgs.reverse(); // DESC → 升序(oldest-first)
        Ok(msgs)
    }
```

> 注:`row_to_message` / `hydrate_children` 是占位名——实现时**照搬 `read_thread` 现有的行映射与子表填充逻辑**(同文件内),保持与既有方法一致;若现有是内联代码,先抽成关联 fn 再被两处复用(DRY)。recipients 对群 transcript 非必需,可先不填(传空)以最小化,但保持返回类型一致。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore messages::store::tests::list_team_messages -- --nocapture`
Expected: 两个测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/teams/messages/store.rs
git commit -m "teams(messages): add list_team_messages for group-chat transcript"
```

---

## Task 2: `targets.rs` — 纯函数解析 fan-out 目标

**Files:**
- Create: `src/teams/broadcast/targets.rs`
- Modify: `src/teams/mod.rs`(加 `pub mod broadcast;`),`src/teams/broadcast/mod.rs`(加 `pub mod targets;` — 见 Task 5 建 mod.rs;本 task 可先在 mod.rs 写 `pub mod targets;` 单行)

把"谁回复 + 宽度门控 + 护栏"做成一个无 IO 纯函数,便于 host 测试。

- [ ] **Step 1: 建 broadcast 模块壳**

Create `src/teams/broadcast/mod.rs`(暂时只声明子模块 + 常量):

```rust
//! 群聊广播编排器(telegram 式 multiagent)。spec 2026-06-16。
//! 纯逻辑在 targets/transcript/member_prompt,IO 编排在本文件(Task 5)。

pub mod targets;

pub const MAX_CHAIN_DEPTH: u32 = 6;
pub const MAX_FANOUT_WIDTH: usize = 5;
pub const TRANSCRIPT_TOKEN_BUDGET: usize = 8000;
pub const RESERVED_USER_HANDLE: &str = "user";
```

在 `src/teams/mod.rs` 加(放在其它 `pub mod` 附近):

```rust
pub mod broadcast;
```

- [ ] **Step 2: 写失败测试**

Create `src/teams/broadcast/targets.rs` 仅含测试(先红):

```rust
//! 纯函数:从一条触发消息解析出本轮要唤醒的 agent 集合。

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<String> {
        ["leader", "alice", "bob", "carol", "dave", "erin", "frank"]
            .iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn user_mention_specific_agents() {
        let t = resolve_targets("@alice @bob 看下", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn user_no_mention_falls_back_to_leader() {
        let t = resolve_targets("随便聊聊", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["leader".to_string()], "没@人时 leader 兜底(仅 user 触发)");
    }

    #[test]
    fn agent_reply_no_mention_stops_chain() {
        // agent 回复没@人 → 不兜底,返回空(链自然停)
        let t = resolve_targets("好的我做完了", "alice", "leader", &roster(), false);
        assert!(t.is_empty(), "agent 回复没@人不应触发 leader 兜底");
    }

    #[test]
    fn at_all_expands_to_everyone_except_sender_capped() {
        let t = resolve_targets("@all 报到", "user", "leader", &roster(), true);
        // roster 7 人,@all 排除 sender(user 不在 roster)→ 7 人,宽度上限 5
        assert_eq!(t.len(), super::super::MAX_FANOUT_WIDTH, "@all 受宽度上限截断");
        assert!(!t.contains(&"user".to_string()));
    }

    #[test]
    fn drops_self_mention_and_reserved_user() {
        // alice 回复里 @ 自己 + @user + @bob → 只剩 bob
        let t = resolve_targets("@alice @user @bob", "alice", "leader", &roster(), false);
        assert_eq!(t, vec!["bob".to_string()], "去掉自@和@user");
    }

    #[test]
    fn unknown_mention_ignored() {
        let t = resolve_targets("@nobody @alice", "user", "leader", &roster(), true);
        assert_eq!(t, vec!["alice".to_string()], "不在名册的@被忽略");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p alephcore broadcast::targets -- --nocapture`
Expected: FAIL(`resolve_targets` not found)。

- [ ] **Step 4: 实现 `resolve_targets`**

在 `src/teams/broadcast/targets.rs` 测试模块**上方**加:

```rust
use crate::teams::messages::{extract_mentions, MENTION_ALL};
use super::{MAX_FANOUT_WIDTH, RESERVED_USER_HANDLE};

/// 解析一条触发消息要唤醒哪些 agent。
///
/// - `content`: 触发消息正文(用户消息或 agent 回复)
/// - `sender`: 发送者 id(`"user"` 表示用户;否则是某 agent id)
/// - `leader_id`: 团队 leader,用于"没@人时兜底"
/// - `roster`: 团队全体成员 agent_id(含 leader)
/// - `user_triggered`: true=用户消息(没@时 leader 兜底);false=agent 回复(没@时不兜底,链停)
///
/// 规则(spec §7):@all/@everyone → 全员(除 sender);具体@ → 取名册内的;
/// 去掉自@和@`user`;用户消息没@ → [leader];agent 回复没@ → [];宽度上限截断。
#[must_use]
pub fn resolve_targets(
    content: &str,
    sender: &str,
    leader_id: &str,
    roster: &[String],
    user_triggered: bool,
) -> Vec<String> {
    let mentions = extract_mentions(content);
    let is_all = mentions.iter().any(|m| m == MENTION_ALL);

    let mut targets: Vec<String> = if is_all {
        roster.iter().cloned().collect()
    } else {
        // 只保留名册内的具体 mention,保持出现顺序
        mentions
            .into_iter()
            .filter(|m| m != MENTION_ALL && roster.iter().any(|r| r == m))
            .collect()
    };

    // 护栏:去掉发送者自己 + 保留 handle "user"
    targets.retain(|a| a != sender && a != RESERVED_USER_HANDLE);

    // 去重(保持首次出现顺序)
    let mut seen = std::collections::HashSet::new();
    targets.retain(|a| seen.insert(a.clone()));

    // 没@人:用户触发 → leader 兜底;agent 回复 → 链停(空)
    if targets.is_empty() && user_triggered {
        let leader = leader_id.to_string();
        if leader != sender {
            targets.push(leader);
        }
    }

    // 宽度上限
    targets.truncate(MAX_FANOUT_WIDTH);
    targets
}
```

> 注:`@all` 测试里 sender=`"user"` 不在 roster,排除后仍 7 人→截到 5。若实现后该测试因排序导致取的前 5 个不稳定,改断言为 `assert_eq!(t.len(), MAX_FANOUT_WIDTH)`(已是按 len 断言,稳定)。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p alephcore broadcast::targets -- --nocapture`
Expected: 6 个测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add src/teams/broadcast/targets.rs src/teams/broadcast/mod.rs src/teams/mod.rs
git commit -m "teams(broadcast): pure fan-out target resolver (@mention gating + width cap + guards)"
```

---

## Task 3: `transcript.rs` — 纯函数格式化群历史 + token 截断

**Files:**
- Create: `src/teams/broadcast/transcript.rs`
- Modify: `src/teams/broadcast/mod.rs`(加 `pub mod transcript;`)

把 `Vec<TeamMessage>` 渲染成注入文本,带 `[from]:` 前缀(openteams 风格,让 agent 看见谁说了什么),超 token 预算从**尾部保留最近**。

- [ ] **Step 1: 写失败测试**

Create `src/teams/broadcast/transcript.rs`:

```rust
//! 纯函数:把群消息历史渲染为注入 prompt 的 transcript 文本(带发言人前缀 + token 预算截断)。

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: &str, content: &str) -> (String, String) {
        (from.to_string(), content.to_string())
    }

    #[test]
    fn formats_with_sender_prefix_oldest_first() {
        let msgs = vec![line("user", "大家好"), line("alice", "你好"), line("bob", "我也在")];
        let out = format_transcript(&msgs, 10_000);
        assert_eq!(out, "[user]: 大家好\n[alice]: 你好\n[bob]: 我也在");
    }

    #[test]
    fn empty_history_yields_empty_string() {
        assert_eq!(format_transcript(&[], 10_000), "");
    }

    #[test]
    fn over_budget_keeps_most_recent_from_tail() {
        // 每行约 ~4 token(粗略 len/4),预算极小 → 只保留最后一行
        let msgs = vec![line("a", "aaaaaaaaaaaaaaaa"), line("b", "bbbbbbbbbbbbbbbb"), line("c", "cc")];
        let out = format_transcript(&msgs, 3); // 极小预算
        assert!(out.contains("[c]: cc"), "必须保留最近一条");
        assert!(!out.contains("[a]:"), "最旧的被截断");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore broadcast::transcript -- --nocapture`
Expected: FAIL(`format_transcript` not found)。

- [ ] **Step 3: 实现 `format_transcript`**

在测试模块上方加(签名用 `&[(String, String)]` = (from, content) 对,让纯函数不依赖 `TeamMessage`,更易测;Task 5 调用时从 `TeamMessage` 映射):

```rust
/// 把 (from, content) 历史渲染为 `[from]: content` 多行文本,oldest-first。
/// 超 `token_budget` 时从尾部(最近)保留,粗略按 `chars/4` 估 token。
#[must_use]
pub fn format_transcript(history: &[(String, String)], token_budget: usize) -> String {
    // 从最近往旧累加,直到预算用尽,再反转回 oldest-first。
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (from, content) in history.iter().rev() {
        let line = format!("[{from}]: {content}");
        let cost = line.chars().count() / 4 + 1; // 粗略 token 估计
        if used + cost > token_budget && !kept_rev.is_empty() {
            break;
        }
        used += cost;
        kept_rev.push(line);
    }
    kept_rev.reverse();
    kept_rev.join("\n")
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore broadcast::transcript -- --nocapture`
Expected: 3 个测试 PASS。

- [ ] **Step 5: 在 mod.rs 注册子模块**

`src/teams/broadcast/mod.rs` 加:

```rust
pub mod transcript;
```

- [ ] **Step 6: Commit**

```bash
git add src/teams/broadcast/transcript.rs src/teams/broadcast/mod.rs
git commit -m "teams(broadcast): pure transcript formatter with sender prefix + token budget"
```

---

## Task 4: `member_prompt.rs` — 纯函数组装成员/leader run 输入

**Files:**
- Create: `src/teams/broadcast/member_prompt.rs`
- Modify: `src/teams/broadcast/mod.rs`(加 `pub mod member_prompt;`)

每个被唤醒的 agent 的 run 输入 = 身份 + 名册 + 共享 transcript + 触发说明 + 接话协议 + team_id。leader 多一段身份。spec §5.2/§8。

- [ ] **Step 1: 写失败测试**

Create `src/teams/broadcast/member_prompt.rs`:

```rust
//! 纯函数:组装一个被唤醒 agent 的 run 输入文本(身份/名册/transcript/协议/team_id)。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_prompt_has_identity_team_id_and_transcript() {
        let out = build_member_input(
            "team-xyz", "alice", "researcher",
            "bob (writer), leader (leader)",
            "[user]: @alice 查下 X",
            false,
        );
        assert!(out.contains("alice"), "含自己身份");
        assert!(out.contains("team-xyz"), "含 team_id(调团队工具必需)");
        assert!(out.contains("[user]: @alice 查下 X"), "含群 transcript");
        assert!(out.contains("bob (writer)"), "含名册");
        assert!(!out.contains("你还是这个群的 leader"), "成员无 leader 身份段");
    }

    #[test]
    fn leader_prompt_appends_leader_identity() {
        let out = build_member_input(
            "team-xyz", "leader", "leader",
            "alice (researcher)",
            "[user]: 这事谁跟进",
            true,
        );
        assert!(out.contains("你还是这个群的 leader"), "leader 身份段");
        assert!(out.contains("task_create"), "leader 段提到编排工具");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore broadcast::member_prompt -- --nocapture`
Expected: FAIL(`build_member_input` not found)。

- [ ] **Step 3: 实现 `build_member_input`**

```rust
/// 组装被唤醒 agent 的 run 输入。`is_leader` 时追加 leader 身份段(R7/R9:身份在 prompt,非代码强制)。
#[must_use]
pub fn build_member_input(
    team_id: &str,
    agent_id: &str,
    role: &str,
    roster: &str,
    transcript: &str,
    is_leader: bool,
) -> String {
    let leader_block = if is_leader {
        "\n\n你还是这个群的 leader——除了平等参与讨论,当任务需要严肃编排时,\
         你可以用 `task_create` / `team_delegate` 派活给成员、汇总产出给用户。但这是你的判断,不是义务。"
    } else {
        ""
    };
    format!(
        "你是团队群聊里的成员 `{agent_id}`({role}),team_id: `{team_id}`。\n\
         群成员名册:{roster}。{leader_block}\n\n\
         下面是群聊记录(每行 `[发言人]: 内容`):\n{transcript}\n\n\
         请以你的身份在群里回应。约定:\n\
         - 要不要发言、说什么由你判断;与你无关可以简短跳过。\n\
         - 想让某成员接话,在回复里 `@<agent_id>`(用名册里的 id);@all 叫全员。\n\
         - 调任何团队工具(task_create / team_delegate / team_status 等)时,team_id 必须填 `{team_id}`。\n\
         - 不要 @ 自己,也不要 @ user。"
    )
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore broadcast::member_prompt -- --nocapture`
Expected: 2 个测试 PASS。

- [ ] **Step 5: 注册子模块 + Commit**

`src/teams/broadcast/mod.rs` 加 `pub mod member_prompt;`,然后:

```bash
git add src/teams/broadcast/member_prompt.rs src/teams/broadcast/mod.rs
git commit -m "teams(broadcast): pure member/leader run-input builder (identity + transcript + protocol)"
```

---

## Task 5: `GroupChatBroadcaster` 编排器（run agent + 回流 + chain_depth 守卫）

**Files:**
- Modify: `src/teams/broadcast/mod.rs`(加 `GroupChatBroadcaster` struct + dispatch 逻辑)

这是唯一碰 IO/异步的层。核心:递归 `dispatch(content, sender, chain_depth, user_triggered)` → `resolve_targets` → 并发为每个目标 `run_member` → `execute().await` 取 `final_response` → 存回 transcript → 递归 `dispatch(reply, agent, depth+1, false)`。

- [ ] **Step 1: 先核实两个精确字段(写实现前必做)**

Run:
```bash
grep -rn "RunComplete" src/gateway/event_emitter/ | head
grep -rn "final_response\|struct RunSummary" src/ | grep -i summary | head
grep -rn "pub fn new_in_memory\|impl CollectingEventEmitter\|pub async fn events" src/gateway/event_emitter/
```
Expected: 确认 `StreamEvent::RunComplete { summary, .. }` 与 `RunSummary.final_response: Option<String>` 的**确切字段名**,以及 `CollectingEventEmitter::new()` + `events()`。若字段名不同(如 `response`/`final_text`),在 Step 3 代码里替换为实际名。

- [ ] **Step 2: 写 chain_depth 守卫单测(纯逻辑可测部分)**

在 `src/teams/broadcast/mod.rs` 末尾加:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_depth_guard_blocks_at_max() {
        assert!(over_depth(MAX_CHAIN_DEPTH), "到上限应阻断");
        assert!(over_depth(MAX_CHAIN_DEPTH + 1), "超上限应阻断");
        assert!(!over_depth(MAX_CHAIN_DEPTH - 1), "未到上限放行");
        assert!(!over_depth(0), "初始放行");
    }
}
```

- [ ] **Step 3: 实现编排器**

在 `src/teams/broadcast/mod.rs`(子模块声明下方)加。`extract_final_response` 用 Step 1 核实的字段名:

```rust
use std::sync::Arc;
use crate::error::Result;
use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
use crate::gateway::event_emitter::team_fanout::{team_event_bus, TeamFanoutEmitter};
use crate::gateway::execution_engine::RunRequest;
use crate::routing::session_key::SessionKey;
use crate::teams::messages::{MessageStore, MessageType, NewMessage, Recipient, RecipientRole};
use crate::teams::{TeamStore};

/// 是否已达/超过接话深度上限。
#[must_use]
pub fn over_depth(chain_depth: u32) -> bool {
    chain_depth >= MAX_CHAIN_DEPTH
}

/// 从收集到的事件里取 agent 最终回复文本。
fn extract_final_response(events: &[StreamEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        // 字段名以 Step 1 核实为准
        StreamEvent::RunComplete { summary, .. } => summary.final_response.clone(),
        _ => None,
    })
}

/// 群聊广播编排器。无状态:每次 dispatch 现场拉 team/roster/transcript。
#[derive(Clone)]
pub struct GroupChatBroadcaster {
    ctx: Arc<GatewayContext>,
    team_store: Arc<dyn TeamStore>,
    msg_store: Arc<dyn MessageStore>,
}

impl GroupChatBroadcaster {
    pub fn new(
        ctx: Arc<GatewayContext>,
        team_store: Arc<dyn TeamStore>,
        msg_store: Arc<dyn MessageStore>,
    ) -> Self {
        Self { ctx, team_store, msg_store }
    }

    /// 入口:用户消息触发(没@时 leader 兜底)。假定 user 消息已由调用方存进 msg_store。
    pub async fn dispatch_user(&self, team_id: String, content: String) {
        self.dispatch(team_id, content, RESERVED_USER_HANDLE.to_string(), 0, true).await;
    }

    /// 递归核心。`user_triggered`=false 时没@不兜底(链自然停)。
    async fn dispatch(
        &self,
        team_id: String,
        content: String,
        sender: String,
        chain_depth: u32,
        user_triggered: bool,
    ) {
        if over_depth(chain_depth) {
            self.post_system(&team_id, "讨论已达深度上限,等你接话。").await;
            return;
        }
        let Some(team) = self.team_store.get_team(&team_id).await.ok().flatten() else { return; };
        let members = self.team_store.get_members(&team_id).await.unwrap_or_default();
        let roster_ids: Vec<String> = members.iter().map(|m| m.agent_id.clone()).collect();

        let targets = targets::resolve_targets(
            &content, &sender, &team.leader_id, &roster_ids, user_triggered,
        );
        if targets.is_empty() {
            return; // 链自然停
        }

        // 名册展示串(给 prompt 用),排除当前目标自身在 run_member 内再过滤
        let roster_label = members
            .iter()
            .map(|m| format!("{} ({})", m.agent_id, m.role))
            .collect::<Vec<_>>()
            .join(", ");

        // 并发跑每个目标 agent;各自完成后回流。
        let mut handles = Vec::new();
        for agent_id in targets {
            let this = self.clone();
            let team_id = team_id.clone();
            let leader_id = team.leader_id.clone();
            let roster_label = roster_label.clone();
            handles.push(tokio::spawn(async move {
                this.run_member(team_id, agent_id, leader_id, roster_label, chain_depth).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    }

    /// 跑单个成员 agent,拿回复 → 存 transcript → 解析@递归回流。
    async fn run_member(
        &self,
        team_id: String,
        agent_id: String,
        leader_id: String,
        roster_label: String,
        chain_depth: u32,
    ) {
        let Some(agent) = self.ctx.agent_registry().get(&agent_id).await else { return; };

        // 拉最新 transcript(含刚到的消息)并格式化
        let history = self
            .msg_store
            .list_team_messages(&team_id, 200)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.from_agent, m.content))
            .collect::<Vec<_>>();
        let transcript = transcript::format_transcript(&history, TRANSCRIPT_TOKEN_BUDGET);

        let role = "member"; // 实现时:从 members 找该 agent 的 role;简化用 "member"
        let is_leader = agent_id == leader_id;
        let input = member_prompt::build_member_input(
            &team_id, &agent_id, role, &roster_label, &transcript, is_leader,
        );

        // collector 收集回复;TeamFanoutEmitter 同时广播到 team.<id>.*(Panel 气泡)
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            match team_event_bus() {
                Some(bus) => Arc::new(TeamFanoutEmitter::new(
                    bus, team_id.clone(), agent_id.clone(), Some(collector.clone()),
                )),
                None => collector.clone(),
            };

        let run_id = uuid::Uuid::new_v4().to_string();
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("team_id".to_string(), team_id.clone());
        metadata.insert("chain_depth".to_string(), chain_depth.to_string());
        let req = RunRequest {
            run_id,
            input,
            session_key: SessionKey::task(&agent_id, "team_chat", &team_id),
            timeout_secs: None,
            metadata,
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        };

        if let Err(e) = self.ctx.execution_adapter().execute(req, agent, emitter).await {
            tracing::warn!(team_id=%team_id, agent_id=%agent_id, error=%e, "group-chat member run failed");
            return;
        }

        let Some(reply) = extract_final_response(&collector.events().await) else { return; };
        if reply.trim().is_empty() { return; }

        // 存回复进 transcript(广播气泡已由 emitter 发出;这里是持久化 + 给下一轮注入)
        let _ = self.msg_store.send_message(NewMessage {
            team_id: team_id.clone(),
            from_agent: agent_id.clone(),
            msg_type: MessageType::Message,
            subject: String::new(),
            content: reply.clone(),
            recipients: Vec::new(),
            reply_to: None,
            attachments: Vec::new(),
        }).await;

        // 回流:解析回复里的@,递归(agent 触发→没@不兜底)。深度+1。
        Box::pin(self.dispatch(team_id, reply, agent_id, chain_depth + 1, false)).await;
    }

    async fn post_system(&self, team_id: &str, text: &str) {
        let _ = self.msg_store.send_message(NewMessage {
            team_id: team_id.to_string(),
            from_agent: "system".to_string(),
            msg_type: MessageType::SystemNotification,
            subject: String::new(),
            content: text.to_string(),
            recipients: Vec::new(),
            reply_to: None,
            attachments: Vec::new(),
        }).await;
    }
}
```

> 关键实现注意:
> - `dispatch` 递归 → `run_member` → `dispatch`,Rust async 递归需 `Box::pin`(已用)。
> - `role` 简化为 `"member"`;若要精确角色,在 `dispatch` 把 `members` 传进 `run_member` 查 role。MVP 可接受 "member"。
> - `RunRequest` 字段以 Build-time 的真实定义为准(本计划头部已列);若 `pending_media` 类型不符,照搬 `handle_chat_send` 现有构造。
> - `EventEmitter`/`CollectingEventEmitter`/`StreamEvent` 的精确 import 路径以 Step 1 grep 为准。

- [ ] **Step 4: 跑测试 + 编译**

Run: `cargo test -p alephcore broadcast:: -- --nocapture`
Expected: targets/transcript/member_prompt/over_depth 全部 PASS,且 crate 编译通过(`cargo check -p alephcore`)。

- [ ] **Step 5: Commit**

```bash
git add src/teams/broadcast/mod.rs
git commit -m "teams(broadcast): GroupChatBroadcaster — fan-out run + reply reflux + chain_depth guard"
```

---

## Task 6: 改造 `handle_chat_send` 接入广播编排器

**Files:**
- Modify: `src/gateway/handlers/teams.rs`(`handle_chat_send`,约 line 2852-2960)

把"build leader prompt + spawn 单 leader run"替换为"存 user 消息 + 调 `GroupChatBroadcaster::dispatch_user`"。

- [ ] **Step 1: 核实 handler 能拿到 MessageStore**

Run: `grep -rn "MessageStore\|msg_store\|message_store" src/gateway/context.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs | head`
Expected: 确认 `GatewayContext` 或注册处能取到 `Arc<dyn MessageStore>`。若 `GatewayContext` 没有,在注册 `teams.chat.send` 处(`agent_init/mod.rs:1552` 附近)把已构造的 msg_store 一并 move 进闭包传给 handler。记录实际获取方式。

- [ ] **Step 2: 改写 `handle_chat_send` 主体**

把现有(line ~2882-2959)从 `let members = store.get_members...` 到 `tokio::spawn(... execute ...)` 整段,替换为:

```rust
    // 存用户消息进群 transcript(广播范式:共享 transcript 是唯一事实源)
    let _ = msg_store.send_message(crate::teams::messages::NewMessage {
        team_id: params.team_id.clone(),
        from_agent: crate::teams::broadcast::RESERVED_USER_HANDLE.to_string(),
        msg_type: crate::teams::messages::MessageType::Message,
        subject: String::new(),
        content: params.message.clone(),
        recipients: Vec::new(),
        reply_to: None,
        attachments: Vec::new(),
    }).await;

    // 平等广播:按 @mention fan-out(没@→leader 兜底),agent 可互相接话(chain_depth 守卫)。
    let broadcaster = crate::teams::broadcast::GroupChatBroadcaster::new(
        Arc::clone(&context),
        Arc::clone(&store),
        Arc::clone(&msg_store),
    );
    let run_id = uuid::Uuid::new_v4().to_string();
    let team_id = params.team_id.clone();
    let message = params.message.clone();
    tokio::spawn(async move {
        broadcaster.dispatch_user(team_id, message).await;
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "run_id": run_id }))
```

> 注:
> - `context` 是 `Arc<GatewayContext>`(handler 已有);`store` 是 `Arc<dyn TeamStore>`(已有);`msg_store` 按 Step 1 接入。
> - 删掉不再用的 `leader_prompt::build` 调用、`roster` 拼接、`leader_agent` resolve、`TeamFanoutEmitter` 直建、`RunRequest` 直建那几段(它们移进了 broadcaster)。**保留** team 存在性校验(line ~2870-2880 的 `get_team` → 404)。
> - 本会话修的 `leader_prompt.rs` team_id bug 保留不动(其它路径仍引用 `build`;若 grep 确认 `leader_prompt::build` 已无任何调用者,可在后续清理轮删除,本计划不删)。

- [ ] **Step 3: 编译**

Run: `cargo check -p alephcore`
Expected: 通过。修掉因删除旧代码产生的 unused import(如不再用的 `leader_prompt`)。

- [ ] **Step 4: 跑相关测试**

Run: `cargo test -p alephcore teams:: -- --nocapture`
Expected: 现有 teams handler 测试 + 新 broadcast 测试全绿。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/teams.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "gateway(teams): rewrite chat.send to equal-broadcast group chat (was hub-and-spoke)"
```

---

## Task 7: 部署 + 手动 E2E 验证

**Files:** 无(部署 + 验证)

后端改动需重编 `aleph-server` 二进制并 relaunch daemon(panel 未改,无需 `just wasm`)。

- [ ] **Step 1: Release 构建**

Run: `cargo build --release -p alephcore --bin aleph-server`
Expected: 编译成功。

- [ ] **Step 2: 替换运行中的 binary 并重启(dev daemon)**

Run:
```bash
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```
(若是 .app daemon,按 CLAUDE.md 的 mv/cp/kill relaunch 链。)

- [ ] **Step 3: 手动 E2E(用户参与)**

在 Panel 开一个团队群聊(leader + ≥2 成员):
1. 发 `@alice @bob 各报到一句` → **预期 alice、bob 各自在群里回一句**(不再只有 main),Panel 显示各自 attributed 气泡。
2. 发不带@的消息 → **预期只有 leader 兜底回**。
3. 发 `@alice 跟 @bob 讨论下方案 X` → **预期 alice/bob 互相 @ 接话几轮后自然停**(不超 6 跳)。
4. 重开该群 → **预期历史还在**(transcript 从 DB 加载)。
5. 观察日志无 `Team '...' not found`、无 8×generic subagent 空转。

- [ ] **Step 4: 回归确认 team_id bug 已消除**

Run: `grep -c "not found" ~/.aleph/logs/aleph-server.log.$(date +%Y-%m-%d)` (大致;以实际日志文件名为准)
Expected: 群聊操作期间不再出现 `Team '<name>' not found`。

---

## Self-Review（plan vs spec）

**Spec coverage:**
- spec §2 平等广播 → Task 5/6 ✓
- spec §3 Q1 @门控+leader兜底 → Task 2 `resolve_targets`(user_triggered 分支)✓
- spec §3 Q2 chain_depth+宽度 → Task 2(宽度)+ Task 5(`over_depth`)✓
- spec §3 Q3 落库+transcript注入+截断 → Task 1(list)+ Task 3(format/截断)+ Task 5(注入)✓
- spec §7 三道闸 + 禁自@/禁@user + 到顶系统提示 → Task 2(护栏)+ Task 5(`post_system`)✓
- spec §8 leader 身份 → Task 4(`is_leader` 段)✓
- spec §6 数据流(同一 fan_out 入口、回流 depth+1)→ Task 5(`dispatch` 递归)✓
- spec §9 MVP 不做项(后台摘要/长期记忆/聚合渲染/SessionSnapshot)→ 计划未涉及 ✓(正确排除)

**Placeholder scan:** Task 1 的 `row_to_message`/`hydrate_children`、Task 5 的 `role="member"`、字段名核实——均**显式标注为"实现时照搬现有/grep 确认"的具体动作**,非模糊占位;每个都给了确认命令或参照来源。

**Type consistency:** `resolve_targets(content, sender, leader_id, roster, user_triggered)` 在 Task 2 定义、Task 5 调用一致;`format_transcript(&[(String,String)], usize)` Task 3 定义、Task 5 用 `.map(|m|(m.from_agent,m.content))` 适配一致;`build_member_input(team_id, agent_id, role, roster, transcript, is_leader)` Task 4 定义、Task 5 调用一致;`over_depth`/`MAX_CHAIN_DEPTH`/`MAX_FANOUT_WIDTH`/`RESERVED_USER_HANDLE` 全局一致。

**已知实现期风险(非占位,需执行者留意):** Task 5 的 `StreamEvent::RunComplete{summary}.final_response` 字段名、`RunRequest` 字段集、`MessageStore` 获取路径——三处均有 Step 1 grep 兜底。
