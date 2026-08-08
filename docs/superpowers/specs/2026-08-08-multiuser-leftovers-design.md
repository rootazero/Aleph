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

---

## 结果（2026-08-08 收尾）

| 项 | 结果 |
|---|---|
| A | 已实现并提交。`workspace.` 整族进 admin 闸；⚠️ 实施中修正了原计划的一条（见上文第 4 点：五个方法必须**移出** `SCOPED_METHODS`）。**真机复验**：member Panel 上 `workspace.list` / `workspace.archive` 双双被拒。 |
| B | **10/10 通过**。另记两条产品后果：member 升级前的自有会话被 operator 收养且对他永久消失；房间还在但房间历史对成员消失（`scope_id` NULL ⇒ 那行不声明自己是房间，名册无从介入）。 |
| C | **14/14 通过**。RPC 面立刻关 / 字节面服务到 TTL，两个答案不同且都正确；新铸不可能。 |
| D | R6 文案**验证通过且正确**（诚实点名角色，且匹配服务端同一个常量）。另发现 4 个 UI 层问题（非泄漏，服务端一律拒绝）：设置树全可见 · 引导清单对 member 撒谎 · 执行档位 pill 静默退化 · enroll 写失败复用读文案。 |

### 本轮踩到、值得记住的三个坑

1. **`ALEPH_HOME` 指向 `.aleph` 目录本身**（`utils/paths.rs:172`），不是一个替身 `$HOME`。指错一层会**静默新建整套空库**，所有探针于是在跟一个空实例说话——症状读起来是"凭据过期了"，不是"路径写错了"。第一轮 item B 的 6 个 PASS 全是这么来的假绿。
2. **异步入队的 RPC + 完美的拒绝形状 = 时序 bug 伪装成安全行为。** `chat.send` 返回即入队，会话行由 run 内 `ensure_session` 稍后写；紧接着的 `artifacts.list` 得到的 `session not found` 与可见性拒绝**逐字节相同**。要轮询。
3. **隔离 HOME 的 QA 结构上只测得到新建对象。** 存量是它测不到的另一个类别，补法是把已有行改成迁移前的形态，而不是让 fixture 造一个"看起来像旧的"状态。

## 验证命令

判据清单 §10 的五条，最少要跑：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo clippy --all-targets
```

`cargo check -p aleph-panel` 本轮不涉及 webchat 改动，但若 `interfaces/webchat/`
有任何改动（哪怕不是本轮改的）仍需跑一次。

---

# 附录 · 第二轮（2026-08-08 下午）—— 四个 UI 层发现 + 一个功能性死路

上一轮 item D 记录了 4 个 UI 层发现但未修。本附录是它们的收尾，外加侦察中撞见的
第五项（member 无法批准自己的工具调用）。

## 侦察结论：四条里有三条是同一个根因

`Err` 被折成一个**值**，于是 UI 替服务器发明了一个它从未说过的答案：

| 面 | 代码 | `Err` 被读成 |
|---|---|---|
| 引导清单 | `Err(_) => ready.set(Some(false))` | 「没配置」← **唯一的假话** |
| 执行档位 pill | `Err(e) => console::warn` | 「没有可选档位」+ 空标签 |
| 会话模式 pill | 同上 | 「本版没有这个功能」（pill 整个消失） |
| ~20 个设置页 | `format!("Failed to load X: {e}")` | 原始英文协议串 |

新判据已进 CLAUDE.md §0：**「被拒」不许读作「没有」**。

侦察还发现两处记录里没有的**同构第二实例**：会话模式 pill 与档位 pill 读同一个 RPC；
cluster 页的 `注销` 与 `+ Enroll` 两个**写**动词共用了**读**的拒绝文案。

## 决定与实现

### A · 设置树（发现 1）：不藏，只把拒绝说清楚

推翻「隐藏 admin 页」的直觉——2026-08-07 刚刻意删掉 `DashboardState::is_operator()`
并留了源码级 pin（`cluster.rs::the_cluster_page_holds_no_client_side_role_gate`），理由是
客户端捕获的角色在 `restamp_live_connections` 之后两个方向都是错的。隐藏＝同一个闸换名字。

- 新 `components/admin_refusal.rs`：`is_admin_refusal` / `labeled` / `settings_load_error`，
  单一源 `aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE`（服务端发的那个常量本身）。
- 19 个设置页的 load 错误改走 `settings_load_error`，**非拒绝一律原样透传**
  （degraded copy 胜过错误断言）。新增 i18n 组 `settings.admin_refusal`（en/zh）。

### B · 引导清单（发现 2，唯一的假信息）

`Option<bool>` → 四态 `StepStatus{Unknown, Ready, Pending, Restricted}`；
**只有 `Ok` 有资格产生 `Pending`**。被拒的步骤不再提供 CTA 链接（那是同一句假话的后半），
计数器另报 `· N restricted` 而不把被拒项算成待办。

### C · 两个 pill（发现 3 + 其同构实例）

根因在服务端：`config.get_tool_permissions` 被 `config.` 整族闸住，而这两个旋钮的
**写面对 member 一直是开的**（`sessions.patch` / `chat.send` 的 `exec_tier`+`mode`）——
门开着、菜单锁着。照 `gateway.metrics.run_concurrency` 的既有形状 carve-out，
非 admin 响应**按移除构造**（掉 `default` + `overrides`，留四个 id 枚举），
写兄弟 `config.update_tool_permissions` 照旧闸住。UI 侧两个 pill 在被拒时显式说明；
模式 pill 的 hide-on-empty 规则新增 `|| refused`，因为"消失"是读者唯一分不出
「没权限」和「本版没这功能」的结局。

### D · cluster 写动词文案（发现 4）

`fleet_error_label(err, action)`，三个动词各自命名（`ACTION_READ_FLEET` /
`ACTION_ENROLL` / `ACTION_DEREGISTER`）。`注销` 此前把失败塞进**读**用的 `error` 信号，
故 `error` 改为 `(String, &'static str)`。

### E · member 无法批准自己的工具调用（本轮新增，用户确认纳入）

`exec.` 整族 admin-gated、无 carve-out ⇒ 默认 `Auto` 档下 member 的每个非幂等工具调用
park 满 120s 后死于 `Timeout`，**结构上无解**。

两半一起修（只修 RPC 半边是哑弹——Panel 的 `pending_approvals` 只由 `approval.*` 事件刷新）：

1. **RPC 面**：`exec.approvals.pending` / `exec.approval.resolve` 进 `MEMBER_CARVE_OUTS`，
   在 handler 内按记录自带的 `session_key` 作用域化。"不是你的"与"不存在"**同一条臂、同一个 code**。
2. **事件面**：`EventScopeGuard` 删掉 `approval.` 规则（角色回答不了"他自己的"），
   `event_visibility` 新增 `SessionIdentity::BySessionKeyOrAdmin` —— member 收自己的，
   operator 收全部（＝它此前的行为逐位不变）。`surface.approval` 横幅腿保持角色闸。

⚠️ **实施中自我抓到一个缺陷**：RPC 面第一版用 `visible_owner_filter().is_none()` 短路，
但 **operator 的 `CALLER_USER` 是 `OWNER_USER_ID` 而非 `None`** ⇒ 过滤对 operator 也生效
⇒ 事件面放行、列表面过滤，而 Panel 每收一帧就按列表面重建 ⇒ **卡片到达后当场消失**。
改用 `caller_identity::caller_is_member()`（＝ admin 闸自己的谓词），并加
`an_operator_still_sees_a_members_parked_approval` 钉住；已用真实变异证其 RED。

## 顺手修掉的两个先于本轮存在的问题

1. **Panel 整个单测目标编译不过**（`context.rs` 的 `use super::role_is_operator` 悬空，
   随 2026-08-07 删 `is_operator()` 留下）。`cargo check` 不编译 `#[cfg(test)]`，所以它
   静默地让这个 crate 的 732 个测试一天没跑过——本轮新写的 Panel 测试同样一条都不会跑。
2. **CI fmt 门红**：`src/gateway/event_bus.rs` 一处（非本轮文件）。fmt 挂了同 job 的
   clippy 永远走不到。

## 未做 / 已知缺口

- ~~**CLI `aleph workspace create|archive` 仍然是坏的**~~ —— **已于第三轮修掉**，见下。
- **本轮零真机验证**：全部是编译 + 单测 + 变异验证。member 端 Panel 的实际观感
  （四态徽标、两个 pill 的说明行、19 个设置页的文案）需要一次两用户真机 QA 才算闭环。
- **`clarification.pending` 有与 E 同形的 operator 缺口**：它用 `visible_owner_filter()`，
  所以 operator 看不到 member 的待答问题。未改——它没有本轮那个"事件面放行/列表面过滤"
  的自相矛盾（`clarification` 的帧走 `BySessionKey`，两面一致），所以是产品决定不是 bug。

## 验证

```
cargo test -p alephcore --lib                                  15604 passed / 0 failed
cargo test -p alephcore --features test-helpers --test '*' --no-run   0 errors
cargo test -p aleph-panel                                        732 passed / 0 failed
cargo check -p aleph-desktop-macos                               0 errors
cargo clippy --all-targets                                       0 errors（4 条既存 warning，均非本轮文件）
cargo fmt --all -- --check                                       clean
```

---

# 附录 · 第三轮（2026-08-08）—— CLI `workspace` 家族

前两轮把这一项**存档不修**（"混进来会让这轮的 diff 讲两个故事"）。本轮按用户指令修掉。

## 实际有三处坏，不是一处

| # | 症状 | 真因 |
|---|---|---|
| 1 | `aleph workspace create` 恒 `INVALID_PARAMS` | CLI 发 `{"name", "description"?}`，handler 要 `{"id", "name", …}` |
| 2 | `aleph workspace archive` 恒 `INVALID_PARAMS` | CLI 发 `{"name"}`，handler 要 `{"id"}` |
| 3 | `aleph workspace list` 的 `Status`/`Created` **每行都是 `-`** | 读 `status`/`created`，而 `AgentEnv` 序列化出来的是 `is_archived`/`created_at` |

第 3 条此前无人报告，因为它**不报错**——`.unwrap_or("-")` 把"我问的字段不存在"渲染成
"这个工作区还没有状态"。它和 1、2 是同一个根因的两种表现。

## 根因：契约写了两遍，而"测试"是把字面量跟自己比

`aleph-cli` 的 `Cargo.toml` 用大写字母写着 **MUST NOT depend on alephcore**（它同时是协议的
参考实现）。于是 wire 形状被写了两遍：handler 一份 `#[derive(Deserialize)]`，CLI 一份
`serde_json::json!` 字面量，中间**没有任何东西连着**。而 CLI 那侧仅有的两个测试是：

```rust
let params = serde_json::json!({ "name": "test-ws" });
assert_eq!(params["name"], "test-ws");   // 恒真；测的是 serde_json
```

**一个只读自己刚写下的字面量的断言永远绿**，所以三处坏了多久都没人知道。

## 做法

1. **`shared/protocol/src/workspace.rs`（新）＝ wire 契约单一源**：`WorkspaceCreateParams` /
   `WorkspaceRef`（`get` 与 `archive` 共用，它们寻址同一个东西）/ `WorkspaceUpdateParams` /
   `WorkspaceRow` / `WorkspaceList`。两侧共用 ⇒ **重命名是编译错**，这是唯一挡得住
   "名字看起来对"的守卫。`shared/protocol` 是双方都依赖的那一侧（判据：真源必须在被依赖的一侧），
   且已有先例（`jsonrpc::ToolCallParams`）。
2. **handler 删掉三个本地 param struct**，改用协议类型。
3. **CLI 位置参数改为 `id`**（＝`archive` 唯一能寻址的键），新增 `--name`（缺省＝id，
   镜像 `AgentEnvStore::create` 的服务端缺省）、`--icon`。位置参数的**调用形式不变**
   （`aleph workspace create foo`），只有帮助文本和语义变了——此前它没有任何能工作的行为可破坏。
4. **`list` 表头改成 `ID | Name | Description | Created`**，读真实字段。
   - `ID` 排第一：另外两个子命令只认它，一张只显示 display name 的表让 `archive` 无处可抄。
   - **刻意不留 `Status` 列**：`handle_list` 硬编码 `list(false)`（不含已归档），所以那一列
     结构上恒为 "active" —— 恒真的列等于没列。`WorkspaceRow` 因此**不含 `is_archived`**
     （一个没有渲染者的展示字段就是断线）。
   - `--json` 是**原样透传**，不经投影：客户端渲染不了的形状不该阻断原始输出。
     表格路径则相反——解析失败是要喊出来的错误，不是一表破折号。

## 验证过的、以及故意没做的

- **变异验证**：给 `WorkspaceRow.created_at` 加 `#[serde(rename="created")]`（＝历史 bug 的
  形状，且能编译），`every_column_the_cli_renders_is_present_in_the_list_response`
  当场 RED（`missing field 'created'`）。直接改字段名则是**编译错**——比测试更早。
- **查过但不是 bug**：`handle_create` 硬编码 profile `"default"`，而 `AgentEnvStoreConfig`
  另有 `default_profile` 字段——同一事实的第三份拷贝。**没改**：`load_profiles` 自己会补种
  `"default"`（`mod.rs:589`），而 `default_profile` 无任何用户配置入口、全仓恒为 `"default"`，
  所以三份拷贝今天字节相同、无法分歧。记在这里以免下次重新推理一遍。
- **`workspace.get` / `workspace.update` 至今没有任何客户端**（Panel 也没有）。本轮**没有**
  顺手加 CLI 子命令——那是新功能不是修 bug。
- **归档后无法从 CLI 复看**：`workspace.list` 不接 `include_archived`，所以 `archive` 的确认
  只能靠"那行从列表里消失"。没加参数（新 wire 面），记为已知缺口。
- **零真机验证**：本轮全部是编译 + 单测 + 变异验证。真机需要一个在跑的 server 上
  `aleph workspace create/list/archive` 走一遍。
- **同一个形状还有第三个实例，本轮只上报不修**：`channels.set_agent` 的参数在 Panel
  （`interfaces/webchat/src/api/workspace.rs`，手写 `json!({channel_id, agent_id})`）与
  handler（`SetAgentParams`）之间同样是两份互不相干的声明。**今天字节相同，逐行读过**，
  所以它不是 bug；但它是同一类漂移风险，且收敛它要把 Panel crate 拖进这次 diff。
  留作单独讨论。

## 顺手确认（非本轮引入）

`cargo test -p aleph-cli` 有一个**先于本轮**就红的测试：`parse_webhook_and_proxy_preview_surfaces`
断言 `aleph webhook add` 能解析，实际不能。已用 `git stash` 确认与本轮改动无关，**未修**。
