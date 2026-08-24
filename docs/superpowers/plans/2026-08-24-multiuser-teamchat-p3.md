# Multi-User × Team-Chat Humanization × P3 Implementation Plan / 多用户 × 团队群聊真人化 × P3 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 真实用户（operator + members）与 agent 团队在同一群聊线程里署名协作（提及闸+旁听），审批卡路由到发言人；项目页五个 tab 全活（Kanban/Workspace/Memory）+ `projects.changed` 推送；模型经 `project_manage` 工具管理项目；房间 agent 知道谁在说话。

**Architecture:** 三条主线互相独立、共享谓词层：(A) `team_messages` 长出 `author_user_id` 列，`teams.chat.send` 用 `visibility::ambient_actor()` 盖戳，激活决策是纯函数 `multi_human × has_activation_mention`，旁听 = 落库+推事件+不铸 run；审批发言人路由靠**已存在**的 `originator_user_id` 管道（`ExecApprovalRecord`/`run_loop`/manager resolve 闸都已在，缺的只是 `member_run_metadata` 一行盖戳）。(B) P3 三个 tab 全部是**连线不是新建**：Kanban 读 room-scoped 团队（goals/loops 的 ambient scope 盖戳**已存在**，只欠验证+读侧过滤）、Workspace 是两个新的名册闸只读 RPC、Memory 复用既有分区 RPC；`projects.changed` 镜像 `TeamChanged` 帧模式。(C) 谓词下沉 `src/projects/authz.rs` 让 RPC 与新工具共用同一份推导。

**Tech Stack:** Rust（rusqlite / tokio task-local / serde）+ Leptos WASM Panel。无新依赖。

**Spec:** `docs/superpowers/specs/2026-08-24-multiuser-teamchat-p3-design.md`（决策 D1–D4、边界语义 §9 七条、刻意不做 §11 八项——执行者先读 spec 再读本计划）。

---

## Global Constraints（每个任务隐含包含本节）

1. **单真人零变化**：单真人团队线程行为逐字节等于现状（激活/响应/transcript 格式）。回归测试以字节断言。
2. **Fail-closed**：授权决策处 store `Err` / 解析失败 ⇒ 拒绝。发言人角色缺失按 member 处理，**不得**依赖 `role_is_operator(None)==true` 的 fail-open 缺省。`.ok().flatten()` 在闸上是禁止形状。
3. **无存在性预言机**：「不是成员」与「不存在」逐字节相同响应（复用 `gate_project` / `gate_team` 既有形状）。
4. **不受限调用者优先**：谓词第一臂永远 `None => true`（cron/后台/进程内测试），新增分支排在它之后。
5. **写者与读者同批过同一道闸**：`workspace_path` 新写入者（`project_manage`）必须过 `caller_may_choose_directory` 同款闸；display_name 是读时投影不落库。
6. **单一源纪律**：@ 词法只有 `targets.rs` 一份；speaker label 只有一个推导函数；authz 谓词 RPC/工具共用 `src/projects/authz.rs`；**不新造第二份**。
7. **测试断言效果不断言调用**：「bob 发言 → 不激活/激活了谁 → transcript 里是什么」，不断言"函数被调了"。
8. **R7/R10**：激活决策是确定性脚手架（不看语义内容，只看 @ 词法与作者计数）；`src/harness/` 一行不动——认为需要动就**停下上报**。
9. **验证门（Windows）**：每任务 `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib <module>`（前台、不接管道、timeout 600000）。Panel 任务加 `cargo test -p aleph-panel --lib`（**不是** check）。最后任务跑全量五条（见 Task 13）。
10. **rustfmt 只按叶子文件**：`rustfmt --edition 2021 --check <改过的文件>`；永远不要 `cargo fmt -p alephcore`。
11. **提交**：`<scope>: <description>` 英文，每任务至少一个提交，worktree 分支 `worktree-multiuser-teamchat-p3`，**不 push、不碰 main**。
12. **CRLF**：本仓 CRLF/LF 混排；源码级守卫的分隔符先 `.replace('\r', "")` 再 split；批量替换前确认行尾。
13. **worktree 路径**：所有编辑必须落在 `D:\Workspace\Aleph\.claude\worktrees\multiuser-teamchat-p3\` 之下（round-2 曾把 17 处编辑写进 main 工作树）。
14. **NewMessage 构造点**：给 `NewMessage` 加字段前先 `grep -rn "NewMessage {" src/` 数全构造点，逐个补——漏一个是编译错（好事），但别用 `#[serde(default)]` 糊（wire 上要显式）。

---

## File Structure (new / modified)

| File | Responsibility |
|---|---|
| Modify `src/teams/messages/types.rs` + `store.rs` | `author_user_id` 列 + 迁移 + `distinct_human_authors` 查询 |
| Modify `src/teams/broadcast/targets.rs` | `has_activation_mention` 纯函数（@ 词法单一源内） |
| Modify `src/gateway/handlers/teams/canvas.rs` | 署名盖戳、激活谓词、旁听分支、`.message` 事件发布、响应形状 |
| Modify `src/teams/broadcast/mod.rs` | `run_member` 的 speaker-label 历史映射、`member_run_metadata` 盖 `originator_user_id` |
| Create `src/teams/broadcast/speaker.rs` | `speaker_label` 单一推导 + 用户名批量解析 |
| Modify `src/teams/broadcast/member_prompt.rs` | 真人参与者名册 + 措辞更新 |
| Modify `src/gateway/handlers/teams/tasks.rs` | `map_history` 行携带 author 字段 |
| Modify `interfaces/webchat/src/platform/wide/views/chat/team_events.rs` + `api/team_chat.rs` + composer | 署名气泡、`run_id:null` 旁听态、自回声去重 |
| Modify `src/gateway/events/frame.rs` + `event_visibility.rs` | `ProjectsChanged` 帧 + roster 分类臂 |
| Modify `src/gateway/handlers/projects.rs` | 7 处 mutation 发帧 + `workspace.list/read` 两个新 RPC |
| Create `src/projects/authz.rs` | 显式 actor 谓词（RPC/工具共用推导） |
| Create `src/builtin_tools/project_manage.rs` | R8 工具面 |
| Modify `src/projects/store.rs` | `project_of_session_key` 查询 |
| Modify `src/gateway/execution_engine/execute.rs`（`ensure_session_under_request_scope`） | 房间键强制 project scope |
| Create `src/thinker/layers/room_roster.rs` | 房间名册 Dynamic 层 |
| Modify thinker 历史渲染点（Task 11 定位） | 房间用户消息 `[alice]:` 投影 |
| Modify `interfaces/webchat/src/components/project_page.rs` + 新建 `project_page/{kanban,workspace,memory}.rs` | 三个 tab 实体化，删 `PlaceholderTab` |
| Modify `interfaces/webchat/src/api/projects.rs` + `sidebar/projects.rs` | workspace API + `projects.changed` 订阅 |
| Modify `docs/reference/{FEATURE_LOCATOR,SECURITY}.md`、`src/gateway/CLAUDE.md`、locales | 文档回填 + i18n |

---

### Task 1: `team_messages.author_user_id` 列 + 线程作者查询

**Files:**
- Modify: `src/teams/messages/types.rs`（`TeamMessage` :114、`NewMessage` :133 附近）
- Modify: `src/teams/messages/store.rs`（schema :170-200、`send_impl`、行物化、新查询）
- Modify: 全部 `NewMessage {` 构造点（先 grep 数全，已知：`canvas.rs` 用户消息、`broadcast/mod.rs` 成员回复 + `post_system`、store 测试）

**Interfaces (produced):**
```rust
// types.rs — 两个 struct 各加一个字段
pub struct TeamMessage { /* 既有字段不动 */ pub author_user_id: Option<String>, }
pub struct NewMessage  { /* 既有字段不动 */ pub author_user_id: Option<String>, }

// store.rs — MessageStore trait 新方法
/// Distinct human authors ever seen in this team's transcript
/// (rows where author_user_id IS NOT NULL). Agent/system rows excluded.
async fn distinct_human_authors(&self, team_id: &str) -> crate::error::Result<Vec<String>>;
```

- [ ] **Step 1: 写失败测试**（`store.rs` 测试模块）

```rust
#[tokio::test]
async fn author_round_trips_and_distinct_counts_only_humans() {
    let store = test_store().await; // 既有测试夹具
    let mut msg = new_message("t1", "user", "hi");     // 既有夹具函数，补 author 字段
    msg.author_user_id = Some("u-alice".into());
    store.send_message_with_ttl(msg, chrono::Duration::days(1)).await.unwrap();
    let mut agent = new_message("t1", "coder", "ok");
    agent.author_user_id = None;
    store.send_message_with_ttl(agent, chrono::Duration::days(1)).await.unwrap();
    let mut bob = new_message("t1", "user", "me too");
    bob.author_user_id = Some("u-bob".into());
    store.send_message_with_ttl(bob, chrono::Duration::days(1)).await.unwrap();

    let history = store.list_team_messages("t1", 10).await.unwrap();
    assert_eq!(history.iter().filter(|m| m.author_user_id.is_some()).count(), 2);
    let mut authors = store.distinct_human_authors("t1").await.unwrap();
    authors.sort();
    assert_eq!(authors, vec!["u-alice".to_string(), "u-bob".to_string()]);
    // 隔离：另一个 team 看不到
    assert!(store.distinct_human_authors("t2").await.unwrap().is_empty());
}

#[tokio::test]
async fn schema_migration_is_idempotent_on_existing_db() {
    // 打开同一路径两次 —— 第二次 open 不得因列已存在而失败
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.db");
    { let _s = SqliteMessageStore::open(&path).await.unwrap(); }
    let _s2 = SqliteMessageStore::open(&path).await.unwrap();
}
```

- [ ] **Step 2: 跑测试确认编译失败**（`NewMessage` 无 author 字段）
- [ ] **Step 3: 实现**——types.rs 加两个字段；store.rs：schema `CREATE TABLE` 加列 + 幂等 `ALTER TABLE team_messages ADD COLUMN author_user_id TEXT`（照本文件既有迁移模式：执行、忽略 "duplicate column" 错误）；`send_impl` INSERT 带列；行物化 `SELECT` 与 `RawMessage` 带列；`distinct_human_authors` = `SELECT DISTINCT author_user_id FROM team_messages WHERE team_id=?1 AND author_user_id IS NOT NULL`。grep 全部 `NewMessage {` 构造点补 `author_user_id: None`（本任务只有测试与既有生产点补 `None`；盖真值在 Task 2）。
- [ ] **Step 4: 跑 `cargo test -p alephcore --lib teams::messages` 全绿**
- [ ] **Step 5: rustfmt 改过的叶子文件；提交** `teams: add author_user_id to team_messages with distinct-authors query`

---

### Task 2: `teams.chat.send` 署名 + 激活谓词 + 旁听分支 + `.message` 事件

**Files:**
- Modify: `src/teams/broadcast/targets.rs`（新纯函数）
- Modify: `src/gateway/handlers/teams/canvas.rs`（`handle_chat_send` :33-175）
- Modify: `handle_chat_send` 注册点（grep `handle_chat_send(` 的调用处，新增 `Arc<SecurityStore>` 参数注入——与既有 store 注入同形）

**Interfaces:**
- Consumes: Task 1 的 `author_user_id` 字段与 `distinct_human_authors`。
- Produces:
```rust
// targets.rs
/// True iff content @-names a roster member or @all/@everyone.
/// This is the ONLY activation-mention lexer (single source with resolve_targets).
pub fn has_activation_mention(content: &str, roster: &[String]) -> bool;

// canvas.rs 响应形状（旁听时）
// { "run_id": null, "observed": true, "message_id": "<TeamMessage.id>" }
// 激活时: { "run_id": "<uuid>", "observed": false, "message_id": "<TeamMessage.id>" }
```

- [ ] **Step 1: targets.rs 失败测试**

```rust
#[test]
fn activation_mention_lexicon_matches_resolve_targets() {
    let roster = vec!["leader".to_string(), "coder".to_string()];
    assert!(has_activation_mention("@coder fix it", &roster));
    assert!(has_activation_mention("@all 报到", &roster));
    assert!(has_activation_mention("@everyone hi", &roster));
    assert!(!has_activation_mention("no mention here", &roster));
    assert!(!has_activation_mention("@stranger hi", &roster));   // 不在名册
    assert!(!has_activation_mention("@user hi", &roster));       // 保留 handle
    assert!(!has_activation_mention("a@b.com", &roster));        // email 不是提及（复用 extract_mentions 既有词法）
}
```

- [ ] **Step 2: 确认失败；实现 `has_activation_mention`**——内部用既有 `extract_mentions` + `MENTION_ALL`，语义 = `resolve_targets` 的 mention 臂（不复制词法）。跑绿。
- [ ] **Step 3: canvas.rs 集成测试（handler 级）**——本文件/`tests.rs` 既有 handler 测试模式：

```rust
#[tokio::test]
async fn multi_human_without_mention_observes_instead_of_dispatching() {
    // 夹具：team t1（leader=leader），transcript 里已有 u-alice 的一条人类消息
    // caller 身份 = u-bob（用 caller_identity 测试夹具设 CALLER_USER）
    // 调 handle_chat_send { team_id, message: "我觉得可以" }
    // 断言:
    //   1) 响应 success，run_id == null，observed == true，message_id 非空
    //   2) store 里新行 author_user_id == Some("u-bob")
    //   3) 没有注册 fanout（GroupChatBroadcaster::fanout_registry 无新条目 / run_id 为空即证）
}

#[tokio::test]
async fn single_human_path_is_byte_identical_to_before() {
    // transcript 只有 u-alice 说过话，caller 仍是 u-alice，无 @
    // 断言: run_id != null, observed == false（现状激活 leader）
}

#[tokio::test]
async fn multi_human_with_mention_still_dispatches() {
    // 两作者在场，message = "@coder 实现它" → run_id != null
}
```

- [ ] **Step 4: 实现 canvas.rs 改动**（顺序照现有代码流）：
  1. 落 transcript 前：`let author = crate::gateway::visibility::ambient_actor();`，`NewMessage.author_user_id = author.clone()`；保存 `send_message_with_ttl` 返回的 `TeamMessage`（拿 `id`；今天返回值被丢弃——改为绑定，持久化失败仍按既有 warn 分支处理并继续，`message_id` 置 null）。
  2. 激活决策（读名册用既有 `store.get_members`，读作者用 `distinct_human_authors`；两处 `Err` 按 Global-2 = 保守视作**激活**现状路径，不因新机件把消息吞成旁听）：
     ```rust
     let mut humans: std::collections::HashSet<String> =
         msg_store.distinct_human_authors(&params.team_id).await.unwrap_or_default().into_iter().collect();
     if let Some(a) = &author { humans.insert(a.clone()); }
     let roster_ids: Vec<String> = store.get_members(&params.team_id).await.unwrap_or_default()
         .into_iter().map(|m| m.agent_id).collect();
     let observe = humans.len() > 1
         && !crate::teams::broadcast::targets::has_activation_mention(&params.message, &roster_ids);
     ```
  3. **两种模式都**发 `.message` 事件（这是"其他真人实时看到"的补线）：
     ```rust
     crate::gateway::event_emitter::team_fanout::publish_team_event(
         &params.team_id, "message",
         serde_json::json!({
             "text": params.message, "message_id": persisted_id,
             "author_user_id": author, "author_display_name": display_name,
         }));
     ```
     `display_name`：`security_store.get_user(author)` 取 display_name（新注入的 `Arc<SecurityStore>` 参数；查失败回退 user id；**不落库**）。注意：事件里**没有 `agent_id` 字段**——Panel 既有 Message 臂用 `agent_id` 区分（Task 5 处理）。
  4. `observe == true` ⇒ 跳过 auto-name/broadcaster/fanout/spawn，直接返回 `{run_id: null, observed: true, message_id}`；否则走现状路径，响应加 `observed: false, message_id`。
- [ ] **Step 5: 跑 `cargo test -p alephcore --lib teams` 全绿（含 Task 1 与既有回归）；提交** `teams: author-stamped chat.send with mention-gate observe mode`

---

### Task 3: transcript `[alice]:` 渲染 + member_prompt 真人名册

**Files:**
- Create: `src/teams/broadcast/speaker.rs`
- Modify: `src/teams/broadcast/mod.rs`（`run_member` :693-699 历史映射；`GroupChatBroadcaster::new` 增 `Option<Arc<SecurityStore>>`，构造点只有 `canvas.rs` 一处）
- Modify: `src/teams/broadcast/member_prompt.rs`

**Interfaces:**
- Produces:
```rust
// speaker.rs
/// The ONE derivation of "what label does this row speak under".
/// Human row (author present): display-name from `labels`, else the raw user id.
/// Agent/system row: from_agent unchanged.
pub fn speaker_label(msg: &TeamMessage, labels: &HashMap<String, String>) -> String;
/// Batch-resolve display names for the authors present in `history`.
/// Store absent/err ⇒ empty map (labels degrade to user ids, never blocks a run).
pub async fn resolve_labels(store: Option<&Arc<SecurityStore>>, history: &[TeamMessage]) -> HashMap<String, String>;
```

- [ ] **Step 1: speaker.rs 失败测试**

```rust
#[test]
fn human_rows_render_display_name_agent_rows_keep_agent_id() {
    let labels = HashMap::from([("u-alice".to_string(), "Alice".to_string())]);
    assert_eq!(speaker_label(&human_msg("u-alice"), &labels), "Alice");
    assert_eq!(speaker_label(&human_msg("u-bob"), &labels), "u-bob"); // 无名字回退 id
    assert_eq!(speaker_label(&agent_msg("coder"), &labels), "coder");
}
```

- [ ] **Step 2: 确认失败 → 实现 → 绿**
- [ ] **Step 3: `run_member` 历史映射改为**：
  ```rust
  let raw = self.msg_store.list_team_messages(&team_id, 200).await.unwrap_or_default();
  let labels = speaker::resolve_labels(self.security_store.as_ref(), &raw).await;
  let history: Vec<(String, String)> = raw.into_iter()
      .map(|m| (speaker::speaker_label(&m, &labels), m.content)).collect();
  ```
  `format_transcript` 不动（它已渲染 `[label]: content`）。单真人时 label 序列 == 旧序列（`author` 有值但 labels 查到名字——⚠️ 这会把 `[user]:` 变成 `[Alice]:`，**这是有意的行为增强而非破坏**：spec A5；"单真人零变化"约束限定在**激活语义**上，transcript 署名是本轮的目标产出，在回归测试里显式断言新形状）。
- [ ] **Step 4: member_prompt.rs**——`build_member_input` 增 `human_roster: &str` 参数（`run_member` 从 labels 值 + 线程作者集拼，如 `Alice(human), u-bob(human)`，空则空串）；模板名册行变为 `群成员名册:{roster}。真人参与者:{human_roster}。`（空时省略后半句）；`- 不要 @ 自己,也不要 @ user。` 改为 `- 不要 @ 自己,也不要 @ user 或任何真人参与者。`。更新本文件既有测试断言。
- [ ] **Step 5: 跑 `cargo test -p alephcore --lib teams::broadcast` 全绿；提交** `teams: speaker-labelled transcript and human roster in member prompts`

---

### Task 4: 审批发言人路由（originator 盖戳 + operator_only 回归）

**Files:**
- Modify: `src/teams/broadcast/mod.rs`（`member_run_metadata` :136-200）
- Test: 同文件测试模块 + `src/exec/manager.rs` 既有 originator 闸测试扩展

**Interfaces:**
- Consumes: `run_loop/mod.rs:366` 已读 `request.metadata["originator_user_id"]`；`ExecApprovalRecord.originator_user_id`（`src/exec/manager.rs:72`）与 resolve 闸（`manager.rs:735`）已在；`OperatorApprovalRequester` owner-scoped/operator_only 双实例已在。

- [ ] **Step 1: 失败测试**

```rust
#[test]
fn member_run_metadata_carries_originator_for_approval_gate() {
    // 在 with_room_author(Some("u-bob")) + caller scope 内调 member_run_metadata
    // 断言 metadata["originator_user_id"] == "u-bob"
    // 裸调用（无 ambient）: 断言键不存在（背景 dispatcher 不发明作者）
}
```

- [ ] **Step 2: 确认失败；实现**——`member_run_metadata` 在既有 `AUTHOR_USER_KEY` 块之后：
  ```rust
  // Approval-originator gate (run_loop reads this exact key at :366): the
  // human whose message woke this member is the one who may answer its
  // approval cards. Same value as AUTHOR_USER_KEY on this path, but the
  // consumer keys are different wires — stamp both, derive from one place.
  if let Some(author) = crate::gateway::visibility::ambient_actor() {
      metadata.insert("originator_user_id".to_string(), author);
  }
  ```
- [ ] **Step 3: 回归三条**（放对应模块既有测试旁）：
  - manager resolve 闸：originator=Some("u-bob") 的 record，"u-carol" resolve 被拒、"u-bob" 通过、admin 通过（若 `manager.rs:735` 既有测试已覆盖前两条则只补 team 路径来源注释与缺失臂）。
  - `OperatorApprovalRequester::for_config_tier` 的卡：`frame_session_key` 为空 ⇒ OperatorOnly（既有测试在 :369-466，确认未被本轮破坏即可，不重写）。
  - `member_run_metadata` 单真人 operator 路径：originator == operator id，行为无变化（卡本来就到他）。
- [ ] **Step 4: 跑 `cargo test -p alephcore --lib teams::broadcast exec::manager` 全绿；提交** `teams: route member-run approval cards to the speaking user`

---

### Task 5: Panel 团队群聊署名渲染 + 旁听态

**Files:**
- Modify: `interfaces/webchat/src/api/team_chat.rs`（send 响应 DTO、history 行 DTO）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/team_events.rs`（Message 臂 :48-53、`push_bubble` :162-184）
- Modify: `src/gateway/handlers/teams/tasks.rs`（`map_history` 行加 `author_user_id`/`author_display_name`——display 解析同 Task 2 的 SecurityStore 注入，`handle_chat_history` 已收 store 参数则复用注册点注入）
- Modify: composer 团队分支（`platform/wide/views/chat/composer/mod.rs` :243/:493、`platform/phone/chat/composer.rs` :408/:438）

**Interfaces:**
- Consumes: Task 2 的事件 payload `{text, message_id, author_user_id, author_display_name}` 与响应 `{run_id: Option, observed, message_id}`。
- Produces: Panel `ChatMessage.author_user_id`（已存在于 struct）真正被团队线程填充。

- [ ] **Step 1: Rust 侧 `map_history` 失败测试**（tasks.rs 测试模块）：人类行映射出 author 两字段、agent 行为 None。实现 → 绿。
- [ ] **Step 2: Panel DTO**——`team_chat.rs` send 响应结构体：`run_id: Option<String>` + `#[serde(default)] observed: bool` + `#[serde(default)] message_id: Option<String>`；history 行加 `#[serde(default)]` 双字段（老 server 兼容）。
- [ ] **Step 3: team_events.rs Message 臂**——payload 无 `agent_id` 但有 `author_user_id` ⇒ 人类消息：`push_bubble(chat, "user", text, None)` 且 `author_user_id: Some(..)`（`push_bubble` 增 author 参数或新兄弟函数，`ChatMessage.author_user_id` 字段已在）；**自回声去重**：composer 发送成功后把响应 `message_id` 记入 `ChatState` 新增的 `own_team_msg_ids: RwSignal<HashSet<String>>`（有界：保留最近 32 个），Message 臂命中即跳过（判据是 message id，不是"作者是我"——同人双标签页要收到彼此）。
- [ ] **Step 4: composer 旁听态**——`run_id == None && observed` ⇒ 不进"运行中"状态（不显示 Stop、不注册 run 路由），消息气泡照常上屏。history 渲染路径给人类行带 display_name 标签（复用房间 peer 消息的署名渲染样式，锚 `views/chat/events.rs:833` `push_peer_user_message` 的样式类）。
- [ ] **Step 5: `cargo test -p aleph-panel --lib` + `cargo test -p alephcore --lib gateway::handlers::teams` 全绿；提交** `panel: attributed team-chat bubbles and observe-mode send`

---

### Task 6: `projects.changed` 帧 + roster 分类 + Panel 订阅

**Files:**
- Modify: `src/gateway/events/frame.rs`（新变体，镜像 `TeamChanged` 形状）
- Modify: `src/gateway/event_visibility.rs`（分类臂，镜像 :2609 `TeamChanged` 臂 + 团队 owner/scope 解析模式 :983）
- Modify: `src/gateway/handlers/projects.rs`（7 个 mutation handler 提交后发帧：create/add/create_blank/rename/archive/bind_workspace/remove/member_add/member_remove——以实际 mutation handler 为准 grep `store.` 写调用）
- Modify: `interfaces/webchat/src/components/sidebar/projects.rs`（订阅刷新，删 :33-35 的"无推送 topic"pin 注释）+ `project_page.rs`（名册/设置刷新）

**Interfaces:**
```rust
// frame.rs
ProjectsChanged {
    project_id: String,
    change: ChangeKind,                 // 复用既有 ChangeKind
    /// member_remove 时 = 被移除者：roster 谓词已不含他，但他需要收到
    /// 这一帧来刷新自己的列表。其余 verb 为 None。
    affected_user: Option<String>,
},
```

- [ ] **Step 1: 让 `every_frame_variant_is_classified` pin 先红**——加帧变体、跑 `cargo test -p alephcore --lib event_visibility`，确认 pin 按名字红（这就是失败测试）。
- [ ] **Step 2: 分类臂**——`session_identity_of` 给 `ProjectsChanged` 一个 project-roster 归类：可见 iff `projects::roster::is_member(project_id, actor) || affected_user == actor || actor 是 operator/admin`（admin 臂照 `TeamChanged` 既有处理）。写投递测试：成员可见 / 非成员不可见 / 被移除者对 member_remove 帧可见、对其他帧不可见。
- [ ] **Step 3: 生产者**——每个 mutation handler 在 store 调用成功后 `event_bus.publish_frame(&GatewayEventFrame::ProjectsChanged{..})`（handler 已持有 bus 或经注册点注入，照 `canvas.rs` TeamChanged 发布形状）。member_remove 填 `affected_user`。
- [ ] **Step 4: Panel**——`sidebar/projects.rs` 订阅 `projects.*`（照 teams `team.changed` 订阅模式）→ `refresh()`；`project_page.rs` 同帧刷新 project + 名册；删 pin 注释与其描述。
- [ ] **Step 5: `cargo test -p alephcore --lib event_visibility gateway::handlers::projects` + `cargo test -p aleph-panel --lib` 全绿；提交** `gateway,panel: projects.changed push topic with roster-gated delivery`

---

### Task 7: Kanban 数据面（scope 过滤 + goals/loops 验证钉住）

**Files:**
- Test: `src/goal/`、`src/looping/` 测试模块（盖戳验证）
- Modify: teams 列表 wire（`teams.list` 行含 `scope_id`——grep 响应构造点确认，缺则加）
- Modify: goals/loops 列表 RPC（grep `"goal.list"` / `"loop.list"` 注册名）增可选 `scope_id` 过滤参数，谓词经 `visibility::project_visible_to`

**Interfaces:**
- Produces: `teams.list` 行含 `scope_id: Option<String>`；goal/loop list RPC 接受 `{scope_id?: String}`，按 scope 过滤且非成员对 project scope 拿空集。

- [ ] **Step 1: 盖戳验证测试（钉住既有行为）**

```rust
#[tokio::test]
async fn a_goal_created_inside_a_room_run_lands_in_project_scope() {
    // scope::with_scope(project attribution) 内走 goal 创建路径（builtin_tools/goal.rs 的
    // 创建臂调的 manager 函数），断言落库行 scope_id == Some("project:p-x")
    // 房间外：scope_id == personal（现状）
}
// loop_manage.rs:358 同形一条
```

- [ ] **Step 2: 若红（盖戳断线）修 `with_owner_scope` 调用点；若绿则测试即钉子。**
- [ ] **Step 3: 读侧过滤**——goal/loop list handler 加 `scope_id` 可选参数：`Some("project:p-x")` ⇒ 先 `project_visible_to(p, actor)` 闸（非成员 ⇒ 空集，**不是**错误——无预言机），再按列过滤；`None` ⇒ 现状。`teams.list` 行补 `scope_id` 字段。各写一条成员/非成员测试。
- [ ] **Step 4: `cargo test -p alephcore --lib goal looping gateway::handlers::teams` 全绿；提交** `gateway: project-scope filters for teams/goals/loops list surfaces`

---

### Task 8: Panel Kanban tab

**Files:**
- Create: `interfaces/webchat/src/components/project_page/kanban.rs`
- Modify: `project_page.rs`（`RoomSubTab::Kanban` 臂换成 `<KanbanTab project=p.clone() />`）
- Modify: `interfaces/webchat/src/api/projects.rs`（goal/loop scope 调用包装）+ locales 两文件（新 key，删不掉的 `coming_soon` 留给 Task 9 一起清）

- [ ] **Step 1: KanbanTab 组件**——三段布局：
  1. **团队看板区**：`TeamsApi::list()` 客户端过滤 `scope_id == format!("project:{}", project.id)`，每个团队渲染既有看板组件（锚 §6.6 组件，`views/teams/` 下 grep kanban board 组件名后内嵌复用；空态文案 `project_room.kanban_empty`）+ 每团队一个"打开群聊"按钮 → 既有 `on_open_group` 流（`chat_sidebar.rs` 的进群入口函数，公开或提取后调用）。
  2. **Goals/Loops 进度条**：新 API 包装调 goal/loop list 带 `scope_id`，渲染名称+状态 chip 列表（只读）。
  3. 订阅 `team.<id>.task.*`（既有 `team_events` 通道）+ `projects.changed` 刷新。
- [ ] **Step 2: `cargo test -p aleph-panel --lib` 绿（组件级纯逻辑测试：scope 过滤函数一条）**
- [ ] **Step 3: 提交** `panel: project kanban tab over room-scoped teams and goals`

---

### Task 9: Workspace tab（服务端两 RPC + Panel 只读浏览）+ Memory tab

**Files:**
- Modify: `src/gateway/handlers/projects.rs`（`handle_workspace_list` / `handle_workspace_read` + 注册两处 + params）
- Modify: `src/gateway/method_visibility.rs`（两条 `KeyChecked`）、`method_census.rs`（`Class::Open`）、`src/gateway/lane.rs::override_for`（只读名单）
- Create: `interfaces/webchat/src/components/project_page/{workspace,memory}.rs`
- Modify: `project_page.rs`（两臂换实体组件，**删 `PlaceholderTab` 与 locales 的 `project_room.coming_soon`**）、`api/projects.rs`

**Interfaces:**
```rust
// projects.workspace.list  params: { project_id, rel_path?: String }
//   → { entries: [{name, is_dir, size}], root_bound: bool }   // 未绑定 ⇒ root_bound:false, entries:[]
// projects.workspace.read  params: { project_id, rel_path }
//   → { content: String, truncated: bool }                    // 上限 64 KiB，二进制拒绝
```

- [ ] **Step 1: 服务端失败测试**（handlers/projects.rs 测试模块，照既有 handler 测试夹具）：

```rust
// 1) roster 成员 list 绑定目录 → 条目返回；非成员 → 与"项目不存在"逐字节相同 not_found
// 2) rel_path = "../outside" / 软链逃逸 → PERMISSION_DENIED（canonicalize 后 starts_with 失败）
// 3) deny_read_globs 命中的文件：不出现在 list、read 返回 not_found 形状（无 oracle）
// 4) 未绑定 workspace → root_bound:false 空集（不是错误）
// 5) read 超 64 KiB → truncated:true；二进制（首 8KiB 含 NUL）→ 拒绝
```

- [ ] **Step 2: 实现**——闸序：`gate_project` → `workspace_path` 存在且 canonicalize → join(rel_path).canonicalize 后 `starts_with(root)`（两边同函数归一化；显示用 `utils::paths::display_string`）→ deny 检查复用 `deny_globs::glob_to_anchored_regex` 的展开（grep `deny_read_globs` 的现有读者取同一份展开函数）→ 读/列。**不建目录**（纯查找）。
- [ ] **Step 3: 登记三处**——method_visibility（pin 测试会点名）、method_census、lane override_for。跑对应 census 测试绿。
- [ ] **Step 4: Panel WorkspaceTab**——面包屑 + 条目列表 + 文本预览抽屉（只读；样式照 `DirectoryBrowser` 但走新 API，不复用其"选目录"语义）。MemoryTab——锚 §6.7 Panel 记忆组件（`grep -rn "listFacts" interfaces/webchat/src/api/` 取 API 名），锁定分区 `{agent}__p-<id>`（agent id 取房间会话的 agent——`ProjectInfo`/room session key 里已有），渲染 curated 事实列表 + notes 列表（只读）。**删 `PlaceholderTab` 组件 + `project_room.coming_soon` 两 locale 条目**，新增 tab 文案 key（en/zh 成对）。
- [ ] **Step 5: `cargo test -p alephcore --lib gateway` + `cargo test -p aleph-panel --lib` 全绿；提交** `gateway,panel: project workspace read-only browse and memory tab; drop placeholders`

---

### Task 10: `src/projects/authz.rs` 下沉 + `project_manage` 工具

**Files:**
- Create: `src/projects/authz.rs`；Modify: `src/gateway/handlers/projects.rs`（gates 改调 authz）
- Create: `src/builtin_tools/project_manage.rs`
- Modify: 工具注册面（`BUILTIN_TOOL_DEFINITIONS` 所在 `builder/agent_init/` 目录条目指常量、`create_tool_boxed`、`core_tools::reg`、`ToolRegistry::execute_tool` dispatch 臂、`groups.rs` 分类——`builtin_registry/dispatchable.rs` 守卫会按名字点出漏项）
- Modify: 描述字节棘轮测试（按实测抬，账本里答 R10 三问）

**Interfaces:**
```rust
// src/projects/authz.rs — 显式 actor（None = 不受限进程内调用者）
pub enum ProjectAccess { Ok(Project), NotFound, Forbidden }
pub fn project_visible_for(store: &ProjectStore, id: &str, actor: Option<&str>) -> ProjectAccess;
pub fn require_owner_for(project: &Project, actor: Option<&str>, actor_is_admin: bool) -> bool;
```

- [ ] **Step 1: authz 失败测试**（成员可见/非成员 NotFound 与不存在同值/owner 与 admin 过 require_owner/None actor 全通）→ 实现 → handlers/projects.rs 的 `gate_project`/`require_owner` 改为薄包装调 authz（响应形状字节不变——既有 handler 测试是回归网）→ 绿。
- [ ] **Step 2: 工具失败测试**（`builtin_tools/project_manage.rs` 测试模块）：

```rust
// 1) action=list: ambient_actor=u-bob 只见其名册项目
// 2) action=member_add: 非 owner actor → 错误文案与 RPC PERMISSION_DENIED 同因
// 3) action=bind_workspace: ambient caller_role 非 operator → 拒绝（fail-closed:
//    role 取 scope::CarriedAttribution 携带的 caller_role task-local，None ⇒ 拒绝，
//    ——工具面孪生，与 RPC 的 caller_may_choose_directory 同一判据不同载体）
// 4) action=get: 非成员 → "project not found"（与 list 缺席一致，无 oracle）
```

- [ ] **Step 3: 实现工具**——actions `list/get/create/rename/archive/member_add/member_remove/member_list/bind_workspace`，全部经 `ProjectStore` 句柄（boot 注入，照兄弟工具构造模式）+ authz 谓词；mutation 成功后发 `ProjectsChanged` 帧（与 RPC 生产者同一发布函数，提取 `projects::events::publish_changed` 免第二份）。描述写成常量（目录条目引用它）。
- [ ] **Step 4: 注册五处**——跑 `dispatchable.rs` census + 目录守卫，红一处补一处直到全绿；棘轮抬升提交里写三问答案（脚手架非认知 / 模型升级仍需要工具面 / 消费者 = 房间会话的模型 + R8 循环）。
- [ ] **Step 5: `cargo test -p alephcore --lib projects builtin_tools` 全绿；提交** `projects: shared authz module and project_manage tool (R8)`

---

### Task 11: 房间名册 prompt 层 + `[alice]:` 历史投影

**Files:**
- Create: `src/thinker/layers/room_roster.rs`（+ `layers/mod.rs` 注册）
- Modify: 历史渲染点（Step 1 定位）
- Modify: prompt 组装的 prefetch 载体（roster 名字异步预取）

- [ ] **Step 1: 定位两个接缝（有界侦察，产出写进提交信息）**——
  a. 历史渲染点：`grep -rn "UserMessage" src/thinker/ src/context/ | grep -v test` + 读 `src/session/events.rs:226` 的 `author_user_id` 从投影到 prompt 消息的路径；判据：**那个把 session 历史行变成 provider message 的唯一函数**。
  b. 层输入载体：读 `src/thinker/layers/identity_files.rs`（prefetch 型层范本）确认层从哪个 struct 拿预取数据。
- [ ] **Step 2: 投影失败测试**——房间会话（scope=project）的用户历史行带 `author_user_id: Some("u-alice")` ⇒ 渲染文本以 `[Alice]: ` 前缀（名字经 prefetch map，缺失回退 id，`escape_xml`）；**非房间会话零变化**（无前缀，字节断言）；模型/工具行不受影响。实现 → 绿。
- [ ] **Step 3: room_roster 层**——条件：会话 scope 为 `project:*` 且名册非空；渲染：
  ```text
  <room_context>
  项目房间成员: Alice (owner), Bob, Carol
  </room_context>
  ```
  `stability() = Dynamic`；`priority()` 照 `identity_files.rs` 邻位；名册上限 24 人、单名 64 chars 截断；自带字节界测试（`层输出 <= N` 断言 + 空名册不渲染）。名字来自 prefetch（assembly 阶段经 `SecurityStore` 批量解析 roster ids，塞进层输入载体；prefetch 失败 ⇒ 层渲染 user ids——不空转不阻塞）。
- [ ] **Step 4: 缓存纪律验证**——跑 `cargo test -p alephcore --lib thinker`（含 `stable_prefix_ignores_per_run_facts` 与层棘轮/守卫套件）；跑 `aleph-server prompt-size` 记录增量。
- [ ] **Step 5: 提交** `thinker: room roster layer and speaker-attributed history projection`

---

### Task 12: 裸 `chat.send` 房间戳修复（§5.22 round-2 ②）

**Files:**
- Modify: `src/projects/store.rs`（新查询）+ `src/gateway/execution_engine/execute.rs`（`ensure_session_under_request_scope`）

**Interfaces:**
```rust
// store.rs
/// The project (if any) whose claimed room key equals `key`. Read-only.
pub async fn project_of_session_key(&self, key: &str) -> Option<String>;
```

- [ ] **Step 1: 失败测试**（execute.rs 既有 `ensure_session_under_request_scope` 测试旁）：

```rust
// 房间已 claim key K（projects.room_session 夹具），对 K 发一个 metadata 里
// 没有 project_id 的 run → 断言会话行 scope_id == "project:p-x"（不是 personal）
// 且 owner 列不变；对非房间 key → 现状路径字节不变
```

- [ ] **Step 2: 实现**——`ensure_session_under_request_scope` 在从 request metadata 推导 scope **之前**：`if let Some(pid) = project_store.project_of_session_key(&key).await { 强制 project scope }`（网关拥有的映射优先于模型/客户端可写的 metadata——§0 首条判据；store 查询走索引列 `current_session_key`，加 `CREATE INDEX IF NOT EXISTS idx_projects_session_key`）。
- [ ] **Step 3: `cargo test -p alephcore --lib execution_engine projects` 全绿；提交** `gateway: room-claimed session keys always stamp project scope`

---

### Task 13: 文档回填 + 全量验证 + wasm

- [ ] **Step 1: 文档**——FEATURE_LOCATOR §5.22 新 round 条目（真人化 + P3 + 工具面 + 修复清单）与 §4.5（激活语义/署名）；SECURITY.md：审批发言人路由、workspace 只读面威胁模型（owner 绑过宽目录即向 roster 披露——文档警示）、旁听语义；`src/gateway/CLAUDE.md` 若有新地雷形状（如"事件 payload 无 agent_id 即人类消息"的判据归属）补条目；spec 文件头加"已实施"状态行。**同一事实两份表述同批改**：删掉的 pin 注释所描述的旧世界不得残留在任何 doc。
- [ ] **Step 2: Panel 产物**——worktree 内 `npm ci`（若 `interfaces/webchat` 需要）+ `just wasm`（Bash 工具跑，PowerShell PATH 缺 cygpath）；失败则记录并上报（round-2 曾因此三处修复未上线，**不许静默跳过**）。
- [ ] **Step 3: 全量验证五条 + 客户端 crate**：
  ```
  cargo test -p alephcore --lib                       # 期望 0 failed；记录总数
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib
  cargo check -p aleph-desktop-windows
  cargo clippy --all-targets --workspace --exclude aleph-desktop-macos --exclude aleph-desktop-linux
  cargo test -p aleph-tui -p aleph-cli                # wire 契约常客
  ```
  ⚠️ `--test '*'` 需 `-j 1`（见记忆 alephcore-integration-tests-need-j1）；`--lib` 必须看到 `test result:` 行才算跑过；clippy 新警告逐条与 `git diff --stat origin/main..HEAD -- <file>` 对账。
- [ ] **Step 4: 真机 QA（panel-realmachine-qa-harness 模式）**——隔离 `ALEPH_HOME` + mock provider + `pair --user` 第二身份 + Playwright：
  1. 两真人一 agent 团队线程：alice 发言（单人激活）→ bob 加入发言（旁听不激活）→ bob `@coder`（激活）→ 双端署名气泡实时可见；
  2. bob 触发的 run 举卡 → bob 的 pending 列出并可批 → operator 也可见；
  3. 项目页五 tab 各一条效果断言（kanban 列出 room 团队 / workspace 列出绑定目录 / memory 列出分区事实 / `projects.changed` 改名实时刷新侧栏）；
  4. 房间 prompt：agent 回复能正确称呼发言人（transcript 投影生效的行为证据）。
  结果逐条记录（PASS/FAIL + 证据行），FAIL 不许收摊。
- [ ] **Step 5: 提交** `docs: record multiuser team-chat and P3 round`（文档单独一笔）

---

## Self-Review 已执行

- **Spec 覆盖**：A1→T1-2 · A2→T2 · A3→T4 · A4→T2/T5 · A5→T3 · B1→T7/T8 · B2/B3→T9 · B4→T6 · C→T10 · D→T11 · E1→T12 · E2→T9 · E3→各任务 census 步 · E4→T13 · 边界语义七条→T2/T4/T6/T9 测试 · 测试策略→T13。无缺口。
- **类型一致性**：`author_user_id: Option<String>` 贯穿 T1-T5/T11；`has_activation_mention(content,&[String])->bool` T2 定义 T2 消费；`speaker_label`/`resolve_labels` T3 定义 T3 消费；`ProjectsChanged{project_id,change,affected_user}` T6 定义 T6/T10 消费；`ProjectAccess` T10 内闭合；`project_of_session_key` T12 内闭合。
- **占位符**：T9 Memory 与 T8 kanban 组件名、T11 渲染点为**有界侦察步**（给定 grep 命令 + 唯一性判据 + 产出契约），非 "TBD"。
