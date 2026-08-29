# 多用户 × 项目管理融合 — 频道群进房间（P4-1）设计规格
# Multi-User × Project Management — Channel Groups into Project Rooms (P4-1) — Design Spec

- **日期**: 2026-08-28
- **分支**: `worktree-multiuser-round9`（工作树 `D:/Workspace/Aleph-wt-multiuser-round9`，**严禁触碰 main**）
- **参考项目**: `T:\Github\qm`（TypeScript + Postgres；对照结论见附录 A）
- **上游 spec**: `2026-08-04-multi-user-org-project-design.md`（P0/P1/P2 已执行）· `2026-08-24-multiuser-teamchat-p3-design.md`（P3 已执行）
- **兑现的账本项**: `SECURITY.md` Known gap 2「channels-into-rooms is spec §11-3 / P4」· round-5→round-8 ③「房间存量归属回填」· round-8 刻意未做 ①「`bind_workspace` 工具面」④「子代理房间名册真机验证」
- **记录文档**: 完成后回填 `FEATURE_LOCATOR.md` §5.22 第九轮、`SECURITY.md`（Known gap 表重写）、`GATEWAY.md`、`src/gateway/CLAUDE.md`
- **状态**: 📐 设计已定稿，待实施

---

## 1. 背景与动机 (Background)

P0（身份）/ P1（隔离）/ P2（项目房间）/ round 3–8 之后，「谁能看」这一问已经被回答得相当完整：谓词、面数、actor、凭据、准入、花费、群聊真人化。

但**项目房间只存在于 Panel 里**。频道（Telegram / Slack / 飞书 / …）是另一个世界：

**实测（2026-08-28，读码确认）**：

- 一个频道群 = **一个共享的 session key**：`SessionKey::group(agent_id, channel, PeerKind::Group, conversation_id)`（`src/gateway/inbound_router/agent_resolver.rs:258-270`）。
- 每一轮按**说话人**盖 `ScopeAttribution::personal(<u-id>)`（`src/gateway/inbound_router/executor.rs:305-318`）；**未配对的人什么都不盖**，于是走 adoption-by-absence 回落到机主。
- `src/projects/*.rs` 全目录 **零处** `channel`。

三条后果：

1. **会话行被首个说话的人永久认领**（`SessionMetadata::stamp_attribution` 是 create-only，`src/gateway/session_store/types.rs:207`）⇒ 其余成员在 Panel 的 `sessions.list` / `chat.history` 里**看不见这个群**。
2. **群的记忆按说话人碎成 N 份分区**；未配对者那一份落进**机主私人分区**。
3. 无名册 ⇒ 无 `<room_context>`、审批卡无房间收窄、`RoomRosterLayer` 永不渲染。

也就是说：**一个多人频道群，在数据模型里是「第一个说话的人的私人会话，别人碰巧能往里打字」。** 这既是「多用户 × 项目管理融合」的核心缺口，也是一处现存缺陷，而不只是一个缺失的功能。

`SECURITY.md` 的 Known gap 2 已经预告了这一轮：

> Channel-originated runs bypass `build_run_request`, so a channel session cannot acquire a room's bound workspace (or a room scope at all). The P2 acceptance surface is the Panel; channels-into-rooms is spec §11-3 / P4.

---

## 2. 已锁定的决策 (Settled Decisions)

以下由用户在 2026-08-28 brainstorming 中裁定，不再重开：

| # | 决策 | 裁定 |
|---|------|------|
| D1 | 本轮范围 | **P4-1 主线 + 同路打磨**：频道群 ⟷ 项目房间绑定，外加与它同一条路径上的断线/缺陷/文档腐烂 |
| D2 | 名册真源 | **名册仍是唯一真源，频道群成员不自动入册**。已入册者发言 ⇒ 房间 scope；未入册者发言 ⇒ 保持 personal，不回溢房间数据。**不移植 qm 的 `syncChannelMembers`** |
| D3 | 绑定动词的闸 | **operator 专属**（绑定与解绑皆是）。理由：它的暴露方向朝外——绑定后 agent 用房间共享记忆作答，而回复投递给整个群，包括名册管不到的人 |
| D4 | 绑定键 | **`(channel_id, peer_id)`**，即绑定「那个群」而不是「那个会话」。`agent_id` 不进键 ⇒ 对 `agent_switch` 免疫；唯一约束落在会话一侧 ⇒ 一个房间可绑多个群（R6 一核多端） |
| D5 | 存量会话行 | **绑定时 re-stamp**（§10 会话 scope 不可变性的显式、受限、可审计的例外），换取「房间成员在 Panel 里真的看得见这个群」，并顺带结清存量回填账本 |
| D6 | 解绑语义 | 未来方向可逆（新回合退回 personal）；**历史方向不追溯改写**（已落盘的行保持房间 scope） |
| D7 | 工具面 | `bind_channel` 本轮**不**上 `project_manage`；`bind_workspace` 借本轮的谓词收窄补上工具面 |
| D8 | `LoopState::scope_id` | **保留零读者裁定**，并把这条散文裁定升格为会红的守卫（P4，见 §6） |

---

## 3. 现状基线 (Current-State Anchors)

实施前的关键锚点（2026-08-28 实测）：

- **判定汇流已存在**：`src/gateway/execution_engine/run_loop/mod.rs:68-95`
  ```rust
  fn request_scope(request: &RunRequest) -> Option<ScopeAttribution> {
      let stamped = scope_from_metadata(&request.metadata);
      let Some(pid) = room_claiming(&request.session_key) else { return stamped };
      let mut attr = stamped?;
      attr.scope = ScopeId::Project(pid);
      Some(attr)
  }
  ```
  round-8 ⑤ 已让**全部七个生产者**（含频道 inbound router）经过它。`room_claiming` 走
  `ProjectStore::project_for_session_key`。**这条线已经建好，缺的只是让频道群的会话被房间认领，以及一道名册闸。**
- **准入侧闸只在 Panel 路径**：`src/gateway/handlers/agent.rs:547` `resolve_attribution` 的 Path 2（会话尚不存在 + 显式 `project_id`）跑 `visibility::project_visible`；Path 1（行已带 scope）**不做成员检查**——Panel 路径的安全性来自到达该 key 本身过了 `session_visible`。频道路径**没有任何等价的闸**。
- **归属盖戳**：`ScopeAttribution` / `stamp_metadata` / `scope_from_metadata` 在 `src/scope/mod.rs`；`ScopeId::{Org, Personal, Project}`，`Org` 无生产者（刻意保留）。
- **会话行归属**：`stamp_attribution` create-only（`session_store/types.rs:207`）；`backfill_attribution` 在 trait 上默认 `Unsupported`、唯一调用者是 `projects::attribution_backfill`（`session_store/mod.rs:143`）。
- **房间存储**：`src/projects/store.rs`（`Project{id,name,owner_user_id,workspace_path,status,…,current_session_key}` + `project_members`）；名册投影 `src/projects/roster.rs` 在 store 写锁内 `republish_roster_locked` 发布；事件单一发布者 `src/projects/events.rs::publish_changed`。
- **RPC 面**：16 个 `projects.*`（`src/gateway/handlers/mod.rs:545-650`）；`method_census.rs` 逐一钉住 405 个方法的 Admin/Open 裁决。
- **CLI**：`interfaces/cli/src/commands/` 有 `users_cmd` / `audit_cmd` / `spend_cmd` / `channels_cmd`，**没有 `projects_cmd`** —— operator-only 家族里唯一缺 CLI 的那张脸。
- **两条断线/缺陷（先于本轮存在）**：
  - `AUTHOR_USER_KEY`（`execution_engine/mod.rs:193`）**只有一个生产者** `handlers/agent.rs:828`，而 `run_loop/mod.rs:100` 的 doc 逐字写着有两个（含频道 inbound router 的 `execute_for_context_inner`）。
  - `caller_may_choose_directory()`（`caller_identity.rs:150`）对 `role == None` **恒真**，而 `None` 正是 cron / A2A / 进程内 / 工具面的取值。

---

## 4. 架构设计 (Architecture)

### 4.1 绑定模型

新增一条关系，**不新增任何 scope 类别**：

```
RoomChannelBinding {
    project_id, channel_id, peer_id, peer_kind,
    bound_by, bound_at, label
}
```

- 唯一约束在**会话一侧**：`UNIQUE(channel_id, peer_kind, peer_id)` ⇒ 一个群最多属于一个房间。`peer_kind` 进键是因为 `SessionKey::Group` 变体同时承载 `Group` 与 `Thread` 两种 `PeerKind`（`src/routing/session_key.rs:30`），两者的 `peer_id` 命名空间不保证不相交。
- 房间一侧无唯一约束 ⇒ 一个房间可同时活在多个群（Telegram + Slack）。
- `agent_id` **不在键里** ⇒ `agent_switch` 不解绑。

**为什么不用 qm 的「Project 加一列」**：qm 的 `Project.slackChannel` 是单值列，因为它只支持一个频道；Aleph 支持多个，列会立刻变成 JSON 数组——那是第二个真源的常见入口。

### 4.2 存储

新表 `project_channel_bindings`，落在 `ProjectStore` 的同一个 catalogue、同一把写锁（名册投影已在那把锁内发布，绑定变更同样要在锁内发布 `projects.changed`）。

读路径新增 `project_for_conversation(channel, peer_id) -> Result<Option<String>>`，与既有 `project_for_session_key` **并列为同一方法族**：同样带索引、同样的 `Err` 语义。

**迁移顺序判据**（round-8 ⑥ 的教训直接适用）：新表进 `SCHEMA` 是安全的（`CREATE TABLE IF NOT EXISTS` 对旧库是 no-op），但它的索引必须与表同批、**绝不能引用任何由后续迁移补上的列**。配一条 `a_pre_rooms_catalogue_still_opens_and_binds`，用手工构造的旧形状库钉住——隔离的测试 HOME 对这一类结构性失明（每个夹具都建全新库，永远是新形状）。

### 4.3 判定汇流 —— 全轮唯一的判定改动点

`run_loop::room_claiming` 从一问变两问，**汇成同一个 `Option<project_id>`**，下游一个字不改：

```
fn room_claiming(session_key) -> Option<String>
    ① 既有：这条 key 被哪个房间认领（Panel 房间会话，current_session_key）
    ② 新增：key 是 Group{channel, peer_id} 时，这个群绑给了哪个房间
    两条都命中且不一致 ⇒ warn! 并取 ①（更具体的认领赢）
    任一条 Err ⇒ None + warn!（保留既有 "leave the producer's stamp alone"）
```

**频道 inbound router 的 scope 盖戳一行不改**（仍是 `personal(speaker)`）。「这一 run 是什么 scope」保持 round-8 ⑤ 建立的**单一推导**，不给它第二个答案。

> ⚠️ **本段已被 Ruling AE 推翻（2026-08-29，commit `4da644834`）。原文保留在下方，
> 因为「当初为什么这么裁」和「后来为什么改」都要看得见；但**它描述的世界已经不在了**，
> 读到这里不要照它行事。**
>
> **现状**：两个孪生**已经合并**到唯一的 `ProjectStore::room_claiming`
> （`src/projects/store.rs:945`，`pub(crate)`），返回 `Option<(String, ClaimSource)>`；
> `handlers::agent::resolve_attribution`（准入面）与 `run_loop::request_scope`
> （准入之后）各自 `match` 那个 `ClaimSource` 来决定自己的处置。
>
> **原裁定错在哪**：它把「两个消费者对同一个答案做不同的事」当成了「两个消费者要各自
> 算一遍那个答案」。前者是对的、且现在仍然成立（arm 1 拒绝、arm 2 落回 personal，
> 不对称是设计）；后者是一条规则有了两个作者——正是这类缺陷的标准形状。**处置的分歧
> 不是重新推导的理由。**
>
> 守卫：`the_two_room_claim_twins_agree_on_which_project_governs`
> （`run_loop/tests.rs`）与 `the_two_claim_arms_are_gated_differently_for_the_same_non_member`
> （`handlers/agent.rs`）——前者钉住两面同答，后者钉住两臂不同闸，**两条必须同时绿**：
> 只有前者会诱使下一个人把两臂"对称化"，只有后者会诱使他把两面重新拆开。

~~`handlers::agent::room_claiming` 与 `run_loop::room_claiming` 这对孪生**继续不合并**——它们的 `None` 喂给不同分支（一个可拒绝、一个在准入之后），doc 已写明 "deliberately not shared"。~~

### 4.4 名册闸 —— 本轮真正的安全新增

今天 `request_scope` 拿到 pid 就**无条件**升格。Panel 路径之所以安全，是因为到达那条 key 本身过了 `session_visible`；**频道路径没有任何等价的闸**。一旦群会话被房间认领，「在这个 Telegram 群里」就等价于「在名册上」——正是 D2 要禁止的那件事。

```
let mut attr = stamped?;                                   // 未配对 ⇒ 无身份 ⇒ 不取房间 scope
let target = ScopeId::Project(pid);
if attr.scope == target { return Some(attr); }             // 生产者已经房间化：别再判一次
if !visibility::project_visible_to(&pid, Some(&attr.owner_user_id)) {
    return Some(attr);                                     // 非成员：保持 personal
}
attr.scope = target;
```

三条必须写下理由的细节：

1. **闸只作用于「升格」，不作用于「已经是房间的」**（`attr.scope == target` 的早返回）。这一条是**防回归**的：`resolve_attribution` 的 Path 2 会给 cron 之类的无限制调用者产出 `owner = OWNER_USER_ID` + `scope = Project(pid)`，而 `OWNER_USER_ID` 未必在名册上——无条件跑闸会把一个**已经过准入**的房间 run 静默降级成个人 run。谓词只该问它被引入来回答的那一问：「一次由会话认领**推导**出来的房间化，可信吗」。
2. **这条闸对 Panel 路径逐字节 no-op**：Panel 的两条路径都让 `stamped.scope` 已经等于 `Project(pid)`，命中第一条早返回。统一规则的代价因此是零——不给频道路径开一张单独的许可证。
3. **谓词用 `visibility::project_visible_to(pid, Some(actor))`，不直接调 `roster::is_member`**。⚠️ **规划期读实更正**：`roster::is_member(project_id, user_id) -> bool` 是**同步内存投影读**，**没有 `Err` 臂**（`src/projects/roster.rs:71`）——设计初稿里「`Err` 臂方向与 Ruling P9 相反」那段论证**不成立**，据此作废。真实的三态是：投影里没有该房间 / 没有该成员 / 本进程尚未发布投影，三者都读作 `false`，即**未知一律 fail-closed**，而 fail-closed 正是这道闸要的方向。Ruling P9 读的是**另一个**东西（`ProjectStore::members()`，那个才返回 `Result`），两条规则因此不冲突也不需要互相让步；把这句话写进代码注释，否则下一个人会去找那个并不存在的 `Err` 臂。

### 4.5 未配对回合的收窄

⚠️ **规划期读实更正：这一条不需要新代码，它已经由 `stamped?` 那一行 fail-closed 了。**

未配对的人没有 `u-` 身份 ⇒ `scope_from_metadata` 返回 `None` ⇒ `request_scope` 在 `let mut attr = stamped?` 处直接返回 `None` ⇒ **该 run 没有 scope task-local** ⇒ 既不会取房间分区，也不会渲染 `RoomRosterLayer`（它读的正是那个 task-local）。设计初稿提出的「收窄」在现有代码里已经成立。

因此本条从「新增收窄」降级为**两件事**：

1. **补一条断言把它钉住**（`an_unpaired_speaker_in_a_bound_conversation_takes_no_room_scope`）——它今天为真是**推导**出来的，不是被守着的；`stamped?` 那一行将来若被改成 `unwrap_or_default()` 之类，房间分区会在没有任何测试变红的情况下对陌生人打开。
2. **把残留边界记录下来而不是修它**：未盖戳的回合仍按 adoption-by-absence 落进机主分区——这是**先于房间存在**的行为，与绑定无关（每一个群今天都如此）。修它需要一个「无归属 run」的新概念，而它此刻**零消费者**（R10 撤回模式）。写进 §9 不做清单。

---

## 5. 存量与可逆性 (Existing Rows & Reversibility)

### 5.1 会话行 re-stamp —— 本轮唯一修订既有裁定的地方

`stamp_attribution` 的 doc 写着「stamping is create-only（spec §10: session scope is immutable once set）」。绑定一个**已存在**的群会话会出现分叉：run 取房间 scope，而会话行仍是 `personal:<第一个说话的人>` ⇒ 其余成员 `session_visible_to` 为 false ⇒ Panel 里看不见这个群。「行和循环用两个答案」正是 round-8 ⑤ 点名的那台机器。

**修订**（显式记录，不是悄悄放宽）：

> §10 的不可变性保留为**默认**。唯一的例外是 operator 发起的房间绑定——它是一次**授予可见性**的动作，有人、有理由、有审计行。

新增 `SessionStore::rescope_attribution`，与既有 `backfill_attribution` **同一族**（trait 默认 `Unsupported`、两个 backend 各实现、谓词写在语句里而不在调用方）。三条收窄使这个例外不能被别处借用：

1. 只接受 `Group` 形状的 key，且目标 scope 只能是 `Project`（个人 / org 目标编译期不可达）；
2. 唯一调用点是 bind handler，配**源码级** census（第二个调用者出现即红）；`backfill_attribution` 的 "its only caller is …" 是一句没有测试的散文，本轮把两条一起补上；
3. 落一行审计（`AuthorityChange` 家族）。理由与 round-6 ④ 逐字相同：**铸凭据有记账、撤凭据没有**这类不对称，是靠「记在共用管道上而不是 handler 上」修掉的。

**顺带结清账本**：round-5 记下、round-8 ③ 重申的「房间存量归属回填」一直开着，理由是「它是一次授予可见性的迁移，且要在两个 backend 上加 `SessionStore` 方法」。本轮把那个方法做出来了，且以**逐会话、operator 发起、可审计**的形态出现——比开机时的无差别回填更安全，正是当初推迟它的那条理由本身在要求的形态。

### 5.2 解绑

「这个决定有没有反悔的路」两个方向都要答：

- **未来方向可逆**：解绑后新回合不再取房间 scope（`room_claiming` 立刻查不到）。
- **历史方向不可逆，且这是正确的**：已落盘的行保持房间 scope。反向 re-stamp 需要挑一个「还给谁」的个人 scope，而唯一候选（第一个说话的人）可能早已离开名册——那会是一次凭空的归属发明。绑定期间房间成员**确实**看得见那段对话，历史不因解绑而追溯变私。
- 名册照旧是活闸：移出名册的人立刻看不到，无需改动。

---

## 6. 断线修复与打磨 (Wiring Fixes & Polish)

| # | 项 | 类别 | 内容 |
|---|---|---|---|
| P1 | `AUTHOR_USER_KEY` 补第二个生产者 | **断线** | 见下 |
| P2 | `caller_may_choose_directory` fail-OPEN 收窄 | **缺陷** | 见下 |
| P3 | `backfill_attribution` / `rescope_attribution` 的「唯一调用者」升格为 census | 加固 | doc 已点名，但 doc comment 没有测试 |
| P4 | `LoopState::scope_id` 零读者裁定升格为守卫 | 加固 | 一条只写在散文里的裁定防不住下一个真诚的修复者 |
| P5 | `SECURITY.md` Known gap 1 已腐烂 | 文档熵减 | 它写着「`projects.*` 无工具面」，而 round-8 ③ 已落 `project_manage` |
| P6 | `SECURITY.md` Known gap 2 收窄重写 | 文档 | 本轮部分兑现，剩余部分（主动推送方向）要写清边界 |
| P7 | 子代理房间名册真机验证 | 账本 | round-8 刻意未做 ④；本轮既然要建真机夹具，边际成本接近零 |

### P1 · `AUTHOR_USER_KEY` 的第二个生产者不存在

`run_loop/mod.rs:100` 的 doc 逐字写着两个原产地：`build_run_request` **与频道 inbound router 的 `execute_for_context_inner`**。全仓 grep：生产者只有 `handlers/agent.rs:828` 一个。

这条断线**今天就在生效**（先于 P4）：频道群回合的 `current_room_author()` 回落成会话属主，于是 guard 侧审计的 `actor_user`（2026-08-16 openclaw 轮专门接的那条）在群里记的是**首个说话的人**而不是当事人；`nudges::speaker_label` 同理。DM 会话因属主 == 说话人而恰好正确，**只有群会话是错的**——这正是它四轮没被发现的原因。

修法就在频道执行器已有的那个 `if let Some(user) = pairing_store.sender_user(...)` 块里，与 scope 盖戳同一笔（它已经解析出了 `u-` id，只是没把它交给第二个消费者）。守卫写成**源码级双文件推导**：先证明 `run_loop` 确实从该键 seed 了 `CURRENT_ROOM_AUTHOR`，再要求 doc 点名的两个原产地都真的写它——即让那句 doc **自我执行**。

> 判据实例：**用名字 grep 找断线，会漏掉那种「唯一的外部引用就是撒谎的注释」的断线**。这条线的搜索命中里，有一条就是替它作伪证的那句 doc。

### P2 · `caller_may_choose_directory()` 的 fail-OPEN 收窄

```rust
let role = current_caller_role();
let is_config_tier = !matches!(role.as_deref(), Some(r) if r != "operator");
is_config_tier || current_caller_is_loopback()
```

`role == None` ⇒ `matches!` 为 false ⇒ `!false = true` ⇒ **恒真**。

修法用现成的形状——**「我手上没有句柄所以只能沉默」时，先问那个执法者**：run 的 `caller_role` 从来没有丢，它被盖进 metadata 并由 `ScopedToolService` 用来跑配置闸。所以给这个谓词一个**显式 actor 孪生** `caller_may_choose_directory_as(role, is_loopback)`，裸形式改为该孪生的薄包装（**单一推导**，这是本项的熵减部分），工具面从那个**已经在裁决同一件事**的对象取值。

- 对 RPC 面逐字节 no-op（task-local 在那里是活的）；
- 对工具面从**恒真**变成**按 run 的真实档位裁决**；
- 收窄之后 `bind_workspace` 的工具面不再被阻塞 ⇒ round-8 刻意未做 ① 结清（D7）。

---

## 7. 面 (Surfaces)

| 面 | 本轮 | 理由 |
|---|---|---|
| `projects.channel.bind` / `.unbind` / `.list` RPC | ✅ | admin-gated；必须同批更新 `method_admin` 表与 `method_census`（未分类即红） |
| Panel 房间页「频道」区 | ✅ | 房间页已是五个 tab 的宿主；绑定状态与 `bound_by`/`bound_at` 渲染在设置区，与 `bind_workspace` 同形 |
| `aleph projects`（`list` / `channel bind\|unbind\|list`） | ✅ | operator-only 家族（`users`/`audit`/`spend`）都有 CLI 而房间没有；无头部署没有 Panel |
| `project_manage` 加 `bind_channel` | ❌ 刻意未做 | 与 round-8 ③ 对 `bind_workspace` 的裁定同构：它是**一次授权**而非一次偏好，且暴露方向朝外。第一轮留在已认证的 operator 面上 |
| `project_manage` 加 `bind_workspace` 动词 | ✅（D7） | P2 的收窄兑现；它的暴露方向**朝内**（绑一个本机目录），与 `bind_channel` 相反。注意：这是 `projects.bind_workspace`（房间绑目录），与既有的 `workspace_manage` 工具（管理 workspace 实体）是两件事，不要合并 |

---

## 8. 验证策略 (Verification)

### 8.1 最小验证集

本轮动了 CLI 与 Panel，所以比默认六条多一条：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins                                        # 唯一真跑的一条
cargo test -p alephcore --features test-helpers --test '*' --no-run   # 注意 -j 1
cargo test -p aleph-panel --lib --no-run
cargo test -p aleph-tui -p aleph-cli                                  # 新增 aleph projects 子命令
cargo check -p aleph-desktop-windows                                  # 本机平台
cargo clippy --workspace --all-targets                                # 先 just _stage-shell-placeholders
```

### 8.2 四条硬纪律

1. **每条守卫写完立刻手动破坏一次**，按判读顺序四分类：`running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → 剩下的（连 `test result:` 行都没有）才是 BUILD-ERROR。**红的条数比预期少时先怀疑自己的判断，不怀疑守卫。**
2. **跨 crate wire 契约进 `aleph_protocol`**：`aleph projects channel …` 的请求/响应形状由契约类型**构造**（不只是解析），两个方向各一条对账——请求侧 deserialize、响应侧**键集相等且期望从契约类型自身序列化派生**。这一族在本仓复发过三次。
3. **`method_census` 同批更新**：新增三个 `projects.channel.*` 未分类即红；它与 `method_admin` 是必须一致的两张表。
4. **描述字节棘轮实测不手算**：给 `project_manage` 加 `bind_workspace` 动词会动它的 `DESCRIPTION`，`catalog_description_bytes_ratchet` 的新数字必须来自**真的跑过一次**的输出，不是算术（round-7 的教训：闸跑不了的时候作者只能算，而那次算术差了 510 字节）。

### 8.3 规划期必须单独跑的一次扫描

round-7 最贵的教训——两两冲突扫描**抓不到**「一个句柄没有任何任务声称要装它」：

> 「这份计划读的每一个句柄 / 注册表，哪个任务在生产路径里**真的装它**？」

本轮的答案已先验证：**不新增任何进程级句柄**——绑定读写全部经既有 `ProjectStore::shared()`，不触 §5.25 的 `CapabilitySlot` 那一族。这句话必须写进计划，否则下一个人会以为它没被问过。

### 8.4 真机 QA

`qa/rooms_channel_bind/run.sh`，扩展 `qa/teamchat_rooms`（43 条断言）与 `qa/channels`（24 条断言）的既有夹具：隔离 `ALEPH_HOME` + mock 频道 + 三个真实身份（operator 走 loopback，两个 member 各自用一次性配对票从 LAN 腿连回来——loopback 在 `resolve_connect_auth` 第一行就被短路成 operator，所以第二、第三个身份**结构上**只能从 LAN 腿进来）。

逐条带**效果**断言：

1. 未绑定的群：两个成员各写各的分区（现状基线，先证明它确实如此）
2. operator 绑定后：名册成员的回合落 `main__p-<id>`；另一个成员在 Panel 的 `sessions.list` 里**看得见**这个群（re-stamp 生效）
3. 名册**外**的已配对者在同一群说话：回合仍是 personal，**不**碰房间分区（§4.4 的正面证据）
4. 未配对陌生人说话：不写记忆、不落机主分区（§4.5）
5. `<room_context>` 在**频道**回合里出现且点名没说话的成员（P7 顺带覆盖子代理继承）
6. 解绑后新回合退回 personal，历史行保持房间 scope（§5.2）
7. `agent_switch` 之后绑定仍然生效（D4 的正面证据）

**夹具注意**（round-8 三条教训直接适用）：`{method:"event",params:{topic,data}}` 是 bus 事件的第三种信封形状；`<system-reminder>` 不是「这是脚手架」的可靠判据；房间分区是 `main__p-<id>` 而不是 `main__p-p-<id>`。

---

## 9. 明确不做 (Deliberately Out of Scope)

写下来防止重提：

- **qm 的逐资源 Grant ACL** —— round-4 已裁决不移植（名册一行原子地答四个问题，五种 `ResourceKind` 的 grant 表做不到这个承诺，只会给房间造第二个真源）。
- **`syncChannelMembers` 频道成员自动入册** —— D2。它会给名册加第二个写者，破坏「名册变更 owner-only ⇒ 转授结构上不可能」；且列不全群成员的频道会静默少人，而「少了一个」与「他本来就不在」在日志里同形。
- **房间 → 群的主动推送**（AI 主动到达方向）—— 本轮只做入方向；主动播报要回答「谁触发、发给谁、谁看得到」三个新问题，是独立一轮。
- **`audienceEgressFloor`**（qm 的房间出网地板）—— 仍缺 per-principal egress 轴，移植等于先造一个子系统再给它一条规则。round-6 ② 的裁定不变。
- **`ScopeId::Org`** —— 仍无生产者，不碰。
- **给频道路径加第二个 scope 盖戳** —— 房间 scope 只由 `request_scope` 一个推导决定。
- **合并两个 `room_claiming` 孪生** —— doc 已写明 deliberately not shared。
- **「无归属 run」概念** —— 未盖戳的回合落进机主分区是**先于房间存在**的行为（每个群今天都如此），与绑定无关。给它一个 `Unattributed` run 语义此刻零消费者（R10 撤回模式）。见 §4.5。
- **Kanban 拖拽写 RPC**（round-8 ⑤）· **Memory tab 的 curated 段**（待产品裁决）—— 与频道绑定不同路，留账本。

---

## 10. 实施顺序 (Phases)

每阶段可独立提交，阶段 0 先落地以便后续阶段站在已修好的地基上。

| 阶段 | 内容 | 依赖 |
|---|---|---|
| 0 | P1–P4（断线 / fail-open 收窄 / 两条 census 加固），各自带 RED 证伪 | — |
| 1 | `project_channel_bindings` 表 + `project_for_conversation` + 旧形状库开机测试 | — |
| 2 | `room_claiming` 汇流臂 + **名册闸** + 未配对收窄 | 1 |
| 3 | `rescope_attribution`（两个 backend）+ 审计行 + 单一调用者 census | 1 |
| 4 | 三个 RPC + `method_admin`/`method_census` + `aleph_protocol` 契约 + CLI + Panel 区块 | 2, 3 |
| 5 | `bind_workspace` 工具面（P2 的兑现）+ 棘轮实测 | 0 |
| 6 | 真机 QA 夹具 `qa/rooms_channel_bind/` | 4 |
| 7 | 文档：FEATURE_LOCATOR §5.22 第九轮 + SECURITY.md gap 表重写 + GATEWAY.md + `src/gateway/CLAUDE.md` | 全部 |

**规模估计**：13–16 个任务。纯新增代码集中在阶段 1/3/4；阶段 0 与 2 基本是**改判定位置**而不是加逻辑。

---

## 附录 A · qm 对照结论 (Gap Analysis vs `qm`)

`T:\Github\qm` —— multiplayer agent harness，TypeScript + Postgres，1303 文件。

| 维度 | qm | Aleph | 本轮处置 |
|---|---|---|---|
| Scope 原语 | `ScopeId = "kind:ref"`，5 类（personal/channel/team/org/group），`src/types.ts` | `ScopeId::{Org,Personal,Project}` 枚举 | 不动（Aleph 类型更严；类别更少是刻意的） |
| 授权谓词 | `canRead/canWrite/canManage/membershipControls/currentMembers`，`src/resolution/scope-membership.ts` | `visibility.rs` 谓词族 + `roster::is_member` + `projects::authz` | 打平；Aleph 的「一个谓词体服务四张脸」已优于 qm（qm 在一个文件里重复五次同一检查） |
| 逐资源 Grant ACL | `src/acl/{acl-store,resource-ref}.ts`，`Grant{ownerScope,ref,granteeScope,permission}` | 无 —— 名册即授权 | **已裁决不移植**（round-4），本轮不重提 |
| 成员资格三态 | `boolean \| undefined` + 会话历史兜底 | `is_member` 的 `Err` 臂已按 Ruling P9 处理 | 打平；本轮为 scope 决定新增一条**方向相反**的 `Err` 裁决（§4.4） |
| **项目 ⟷ 频道绑定** | `Project.slackChannel` + `syncChannelMembers` + 项目实现 `ManagedGroupDirectory`，`src/projects/project-store.ts` | **无** | ✅ **本轮主线**；绑定关系移植，`syncChannelMembers` 不移植（D2） |
| 乐观并发 | `withVersion(groupRef, version, fn)` | 名册投影在 store 写锁内发布（更强） | 不移植；Aleph 的写锁方案严格更强 |
| 一次解析产出全部 | `Resolution{layers[ro/rw], prompt, egress, commandPolicy, securityPolicy, grants}` | 分散在 `resolve_agent_dir` / sandbox jail / `tool_permissions` / exec_tier，各自有守卫 | 记为**未来候选**，本轮不做（触 R10 边界，收益比需先验证） |
| 房间出网地板 | `audienceEgressFloor` | 无 per-principal egress 轴 | 刻意未做（沿用 round-6 ②） |
| `personKey` 归一化 | 含 `@` 的 principal id 小写归一 | id 是服务端铸的 `u-<uuid v4>` | 该缺陷族结构上不存在，不移植（round-6 ①） |
| 目录同步双源停用 | `recordDirectorySync` | 无目录同步 | 零消费者，不建（round-6 ②） |
| per-principal 预算 | `budget.ts` 人 + org 双层 | ✅ round-7 已落地 `[policies.spend]` | 已追平 |

---

## 附录 B · 本轮踩到的既有判据 (Applicable Criteria)

来自 `CLAUDE.md` 工程判据清单，实施时逐条对照：

1. **一个动词有 N 个面时，「谁能看」要在每个面用同一个推导** —— §4.4 的闸放在汇流处而非频道路径上，正是为此。
2. **用名字 grep 找断线，会漏掉「唯一的外部引用就是撒谎的注释」的那种** —— P1。
3. **恒真的谓词等于没判** —— P2（`role == None ⇒ true`）。
4. **一条只写在散文里的裁定，防不住下一个真诚的修复者** —— P3、P4、以及 §5.1 对 §10 的显式修订。
5. **收敛写者时要数一遍写者** —— `rescope_attribution` 的单一调用者 census。
6. **契约的两半住在两个 crate 里时，「有测试」这件事本身会骗人** —— §8.2 第 2 条（本仓已复发三次）。
7. **一个「报成功的 no-op」，修在执行者身上比修在每个已知实例上便宜** —— 绑定不存在的群 / 已绑给别的房间的群，必须**出声拒绝**而不是静默成功。
8. **这个决定有没有反悔的路** —— §5.2 两个方向各答一遍。
9. **隔离环境的 QA 结构上只测得到「新建的对象」** —— §4.2 的旧形状库测试、§8.4 的存量会话场景。
10. **「这份计划读的每一个句柄，哪个任务真的装它」是一次独立扫描** —— §8.3。
