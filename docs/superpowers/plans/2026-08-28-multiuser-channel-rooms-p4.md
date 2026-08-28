# P4-1 频道群进项目房间 — 实施计划 (Channel Groups into Project Rooms)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让一个频道群会话可以被绑定到项目房间，从而使房间成员在群里的回合取房间 scope（共享记忆分区、名册、房间上下文），而不再按说话人碎成 N 份个人分区。

**Architecture:** 绑定关系存在 `ProjectStore` 的新表 `project_channel_bindings`，键为 `(channel_id, peer_kind, peer_id)`。判定汇流点是既有的 `run_loop::room_claiming` —— 它已经是全部七个 run 生产者共用的单一推导，只需增加第二条解析臂；`request_scope` 在**升格**（把非房间 stamp 换成房间 scope）时加一道名册闸，因为频道路径没有 Panel 路径的 `session_visible` 准入检查。绑定时用一个受限、可审计的 `rescope_attribution` 把已存在的会话行改成房间 scope。

**Tech Stack:** Rust（tokio + serde + rusqlite）· `aleph_protocol` 跨 crate 契约 · Leptos/WASM Panel · clap CLI

**Spec:** `docs/superpowers/specs/2026-08-28-multiuser-channel-rooms-p4-design.md`

## Global Constraints

从 spec 逐字继承，每个任务都隐含包含：

- **分支隔离**：全程在 worktree `D:/Workspace/Aleph-wt-multiuser-round9`（分支 `worktree-multiuser-round9`）。**严禁触碰 main。**
- **名册是唯一真源**（D2）：不新增任何名册写者。频道群成员**不**自动入册。
- **绑定/解绑 operator 专属**（D3）：`projects.channel.bind` / `.unbind` 进 `method_admin.rs::ADMIN_METHODS`；`.list` 保持 Open 并由 `gate_project` 按名册裁决。
- **不新增进程级句柄**：绑定读写全部经既有 `ProjectStore::shared()`，不触 `CapabilitySlot` 那一族。任何任务若发现自己需要一个新的 `OnceLock`/`ArcSwap`，**停下来报告**——那意味着设计前提变了。
- **不给频道路径加第二个 scope 盖戳**：`inbound_router/executor.rs` 的 `ScopeAttribution::personal(speaker)` 一行不改。房间 scope 只由 `request_scope` 一个推导决定。
- **每条新守卫写完必须手动破坏一次证 RED**，并按判读顺序四分类：`running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → 剩下的（连 `test result:` 行都没有）才是 BUILD-ERROR。**红的条数比预期少时先怀疑自己的判断，不怀疑守卫。**
- **源码级守卫必须先 `.replace('\r', "")`**：本仓 Windows 检出是 CRLF，`split("\n#[cfg(test)]\n")` 之类的分隔符永不匹配，守卫会变成扫自己的测试模块。
- **提交信息**：英文，`<scope>: <description>` 格式，结尾附 `Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr`。
- **代码注释用英文**，文档中英双语。

## 验证命令（每个任务的「跑测试」步骤从这里取）

```bash
cd /d/Workspace/Aleph-wt-multiuser-round9

# 单条测试
cargo test -p alephcore --lib <test_name> -- --exact --nocapture

# 任务收尾（改了 core）
cargo test -p alephcore --lib --no-run
cargo clippy -p alephcore --lib --tests

# 任务收尾（改了 CLI / Panel）
cargo test -p aleph-tui -p aleph-cli
cargo test -p aleph-panel --lib --no-run

# 全轮收尾（Task 14 之前跑一次）
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1
cargo test -p aleph-panel --lib --no-run
cargo test -p aleph-tui -p aleph-cli
cargo check -p aleph-desktop-windows
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
```

---

## 文件结构 (File Structure)

| 文件 | 职责 | 任务 |
|---|---|---|
| `src/gateway/inbound_router/executor.rs` | 修改：频道 run 补盖 `AUTHOR_USER_KEY` | 1 |
| `src/gateway/execution_engine/run_loop/author_census.rs` | 新建：`AUTHOR_USER_KEY` 生产者的源码级 census | 1 |
| `src/gateway/caller_identity.rs` | 修改：`caller_may_choose_directory` 拆出显式 actor 孪生 | 2 |
| `src/gateway/session_store/mod.rs` | 修改：`rescope_attribution` trait 方法 + 单一调用者 census | 3, 7 |
| `src/looping/scope_census.rs` | 新建：`LoopState::scope_id` 零读者守卫 | 4 |
| `src/projects/store.rs` | 修改：`project_channel_bindings` 表 + 三个方法 | 5 |
| `src/projects/binding.rs` | 新建：`ChannelBinding` 类型 + `conversation_of_session_key` | 5 |
| `src/gateway/execution_engine/run_loop/mod.rs` | 修改：`room_claiming` 第二臂 + `request_scope` 名册闸 | 6 |
| `src/gateway/session_store/file_backend/mod.rs` | 修改：`rescope_attribution` 实现 | 7 |
| `src/gateway/session_store/sqlite_backend/mod.rs` | 修改：`rescope_attribution` 实现 | 7 |
| `shared/protocol/src/projects.rs` | 新建：三个 RPC 的跨 crate 契约类型 | 8 |
| `src/gateway/handlers/projects_channel.rs` | 新建：三个 handler | 9 |
| `src/gateway/handlers/mod.rs` · `method_admin.rs` · `method_census.rs` | 修改：注册 + 闸 + census | 9 |
| `interfaces/cli/src/commands/projects_cmd.rs` | 新建：`aleph projects` | 10 |
| `interfaces/cli/src/commands/cli_args.rs` · `mod.rs` | 修改：子命令接线 | 10 |
| `interfaces/webchat/src/api/projects_channel.rs` | 新建：Panel RPC 客户端 | 11 |
| `interfaces/webchat/src/components/project_page/settings.rs` | 修改：频道绑定区 | 11 |
| `src/builtin_tools/project_manage.rs` | 修改：加 `bind_workspace` 动词 | 12 |
| `qa/rooms_channel_bind/run.sh` | 新建：真机 QA | 13 |
| `docs/reference/FEATURE_LOCATOR.md` · `SECURITY.md` · `GATEWAY.md` · `src/gateway/CLAUDE.md` | 修改：回填 | 14 |

---

# Phase 0 — 断线与缺陷（先落地，后续阶段站在修好的地基上）

## Task 1: `AUTHOR_USER_KEY` 的第二个生产者

**Files:**
- Modify: `src/gateway/inbound_router/executor.rs:305-318`
- Create: `src/gateway/execution_engine/run_loop/author_census.rs`
- Modify: `src/gateway/execution_engine/run_loop/mod.rs`（挂 `mod author_census;`）

**Interfaces:**
- Consumes: `crate::gateway::execution_engine::AUTHOR_USER_KEY`（既有常量，`execution_engine/mod.rs:193`）
- Produces: 无新 API。副作用是频道 run 的 `request.metadata` 多一个键。

**背景**：`run_loop/mod.rs:100` 的 doc 逐字写着 `AUTHOR_USER_KEY` 有两个原产地（`build_run_request` 与频道 inbound router 的 `execute_for_context_inner`），而全仓只有 `handlers/agent.rs:828` 一个。后果是频道**群**回合的 `current_room_author()` 回落成会话属主 ⇒ guard 审计的 `actor_user` 与 `speaker_label` 都记错人。DM 因属主 == 说话人而恰好正确，只有群是错的。

- [ ] **Step 1: 写会红的守卫**

新建 `src/gateway/execution_engine/run_loop/author_census.rs`：

```rust
//! Source-level census over the producers of `AUTHOR_USER_KEY`.
//!
//! `run_loop::with_request_scope`'s doc names TWO origin sites for this key.
//! Before 2026-08-28 only one of them existed, and the doc was the only
//! external reference to the missing wire — grepping the key's name found the
//! comment that vouched for the absent producer, not the absence.
//!
//! This census makes that sentence self-enforcing: it first proves the run
//! loop really does seed `CURRENT_ROOM_AUTHOR` from the key, then requires
//! every named origin site to actually write it.

#[cfg(test)]
mod tests {
    /// Every file that must stamp `AUTHOR_USER_KEY`, and the function whose
    /// body has to contain the write. Named, not globbed: a producer that
    /// stops stamping must fail by name.
    const ORIGIN_SITES: &[(&str, &str, &str)] = &[
        (
            "src/gateway/handlers/agent.rs",
            include_str!("../../handlers/agent.rs"),
            "build_run_request",
        ),
        (
            "src/gateway/inbound_router/executor.rs",
            include_str!("../../../inbound_router/executor.rs"),
            "execute_for_context_inner",
        ),
    ];

    fn production_prefix(src: &str) -> String {
        // CRLF-safe: the repo is checked out with CRLF on Windows, so a
        // separator anchored to "\n#[cfg(test)]\n" never matches and the whole
        // file (tests included) would be scanned.
        let normalized = src.replace('\r', "");
        match normalized.find("#[cfg(test)]") {
            Some(at) => normalized[..at].to_string(),
            None => normalized,
        }
    }

    #[test]
    fn the_run_loop_seeds_the_room_author_from_the_author_key() {
        let src = production_prefix(include_str!("mod.rs"));
        assert!(
            src.contains("AUTHOR_USER_KEY"),
            "run_loop must read AUTHOR_USER_KEY — without this the census below \
             would be requiring producers for a key nobody consumes"
        );
        assert!(
            src.contains("with_room_author") || src.contains("CURRENT_ROOM_AUTHOR"),
            "run_loop must seed the room-author task-local from that key"
        );
    }

    #[test]
    fn every_named_origin_site_stamps_the_author_key() {
        let mut checked = 0usize;
        for (path, src, function) in ORIGIN_SITES {
            let prod = production_prefix(src);
            assert!(
                prod.contains(function),
                "{path}: the census names `{function}` but that function is not in \
                 the production half of the file — the census input rotted"
            );
            assert!(
                prod.contains("AUTHOR_USER_KEY"),
                "{path}: `run_loop::with_request_scope`'s doc names this file as an \
                 origin site for AUTHOR_USER_KEY, but nothing here stamps it. Either \
                 stamp it, or delete the claim from that doc — a doc comment naming \
                 a producer is not that producer."
            );
            checked += 1;
        }
        assert_eq!(
            checked,
            ORIGIN_SITES.len(),
            "the census must have inspected every origin site"
        );
    }
}
```

在 `src/gateway/execution_engine/run_loop/mod.rs` 顶部（其它 `mod` 声明旁）加：

```rust
mod author_census;
```

- [ ] **Step 2: 跑守卫确认它红**

```bash
cargo test -p alephcore --lib author_census -- --nocapture
```

Expected: `every_named_origin_site_stamps_the_author_key` **FAILED**，消息点名 `src/gateway/inbound_router/executor.rs`。
`the_run_loop_seeds_the_room_author_from_the_author_key` 应当 PASS（它描述的是既有事实）。
按四分类器判读：看到 `test result: FAILED` 才算 RED；只看到 `running 0 tests` 说明 `mod author_census;` 没挂上。

- [ ] **Step 3: 补上缺失的生产者**

在 `src/gateway/inbound_router/executor.rs` 的既有配对块里（`// P1 data isolation: stamp the run's owner/scope attribution ...` 那一段），把它改成同时写两个键：

```rust
        // P1 data isolation: stamp the run's owner/scope attribution from the
        // P0 sender→user link (`pairing_store`), not any task-local — channel
        // dispatch runs outside `process_request`'s task tree. An unlinked
        // peer (`None`) stamps nothing — legacy owner semantics.
        //
        // The same resolved principal is ALSO this turn's author. Scope names
        // the room, author names whoever is typing, and in a group they differ:
        // without the second stamp `run_loop::with_request_scope` falls back to
        // the session owner, so the guard audit's `actor_user` and
        // `nudges::speaker_label` both name whoever spoke FIRST in that group
        // rather than the person who just spoke. A DM is accidentally correct
        // (owner == speaker), which is why only groups were wrong.
        if let Some(user) = self
            .pairing_store
            .sender_user(ctx.message.channel_id.as_str(), &ctx.sender_normalized)
            .await
        {
            crate::scope::stamp_metadata(
                &mut metadata,
                &crate::scope::ScopeAttribution::personal(&user),
            );
            metadata.insert(
                crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
                user,
            );
        }
```

- [ ] **Step 4: 跑守卫确认它绿**

```bash
cargo test -p alephcore --lib author_census -- --nocapture
```

Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 破坏验证（必做）**

把 Step 3 新增的 `metadata.insert(...AUTHOR_USER_KEY...)` 三行注释掉，重跑 Step 4，确认 **RED 且消息点名 executor.rs**；然后恢复，`git diff` 确认逐字节回到 Step 3 的状态。

- [ ] **Step 6: 编译 + lint**

```bash
cargo test -p alephcore --lib --no-run && cargo clippy -p alephcore --lib --tests
```

Expected: 编译通过；clippy 无新增告警。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/inbound_router/executor.rs src/gateway/execution_engine/run_loop/
git commit -m "gateway: stamp the turn author on channel-originated runs

run_loop::with_request_scope's doc names two origin sites for
AUTHOR_USER_KEY; only build_run_request ever wrote it. Channel group
turns therefore fell back to the session owner, so the guard audit's
actor_user and the speaker label both named whoever spoke first in the
group rather than the person who just spoke. A DM was accidentally
correct because owner == speaker there.

The census makes that doc sentence self-enforcing rather than leaving it
as prose with no test.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 2: `caller_may_choose_directory()` 的 fail-OPEN 收窄

**Files:**
- Modify: `src/gateway/caller_identity.rs:150-154`

**Interfaces:**
- Produces: `pub fn caller_may_choose_directory_as(role: Option<&str>, is_loopback: bool) -> bool`
- Produces（不变）：`pub fn caller_may_choose_directory() -> bool`，改为上者的薄包装

**背景**：现形式对 `role == None` 恒真，而 `None` 正是 cron / A2A / 进程内 / **工具面**（task-local 过不了 spawn）的取值。修法是给它一个显式 actor 孪生，让工具面从**已经在裁决同一件事**的对象取值，而不是新造第二个推导。本任务只做拆分与守卫；工具面的消费在 Task 12。

- [ ] **Step 1: 写失败的测试**

在 `src/gateway/caller_identity.rs` 的 `mod tests` 里追加：

```rust
    /// The predicate's whole reason for existing is to separate a caller who
    /// may point the server at an arbitrary folder from one who may not. An
    /// arm that is constant-true is not a gate, and `None` — the value every
    /// spawned run sees — used to take exactly that arm.
    #[test]
    fn an_unknown_role_may_not_choose_a_directory_without_loopback() {
        assert!(
            !caller_may_choose_directory_as(None, false),
            "a caller with no connection role and no loopback must be refused: \
             None is what every tool call inside a spawned run sees, so admitting \
             it makes the gate constant-true exactly where it matters"
        );
    }

    #[test]
    fn the_three_admitted_shapes_are_unchanged() {
        assert!(
            caller_may_choose_directory_as(Some("operator"), false),
            "an operator-tier connection still chooses"
        );
        assert!(
            caller_may_choose_directory_as(None, true),
            "a loopback caller still chooses — this is the zero-config desktop \
             install, and narrowing it would break single-user deployments"
        );
        assert!(
            !caller_may_choose_directory_as(Some("guest"), false),
            "a chat-tier connection is still refused"
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p alephcore --lib caller_identity -- --nocapture
```

Expected: BUILD-ERROR（`caller_may_choose_directory_as` 未定义）。注意：这一步**没有** `test result:` 行是正常的——它是 BUILD-ERROR 而不是 VACUOUS。

- [ ] **Step 3: 实现孪生 + 把裸形式改成包装**

替换 `src/gateway/caller_identity.rs:150-154`：

```rust
/// Whether a caller may point the server at an arbitrary folder.
///
/// The ambient form reads the two task-locals; use it from an RPC handler,
/// where the gateway scopes them per request.
///
/// **Do not call it from inside a run.** `CALLER_ROLE` is dead past the
/// `tokio::spawn` every run crosses, so `role` is `None` there, and `None`
/// used to take the admitted arm — the gate was constant-true for exactly the
/// callers it most needed to judge (tool face, cron, A2A). A run's role is not
/// lost, it is stamped into `request.metadata` and enforced by
/// `ScopedToolService`; a tool face must ask that object and pass the answer to
/// [`caller_may_choose_directory_as`] rather than derive a second one.
#[must_use]
pub fn caller_may_choose_directory() -> bool {
    caller_may_choose_directory_as(
        current_caller_role().as_deref(),
        current_caller_is_loopback(),
    )
}

/// [`caller_may_choose_directory`] with the actor supplied explicitly.
///
/// `role` is the caller's connection tier: `Some("operator")` is config tier,
/// any other `Some` is chat tier, and `None` means "this surface could not
/// resolve a role at all" — refused unless the connection is loopback. That
/// refusal is the whole point of splitting this out: the ambient form's `None`
/// arm was admitting every spawned run.
#[must_use]
pub fn caller_may_choose_directory_as(role: Option<&str>, is_loopback: bool) -> bool {
    let is_config_tier = matches!(role, Some("operator"));
    is_config_tier || is_loopback
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo test -p alephcore --lib caller_identity -- --nocapture
```

Expected: `test result: ok`

- [ ] **Step 5: 跑全量确认没有别的消费者依赖旧的 fail-open 行为**

```bash
cargo test -p alephcore --lib --no-run && cargo test -p alephcore --lib projects -- --nocapture
```

Expected: 编译通过；`projects` 相关测试全绿。
⚠️ **如果有测试红了**：它大概率是在断言「无角色调用者可以选目录」——那正是本任务要改掉的行为。**先读那条测试的 doc**，确认它钉的是旧缺陷而不是一个承重的 fail-open 分支（cron 建房间之类）。若确属承重，**停下来报告**，不要给孪生加回落。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/caller_identity.rs
git commit -m "gateway: caller_may_choose_directory is no longer constant-true

The predicate admitted role == None, which is the value every spawned run
sees, so the gate was open for exactly the callers it needed to judge.
Split into an explicit-actor twin the tool face can feed from the run's
enforced role, with the ambient form as a thin wrapper so there is still
one derivation.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 3: `backfill_attribution` 的「唯一调用者」从散文升格为 census

**Files:**
- Modify: `src/gateway/session_store/mod.rs`（在文件末尾的 `#[cfg(test)] mod tests` 里追加；若无则新建）

**Interfaces:**
- Consumes: `SessionStore::backfill_attribution`（既有）
- Produces: 无 API。Task 7 会把 `rescope_attribution` 加进同一张表。

**背景**：`backfill_attribution` 的 doc 写着 "Its only caller is the legacy-room backfill"。那是一句没有测试的散文，而这一族的下一个成员（Task 7 的 `rescope_attribution`）依赖同样的保证。

- [ ] **Step 1: 写守卫**

在 `src/gateway/session_store/mod.rs` 末尾追加：

```rust
#[cfg(test)]
mod caller_census {
    //! The narrow-writer family: session-attribution verbs whose safety
    //! argument is "there is exactly one caller".
    //!
    //! Each entry's doc names its sole caller. A doc comment has no test, so
    //! this is where that sentence becomes enforceable: a second production
    //! call site must fail by name rather than quietly widen a verb whose
    //! whole justification was that it is unreachable from anywhere else.

    /// (verb, the one file allowed to call it).
    const SOLE_CALLERS: &[(&str, &str)] = &[(
        "backfill_attribution",
        "src/projects/attribution_backfill.rs",
    )];

    /// Every production `.rs` under `src/`, with comment lines stripped.
    ///
    /// Comments are stripped first because a doc that *explains* a verb
    /// mentions its name, and a scanner that counts those reports a violation
    /// for the very sentence documenting the rule.
    fn production_sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![std::path::PathBuf::from("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let normalized = raw.replace('\r', "");
                let prod = match normalized.find("#[cfg(test)]") {
                    Some(at) => &normalized[..at],
                    None => &normalized[..],
                };
                let stripped: String = prod
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push((path.to_string_lossy().replace('\\', "/"), stripped));
            }
        }
        out
    }

    #[test]
    fn each_narrow_writer_still_has_exactly_one_caller() {
        let sources = production_sources();
        assert!(
            sources.len() > 500,
            "the scanner found only {} files — it is not reading the tree it \
             thinks it is, and a shrinking sample reports 'all clear'",
            sources.len()
        );
        for (verb, owner) in SOLE_CALLERS {
            let needle = format!(".{verb}(");
            let callers: Vec<&str> = sources
                .iter()
                .filter(|(_, body)| body.contains(&needle))
                .map(|(path, _)| path.as_str())
                .filter(|path| !path.ends_with("session_store/mod.rs"))
                .filter(|path| !path.contains("session_store/file_backend"))
                .filter(|path| !path.contains("session_store/sqlite_backend"))
                .collect();
            assert_eq!(
                callers,
                vec![*owner],
                "`{verb}` documents `{owner}` as its ONLY caller. Found: {callers:?}. \
                 The trait declaration and the two backend impls are excluded (they \
                 define the verb, they do not call it). A new caller means the verb's \
                 safety argument changed — update the doc and this census together."
            );
        }
    }
}
```

- [ ] **Step 2: 跑守卫确认它绿（这一条是描述现状的，先绿再破坏）**

```bash
cargo test -p alephcore --lib caller_census -- --nocapture
```

Expected: `test result: ok. 1 passed`

- [ ] **Step 3: 破坏验证（必做）**

在 `src/gateway/handlers/projects.rs` 的任意生产函数体里临时插一行 `let _ = store.backfill_attribution;`（不需要能编译成一次真调用，只要文本包含 `.backfill_attribution(` 即可 —— 用 `let _ = |s: &dyn SessionStore| { let _ = s.backfill_attribution(todo!(), "", ""); };` 这种形式），重跑 Step 2。

Expected: **RED**，消息里列出 `src/gateway/handlers/projects.rs`。然后删掉该行，`git diff` 确认树干净。

- [ ] **Step 4: 提交**

```bash
git add src/gateway/session_store/mod.rs
git commit -m "session_store: pin the sole-caller claim with a census

backfill_attribution's doc says it has exactly one caller. That sentence
had no test, and Task 7 adds a sibling verb whose safety argument is the
same shape, so the claim needs to be enforceable before the family grows.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 4: `LoopState::scope_id` 零读者裁定升格为守卫

**Files:**
- Create: `src/looping/scope_census.rs`
- Modify: `src/looping/mod.rs`（挂 `mod scope_census;`）

**Interfaces:**
- Consumes: `crate::looping::LoopState`（既有；`scope_id` 字段）
- Produces: 无 API。

**背景**：`gateway/visibility.rs::stamped_owner_visible` 的 doc 明确裁决：房间**不**拥有成员的后台工作，`LoopState::scope_id` / `Goal::scope_id` / `CronJob::scope_id` 被**刻意不读**，其中 loop 的那份**一个读者都没有**，是「最容易被误认为断线」的那一个。这是一条只写在散文里的裁定，防不住下一个真诚的修复者。

⚠️ **这个守卫只覆盖 loop**。goal 与 cron 的 `scope_id` **有**执行侧读者（`goal_wait::rehydrate_owner_scope` / `cron::executor`），把它们一起写进来会立刻误报。

- [ ] **Step 1: 写守卫**

新建 `src/looping/scope_census.rs`：

```rust
//! `LoopState::scope_id` has no reader, on purpose.
//!
//! `gateway::visibility::stamped_owner_visible`'s doc rules that a project
//! room does not own its members' background work, and names this field as a
//! scope fact it deliberately leaves unread. The field is still written and
//! carried across state transitions so the decision stays reversible, which is
//! precisely what makes it look like a severed wire to the next reader.
//!
//! A ruling that lives only in prose does not survive a sincere fixer. This
//! census is that ruling with a failing test attached.
//!
//! Deliberately loop-only: `Goal::scope_id` and `CronJob::scope_id` DO have
//! execution-side readers (`goal_wait::rehydrate_owner_scope`,
//! `cron::executor`), so listing them here would fire on correct code.

#[cfg(test)]
mod tests {
    /// Files allowed to mention `LoopState`'s scope field: the type that
    /// declares it, and the persistence layer that round-trips it.
    const DECLARING_AND_PERSISTING: &[&str] = &[
        "src/looping/mod.rs",
        "src/looping/state.rs",
        "src/looping/store.rs",
        "src/looping/persistence.rs",
    ];

    fn production_sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![std::path::PathBuf::from("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let normalized = raw.replace('\r', "");
                let prod = match normalized.find("#[cfg(test)]") {
                    Some(at) => &normalized[..at],
                    None => &normalized[..],
                };
                let stripped: String = prod
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push((path.to_string_lossy().replace('\\', "/"), stripped));
            }
        }
        out
    }

    #[test]
    fn no_consumer_reads_the_loop_scope_id() {
        let sources = production_sources();
        assert!(
            sources.len() > 500,
            "the scanner found only {} files — a shrinking sample reports 'all clear'",
            sources.len()
        );
        let offenders: Vec<&str> = sources
            .iter()
            .filter(|(path, body)| {
                !DECLARING_AND_PERSISTING.contains(&path.as_str())
                    && !path.ends_with("looping/scope_census.rs")
                    && body.contains("scope_id")
                    && (body.contains("LoopState") || path.starts_with("src/looping/"))
            })
            .map(|(path, _)| path.as_str())
            .collect();
        assert!(
            offenders.is_empty(),
            "`LoopState::scope_id` is deliberately unread (see \
             gateway::visibility::stamped_owner_visible's doc: a room does not own \
             its members' background work — ruled 2026-08-07). New readers here: \
             {offenders:?}. Widening that boundary needs a product ruling first, and \
             then ONE scope-aware sibling in visibility.rs — never an inlined \
             membership check at a call site."
        );
    }
}
```

在 `src/looping/mod.rs` 加 `mod scope_census;`。

- [ ] **Step 2: 跑守卫**

```bash
cargo test -p alephcore --lib scope_census -- --nocapture
```

Expected: `test result: ok. 1 passed`
⚠️ **如果它红了**：先读它点名的文件——`DECLARING_AND_PERSISTING` 那张表是按当前 `src/looping/` 的文件名写的，实际文件名不同就是这张表错了，不是有新读者。改表，不改裁定。

- [ ] **Step 3: 破坏验证（必做）**

在 `src/gateway/visibility.rs` 的任意生产函数里临时加一行 `let _loop_scope_reader: Option<&crate::looping::LoopState> = None; // scope_id`，重跑 Step 2，确认 **RED 且点名 visibility.rs**；删除并确认树干净。

- [ ] **Step 4: 提交**

```bash
git add src/looping/
git commit -m "looping: pin the deliberate no-reader ruling on LoopState::scope_id

visibility::stamped_owner_visible rules that a project room does not own
its members' background work and names this field as one it leaves unread
on purpose. The field is still written, which is exactly what makes it
read as a severed wire. A ruling that lives only in prose does not survive
a sincere fixer.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 1 — 存储与读路径

## Task 5: `project_channel_bindings` 表 + 读路径

**Files:**
- Create: `src/projects/binding.rs`
- Modify: `src/projects/store.rs`（`SCHEMA` 之后新增 DDL；三个方法；`create_schema` 挂 DDL）
- Modify: `src/projects/mod.rs`（`pub mod binding;` + re-export）

**Interfaces:**
- Produces: `crate::projects::binding::ChannelBinding { project_id, channel_id, peer_kind, peer_id, bound_by, bound_at, label }`
- Produces: `crate::projects::binding::conversation_of(&SessionKey) -> Option<(String, &'static str, String)>`
- Produces: `ProjectStore::bind_conversation(project_id, channel, peer_kind, peer_id, bound_by, label) -> Result<ChannelBinding, ProjectError>`
- Produces: `ProjectStore::unbind_conversation(channel, peer_kind, peer_id) -> Result<bool, ProjectError>`
- Produces: `ProjectStore::project_for_conversation(channel, peer_kind, peer_id) -> Result<Option<String>, ProjectError>`
- Produces: `ProjectStore::bindings_for(project_id) -> Result<Vec<ChannelBinding>, ProjectError>`

- [ ] **Step 1: 写失败的测试**

新建 `src/projects/binding.rs`：

```rust
//! Which channel conversation belongs to which project room.
//!
//! Keyed on the CONVERSATION — `(channel, peer_kind, peer_id)` — and not on the
//! session key, because a session key carries the agent id: an `agent_switch`
//! would mint a different key and silently un-bind the room while every
//! surface kept showing it as bound.
//!
//! `peer_kind` is part of the key because `SessionKey::Group` carries both
//! `PeerKind::Group` and `PeerKind::Thread`, whose `peer_id` namespaces are not
//! guaranteed disjoint.

use crate::routing::session_key::{PeerKind, SessionKey};

/// One room ⟷ conversation binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBinding {
    pub project_id: String,
    pub channel_id: String,
    /// `"group"` or `"thread"` — see [`peer_kind_str`].
    pub peer_kind: String,
    pub peer_id: String,
    /// The operator who bound it. `None` only for an unrestricted in-process
    /// caller; the RPC face always resolves one.
    pub bound_by: Option<String>,
    pub bound_at: i64,
    /// Human label for the conversation, as the operator named it. Purely for
    /// rendering — never used to address anything.
    pub label: Option<String>,
}

/// Stable storage spelling for a peer kind.
///
/// Written out rather than reusing serde so the column's contents cannot drift
/// with a `rename_all` change on the wire type.
#[must_use]
pub const fn peer_kind_str(kind: PeerKind) -> &'static str {
    match kind {
        PeerKind::Group => "group",
        PeerKind::Thread => "thread",
    }
}

/// The conversation a session key addresses, when it addresses one.
///
/// `None` for every other key shape — a DM, a task, a subagent and a main
/// session are not conversations a room can be bound to.
#[must_use]
pub fn conversation_of(key: &SessionKey) -> Option<(String, &'static str, String)> {
    match key {
        SessionKey::Group {
            channel,
            peer_kind,
            peer_id,
            ..
        } => Some((
            channel.clone(),
            peer_kind_str(*peer_kind),
            peer_id.clone(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_key_resolves_to_its_conversation() {
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C0A1");
        let (channel, kind, peer) = conversation_of(&key).expect("a group key is a conversation");
        assert_eq!(channel, "telegram");
        assert_eq!(kind, "group");
        assert_eq!(peer, "C0A1");
    }

    #[test]
    fn the_agent_id_is_not_part_of_the_conversation() {
        let a = SessionKey::group("main", "telegram", PeerKind::Group, "C0A1");
        let b = SessionKey::group("coder", "telegram", PeerKind::Group, "C0A1");
        assert_eq!(
            conversation_of(&a),
            conversation_of(&b),
            "agent_switch must not un-bind a room: the binding is on the \
             conversation, not on the session key"
        );
    }

    #[test]
    fn a_dm_key_is_not_bindable() {
        let key = SessionKey::dm(
            "main",
            "telegram",
            "u123",
            crate::routing::session_key::DmScope::PerPeer,
        );
        assert!(
            conversation_of(&key).is_none(),
            "a DM has exactly one human on the far side; binding it to a room \
             would put a shared partition behind a private conversation"
        );
    }
}
```

在 `src/projects/mod.rs` 加：

```rust
pub mod binding;
pub use binding::{peer_kind_str, ChannelBinding};
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p alephcore --lib projects::binding -- --nocapture
```

Expected: BUILD-ERROR 或 RED（取决于 `SessionKey::dm` 的确切签名）。
⚠️ 若 `SessionKey::dm` 签名不同，读 `src/routing/session_key.rs` 改测试的构造，**不要**改 `conversation_of`。

- [ ] **Step 3: 让类型测试通过（`binding.rs` 已写完，只需编译过）**

```bash
cargo test -p alephcore --lib projects::binding -- --nocapture
```

Expected: `test result: ok. 3 passed`

- [ ] **Step 4: 写 store 层的失败测试**

在 `src/projects/store.rs` 的 `#[cfg(test)] mod tests` 里追加：

```rust
    /// A binding is keyed on the conversation, so a second room cannot claim a
    /// conversation the first one already holds. The refusal must be loud: a
    /// silent overwrite would move an existing room's traffic somewhere else.
    #[test]
    fn a_conversation_belongs_to_at_most_one_room() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        let b = store.create("room b", Some("u-alice"), None).unwrap();

        store
            .bind_conversation(&a.id, "telegram", "group", "C0A1", Some("u-alice"), None)
            .expect("the first bind succeeds");
        let second = store.bind_conversation(&b.id, "telegram", "group", "C0A1", Some("u-alice"), None);
        assert!(
            matches!(second, Err(ProjectError::Invalid(_))),
            "the second room must be refused, not silently take the conversation over"
        );
        assert_eq!(
            store.project_for_conversation("telegram", "group", "C0A1").unwrap(),
            Some(a.id.clone()),
            "the original binding must survive the refused attempt"
        );
    }

    /// Unbinding is idempotent and reports whether anything actually changed —
    /// "nothing was bound" and "I unbound it" are different answers, and a
    /// caller that renders a receipt needs to tell them apart.
    #[test]
    fn unbinding_reports_whether_it_changed_anything() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(&a.id, "slack", "group", "C9", Some("u-alice"), None)
            .unwrap();

        assert!(store.unbind_conversation("slack", "group", "C9").unwrap());
        assert!(!store.unbind_conversation("slack", "group", "C9").unwrap());
        assert_eq!(
            store.project_for_conversation("slack", "group", "C9").unwrap(),
            None
        );
    }

    /// One room may live in several conversations (Telegram + Slack). The
    /// uniqueness constraint is on the conversation side only — that is what
    /// "one core, many channels" costs here.
    #[test]
    fn a_room_may_be_bound_to_several_conversations() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(&a.id, "telegram", "group", "C1", Some("u-alice"), None)
            .unwrap();
        store
            .bind_conversation(&a.id, "slack", "group", "C2", Some("u-alice"), Some("#eng"))
            .unwrap();
        let bound = store.bindings_for(&a.id).unwrap();
        assert_eq!(bound.len(), 2);
        assert_eq!(bound[1].label.as_deref(), Some("#eng"));
    }

    /// A catalogue created before this table existed must still open. The
    /// isolated test HOME only ever builds the newest shape, so the old one has
    /// to be constructed on purpose — the same reason
    /// `a_pre_rooms_catalogue_still_opens` exists two tables up.
    #[test]
    fn a_pre_binding_catalogue_still_opens_and_binds() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 owner_user_id TEXT,
                 workspace_path TEXT,
                 status TEXT NOT NULL DEFAULT 'active',
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 last_used_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        let store = ProjectStore::new(conn);
        store
            .create_schema()
            .expect("a catalogue predating the binding table must migrate, not fail to open");
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(&a.id, "telegram", "group", "C0", Some("u-alice"), None)
            .expect("binding must work on a migrated catalogue");
    }
```

- [ ] **Step 5: 跑测试确认失败**

```bash
cargo test -p alephcore --lib projects::store -- --nocapture
```

Expected: BUILD-ERROR（四个方法未定义）。

- [ ] **Step 6: 实现 store 层**

在 `src/projects/store.rs` 的 `SCHEMA` 常量**下方**加：

```rust
/// The room ⟷ conversation binding table.
///
/// Kept out of [`SCHEMA`] for the same reason `workspace_uniqueness_ddl` is:
/// it is created together with its own index, and both must be applied AFTER
/// the column migrations above. It references no column added by a migration,
/// so it is safe against a pre-rooms catalogue — but the index is written here
/// rather than in `SCHEMA` so that the table and the constraint that gives it
/// its meaning can never be applied apart.
const BINDING_DDL: &str = "
CREATE TABLE IF NOT EXISTS project_channel_bindings (
    project_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    peer_kind  TEXT NOT NULL,
    peer_id    TEXT NOT NULL,
    bound_by   TEXT,
    bound_at   INTEGER NOT NULL,
    label      TEXT,
    PRIMARY KEY (channel_id, peer_kind, peer_id)
);
CREATE INDEX IF NOT EXISTS idx_project_channel_bindings_project
    ON project_channel_bindings(project_id);
";
```

在 `create_schema` 的 `conn.execute_batch(&workspace_uniqueness_ddl())` 之后、`add_current_session_key_column` 之前插一行：

```rust
            conn.execute_batch(BINDING_DDL).map_err(db_err)?;
```

在 `project_for_session_key` 方法之后加四个方法：

```rust
    /// Bind a conversation to a room.
    ///
    /// The conversation side is the primary key, so a conversation another room
    /// already holds is refused with [`ProjectError::Invalid`] rather than
    /// silently taken over — an overwrite would move a live room's traffic
    /// somewhere its members cannot see.
    ///
    /// Re-binding a conversation to the SAME room is a no-op that succeeds and
    /// refreshes the label: an operator repeating a bind must not be told they
    /// broke something.
    pub fn bind_conversation(
        &self,
        project_id: &str,
        channel_id: &str,
        peer_kind: &str,
        peer_id: &str,
        bound_by: Option<&str>,
        label: Option<&str>,
    ) -> Result<ChannelBinding, ProjectError> {
        let now = now_secs();
        self.with_conn(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT project_id FROM project_channel_bindings
                     WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                    rusqlite::params![channel_id, peer_kind, peer_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?;
            if let Some(owner) = existing.as_deref() {
                if owner != project_id {
                    return Err(ProjectError::Invalid(format!(
                        "{channel_id}:{peer_id} is already bound to project {owner}; \
                         unbind it there first"
                    )));
                }
            }
            let exists: Option<String> = conn
                .query_row(
                    "SELECT id FROM projects WHERE id = ?1 AND status = 'active'",
                    [project_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?;
            if exists.is_none() {
                return Err(ProjectError::NotFound(project_id.to_string()));
            }
            conn.execute(
                "INSERT INTO project_channel_bindings
                     (project_id, channel_id, peer_kind, peer_id, bound_by, bound_at, label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(channel_id, peer_kind, peer_id) DO UPDATE SET
                     label = excluded.label,
                     bound_by = excluded.bound_by,
                     bound_at = excluded.bound_at",
                rusqlite::params![
                    project_id, channel_id, peer_kind, peer_id, bound_by, now, label
                ],
            )
            .map_err(db_err)?;
            Ok(ChannelBinding {
                project_id: project_id.to_string(),
                channel_id: channel_id.to_string(),
                peer_kind: peer_kind.to_string(),
                peer_id: peer_id.to_string(),
                bound_by: bound_by.map(str::to_string),
                bound_at: now,
                label: label.map(str::to_string),
            })
        })
    }

    /// Release a conversation. `Ok(false)` means nothing was bound — a distinct
    /// answer from `Ok(true)`, because a receipt that says "unbound" about a
    /// conversation that never was is a client asserting a result it did not
    /// observe.
    pub fn unbind_conversation(
        &self,
        channel_id: &str,
        peer_kind: &str,
        peer_id: &str,
    ) -> Result<bool, ProjectError> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM project_channel_bindings
                     WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                    rusqlite::params![channel_id, peer_kind, peer_id],
                )
                .map_err(db_err)?;
            Ok(n > 0)
        })
    }

    /// The room a conversation belongs to, if any.
    ///
    /// Sibling of [`Self::project_for_session_key`]: both answer "which room
    /// owns this turn" and both must stay a cheap indexed lookup, because
    /// `run_loop::room_claiming` calls them on every run.
    pub fn project_for_conversation(
        &self,
        channel_id: &str,
        peer_kind: &str,
        peer_id: &str,
    ) -> Result<Option<String>, ProjectError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT project_id FROM project_channel_bindings
                 WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                rusqlite::params![channel_id, peer_kind, peer_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)
        })
    }

    /// Every conversation a room is bound to, oldest first.
    pub fn bindings_for(&self, project_id: &str) -> Result<Vec<ChannelBinding>, ProjectError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT channel_id, peer_kind, peer_id, bound_by, bound_at, label
                     FROM project_channel_bindings WHERE project_id = ?1
                     ORDER BY bound_at, channel_id, peer_id",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([project_id], |r| {
                    Ok(ChannelBinding {
                        project_id: project_id.to_string(),
                        channel_id: r.get(0)?,
                        peer_kind: r.get(1)?,
                        peer_id: r.get(2)?,
                        bound_by: r.get(3)?,
                        bound_at: r.get(4)?,
                        label: r.get(5)?,
                    })
                })
                .map_err(db_err)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(db_err)?);
            }
            Ok(out)
        })
    }
```

在 `store.rs` 顶部的 `use` 里补 `use super::binding::ChannelBinding;`（或 `use crate::projects::binding::ChannelBinding;`，与文件既有风格一致者）。

- [ ] **Step 7: 跑测试确认通过**

```bash
cargo test -p alephcore --lib projects:: -- --nocapture
```

Expected: 四条新测试全 PASS，既有 projects 测试不红。
⚠️ **`TEST_GUARD` 必须持有**：`roster::publish` 会替换进程全局快照，不持锁的并行测试会互相抹掉对方的投影。若 `TEST_GUARD` 不是 `pub`，把它改成 `pub(crate)` 并在 doc 里说明理由。

- [ ] **Step 8: 提交**

```bash
git add src/projects/
git commit -m "projects: store the room to channel-conversation binding

Keyed on (channel, peer_kind, peer_id) rather than on the session key,
because a session key carries the agent id: agent_switch would mint a
different key and silently un-bind the room while every surface kept
showing it as bound. Uniqueness is on the conversation side only, so one
room can live in several conversations.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 2 — 判定汇流与名册闸

## Task 6: `room_claiming` 第二臂 + 升格闸

**Files:**
- Modify: `src/gateway/execution_engine/run_loop/mod.rs:68-95`
- Test: `src/gateway/execution_engine/run_loop/tests.rs`

**Interfaces:**
- Consumes: `ProjectStore::project_for_conversation`（Task 5）· `projects::binding::conversation_of`（Task 5）· `visibility::project_visible_to`（既有）
- Produces: 行为变化。`request_scope` 对已房间化的 stamp 逐字节不变。

- [ ] **Step 1: 写失败的测试**

在 `src/gateway/execution_engine/run_loop/tests.rs` 末尾追加：

```rust
/// Helper: a request whose metadata carries `attr` and whose key is a channel
/// group conversation.
fn channel_group_request(attr: &crate::scope::ScopeAttribution, peer: &str) -> RunRequest {
    let mut metadata = std::collections::HashMap::new();
    crate::scope::stamp_metadata(&mut metadata, attr);
    RunRequest {
        session_key: crate::routing::session_key::SessionKey::group(
            "main",
            "telegram",
            crate::routing::session_key::PeerKind::Group,
            peer,
        ),
        metadata,
        ..run_request_fixture()
    }
}

#[test]
fn a_bound_conversation_upgrades_a_roster_member_to_the_room_scope() {
    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let store = crate::projects::ProjectStore::shared();
    store.create_schema().unwrap();
    let room = store.create("room", Some("u-alice"), None).unwrap();
    store.add_member(&room.id, "u-alice").unwrap();
    store
        .bind_conversation(&room.id, "telegram", "group", "C-up", Some("u-alice"), None)
        .unwrap();

    let attr = crate::scope::ScopeAttribution::personal("u-alice");
    let resolved = super::request_scope(&channel_group_request(&attr, "C-up"))
        .expect("a stamped run resolves a scope");
    assert_eq!(
        resolved.scope,
        crate::scope::ScopeId::Project(room.id.clone()),
        "a roster member speaking in a bound group takes the room scope"
    );
    assert_eq!(
        resolved.owner_user_id, "u-alice",
        "the owner still names whoever spoke — overwriting it would lose the byline"
    );
}

#[test]
fn a_bound_conversation_does_not_upgrade_a_non_member() {
    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let store = crate::projects::ProjectStore::shared();
    store.create_schema().unwrap();
    let room = store.create("room", Some("u-alice"), None).unwrap();
    store.add_member(&room.id, "u-alice").unwrap();
    store
        .bind_conversation(&room.id, "telegram", "group", "C-out", Some("u-alice"), None)
        .unwrap();

    let attr = crate::scope::ScopeAttribution::personal("u-bob");
    let resolved = super::request_scope(&channel_group_request(&attr, "C-out"))
        .expect("a stamped run resolves a scope");
    assert_eq!(
        resolved.scope,
        crate::scope::ScopeId::Personal("u-bob".to_string()),
        "being in the Telegram group must not be equivalent to being on the roster: \
         the channel path has no session_visible admission check, so this is the \
         only place that answers it"
    );
}

#[test]
fn an_unpaired_speaker_in_a_bound_conversation_takes_no_room_scope() {
    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let store = crate::projects::ProjectStore::shared();
    store.create_schema().unwrap();
    let room = store.create("room", Some("u-alice"), None).unwrap();
    store.add_member(&room.id, "u-alice").unwrap();
    store
        .bind_conversation(&room.id, "telegram", "group", "C-anon", Some("u-alice"), None)
        .unwrap();

    let mut request = channel_group_request(
        &crate::scope::ScopeAttribution::personal("ignored"),
        "C-anon",
    );
    request.metadata.clear(); // an unpaired sender stamps nothing at all
    assert!(
        super::request_scope(&request).is_none(),
        "an unstamped turn must resolve no scope: this is what keeps a stranger \
         out of the room partition AND out of RoomRosterLayer, which reads the \
         same task-local. It is true today by derivation, not by guard — hence \
         this test."
    );
}

#[test]
fn a_producer_that_already_stamped_the_room_is_left_alone() {
    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let store = crate::projects::ProjectStore::shared();
    store.create_schema().unwrap();
    let room = store.create("room", Some("u-alice"), None).unwrap();
    // Deliberately NOT on the roster: this is the cron/A2A shape that
    // `resolve_attribution`'s Path 2 produces (owner = OWNER_USER_ID, scope =
    // Project) after its own admission check already passed.
    store
        .bind_conversation(&room.id, "telegram", "group", "C-cron", Some("u-alice"), None)
        .unwrap();

    let attr = crate::scope::ScopeAttribution {
        owner_user_id: crate::gateway::security::store::OWNER_USER_ID.to_string(),
        scope: crate::scope::ScopeId::Project(room.id.clone()),
    };
    let resolved = super::request_scope(&channel_group_request(&attr, "C-cron"))
        .expect("a stamped run resolves a scope");
    assert_eq!(
        resolved.scope,
        crate::scope::ScopeId::Project(room.id),
        "the roster gate answers 'is this DERIVED room-scoping trustworthy'. A run \
         whose producer already stamped the room went through admission; re-judging \
         it would silently demote an admitted room run to a personal one."
    );
}
```

⚠️ `run_request_fixture()` / `store.add_member` 的确切名字以 `run_loop/tests.rs` 与 `projects/store.rs` 现有代码为准；不存在就照现有夹具风格补一个本地 helper，**不要**改生产签名。

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p alephcore --lib run_loop:: -- --nocapture
```

Expected: `a_bound_conversation_upgrades_a_roster_member_to_the_room_scope` 与 `a_bound_conversation_does_not_upgrade_a_non_member` **FAILED**；另两条应当 PASS（它们描述的是既有行为）。
**这是 4 条里恰好 2 条红** —— 若红的条数不是 2，先怀疑自己的判断，去读 `request_scope` 的当前实现。

- [ ] **Step 3: 实现**

替换 `src/gateway/execution_engine/run_loop/mod.rs` 的 `request_scope` 与 `room_claiming`：

```rust
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    let stamped = crate::scope::scope_from_metadata(&request.metadata);
    let Some(pid) = room_claiming(&request.session_key) else {
        return stamped;
    };
    let mut attr = stamped?;
    let target = crate::scope::ScopeId::Project(pid.clone());
    // Already room-scoped by its producer: that stamp came through an admission
    // path (`handlers::agent::resolve_attribution`) that can refuse, and its
    // owner may legitimately be `OWNER_USER_ID` — an unrestricted caller such as
    // cron opening the room's session. Re-judging it here against the roster
    // would silently demote an admitted room run to a personal one.
    if attr.scope == target {
        return Some(attr);
    }
    // An UPGRADE derived from a conversation binding. The Panel path never
    // reaches here (its stamp already equals `target`), so this gate is a
    // byte-for-byte no-op there and costs nothing; the channel path has no
    // equivalent of the `session_visible` check that admits a caller to a room
    // key, so being in the Telegram group would otherwise be equivalent to
    // being on the roster.
    //
    // `project_visible_to` is a synchronous read of the in-process roster
    // projection and has NO error arm: "no such room", "not a member" and "this
    // process has not published a projection yet" all read as false. Unknown
    // therefore fails closed, which is the direction this gate wants. (Ruling
    // P9 — "an unreadable roster activates rather than lurks" — is about
    // `ProjectStore::members()`, a different read with a real `Result`. Do not
    // go looking for an error arm here; there isn't one.)
    if !crate::gateway::visibility::project_visible_to(&pid, Some(&attr.owner_user_id)) {
        return Some(attr);
    }
    attr.scope = target;
    Some(attr)
}

/// The project that owns `session_key`'s turn, by either of the two ways a room
/// can claim a conversation.
///
/// Twin of `handlers::agent::room_claiming`, deliberately not shared with it:
/// that one lives on the admission path and its `None` feeds a branch that may
/// refuse, this one lives after admission and its `None` means "leave the
/// producer's stamp alone". Both read the same columns through the same store
/// methods, which is the part that must not be duplicated.
fn room_claiming(session_key: &crate::routing::session_key::SessionKey) -> Option<String> {
    let store = crate::projects::ProjectStore::shared();
    // (1) The Panel-minted room conversation, claimed by `projects.room_session`.
    let claimed = match store.project_for_session_key(&session_key.to_key_string()) {
        Ok(pid) => pid,
        Err(e) => {
            tracing::warn!(error = %e, "projects: room claim lookup failed; leaving the producer's scope stamp alone");
            None
        }
    };
    // (2) A channel conversation bound to a room. Keyed on the conversation, so
    // an `agent_switch` (which changes the session key's agent component) does
    // not un-bind it.
    let bound = match crate::projects::binding::conversation_of(session_key) {
        None => None,
        Some((channel, kind, peer)) => match store.project_for_conversation(&channel, kind, &peer) {
            Ok(pid) => pid,
            Err(e) => {
                tracing::warn!(error = %e, "projects: conversation binding lookup failed; leaving the producer's scope stamp alone");
                None
            }
        },
    };
    match (claimed, bound) {
        (Some(a), Some(b)) if a != b => {
            tracing::warn!(
                claimed = %a,
                bound = %b,
                "projects: a session key is claimed by one room and its conversation is bound to another; \
                 taking the explicit claim"
            );
            Some(a)
        }
        (Some(a), _) => Some(a),
        (None, b) => b,
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo test -p alephcore --lib run_loop:: -- --nocapture
```

Expected: 4 条新测试全 PASS，既有 `run_loop` 测试不红。

- [ ] **Step 5: 破坏验证（必做，两次）**

1. 把名册闸那三行 `if !project_visible_to(...) { return Some(attr); }` 删掉，重跑 → 期望 `a_bound_conversation_does_not_upgrade_a_non_member` RED。恢复。
2. 把 `if attr.scope == target { return Some(attr); }` 删掉，重跑 → 期望 `a_producer_that_already_stamped_the_room_is_left_alone` RED。恢复。
`git diff` 确认树逐字节回到 Step 3。

- [ ] **Step 6: 编译 + lint**

```bash
cargo test -p alephcore --lib --no-run && cargo clippy -p alephcore --lib --tests
```

- [ ] **Step 7: 提交**

```bash
git add src/gateway/execution_engine/run_loop/
git commit -m "run_loop: resolve a bound channel conversation to its room

room_claiming gains a second arm keyed on the conversation, and both arms
converge on one Option<project_id> so nothing downstream changes.

The upgrade now passes a roster gate. The Panel path never reaches it —
its stamp already equals the target — so the gate is a no-op there, but
the channel path has no equivalent of the session_visible check that
admits a caller to a room key, and without this being in the group would
be equivalent to being on the roster. A producer that already stamped the
room is left alone: that stamp came through an admission path that can
refuse, and its owner may legitimately be the legacy owner.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 3 — 存量会话行

## Task 7: `rescope_attribution`

**Files:**
- Modify: `src/gateway/session_store/mod.rs`（trait 方法 + census 加一行）
- Modify: `src/gateway/session_store/file_backend/mod.rs`
- Modify: `src/gateway/session_store/sqlite_backend/mod.rs`

**Interfaces:**
- Produces: `async fn SessionStore::rescope_attribution(&self, key: &SessionKey, scope_id: &str) -> Result<bool, SessionStoreError>`（trait 默认 `Err(Unsupported)`）
- Consumes（Task 9）：bind handler 是唯一调用者

- [ ] **Step 1: 写失败的测试**

在 `src/gateway/session_store/file_backend/mod.rs` 的测试模块追加（sqlite backend 同形复制一份，改构造）：

```rust
    /// The one exception to "session scope is immutable once set": an operator
    /// binding a channel conversation to a room. Everything else must keep
    /// getting the create-only behaviour.
    #[tokio::test]
    async fn rescoping_a_group_row_to_a_room_moves_only_the_scope() {
        let dir = crate::utils::scratch::temp_dir();
        let store = FileSessionStore::new(dir.path().to_path_buf());
        let key = crate::routing::session_key::SessionKey::group(
            "main",
            "telegram",
            crate::routing::session_key::PeerKind::Group,
            "C1",
        );
        crate::scope::with_scope_sync(
            &crate::scope::ScopeAttribution::personal("u-alice"),
            || futures::executor::block_on(store.get_or_create(&key)),
        )
        .unwrap();

        let changed = store
            .rescope_attribution(&key, "project:p-1")
            .await
            .expect("a file backend supports rescoping");
        assert!(changed, "the row moved");

        let meta = store.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(meta.scope_id.as_deref(), Some("project:p-1"));
        assert_eq!(
            meta.owner_user_id.as_deref(),
            Some("u-alice"),
            "the owner still names whoever spoke first — the room's visibility is \
             decided by the roster, so overwriting the owner would only lose the byline"
        );
    }

    /// A non-group key must be refused. Rescoping is a visibility grant, and a
    /// DM has exactly one human on the far side: there is no roster to grant to.
    #[tokio::test]
    async fn rescoping_refuses_a_key_that_is_not_a_conversation() {
        let dir = crate::utils::scratch::temp_dir();
        let store = FileSessionStore::new(dir.path().to_path_buf());
        let key = crate::routing::session_key::SessionKey::main("main");
        let result = store.rescope_attribution(&key, "project:p-1").await;
        assert!(
            result.is_err(),
            "only a group conversation may be rescoped into a room"
        );
    }
```

⚠️ `temp_dir` / `with_scope_sync` / `SessionKey::main` 的确切名字以现有测试为准；照抄同文件里已有测试的构造方式。

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p alephcore --lib session_store::file_backend -- --nocapture
```

Expected: BUILD-ERROR（`rescope_attribution` 未定义）。

- [ ] **Step 3: 在 trait 上加默认实现**

在 `src/gateway/session_store/mod.rs` 的 `backfill_attribution` 之后：

```rust
    /// Move an EXISTING session row's scope into a project room.
    ///
    /// This is the one exception to spec §10 ("session scope is immutable once
    /// set"), and it is deliberately narrow. `stamp_attribution` stays
    /// create-only for every other path; this verb exists because binding a
    /// channel conversation to a room is a decision a human makes, with a
    /// reason, that is recorded in the audit log — while `backfill_attribution`
    /// can only heal rows that were never stamped at all.
    ///
    /// Without it a bound conversation splits in two: the RUN takes the room
    /// scope (memory partition, roster, room context) while the ROW keeps
    /// `personal:<first speaker>`, so every other member's `session_visible_to`
    /// says false and the group stays invisible in their session list. Row and
    /// loop answering differently is exactly the machine round-8 ⑤ names.
    ///
    /// `key` must be a `SessionKey::Group`. A DM has one human on the far side
    /// and no roster to grant visibility to; refusing here rather than at the
    /// call site makes "only a conversation may be rescoped" a property of the
    /// verb instead of a rule each caller has to remember.
    ///
    /// Its only caller is the channel-binding handler
    /// (`handlers::projects_channel::handle_bind`), pinned by
    /// `session_store::caller_census`.
    ///
    /// `Ok(false)` means there was no such row — a conversation nobody has
    /// spoken in yet, which is the common case for a freshly bound group and
    /// is not an error. Default is `Unsupported` rather than `Ok(false)` for
    /// the same reason `backfill_attribution`'s is: a store that cannot do this
    /// has not "found nothing to move".
    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        let _ = (key, scope_id);
        Err(SessionStoreError::Unsupported)
    }
```

在 Task 3 的 `SOLE_CALLERS` 表里加一行：

```rust
        (
            "rescope_attribution",
            "src/gateway/handlers/projects_channel.rs",
        ),
```

⚠️ 这会让 Task 3 的守卫**立刻变红**（那个文件还不存在，Task 9 才建）。这是正确的顺序信号：Task 7 与 Task 9 必须在同一轮内完成。若要保持每个提交都绿，把这一行留到 Task 9 再加，并在 Task 7 的提交信息里写明它欠着。

- [ ] **Step 4: 两个 backend 实现**

`file_backend` 与 `sqlite_backend` 各自实现，形状一致：

```rust
    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        if !matches!(key, SessionKey::Group { .. }) {
            return Err(SessionStoreError::Unsupported);
        }
        if crate::scope::ScopeId::parse(scope_id)
            .filter(|s| matches!(s, crate::scope::ScopeId::Project(_)))
            .is_none()
        {
            return Err(SessionStoreError::Unsupported);
        }
        // ... backend-specific: load metadata; if absent return Ok(false);
        //     set `scope_id`, leave `owner_user_id` untouched; persist; Ok(true)
    }
```

`file_backend` 的持久化必须走该文件已有的 `MetaGuard` / `MetaLocks::lock` 写入口（读-改-写是一个临界区，父模块够不到裸写函数）。`sqlite_backend` 用一条 `UPDATE ... SET scope_id = ?2 WHERE session_key = ?1`，并按 `rows_affected > 0` 返回。

- [ ] **Step 5: 跑测试确认通过**

```bash
cargo test -p alephcore --lib session_store:: -- --nocapture
```

Expected: 四条新测试（两个 backend 各两条）全 PASS。

- [ ] **Step 6: 破坏验证（必做）**

把 `if !matches!(key, SessionKey::Group { .. })` 那条守卫删掉，重跑 → 期望 `rescoping_refuses_a_key_that_is_not_a_conversation` RED（两个 backend 各一条）。恢复。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/session_store/
git commit -m "session_store: add the narrow rescope verb for room binding

Binding an existing channel conversation to a room otherwise splits it in
two: the run takes the room scope while the row keeps
personal:<first speaker>, so every other member's session_visible_to says
false and the group stays invisible in their list.

The verb refuses anything that is not a group key targeting a project
scope, so 'only a conversation may be rescoped' is a property of the verb
rather than a rule each caller has to remember. It is the single
documented exception to create-only stamping.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 4 — 面

## Task 8: `aleph_protocol::projects` 契约

**Files:**
- Create: `shared/protocol/src/projects.rs`
- Modify: `shared/protocol/src/lib.rs`（`pub mod projects;`）

**Interfaces:**
- Produces: `ChannelBindParams { project_id, channel_id, peer_kind, peer_id, label }`
- Produces: `ChannelUnbindParams { channel_id, peer_kind, peer_id }`
- Produces: `ChannelListParams { project_id }`
- Produces: `ChannelBindingRow { project_id, channel_id, peer_kind, peer_id, bound_by, bound_at, label }`
- Produces: `ChannelBindResult { binding, rescoped_session }` · `ChannelUnbindResult { unbound }` · `ChannelListResult { bindings }`

**背景**：本仓在跨 crate wire 契约上复发过三次（`aleph workspace create`、TUI `agent.run`、`providers list/get/add`），每次成因相同：客户端手写字面量，没有对手可以分歧。形状必须住在两边都依赖的 crate 里，客户端**构造**、服务端**解析**。

- [ ] **Step 1: 写契约 + 自对账测试**

新建 `shared/protocol/src/projects.rs`：

```rust
//! Wire contract for `projects.channel.*`.
//!
//! The CLI cannot depend on `alephcore`, so a hand-written `json!({...})` on
//! one side and a `#[derive(Deserialize)]` on the other have no way to
//! disagree until a user reports that a command has never worked. Three
//! families have shipped that defect here (`aleph workspace create`, the TUI's
//! `agent.run`, `aleph providers list/get/add`), so the shape lives in the
//! crate both halves already depend on, and each half reconciles against it.

use serde::{Deserialize, Serialize};

/// `projects.channel.bind`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindParams {
    pub project_id: String,
    pub channel_id: String,
    /// `"group"` or `"thread"`.
    pub peer_kind: String,
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// `projects.channel.unbind`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelUnbindParams {
    pub channel_id: String,
    pub peer_kind: String,
    pub peer_id: String,
}

/// `projects.channel.list`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelListParams {
    pub project_id: String,
}

/// One bound conversation, as every surface renders it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindingRow {
    pub project_id: String,
    pub channel_id: String,
    pub peer_kind: String,
    pub peer_id: String,
    pub bound_by: Option<String>,
    pub bound_at: i64,
    pub label: Option<String>,
}

/// `projects.channel.bind` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelBindResult {
    pub binding: ChannelBindingRow,
    /// Whether an existing session row was moved into the room scope.
    ///
    /// `false` means nobody had spoken in that conversation yet — a distinct
    /// answer from "I moved it", because a receipt that claims a migration
    /// happened when it did not is a client asserting a result it never saw.
    pub rescoped_session: bool,
}

/// `projects.channel.unbind` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelUnbindResult {
    /// `false` means nothing was bound.
    pub unbound: bool,
}

/// `projects.channel.list` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelListResult {
    pub bindings: Vec<ChannelBindingRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The envelope is a wire key too, and it is usually the last hand-copied
    /// part. Serialising the contract type is how a client learns the key
    /// rather than guessing it.
    #[test]
    fn the_list_result_envelope_is_named_bindings() {
        let v = serde_json::to_value(ChannelListResult { bindings: vec![] }).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["bindings"]);
    }

    #[test]
    fn an_absent_label_is_not_sent() {
        let v = serde_json::to_value(ChannelBindParams {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: "group".into(),
            peer_id: "C1".into(),
            label: None,
        })
        .unwrap();
        assert!(
            !v.as_object().unwrap().contains_key("label"),
            "an omitted optional must be omitted, not sent as null"
        );
    }
}
```

在 `shared/protocol/src/lib.rs` 加 `pub mod projects;`。

- [ ] **Step 2: 跑测试**

```bash
cargo test -p aleph_protocol projects -- --nocapture
```

Expected: `test result: ok. 2 passed`
⚠️ crate 名以 `shared/protocol/Cargo.toml` 的 `name` 为准（可能是 `aleph-protocol`）。

- [ ] **Step 3: 提交**

```bash
git add shared/protocol/
git commit -m "protocol: wire contract for projects.channel.*

The CLI cannot depend on alephcore, so a hand-written json! on one side
and a derive on the other have no way to disagree until a user reports the
command has never worked. Three families have shipped that defect here.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 9: 三个 RPC handler + 闸 + census

**Files:**
- Create: `src/gateway/handlers/projects_channel.rs`
- Modify: `src/gateway/handlers/mod.rs`（`mod projects_channel;` + 三处 `register`）
- Modify: `src/gateway/method_admin.rs`（`ADMIN_METHODS` 加两条）
- Modify: `src/gateway/method_census.rs`（加三条）
- Modify: `src/gateway/session_store/mod.rs`（Task 3 的 `SOLE_CALLERS` 加 `rescope_attribution` 一行）

**Interfaces:**
- Consumes: Task 5 的四个 store 方法 · Task 7 的 `rescope_attribution` · Task 8 的契约类型 · 既有 `gate_project`
- Produces: `handle_bind` / `handle_unbind` / `handle_list`

- [ ] **Step 1: 写失败的测试（handler 行为 + census）**

新建 `src/gateway/handlers/projects_channel.rs` 的测试骨架（生产代码 Step 3 写）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_records_an_authority_change() { /* 见 Step 3 后填 */ }

    #[tokio::test]
    async fn a_non_member_cannot_list_a_rooms_bindings() { /* 见 Step 3 后填 */ }
}
```

在 `src/gateway/method_census.rs` 的 `projects.` 区块按字母序插入三行：

```rust
        ("projects.channel.bind", Class::Admin),
        ("projects.channel.list", Class::Open),
        ("projects.channel.unbind", Class::Admin),
```

- [ ] **Step 2: 跑 census 确认它红**

```bash
cargo test -p alephcore --lib method_census -- --nocapture
```

Expected: **RED** — census 表里有三个方法而注册表里没有（ghost 检查），消息点名这三个名字。这证明 census 真的在看注册表而不是在看自己。

- [ ] **Step 3: 实现三个 handler**

`src/gateway/handlers/projects_channel.rs` 生产部分：

```rust
//! `projects.channel.*` — bind a channel group conversation to a project room.
//!
//! # Why bind is operator-only
//!
//! The exposure runs OUTWARD. After binding, a roster member speaking in the
//! group makes the agent answer from the room's shared memory, notes and
//! workspace — and that answer is delivered to the whole conversation,
//! including people the roster does not control. That is the point of the
//! feature, and it is also why the decision belongs to an operator rather than
//! to a room owner who may be an ordinary member. `bind` and `unbind` are
//! therefore in `method_admin::ADMIN_METHODS`; `list` stays open and is
//! narrowed to the roster by `gate_project`, so a member can see where their
//! room lives without being able to move it.

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::visibility;
use aleph_protocol::projects::{
    ChannelBindParams, ChannelBindResult, ChannelBindingRow, ChannelListParams, ChannelListResult,
    ChannelUnbindParams, ChannelUnbindResult,
};
use crate::projects::{ChannelBinding, ProjectError, ProjectStore};

fn row(b: ChannelBinding) -> ChannelBindingRow {
    ChannelBindingRow {
        project_id: b.project_id,
        channel_id: b.channel_id,
        peer_kind: b.peer_kind,
        peer_id: b.peer_id,
        bound_by: b.bound_by,
        bound_at: b.bound_at,
        label: b.label,
    }
}

#[allow(clippy::result_large_err)]
fn parse<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    serde_json::from_value::<T>(request.params.clone().unwrap_or(Value::Null)).map_err(|e| {
        JsonRpcResponse::error(request.id.clone(), INVALID_PARAMS, format!("Invalid params: {e}"))
    })
}

/// `projects.channel.bind`.
pub async fn handle_bind(
    request: JsonRpcRequest,
    sessions: std::sync::Arc<dyn crate::gateway::session_store::SessionStore>,
) -> JsonRpcResponse {
    let params: ChannelBindParams = match parse(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if !matches!(params.peer_kind.as_str(), "group" | "thread") {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "peer_kind must be \"group\" or \"thread\"",
        );
    }
    let store = ProjectStore::shared();
    // Visibility first, exactly like every other `projects.*` verb: a room the
    // caller cannot see must be indistinguishable from one that does not exist.
    // The operator gate that makes this verb operator-only is upstream, in
    // `method_admin`, so this call does not re-derive it.
    let Some(project) = crate::projects::authz::project_for(
        &store,
        &params.project_id,
        visibility::visible_owner_filter().as_deref(),
    ) else {
        return super::projects::project_not_found(request.id, &params.project_id);
    };

    let actor = crate::gateway::caller_identity::current_caller_user();
    let binding = match store.bind_conversation(
        &project.id,
        &params.channel_id,
        &params.peer_kind,
        &params.peer_id,
        actor.as_deref(),
        params.label.as_deref(),
    ) {
        Ok(b) => b,
        Err(ProjectError::Invalid(msg)) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg)
        }
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };

    // The row half. Without it the run takes the room scope while the row keeps
    // `personal:<first speaker>`, so every other member's session list stays
    // empty — the split round-8 ⑤ names. `Ok(false)` means nobody has spoken in
    // that conversation yet and is reported as such rather than dressed up as a
    // migration that happened.
    let key = crate::routing::session_key::SessionKey::group(
        crate::gateway::agent_env::default_agent_id(),
        &params.channel_id,
        if params.peer_kind == "thread" {
            crate::routing::session_key::PeerKind::Thread
        } else {
            crate::routing::session_key::PeerKind::Group
        },
        &params.peer_id,
    );
    let rescoped = matches!(
        sessions
            .rescope_attribution(&key, &crate::scope::ScopeId::Project(project.id.clone()).render())
            .await,
        Ok(true)
    );

    if let Some(log) = crate::security::audit::global() {
        log.log(crate::security::audit::AuditEntry::authority_change(
            actor.clone(),
            format!(
                "projects.channel.bind: {}:{} → {} (rescoped_session={rescoped})",
                params.channel_id, params.peer_id, project.id
            ),
        ));
    }
    crate::projects::events::publish_changed(&project.id, None);

    JsonRpcResponse::success(
        request.id,
        json!(ChannelBindResult {
            binding: row(binding),
            rescoped_session: rescoped,
        }),
    )
}

/// `projects.channel.unbind`.
pub async fn handle_unbind(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ChannelUnbindParams = match parse(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let store = ProjectStore::shared();
    // Read the owning room BEFORE the delete so the audit line and the
    // `projects.changed` frame can name it: answering "whose binding was that"
    // must not depend on whether the delete kept a record.
    let owner = store
        .project_for_conversation(&params.channel_id, &params.peer_kind, &params.peer_id)
        .unwrap_or(None);
    let unbound = match store.unbind_conversation(
        &params.channel_id,
        &params.peer_kind,
        &params.peer_id,
    ) {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };
    if unbound {
        if let Some(log) = crate::security::audit::global() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                crate::gateway::caller_identity::current_caller_user(),
                format!(
                    "projects.channel.unbind: {}:{} (was {})",
                    params.channel_id,
                    params.peer_id,
                    owner.as_deref().unwrap_or("unbound")
                ),
            ));
        }
        if let Some(pid) = owner.as_deref() {
            crate::projects::events::publish_changed(pid, None);
        }
    }
    JsonRpcResponse::success(request.id, json!(ChannelUnbindResult { unbound }))
}

/// `projects.channel.list` — open, narrowed to the roster by `gate_project`.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ChannelListParams = match parse(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let store = ProjectStore::shared();
    let Some(project) = crate::projects::authz::project_for(
        &store,
        &params.project_id,
        visibility::visible_owner_filter().as_deref(),
    ) else {
        return super::projects::project_not_found(request.id, &params.project_id);
    };
    match store.bindings_for(&project.id) {
        Ok(bs) => JsonRpcResponse::success(
            request.id,
            json!(ChannelListResult {
                bindings: bs.into_iter().map(row).collect()
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}
```

⚠️ `project_not_found` / `publish_changed` / `default_agent_id` 的确切可见性与签名以现有代码为准；`project_not_found` 目前是 `projects.rs` 私有的，需要改成 `pub(super)`（**改可见性，不要复制一份**——那会是同一条拒绝形状的第二个真源）。

在 `src/gateway/handlers/mod.rs` 的 `projects.room_session` 注册之后追加：

```rust
            let s = default_store.clone();
            registry.register("projects.channel.bind", move |req| {
                let s = s.clone();
                async move { projects_channel::handle_bind(req, s).await }
            });
            registry.register("projects.channel.unbind", |req| async move {
                projects_channel::handle_unbind(req).await
            });
            registry.register("projects.channel.list", |req| async move {
                projects_channel::handle_list(req).await
            });
```

在 `src/gateway/method_admin.rs` 的 `ADMIN_METHODS` 加两条：

```rust
const ADMIN_METHODS: &[&str] = &[
    "memory.compress",
    "memory.reembed",
    "memory.reembed.cancel",
    // Binding a room to a channel conversation points the room's shared memory
    // at an audience the roster does not control. `list` stays open — a member
    // may see where their room lives without being able to move it.
    "projects.channel.bind",
    "projects.channel.unbind",
];
```

在 Task 3 的 `SOLE_CALLERS` 加 `rescope_attribution` 那一行。

- [ ] **Step 4: 补齐 handler 测试体**

```rust
    #[tokio::test]
    async fn a_non_member_cannot_list_a_rooms_bindings() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::shared();
        store.create_schema().unwrap();
        let room = store.create("room", Some("u-alice"), None).unwrap();
        store.add_member(&room.id, "u-alice").unwrap();
        store
            .bind_conversation(&room.id, "telegram", "group", "C1", Some("u-alice"), None)
            .unwrap();

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_list(JsonRpcRequest {
                    id: Some(json!(1)),
                    method: "projects.channel.list".into(),
                    params: Some(json!({ "project_id": room.id })),
                    ..Default::default()
                })
                .await
            })
            .await;
        let err = resp.error.expect("a non-member is refused");
        assert!(
            !err.message.contains("forbidden"),
            "the refusal must be indistinguishable from 'no such room' — otherwise \
             the refusal itself tells a stranger the room is real"
        );
    }
```

⚠️ `JsonRpcRequest` 的字段名与 `Default` 可用性以 `protocol.rs` 为准；照抄 `projects.rs` 测试里已有的 `rpc(...)` helper 更稳妥。

- [ ] **Step 5: 跑测试**

```bash
cargo test -p alephcore --lib projects_channel -- --nocapture
cargo test -p alephcore --lib method_census -- --nocapture
cargo test -p alephcore --lib caller_census -- --nocapture
```

Expected: 三组全 PASS。census 现在应当绿——三个方法已注册且已分类，`rescope_attribution` 的唯一调用者存在。

- [ ] **Step 6: 破坏验证（必做）**

把 `ADMIN_METHODS` 里 `"projects.channel.bind"` 一行删掉，重跑 `method_census` → 期望 RED（裁决与表不符）。恢复。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/ 
git commit -m "gateway: projects.channel.bind/unbind/list

bind and unbind are operator-only because the exposure runs outward: a
bound room answers from its shared memory into a conversation the roster
does not control. list stays open and is narrowed by gate_project, so a
member can see where their room lives without being able to move it.

bind also rescopes the existing session row; without that the run takes
the room scope while the row keeps personal:<first speaker> and the group
stays invisible in every other member's list.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 10: `aleph projects` CLI

**Files:**
- Create: `interfaces/cli/src/commands/projects_cmd.rs`
- Modify: `interfaces/cli/src/commands/cli_args.rs`（`Projects` 子命令 + `ProjectsAction` / `ChannelAction` 枚举）
- Modify: `interfaces/cli/src/commands/mod.rs`（`pub mod projects_cmd;` + dispatch 臂）

**Interfaces:**
- Consumes: `aleph_protocol::projects::*`（Task 8）
- Produces: `aleph projects list` · `aleph projects channel list|bind|unbind`

- [ ] **Step 1: 写对账测试（先写，它定义列名）**

在 `interfaces/cli/src/commands/projects_cmd.rs` 里：

```rust
//! `aleph projects` — the headless face of project rooms.
//!
//! Every other operator-only family (`users`, `audit`, `spend`) has a CLI and
//! rooms did not, which matters because a headless deployment has no Panel at
//! all. `channel bind`/`unbind` are admin-gated server-side; the CLI reaches
//! the server over loopback, which resolves to the implicit owner as
//! "operator" — the same posture that put those three here.

use aleph_protocol::projects::{
    ChannelBindParams, ChannelBindResult, ChannelBindingRow, ChannelListParams, ChannelListResult,
    ChannelUnbindParams, ChannelUnbindResult,
};

/// (display header, wire field name) for every column, in print order.
///
/// The wire half is not decorative: the test below asserts it is a real key in
/// a `ChannelBindingRow` serialised from the contract type. That is the guard
/// against the defect `aleph providers list` shipped with — headers that looked
/// plausible and were never backed by anything the server sent, so every row
/// rendered a dash from the day it was written and a dash reads as "no value
/// yet", not as a bug.
const BINDING_COLUMNS: &[(&str, &str)] = &[
    ("Channel", "channel_id"),
    ("Kind", "peer_kind"),
    ("Conversation", "peer_id"),
    ("Label", "label"),
    ("Bound By", "bound_by"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_column_the_cli_renders_is_present_in_the_row_contract() {
        let sample = serde_json::to_value(ChannelBindingRow {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: "group".into(),
            peer_id: "C1".into(),
            bound_by: Some("u-owner".into()),
            bound_at: 0,
            label: Some("#eng".into()),
        })
        .unwrap();
        let keys: std::collections::BTreeSet<&str> =
            sample.as_object().unwrap().keys().map(String::as_str).collect();
        for (header, wire) in BINDING_COLUMNS {
            assert!(
                keys.contains(wire),
                "column {header:?} renders wire key {wire:?}, which the contract type \
                 does not have. A header backed by nothing prints a dash forever."
            );
        }
    }

    #[test]
    fn the_bind_request_carries_exactly_what_the_handler_requires() {
        let v = serde_json::to_value(ChannelBindParams {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: "group".into(),
            peer_id: "C1".into(),
            label: None,
        })
        .unwrap();
        let keys: std::collections::BTreeSet<&str> =
            v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["channel_id", "peer_id", "peer_kind", "project_id"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "the request the CLI sends must be exactly the shape the handler parses"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-cli projects_cmd -- --nocapture
```

Expected: BUILD-ERROR（模块未挂）或 RED。

- [ ] **Step 3: 实现命令 + 接线**

在 `cli_args.rs` 的 `Commands` 枚举里加（照 `Users` 的写法）：

```rust
    /// Project rooms.
    Projects {
        #[command(subcommand)]
        action: ProjectsAction,
    },
```

```rust
#[derive(clap::Subcommand, Debug)]
pub enum ProjectsAction {
    /// List rooms visible to you.
    List,
    /// Manage which channel conversations a room lives in.
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum ChannelAction {
    /// Show the conversations a room is bound to.
    List {
        project_id: String,
    },
    /// Bind a channel group conversation to a room (operator only).
    Bind {
        project_id: String,
        channel_id: String,
        peer_id: String,
        /// "group" (default) or "thread".
        #[arg(long, default_value = "group")]
        peer_kind: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Release a conversation (operator only).
    Unbind {
        channel_id: String,
        peer_id: String,
        #[arg(long, default_value = "group")]
        peer_kind: String,
    },
}
```

⚠️ **短参冲突**：不要给任何新参数加短名。clap 的冲突是 `debug_assert`，在 debug 构建里表现为**二进制启动即 panic**，而 `cargo check` 与单测都看不见（`aleph-tui` 的 `-c` 曾这样炸过）。

`projects_cmd.rs` 的命令体照 `spend_cmd.rs` 的结构写：`AlephClient` → `client.call("projects.channel.list", json!(ChannelListParams{...}))` → 用契约类型 `serde_json::from_value::<ChannelListResult>` 解析 → `output::table(BINDING_COLUMNS, rows)`。

**回执必须说清这次写改变了什么和没改变什么**：

```rust
fn print_bind_effects(result: &ChannelBindResult) {
    println!(
        "Bound {}:{} to {}.",
        result.binding.channel_id, result.binding.peer_id, result.binding.project_id
    );
    if result.rescoped_session {
        println!("The conversation's existing transcript now belongs to the room and is visible to its roster.");
    } else {
        println!("No existing transcript was moved — nobody has spoken in that conversation yet.");
    }
}
```

⚠️ 这段是刻意的：`users_cmd.rs::update` 曾印一句硬编码的「设备已撤销」——**一台设备都没有时它照样这么说**。一个客户端替一次它没有观察到的结果作断言，比沉默更贵。

- [ ] **Step 4: 加 clap 自校验测试**

在 `cli_args.rs` 的测试模块加（若已存在同款则跳过）：

```rust
    /// clap validates argument definitions with debug_assert, so a duplicate
    /// short flag is a startup panic in debug builds and a silent letter
    /// reassignment in release — neither of which `cargo check` or the unit
    /// tests can see.
    #[test]
    fn the_argument_definitions_are_internally_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
```

- [ ] **Step 5: 跑测试**

```bash
cargo test -p aleph-cli -- --nocapture
```

Expected: 全 PASS。

- [ ] **Step 6: 提交**

```bash
git add interfaces/cli/
git commit -m "cli: aleph projects, with the channel binding subcommands

Every other operator-only family has a CLI and rooms did not, which
matters because a headless deployment has no Panel. The bind receipt
reports whether an existing transcript actually moved instead of asserting
a migration it did not observe.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## Task 11: Panel 房间页的频道区

**Files:**
- Create: `interfaces/webchat/src/api/projects_channel.rs`
- Modify: `interfaces/webchat/src/api.rs`（`pub mod projects_channel;`）
- Modify: `interfaces/webchat/src/components/project_page/settings.rs`

**Interfaces:**
- Consumes: `projects.channel.list`（读）· `projects.channel.bind` / `.unbind`（写，可能被 admin 闸拒）
- Produces: `ChannelBindingsSection` 组件

- [ ] **Step 1: 写 API 客户端 + 行解码器测试**

`interfaces/webchat/src/api/projects_channel.rs`：

```rust
//! `projects.channel.*` client.
//!
//! One row decoder, shared by every renderer: the field-by-field copies this
//! replaces are how `aleph providers list` ended up rendering two columns the
//! server had never sent.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BindingRow {
    pub project_id: String,
    pub channel_id: String,
    pub peer_kind: String,
    pub peer_id: String,
    #[serde(default)]
    pub bound_by: Option<String>,
    pub bound_at: i64,
    #[serde(default)]
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DTO must parse what the server actually sends. A missing
    /// `#[serde(default)]` on one optional makes the WHOLE document fail to
    /// deserialize — serde does not degrade field by field, and the symptom is
    /// "the section is empty", not "one field is missing".
    #[test]
    fn a_row_without_optionals_still_parses() {
        let row: BindingRow = serde_json::from_str(
            r#"{"project_id":"p-1","channel_id":"telegram","peer_kind":"group",
                "peer_id":"C1","bound_at":0}"#,
        )
        .expect("optionals must be defaulted, not required");
        assert_eq!(row.label, None);
    }
}
```

- [ ] **Step 2: 跑测试确认失败，再实现到通过**

```bash
cargo test -p aleph-panel --lib projects_channel -- --nocapture
```

Expected: 先 BUILD-ERROR，写完后 `test result: ok. 1 passed`

- [ ] **Step 3: 加设置区组件**

在 `project_page/settings.rs` 加一个 `ChannelBindingsSection`：列出绑定行（Channel / Kind / Conversation / Label / Bound By），每行一个 Unbind 按钮，底部一个 bind 表单。

**读写两侧都必须过拒绝分类器**：

```rust
// A member reaches this page (StepStatus::Restricted only colours the Quick
// Setup checklist; the settings page renders and its buttons are live), and
// bind/unbind are admin-gated. Both directions therefore go through
// `admin_refusal`: fixing only the read path makes the page look handled while
// Save still answers with a raw protocol string, which tells the user their
// action failed rather than that they lack permission.
match result {
    Ok(v) => { /* render */ }
    Err(e) => set_error.set(Some(admin_refusal::classify(&e))),
}
```

⚠️ **不要** `set_error.set(Some(e))`（把协议串直接给用户）——`admin_refusal.rs::no_error_signal_is_fed_an_unclassified_error` 是 crate 级守卫，会直接报。

- [ ] **Step 4: 跑 Panel 测试 + 出厂形态构建**

```bash
cargo test -p aleph-panel --lib
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
```

Expected: 两条都通过。第二条是唯一编译出厂形态的命令（`--lib` 测试构建里 `cfg(test)` 为真，看不见 wasm 上的 `unused_imports` 与错位的 `#[cfg(test)]` 门）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/
git commit -m "panel: room settings show and manage channel bindings

Both the read and the write path go through admin_refusal: a member can
open this page and the buttons are live, so classifying only the read half
would leave Save answering with a raw protocol string — telling the user
their action failed rather than that they lack permission.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 5 — 工具面

## Task 12: `project_manage` 加 `bind_workspace` 动词

**Files:**
- Modify: `src/builtin_tools/project_manage.rs`
- Modify: `src/tools/scoped/`（工具面取 run 的 role 交给 `caller_may_choose_directory_as`）

**Interfaces:**
- Consumes: Task 2 的 `caller_may_choose_directory_as`
- Produces: `project_manage(action="bind_workspace", project_id, path)`

**背景**：round-8 ③ 把 `bind_workspace` 排除在工具面之外，理由逐字是「`caller_may_choose_directory()` 对无连接角色 fail-OPEN，工具面够得到那条臂」。Task 2 已经把那条臂关掉，本任务兑现它。`bind_channel` **不**上工具面（暴露方向朝外，spec §7）。

- [ ] **Step 1: 写失败的测试**

```rust
    /// The tool face must be judged by the run's enforced role, not by the
    /// ambient task-local (which is dead past every spawn). A chat-tier run
    /// binding a workspace would be pointing the server at an arbitrary folder
    /// with no human in the loop.
    #[tokio::test]
    async fn a_chat_tier_run_may_not_bind_a_workspace() {
        let out = run_project_manage_with_role("guest", json!({
            "action": "bind_workspace",
            "project_id": "p-1",
            "path": "/tmp"
        }))
        .await;
        assert!(out.is_err(), "a chat-tier run must be refused");
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p alephcore --lib project_manage -- --nocapture
```

Expected: BUILD-ERROR 或 RED。

- [ ] **Step 3: 实现动词 + 闸**

在 `ProjectAction` 枚举加 `BindWorkspace`，dispatch 臂里先取 run 的 role：

```rust
            ProjectAction::BindWorkspace => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let path = Self::need(args.path.as_ref(), "path", action)?;
                // The run's tier, from the object that already enforces it —
                // not a second derivation, and not the ambient task-local
                // (dead past the spawn, and its None arm used to be the
                // admitted one).
                let role = crate::tools::scoped::enforced_caller_role();
                if !crate::gateway::caller_identity::caller_may_choose_directory_as(
                    role.as_deref(),
                    false,
                ) {
                    return Err(AlephError::tool(
                        "binding a project workspace requires an operator-tier session",
                    ));
                }
                // ... existing projects.bind_workspace store call
            }
```

⚠️ `enforced_caller_role()` 若不存在，**先去 `src/tools/scoped/` 找那个已经在跑配置闸的方法并复用它**；找不到就停下来报告——新造一个 role 读取器就是第二个推导，正是本轮要消灭的东西。

- [ ] **Step 4: 跑测试 + 抬棘轮**

```bash
cargo test -p alephcore --lib project_manage -- --nocapture
cargo test -p alephcore --lib catalog_description_bytes_ratchet -- --nocapture
```

第二条会 RED 并打印**实测**的新字节数。把那个数字抄进棘轮常量——**不要手算**（闸跑不了的时候作者只能算，上一次算术差了 510 字节）。重跑确认绿。

- [ ] **Step 5: 提交**

```bash
git add src/builtin_tools/project_manage.rs src/tools/scoped/
git commit -m "project_manage: bind_workspace, now that the gate is real

round-8 kept this verb off the tool face because
caller_may_choose_directory() was constant-true for a connection-less
caller, which is exactly what a tool call inside a run is. That arm is
closed, so the verb takes the run's enforced tier from the object that
already applies it.

bind_channel deliberately stays off the tool face: its exposure runs
outward, into an audience the roster does not control.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 6 — 真机验证

## Task 13: `qa/rooms_channel_bind/run.sh`

**Files:**
- Create: `qa/rooms_channel_bind/run.sh`
- Create: `qa/rooms_channel_bind/README.md`

**Interfaces:**
- Consumes: 全部前置任务的产物

**夹具形状**（照 `qa/teamchat_rooms/run.sh` 与 `qa/channels/run.sh` 抄）：隔离 `ALEPH_HOME` + mock 频道 + 内容驱动的假 provider + 三个真实身份（operator 走 loopback；两个 member 各自用一次性配对票从 **LAN 腿**连回来——loopback 在 `resolve_connect_auth` 第一行就被短路成 operator，所以第二、第三个身份结构上只能从 LAN 进）。

**七条效果断言**（每条都要断言**效果**，不是「调用返回了 200」）：

- [ ] **Step 1: 场景 1 — 现状基线**
未绑定的群里 alice 与 bob 各说一句，断言两条记忆分别落 `main__u-alice` 与 `main__u-bob`。**这一条必须先绿**——它是本轮动机的证据；它若不绿，前提就要重估，**停下来报告**。

- [ ] **Step 2: 场景 2 — 绑定后升格 + 存量可见**
operator `aleph projects channel bind`，断言 ① alice 的下一条记忆落 `main__p-<id>`；② bob（在名册上）在 Panel 的 `sessions.list` 里**看得见**这个群会话（`rescope_attribution` 生效）。

- [ ] **Step 3: 场景 3 — 名册外的已配对者**
carol（已配对、**不在**名册）在同一群说话，断言她的记忆仍落 `main__u-carol`，且 `main__p-<id>` 分区**没有**新增行。

- [ ] **Step 4: 场景 4 — 未配对陌生人**
未配对 sender 说话，断言该 run 无 scope：`main__p-<id>` 无新增，且回复里**不含** `<room_context>` 的任何成员名。

- [ ] **Step 5: 场景 5 — 房间上下文与子代理继承**
alice 提一个会派子代理的请求，断言 `<room_context>` 在**频道**回合的 prompt 里出现、点名了没说过话的成员，并且子代理的 prompt 里也有（round-8 刻意未做 ④ 的兑现）。

- [ ] **Step 6: 场景 6 — 解绑**
`aleph projects channel unbind`，断言新回合退回 personal，而已落盘的行**仍是**房间 scope。

- [ ] **Step 7: 场景 7 — agent_switch 不解绑**
对该频道 `agent_switch` 到另一个 agent，alice 再说一句，断言仍落 `main__p-<id>`。

- [ ] **Step 8: 跑一遍并提交**

```bash
bash qa/rooms_channel_bind/run.sh
```

Expected: 全部断言绿。

⚠️ **三条夹具陷阱**（round-8 记录，直接适用）：① `{method:"event",params:{topic,data}}` 是 bus 事件的**第三种**信封形状，只读 `msg.topic`/`msg.method` 的抓包器对它完全失明；② `<system-reminder>` 不是「这是脚手架」的可靠判据；③ 房间分区是 `main__p-<id>` 而不是 `main__p-p-<id>`（`scoped_agent_id(base, ns)` 的 `ns` **就是**项目 id），拼错的分区没有写者，空结果读起来和「没落盘」逐字节相同。

```bash
git add qa/rooms_channel_bind/
git commit -m "qa: real-machine coverage for channel-bound project rooms

Seven effect assertions across three real identities. Scenario 1 pins the
pre-existing behaviour this round is motivated by, so a regression in the
premise fails loudly rather than making the later scenarios look correct.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

# Phase 7 — 文档

## Task 14: 回填记录文档

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§5.22 加第九轮）
- Modify: `docs/reference/SECURITY.md`（Known gap 表重写）
- Modify: `docs/reference/GATEWAY.md`
- Modify: `src/gateway/CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-28-multiuser-channel-rooms-p4-design.md`（状态改为已实施）

- [ ] **Step 1: 全轮验证集**

```bash
cd /d/Workspace/Aleph-wt-multiuser-round9
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1
cargo test -p aleph-panel --lib --no-run
cargo test -p aleph-tui -p aleph-cli
cargo check -p aleph-desktop-windows
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
```

把**实测**结果（passed / failed 计数，以及每条失败是否在未改动的基线提交上原样复现）记进第九轮条目。**不要**写「应该全绿」。

- [ ] **Step 2: 写 FEATURE_LOCATOR §5.22 第九轮**

紧跟第八轮之后，包含：本轮组织判据（「这条线已经建好 90%，缺的是认领与一道闸」）· 七个阶段的落点 · Task 1/2 两条既存缺陷的成因与为什么四轮没被发现 · **规划期对 spec 的两处更正**（`is_member` 无 `Err` 臂；未配对回合已由 `stamped?` fail-closed）· 刻意未做清单 · 真机 QA 的断言数与它抓到的东西。

- [ ] **Step 3: 重写 SECURITY.md 的 Known gap 表**

- **gap 1 已腐烂**（它写着 `projects.*` 无工具面，而 round-8 ③ 落了 `project_manage`）——删掉并改写成现状。
- **gap 2 收窄**：channels-into-rooms 的**入方向**已兑现；剩余边界（房间→群的主动推送、`bind_channel` 无工具面、未盖戳回合仍落机主分区）逐条写清。
- 新增一条：§10 会话 scope 不可变性的**唯一例外**及其三条收窄。

- [ ] **Step 4: 提交**

```bash
git add docs/ src/gateway/CLAUDE.md
git commit -m "docs: FEATURE_LOCATOR round-9 entry for channel-bound rooms

Also rewrites SECURITY.md's known-gap table: gap 1 had rotted (it says
projects.* has no tool surface, which round-8 fixed) and gap 2's
channels-into-rooms half is now partly delivered, so its remaining
boundary is spelled out rather than left as a pointer to a phase name.

Claude-Session: https://claude.ai/code/session_01UwdBJyZsoECuz1PR8Ctdfr"
```

---

## 附录 · 规划期自查结果 (Self-Review)

**1. Spec 覆盖**：spec 的 §4.1→T5 · §4.2→T5 · §4.3→T6 · §4.4→T6 · §4.5→T6(Step 1 第三条测试) · §5.1→T7+T9 · §5.2→T13(场景 6) · §6 P1→T1 · P2→T2 · P3→T3 · P4→T4 · P5/P6→T14 · P7→T13(场景 5) · §7 四张脸→T9/T10/T11/T12 · §8→贯穿 · §10 七阶段→T1–T14。**无未覆盖项。**

**2. 占位符扫描**：无 TBD/TODO。三处标 ⚠️ 的「以现有代码为准」是**指令**（去读那个文件的现有夹具风格），不是待填空白；每处都写明了「找不到就停下来报告」而不是「自己发明一个」。

**3. 类型一致性**：`ChannelBinding`（core）↔ `ChannelBindingRow`（wire）↔ `BindingRow`（Panel DTO）三者字段名逐一对齐；`peer_kind` 全程是 `String`（`"group"`/`"thread"`），只有 `binding.rs::peer_kind_str` 一处从 `PeerKind` 转过来；`rescope_attribution` 的签名在 T7 定义、T9 消费、T3 census 点名，三处一致。

**4. 已知的顺序耦合**：T7 Step 3 若同时把 `rescope_attribution` 写进 `SOLE_CALLERS`，T7 的提交会红到 T9 完成为止。计划里已写明两条出路，执行者选一条并在提交信息里说明。
