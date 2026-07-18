# Panel 左侧栏范式回退 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 左侧栏回退到 `panel-sidebar-enrich` 分支之前的原始交互范式（弹窗选 tab），并清理随之变为死代码的后端 pinned/project_root/set_pinned，保留 `updated_at` 真 bug 修复。

**Architecture:** 前端对 3 个文件做字节级 `git checkout 87cd90138 -- <path>`（精确还原原始版本），`components/mod.rs` 删一行模块声明，删除 `sidebar_footer.rs`。后端对 9 个"合并后零改动"的文件用分支 diff 反向打补丁（`git apply -R`），对 `query.rs`/`types.rs`/`session_store/mod.rs` 这 3 个需保留 `updated_at` 或合并后被改过的文件做外科 Edit。

**Tech Stack:** Rust（alephcore / aleph-server）、Leptos 0.8（WASM panel）、git checkout / git apply -R。

**基线 commit:** `87cd90138`（合并提交 `eaa595ac0` 的 pre-merge parent = 原始范式）。

---

## 验证哲学（本计划非 TDD）

这是一次"回退 + 死代码清理"，没有新增行为可写测试。每个任务的验证 = **编译通过**（前端 `just wasm`、后端 `cargo check -p alephcore`）+ 关键文件内容回到原始版本。最终验证 = 人工 E2E（弹窗范式回归、能在各区域间往返回到 Chat）。

> 注：用户当前会话偏好极度节制 cargo 调用。本计划在每个后端任务末尾安排 `cargo check -p alephcore --lib`（仅一次、lib 范围），不要扩到 `--tests`/`--all-targets`，除非用户另行同意。

---

## Task 1: 前端范式回退（commit 1）

**Files:**
- Checkout(还原至 87cd90138): `interfaces/webchat/src/components/nav_menu.rs`
- Checkout(还原至 87cd90138): `interfaces/webchat/src/components/mode_sidebar.rs`
- Checkout(还原至 87cd90138): `interfaces/webchat/src/components/chat_sidebar.rs`
- Modify(外科删一行): `interfaces/webchat/src/components/mod.rs:27`
- Delete: `interfaces/webchat/src/components/sidebar_footer.rs`

- [ ] **Step 1: 字节级还原 3 个完全回退的前端文件**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git checkout 87cd90138 -- \
  interfaces/webchat/src/components/nav_menu.rs \
  interfaces/webchat/src/components/mode_sidebar.rs \
  interfaces/webchat/src/components/chat_sidebar.rs
```

- [ ] **Step 2: 删除 `components/mod.rs` 的 sidebar_footer 模块声明**

文件 `interfaces/webchat/src/components/mod.rs` 第 27 行当前为：

```rust
pub mod sidebar_footer;
```

删除该行（含其换行）。删除后，`pub mod` 声明序列里不再出现 `sidebar_footer`。

- [ ] **Step 3: 删除 sidebar_footer.rs 文件**

```bash
git rm interfaces/webchat/src/components/sidebar_footer.rs
```

- [ ] **Step 4: 确认无残留引用**

Run:
```bash
grep -rn "sidebar_footer\|SidebarFooter" interfaces/webchat/src/
```
Expected: 无任何输出（mode_sidebar.rs 已 checkout 回原始版，不再引用 SidebarFooter；mod.rs 已删声明）。若有输出，说明 checkout/删除不完整，需排查。

- [ ] **Step 5: 编译 WASM 验证前端编译通过**

Run:
```bash
just wasm
```
Expected: 构建成功，生成 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}`，无 Rust 编译错误。

- [ ] **Step 6: 提交前端回退**

```bash
git add interfaces/webchat/src/components/ interfaces/webchat/dist/
git commit -m "panel: revert sidebar to popup-nav paradigm

Restore the original left-sidebar interaction model (bottom popup section
switcher incl. Chat + Settings, flat session list with inline agent
switcher). Removes the persistent 2-col mode grid, header settings gear,
SidebarFooter, and pinned/recent/projects session sections introduced by
panel-sidebar-enrich. The enrich branch overreached: the actual goal was
only to add project/team quick-entries to the chat window, deferred to a
separate round."
```

---

## Task 2: 后端死代码清理（保留 updated_at）（commit 2）

**Files:**
- Reverse-patch(分支新增整段移除): 9 个文件（见 Step 1）
- Modify(外科 Edit): `src/gateway/session_store/mod.rs`
- Modify(外科 Edit): `src/gateway/handlers/session/db_handlers/types.rs`
- Modify(外科 Edit): `src/gateway/handlers/session/db_handlers/query.rs`

- [ ] **Step 1: 反向打补丁移除 9 个"合并后零改动"文件的分支新增**

这 9 个文件的全部分支新增都只关于 `set_pinned`/`set_project_root`/`project_root` 持久化，且合并后从未被改动，可干净反向移除。

```bash
cd /Volumes/TBU4/Workspace/Aleph
git diff 87cd90138 eaa595ac0 -- \
  src/gateway/handlers/session/db_handlers/modify.rs \
  src/gateway/handlers/session/db_handlers/mod.rs \
  src/gateway/handlers/session/mod.rs \
  src/gateway/router.rs \
  src/gateway/session_manager/ops/modify.rs \
  src/gateway/session_store/file_backend/mod.rs \
  src/gateway/session_store/sqlite_backend/mod.rs \
  src/bin/aleph-server/commands/start/builder/handlers/session.rs \
  src/gateway/handlers/agent.rs \
  | git apply -R
```
Expected: 无报错（已 dry-run `--check` 通过）。

- [ ] **Step 2: 外科移除 `session_store/mod.rs` 的 set_pinned / set_project_root trait 默认方法**

该文件合并后被 `8d38ed520` 改过（把两个方法从 required 改成 default no-op），不能用反向补丁，需手动删除。删除以下整段（位于 `set_topic` 与 `set_source_channel` 之间）：

```rust

    /// Persist a session's pinned flag (stored in identity_meta.custom["pinned"],
    /// mirroring how set_topic persists the topic — no schema change required).
    /// Default is a no-op so backends that do not persist identity metadata
    /// compile unchanged (mirrors set_source_channel's default).
    async fn set_pinned(&self, key: &SessionKey, pinned: bool) -> Result<(), SessionStoreError> {
        let _ = (key, pinned);
        Ok(())
    }

    /// Persist the working directory a run was launched in
    /// (identity_meta.custom["project_root"]), so the Panel can group sessions
    /// by project. Written at run start; mirrors set_topic's persistence path.
    /// Default is a no-op so backends that do not persist identity metadata
    /// compile unchanged (mirrors set_source_channel's default).
    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: &str,
    ) -> Result<(), SessionStoreError> {
        let _ = (key, project_root);
        Ok(())
    }
```

删除后，`set_topic(...)` 之后应紧跟 `set_source_channel` 的文档注释行 `/// Record the originating channel...`。

- [ ] **Step 3: 外科移除 `db_handlers/types.rs` 的 pinned / project_root 字段（保留 updated_at）**

在 `pub struct SessionInfo { ... }` 中删除以下两段字段，**保留**其上方的 `pub updated_at: i64,`：

```rust
    /// Whether the user pinned this session (identity_meta.custom["pinned"]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Working directory the session's runs launched in
    /// (identity_meta.custom["project_root"]), for Panel project grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
```

删除后，`pub updated_at: i64,`（含其文档注释）应是 struct 的最后一个字段，后跟 `}`。

- [ ] **Step 4: 外科移除 `db_handlers/query.rs` 的 pinned / project_root 派生与字段（保留 updated_at）**

(a) 删除 `handle_list_db` 中这两段 let 绑定，**保留** `let updated_at = m.last_active_at;`：

```rust
                    // Derive pinned / project_root from identity metadata (mirrors topic).
                    let pinned = m.identity_meta.as_ref().and_then(|im| {
                        im.custom.get("pinned").and_then(serde_json::Value::as_bool)
                    });
                    let project_root = m.identity_meta.as_ref().and_then(|im| {
                        im.custom
                            .get("project_root")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    });
```

删除后该处应仅余注释 `// Derive origin channel before the struct literal moves m's fields.`、`let channel = m.origin_channel();` 以及 `let updated_at = m.last_active_at;`。

(b) 在 `SessionInfo { ... }` 结构体字面量里删除这两行，**保留** `updated_at,`：

```rust
                        pinned,
                        project_root,
```

- [ ] **Step 5: 确认后端无残留引用**

Run:
```bash
grep -rn "set_pinned\|set_project_root" src/gateway/ src/bin/
grep -rn "\.pinned\b" src/gateway/handlers/session/
```
Expected: 第一条无输出（所有 set_pinned/set_project_root 已移除）。第二条无输出（SessionInfo 不再有 pinned 字段被读写）。

> 注意：**不要**动 `src/session/events.rs`、`src/session/state.rs`、`src/context/compact/session_split.rs`、`src/session/store.rs` 里早已存在的 `project_root`——那是与本分支无关的既有概念。

- [ ] **Step 6: 编译验证后端（仅 lib，单次）**

Run:
```bash
cargo check -p alephcore --lib
```
Expected: 编译通过，无 error。（不要扩到 `--tests`/`--all-targets`，除非用户同意。）

- [ ] **Step 7: 提交后端清理**

```bash
git add src/gateway/ src/bin/aleph-server/commands/start/builder/handlers/session.rs
git commit -m "gateway: drop orphaned session pinned/project_root, keep updated_at fix

The panel-sidebar-enrich revert leaves set_pinned/set_project_root, the
sessions.set_pinned RPC, both store impls, and start-path project_root
persistence with no consumer. Remove them. Keep the updated_at wiring in
sessions.list (a genuine correctness fix: the response previously never
carried updated_at)."
```

---

## Task 3: 最终全链验证 + 部署确认（人工）

**Files:** 无代码改动。

- [ ] **Step 1: 重编 server binary（让 rust_embed 烧入新 dist）**

Run:
```bash
cargo build --release -p alephcore --bin aleph-server
```
Expected: 构建成功。

- [ ] **Step 2: 替换运行中的 binary 并重启（dev daemon 路径）**

```bash
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```
（若是 .app daemon，按 CLAUDE.md 的"Panel ↔ Daemon 资源嵌入链"替换 `/Applications/Aleph.app/Contents/MacOS/aleph-server` 后 kill pid 让 supervisor relaunch。）

- [ ] **Step 3: Reload Panel 并肉眼 E2E 验收**

在桌面 App 里 View → Reload Panel（或 Cmd+R），逐项确认：
- 左侧栏底部是**弹窗触发按钮**，点击向上弹出 6 个区域（Chat / Dashboard / Memory / Agents / Teams / Settings），当前项打勾；**无** 2 列网格。
- 选择 Dashboard/Memory/Agents/Teams 后，能再次打开弹窗**回到 Chat**（核心回归点）。
- header **无设置齿轮**；Settings 仅经弹窗进入。
- Chat 侧栏为原始扁平会话列表 + 内联 agent 切换器；**无**置顶/最近/项目分区、**无** pin/unpin 菜单项。

---

## Self-Review 结果

- **Spec 覆盖**：spec 第一部分（前端 5 项）→ Task 1；第二部分（后端清理 12 文件，保留 updated_at）→ Task 2（9 反向补丁 + 3 外科：mod.rs/types.rs/query.rs）；spec 成功标准的编译与人工 E2E → Task 3。第三部分（快速入口）spec 已声明本次不做，无需任务。无遗漏。
- **Placeholder 扫描**：无 TBD/TODO；所有 Edit 给出精确 old 文本；所有命令给出 Expected。
- **类型/签名一致**：`set_pinned`/`set_project_root` 签名在 trait 默认方法、反向补丁移除的 impl、ops 调用方三处一致移除；`updated_at: i64` 字段名在 types.rs 定义与 query.rs 赋值处一致保留。

> 文档位置说明：`docs/superpowers/` 被本仓库 `.gitignore` 忽略，故本计划与对应 spec 仅存于本地，不随代码提交（符合仓库约定）。
