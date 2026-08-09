# Workspace：unarchive 动词 + Panel 客户端面 — 设计

> 2026-08-09。承接 2026-08-08/09 三轮 workspace 收尾（CLI 契约拆分 → get/update 接线 →
> 超发字段 CUT）留下的两项：**archive 是否终态**，以及 **workspace.\* 有没有第二个客户端**。

## 0. 这轮解决什么

上一轮把 `workspace.*` 的线上形状钉死在 `aleph_protocol::workspace` 契约类型上，并把整族
admin-gate 了。留下两个尾巴，都是**语义**而非接线：

1. `archive` 是**明确终态**——归档行可读不可写，id 永久占用，`update` 的 doc 里写着
   "terminal **by construction**, not by omission"。误归档无法撤销。
2. `workspace.*` 只有 CLI 一个客户端。admin gate 的推理正是建立在
   "The Panel has **NONE**" 之上。

## 1. 已查实的事实（决策依据，非推测）

| 事实 | 锚点 |
|---|---|
| `create` 是裸 `INSERT`，`id` 是主键 ⇒ **归档行今天已经占着 id**，撞上是 `AlreadyExists` | `agent_env/manager_ops.rs:58` |
| `archive` 只翻 `archived = 1`，**不动磁盘、不清 channel 绑定** | `manager_ops.rs:279` |
| workspace id **同时是 `~/.aleph/agents/<id>/` 的目录名**——notes vault / memory 分区 / skills / `OPEN_LOOPS.md` 全在里面 | `utils/paths.rs:670`、`config/agent_resolver/mod.rs:491` |
| `AgentEnvStore::archive` 全仓**只有一个生产者**（`handle_archive`），**没有工具面** | grep 全仓 `.archive(` |
| `workspace.*` 整族 admin-gated（前缀 `"workspace."`），新方法**自动**落进闸内 | `gateway/method_admin.rs:152` |
| Panel **可以**依赖 `aleph_protocol`（已有 6 处在用） | `platform/wide/views/chat/team_events.rs` 等 |
| phone 侧新 `/settings/*` 路由自动落 `PhoneSettingsScreen::Wrapped`，无需额外工作 | `platform/phone/settings/mod.rs:83` |

## 2. 决策 A — 归档期间 id 不可被 `create` 抢占

**保持现状（id 占用），但把拒绝话术精确化。**

决定性理由：workspace id 不只是一行记录，它是 `~/.aleph/agents/<id>/` 的**目录名**。让
`create` 抢占归档 id，等于让一个全新 workspace **静默继承**前一个的 notes vault 与 memory
分区，并顺手覆盖归档行的 name/description——两步都合法、合起来是一次无声的数据收养。要做对
就必须连磁盘目录一起搬走或删掉，那是把一个**破坏性文件系统操作藏在 `create` 这个动词后面**。

**代价（要说清楚）**：archive 掉 `crypto` 之后就再也建不出一个「全新的」`crypto`，只能
unarchive 后清理，或换个 id。这是诚实的限制——底层目录本来就没法被两个 workspace 共享。

被否决的两条：
- **`create` 可抢占**——见上。
- **另加 `workspace.delete` 硬删**——语义最干净，但磁盘目录问题不变，且本轮范围翻倍。
  记为 backlog，不在这轮做。

## 3. 决策 B — Panel 做全量 CRUD 设置页

与 CLI 平权（CLI 已有 `list|get|create|update|archive`）。这轮新增的正是**写面语义**
（unarchive），只做只读面等于新动词只有一个面被真机验证过。

`workspace.*` 是 admin-gated，所以 member 打开该页必然吃拒绝。**判据（判据清单 §0）**：
只有 `Ok` 有资格断言被读的那个东西；`Err` 的每一种都只能说"我不知道"。读失败走
`admin_refusal::settings_load_error`，写失败走 `admin_refusal::labeled`——member 读到本地化的
「需要 operator 权限」，**不是空列表，也不是裸协议串**。

**刻意不做**：不在客户端预先按角色隐藏这一页。`admin_refusal.rs` 的模块 doc 明写
"Do not use `is_admin_refusal` to pre-emptively hide a page — that is the same gate under a
new name"，且 Panel 于 2026-08-07 刻意删掉了客户端角色谓词。渲染、调用、如实转述服务端的话。

## 4. 实现

### 4.1 Store — `AgentEnvStore::unarchive`

```rust
pub async fn unarchive(&self, id: &str) -> Result<Option<AgentEnv>, AgentEnvError>
// UPDATE agent_envs SET archived = 0, last_active_at = ?1 WHERE id = ?2
// affected == 0 -> Ok(None)；否则 self.get(id).await
```

三个决定：

- **返回 `Option<AgentEnv>` 而非 `bool`**，与 `update` 同形，让 handler 直接组装 envelope。
- **`Ok(None)` 的含义比 `update` 干净**：这条 UPDATE 没有 `archived` 谓词，所以 `None`
  只可能是「没这行」，不存在 `update` 那种「不存在 or 已归档」的二义。写进 doc——
  否则下一个读者会照抄 `update` 那套二次探测。
- **幂等**：对本来就 active 的行返回成功。后置条件（这个 workspace 是活的）成立，且这与
  `archive` 今天的行为对称（对已归档行 `archive` 同样回 `Ok(true)`，SQLite 的 `changes()`
  数的是被语句匹配的行，不是值真的变了的行）。
- `id == "global"` 同 `archive` 拒 `CannotModifyGlobal`——它是同一张表上的写动词。

### 4.2 Handler — `handle_unarchive`

复用 `WorkspaceRef` 参数与 `WorkspaceEnvelope` 响应，**零新协议类型**。

- `partition_visible` 闸 → 与 `handle_archive` **逐字节相同**的 `not_found`
- `Ok(Some(ws))` → `{"ok": true, "workspace": detail_of(&ws)}`
- `Ok(None)` → `not_found`
- `Err(e)` → `INTERNAL_ERROR`

**故意不对称于 `archive`**（后者只回 `{"ok": true}`）：archive 的结果是"这行从默认视图消失
了"，没什么可展示；unarchive 的结果是"这行回来了"，调用方要拿它重渲染，多一次 `get` 就是
一个竞态窗口。理由写进 doc，否则读者会当成遗漏去"修正"。

### 4.3 `create` 撞归档 id 的拒绝话术

`handle_create` 的 `Err(AlreadyExists)` 臂补一次 `get_including_archived` 探测：

- 归档 → 指名 unarchive 的消息
- 活着 / 探测失败 → **保持原字节不变**（探测只用于把一个已经为真的拒绝升级得更具体，
  答不上来就退回它原本要精化的那个——与 `handle_update` 同一个形状）

**硬约束**：partition-invisible 分支造的是 `AgentEnvError::AlreadyExists` 原值，必须与
**活行**碰撞逐字节相同。现有测试 `the_workspace_writes_deny_a_foreign_partition_composed_id`
比的正是活行，方向对；要用变异证过 RED。

不构成 existence oracle：invisible id 在读库**之前**就返回了，永远拿不到归档那条话术；而
visible id 的归档状态 `workspace.get` 本来就直说。

### 4.4 CLI — `aleph workspace unarchive <id>`

打 `detail_pairs`（复用），`--json` 透传原始 envelope。与 `archive` 同一个 `WorkspaceRef`
构造，走同一条 shared-type 保证。

### 4.5 Panel

**先做一次改名清理**（当前文件自己的 doc 承认命名是错的）：

- `api/workspace.rs`（内容其实是 agent↔channel binding）→ `api/agent_binding.rs` /
  `AgentBindingApi`；改 2 个消费者（`agent_binding_selector`、`agents_sidebar`）
- `api/workspace.rs` 腾给**真正的** `WorkspaceApi::{list,get,create,update,archive,unarchive}`，
  **用 `aleph_protocol::workspace::*` 契约类型**——重命名 ⇒ 编译错，正是那个模块存在的理由

**页面** `/settings/workspaces` → `platform/wide/views/settings/workspaces.rs`，照
`routing_rules.rs` 的两栏式（列表 + 编辑器）：

- 左栏：列表 + `include_archived` 开关；归档行带 Status 徽标，动作位换成 Unarchive
- 右栏：新建 / 改 name·description·icon / archive / unarchive
- 读失败 `settings_load_error(i18n, &e, |e| format!("Failed to load workspaces: {e}"))`
- 写失败 `labeled(&e, <该动作自己的解释>)`——一句话不能同时诚实描述被拒的读和被拒的归档，
  所以解释由调用点给

注册三处：`app.rs::desktop_settings_body` 路由、`components/settings_sidebar.rs`
（`SettingsTab::Workspaces`，放 **Advanced** 组——它是服务器全局治理，不是 AI 配置）、
i18n key `settings.workspaces.*` + `settings.tabs.workspaces`。

### 4.6 连带债：四处会当场变假的陈述

判据清单 §0「同一事实的两份表述，只改一份就是静默说谎」。同批修：

| 位置 | 现在写着 | 改成 |
|---|---|---|
| `manager_ops.rs::get_including_archived` doc | "there is no unarchive verb" | 有了，且说明为什么读仍不需要它 |
| `manager_ops.rs::update` doc | "terminal **by construction**, not by omission" | 可逆；`update` 仍拒归档行，回路是 unarchive |
| `method_admin.rs` workspace 段 | "The Panel has **NONE**" | 两个客户端，**gate 结论不变**（Panel 面同样 operator-only） |
| `shared/protocol/src/workspace.rs` 模块 doc | "The family's only client is the CLI" | 两个客户端；跨 crate 共享类型的理由不变 |

## 5. 测试

**Store**（`manager_ops.rs`）
- `unarchive` 翻回 `archived = 0` 且读回活行
- 未知 id → `Ok(None)`
- 对 active 行幂等成功
- `global` → `CannotModifyGlobal`

**Handler**（`handlers/workspace.rs`）
- **主张**：archive → `get` 报 `is_archived: true` → `update` 被拒 → `unarchive` →
  `get` 报 `is_archived: false` → **`update` 重新落地**。断言在 **store** 上，不在响应上——
  一个返回 `ok` 却没写库的 handler 会通过只看响应的测试。这条测的是"终态变可逆"本身。
- partition-invisible id 的 `unarchive` 拒绝与「不存在」逐字节相同，且**不翻标志位**
- `create` 撞归档 id：消息指名 unarchive；撞活行 + partition 拒绝两者仍逐字节相同
- 扩 `the_read_responses_carry_the_contract_and_nothing_else` 到 unarchive 的 envelope

**Gate**：`method_admin.rs` 的 workspace 枚举表加 `workspace.unarchive`

**CLI**：`unarchive` 的参数形状到达 handler（shared-type 保证）

**Panel**：先跑一次 `cargo test -p aleph-panel --lib --no-run` 确认该 target 是否本来就红。
属实则照实报告，不假装覆盖。

## 6. 真机 QA（本轮重点）

上一轮的教训是真机抓到了编译验证看不见的东西（保存按钮、UTC 列）。这轮改的是线上响应形状。

1. 起 `aleph-server`（loopback）
2. CLI 全链路，**把 `--json` 的实际响应贴出来**：
   `create → list → get → archive → list --include-archived → get(已归档) →
   update(期望 PERMISSION_DENIED 且消息含 archived) → create 同 id(期望新话术指名 unarchive) →
   unarchive → get(active) → update(现在应落地)`
3. Chrome 开 Panel `/settings/workspaces`，做完整 CRUD + archive/unarchive，看 WS 帧
4. **member 拒绝路径**：需 `0.0.0.0` + 从局域网 IP 连（loopback 首行短路成 operator）。
   会尝试；做不到就明说，不跳过不假装。

## 7. 明确不做

- `workspace.delete` 硬删（backlog）
- workspace 的**工具面**（R8 意义上）——今天 `archive` 就没有工具面，本轮不新开
- owner 列 / 每用户 workspace 权限模型——`method_admin.rs` 已论证：那是产品能力，
  落地时叠在这道 gate 之上，不是替换它
- phone 原生 settings 屏——新路由自动走 `Wrapped`，与其余 17 页一致
