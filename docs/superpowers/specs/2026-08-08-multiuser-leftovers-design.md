# 多用户隐私轮遗留四项 — 设计

日期：2026-08-08
前轮：`worktree-multiuser-privacy-round1`（19 提交，已合 main）
记忆锚点：`project-multiuser-realmachine-qa`

前一轮真机 QA 结束时留下四项：一项是未修的确认缺口（产品决定），三项是验证覆盖不到的
角落。本文只覆盖这四项，不扩大范围。

---

## A · item 1 —— `agent_envs` 无 owner 列

### 症状（实测）

member 身份可以 `workspace.update` 改掉 operator 刚建的 workspace 的名字，再
`workspace.archive` 把它归档，两次都返回 `ok`。

前一轮的文档只写了"member 能读另一个人的 `env_vars`"，少了一整个动词类；已在
`handlers/workspace.rs` 与 `method_visibility.rs` 的 doc 里更正，但**缺陷本身没修**——
当时判断"修它要加列 + 迁移，是产品决定"。

### 本轮重新侦察推翻了那个判断

`workspace.*` 这一族的客户端只有一个，而且它恒 operator：

| 事实 | 证据 |
|---|---|
| Panel **零消费者** | `interfaces/webchat/src/api/workspace.rs:6` 自陈 "the original `workspace.list` method was dead … was removed"；全仓 grep 无 Panel 调用 |
| 唯一客户端是 operator CLI | `interfaces/cli/src/commands/workspace_cmd.rs`（`aleph workspace list\|create\|archive`），走 IPC/loopback ⇒ 恒 operator |
| `workspace.update` / `workspace.get` **全仓零客户端** | 只有 handler 与测试 |
| 被"泄漏"的那几列没有任何写入者 | `env_vars` / `system_prompt_override` / `allowed_tools` 在 `agent_envs` 上只有读路径（`manager_ops.rs::row_to_agent_env`），全仓无写者 ⇒ 读侧泄漏的是一列空值 |

于是真实成立的只有**写**那半——纯破坏，且没有任何 member 侧功能依赖它。

同时这是判据清单里「一个动词的两张脸必须共用判据」的形状：同一份 `AgentEnvStore`，
`agents.*` 的写面**早已在 `ADMIN_PREFIXES` 里**（只 carve 出 `list`/`get`），
而 `workspace.*` 是同一张表的第二张脸却整族敞开。

### 决定

**`"workspace."` 整族进 `ADMIN_PREFIXES`，不加 carve-out。**

- 零 schema 变更、零迁移。
- 新 sibling 默认 fail-closed。
- 不做 owner 列：那会为一个**目前没有任何 UI 让 member 使用**的能力（per-user
  workspace）建一套权限模型，违 R10 的 YAGNI。若将来真的做 per-user workspace UI，
  owner 列可以在那时叠上来，本轮的闸不构成障碍。

### 改动清单

1. `src/gateway/method_admin.rs`：`ADMIN_PREFIXES` 加 `"workspace."`，注释写明上表的判据。
2. 同文件测试：`member_daily_methods_stay_open` 删 `"workspace.list"`；
   `credential_and_config_methods_require_admin` 加一个 `workspace.*` 代表方法。
3. 新回归测试，直接以 QA 抓到的动作命名：member 被拒 `workspace.update` / `workspace.archive`。
4. `src/gateway/method_visibility.rs` 模块 doc：现文写着"closing that needs an owner column
   plus a migration — a schema and product decision"，**本轮之后这句话是假的**，必须改写。
   ⚠️ **实施时修正了这一条**：原计划写"登记保留"，但该文件自带 cross-check
   `every_scoped_method_stays_open_to_members_in_method_admin` 断言"这张表声明的每个方法
   都不得是 admin-gated"——保留登记会让它红。仓里已有同一裁决的先例
   （`trace.list` / `trace.get` / `agents.teams` 因 admin-gate 刻意不登记）。故五个
   `workspace.*` **移出 `SCOPED_METHODS`**，改为照先例加"刻意缺席"pin，
   **两半都断言**（`treatment_of == None` 且 `method_requires_admin == true`），
   于是重新对 member 开放会按名字红掉并逼着把登记加回来。
   handler 里的 `partition_visible` **保留**——第二个 `UserRole::Admin` 主体的
   `CALLER_USER` 是他自己的 id 而非 `OWNER_USER_ID`，谓词对他仍生效；该 nuance
   由 handler doc 承载，因为那张表已刻意不再承载它。
5. `src/gateway/handlers/workspace.rs` 三处 handler doc：把"a member renamed and then
   archived a workspace the operator had just created"改成记录残留已在 admin 闸关闭，
   同时保留 `partition_visible` **单独**能买到什么的诚实边界。
6. `docs/reference/SECURITY.md` / `FEATURE_LOCATOR.md` 同步。

### 刻意不做

`aleph workspace create` 与 `aleph workspace archive` **本来就是坏的**：CLI 发
`{"name": …}`，handler 要 `id`（`CreateParams.id` / `GetParams.id` 均无 default）
⇒ 恒 `INVALID_PARAMS`。这不是本轮遗留，是顺手撞见的 operator-only 面 UX 问题；
混进来会让这轮的 diff 讲两个故事。**已与用户确认本轮不修**，在此存档以免下次重新发现。

---

## B · item 4 —— 存量数据在新谓词下的可见性

### 缺口

上一轮的真机 QA 跑在隔离 `HOME` 上 ⇒ 零存量数据 ⇒ 只测了**新建**对象。
而上一轮修的正是"会话行的 owner/scope 戳此前对每一条运行路径都为空"，
所以**真实部署里的每一行存量都是 NULL 戳**——恰恰是没被测到的那一类。

### 做法

隔离 HOME **开机前**用 sqlite 直接种下 `owner_user_id` / `scope_id` **全 NULL** 的
会话行与项目行。这是真实存量的数据形态；**不用 fixture 调 `stamp_owner`**——
上一轮的教训正是"fixture 手工构造的前置状态，必须回头问生产里谁写它"。

### 验收

1. operator `sessions.list` **看得见**这些行（缺省即 owner 收养）。
2. member `sessions.list` **看不到**。
3. member 对其中一行 `sessions.set_topic` → `not found`（而非 permission denied）。
4. `stream.running_set_changed` 的投影把解不出归属的元素**丢弃**，不放行。

---

## C · item 3 —— R7c artifact cap 边界

### 缺口

上一轮只验到"路由存在，且拒绝无效 cap"，没有真的 mint 一个 cap 再移除成员——
假 provider 不调工具。那条边界至今只由上一轮加的单测覆盖。

### 做法

绕开假 provider：member 经**已 carve-open 的 `tools.invoke`** 直接铸一个真 artifact cap。

退路：若 `tools.invoke` 够不到该工具，让假 provider 吐一个 `tool_use` 块——
SSE 内容完全由我们控制。

### 验收

1. member 在籍时用铸出的 cap 取 artifact → 成功。
2. operator 把该 member 移出 roster。
3. **同一个 cap 重放 → 必须被拒。**

---

## D · item 2 —— member 端 Panel UI

### 缺口

Panel 客户端**自己**拒绝明文远程连接（"Insecure transport"），与服务端
`allow_insecure_remote` 无关。所以上一轮 member 端 Panel UI 完全没验，
R6 面向 member 的错误文案只验了服务端拒绝那一半。

### 做法

配自签 TLS（服务端有 SAN 自动发现）+ `[gateway] host = "0.0.0.0"`；
Chrome 开 `https://<局域网IP>:<port>` 点过证书拦截页；operator 建 member 用户 +
`gateway.ticket.create`；第二个浏览器上下文用 `?bt=` 进 Panel。

### 验收

1. **R6 面向 member 的错误文案**（本项的主要目标）。
2. 侧栏只列自己的会话。
3. 用户目录能加载（上一轮 `ensure_loaded` 修复的那条，此前 40 帧里零个 `users.*`）。
4. 运行红点对 member 生效。

---

## 执行顺序

A（代码 + 编译验证）→ B（独立种子 HOME，一次启停）→ C 与 D 共用第二个实例。

## 验证命令

判据清单 §10 的五条，最少要跑：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo clippy --all-targets
```

`cargo check -p aleph-panel` 本轮不涉及 webchat 改动，但若 `interfaces/webchat/`
有任何改动（哪怕不是本轮改的）仍需跑一次。
