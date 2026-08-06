# Multi-User · Organization · Project Rooms — Design Spec
# 多用户 · 组织 · 项目房间 — 设计规格

- **Date**: 2026-08-04
- **Status**: Approved design (brainstorming complete; awaiting implementation plan)
- **Reference**: qm (`smb://mac-mini-m4.local/tbu4/Github/qm`) — "a multiplayer agent harness for work"
- **Origin**: Panel 未规划的「项目管理」功能 → 展开为用户系统 + 组织 + 项目共享 + 记忆隔离的整体设计

---

## 1. 背景与动机 (Background)

Aleph 今天是单用户产品：信任模型是「过了登录墙就是 operator」，记忆按 agent 分区，会话没有归属人。Panel 需要一个项目管理功能，而参考 qm 项目后确认：**多用户共同分享实施项目**正是项目管理功能的正确形态。这要求补齐用户系统与组织能力，并解决记忆的用户/组织隔离。

**qm 的核心启示**（本设计的参考基线）：

- 一切共享机制建立在统一的 **Scope** 抽象上（`kind:ref`；personal / channel / team / org / group 五种）
- **Project 是薄实体**（roster + 物化为一个 group scope）——项目没有任何专属共享机制，全部复用 scope 通用机制
- **Grant** 做跨 scope ACL；每轮对话由 **Resolution** 合成权限/挂载/记忆视图
- **关键简化：一个部署 = 一个组织**（orgId 来自配置）——qm 没有做多租户

## 2. 已锁定的五个决策 (Settled Decisions)

逐个澄清并经用户确认：

| # | 问题 | 决策 |
|---|------|------|
| D1 | 租户形态 | **一台服务器 = 一个组织**（qm 模式）。org 是部署级常量，不贯穿表结构。个人使用 = 单成员组织，现有单机形态零成本兼容 |
| D2 | 信任模型 | **隐私隔离 + admin/member 分权**。不防恶意成员，不做 per-user OS 沙箱；数据默认私密，隔离强制在数据访问层，工具执行复用现有 exec tier + sandbox 硬底线 |
| D3 | 项目实体 | **全形态房间**：共享会话（项目群聊）+ 项目记忆 + 项目工作区 + 任务/目标/后台工作，四者都是项目 scope 的一等公民 |
| D4 | Agent 归属 | **人格可共享，实例按 scope**：agent 定义（人格/soul/配置）是组织级资源；记忆/状态按 (agent × scope) 实例化 |
| D5 | 身份链接 | **User 实体 + 多身份链接**：Panel 设备与渠道 peer 都是链接到 User 的凭据/化身；同一个人在 Telegram 和 Panel 是同一个 personal scope、同一份记忆 |

## 3. 架构方案 (Architecture Choice)

**选定方案 B：分区键组合 + 网关强制点**（已确认）。

- **记忆**：复用 `src/memory/project_scope.rs` 已验证的分区键组合模式（`scoped_agent_id(base, ns)` + `read_scope_ids` 读取并集），scope 成为分区后缀，不加 schema 维度
- **会话**：复活 `SessionIdentityMeta` 休眠接缝，补 owner/scope 归属
- **强制点**：单一网关咽喉解析 `caller → user → visible scopes`（模仿 `src/tools/scoped/` 是 exec tier 唯一强制点的纪律）
- **项目**：升级现有 `src/projects/` 实体，共享全部走 scope 通用机制

**否决的方案**：

- 方案 A（qm 式全量 Scope 抽象 + Grant ACL + 每轮 Resolution）：概念最纯净但要求每个存储子系统改造，Grant 的逐资源授权短期零消费者，违 YAGNI。**B 与 A 词汇兼容**（ScopeId 一致），将来需要逐资源 Grant 时再加表，不堵死演化
- 方案 C（逻辑多实例，每用户一个命名空间子根）：隔离彻底但「共享组织内项目」恰恰变成最难的事，与核心诉求方向相反

## 4. 身份系统 (Identity System)

### 4.1 User 实体

`security.db` 新表 `users`：

```
users(user_id, display_name, role, status, created_at)
  role:   admin | member
  status: active | deactivated
```

一台服务器一个组织 ⇒ **没有 org 表**；组织元数据（名称等）进 `config.toml [org]`。

### 4.2 身份链接（User 是锚，凭据是化身）

复用两张现有表，各加一列 `user_id`：

- **Panel 设备**：`devices.user_id`。配对流程不变，批准时多一步「这台设备属于谁」——admin 批准新用户的首台设备；用户自己批准自己的后续设备
- **渠道 peer**：`pairing_store` 的 `(channel, sender_id)` 行链接到 `user_id`——现有「渠道→人」雏形正式升格

### 4.3 认证 UX：不建密码体系

设备 token 即凭据（现状机制）；「登录」的语义 = 「此设备已链接到某 User」。远程新成员入职 = admin 发一次性配对票（现有 `aleph-bt-*` 机制）。避免自建密码栈的全部安全负担（存储/重置/爆破防护）。

### 4.4 零迁移

现有部署首次启动新版本：自动创建隐式 **owner 用户**（admin 角色），收养全部现有设备、渠道配对、会话、记忆。loopback 连接继续零配置 = 自动以 owner 身份进入。**单机形态的体验一个字节都不变。**

### 4.5 Caller 身份管道

`CALLER_ROLE` / `CALLER_IS_LOOPBACK` task-local 升级为 `CallerIdentity { user_id, role, is_loopback }`——这是 `shared/protocol/src/auth.rs` 休眠的 `IdentityContext` 类型的复活（原消费者 PolicyEngine 已删；给它新消费者而不是再造类型）。

### 4.6 admin / member 分界

`method_authz.rs` 从「channel tier 闸」扩展为「user role 闸」：

| 能力 | admin | member |
|---|---|---|
| 服务器配置 / providers / channels / 集群 / 全局密钥 | ✅ | ❌ |
| 用户管理（邀请/停用/改角色） | ✅ | ❌ |
| 自己的 personal scope（会话/记忆/agent 实例） | ✅ | ✅ |
| 创建项目、管理自己拥有的项目 | ✅ | ✅ |
| 工具执行（受 exec tier + sandbox 硬底线约束） | ✅ | ✅ |

### 4.7 v1 明确不做

OIDC/SSO、密码登录、per-user OS 账户、guest/外部访客角色（`Role::Guest` 类型保留但不启用）。

## 5. Scope 模型与数据隔离 (Scopes & Isolation)

### 5.1 Scope 词汇表（三种，YAGNI）

```
org                    组织唯一 scope（部署级，不带 ref）
personal:<user_id>     每用户一个
project:<project_id>   每项目一个
```

qm 五种 kind 里的 channel/team/group 由 project 一种覆盖。

### 5.2 记忆实例化 = 分区键组合

- 分区后缀即 scope：`main__u-alice`（personal）、`main__p-x7f2`（project）
- **裸 base id（如 `main`）= org scope**——现有单用户数据天然就是 org 层，隐式 owner 收养后语义自洽
- **召回并集**（`read_scope_ids` 扩展）：
  - personal 会话 → `[org, personal:me]`
  - project 会话 → `[org, project:x]`——**默认不召回个人记忆**。理由：项目群聊全员可见，个人私密记忆一旦被 agent 引用即向全项目泄露；比 qm 的 visible 合成收紧一档，将来可加 per-user 开关
- **写入单点**：capture 只写会话的归属 scope（personal 会话→个人记忆，project 会话→项目记忆）
- **Curated 文件**：`USER.md` 变为 per-user（跟 personal scope 走）；`MEMORY.md` 按 (agent × scope) 实例化
- **Floors 分床**：现有「Floors stay global」不变量拆开——user-profile floor（USER.md）跟 personal scope 走，只进该用户的 personal 会话（项目会话是多人房间，「注入谁的 USER.md」无良好答案且有泄露面，与上一条的保守立场一致）；agent 反馈/行为 floor 归 org（对全组织生效）

### 5.3 会话归属

session 元数据补 `owner_user_id` + `scope_id`（复活 `SessionIdentityMeta`）。可见性：personal 会话仅 owner 可见；project 会话项目成员可见。v1 不设 org 级会话。

### 5.4 唯一强制点

gateway dispatch 入口一次性解析 `CallerIdentity → VisibleScopes`，落 task-local；session / memory / project / artifact 的 RPC handler 一律从它取过滤条件，**不自己写第二份判断**。任何新的返回 scoped 数据的 surface 不经过它就自带旁路——与 exec tier 唯一强制点同款纪律，配同款守卫测试。

### 5.5 后台工作归属

goals / loops / crons / 委派子代理记录 `scope_id + owner_user_id`，唤醒时以该 scope 种入 task-local（否则 daemon 自主干活时「以谁的身份读写记忆」无答案）。推送路由同理：project scope 的产出推给项目成员，personal 的只推 owner。

### 5.6 保持全局（admin 治理）

providers / models / channels 配置 / 集群 / 全局密钥 / skills（v1 全局；scope 级技能共享后置）。

## 6. 项目房间 (Project Rooms)

### 6.1 Project 实体升级

现有 `src/projects/` 从「最近目录名册」升格为一等实体，存储从 capped JSON 移入正式表：

```
Project {
  id, name,
  owner_user_id,          // 创建者
  member_ids: [user_id],  // 名册——成员制即授权，v1 不做逐资源 Grant
  workspace_path: Option,  // 绑定的工作区目录（文件/仓库）
  status: active | archived,
  created_at, updated_at
}
```

scope id 由它派生（`project:<id>`）。**项目本身没有任何专属共享机制**——四个一等公民全是 scope 通用机制在这个房间里的表现。现有「最近工作目录」快捷选择器保留给 personal 会话用，两者不混同。

### 6.2 四个一等公民

1. **共享会话（项目群聊）**：会话落 `scope_id = project:<id>`，成员可见可发言。人类消息在 `session_events` payload 携带 `author_user_id`（多人房间「谁说的」是必需信息，也进 agent 的 prompt 视图，如 `[alice]: ...`——作为 payload 的自然携带物，不开旁路）。忙碌车道语义按会话粒度复用现状。实时面复用 per-session 事件订阅
2. **项目记忆**：即 project scope 记忆（§5.2）。Panel 项目页提供记忆浏览面（curated MEMORY.md + notes，成员可见）
3. **项目工作区**：项目会话默认工作目录 = `workspace_path`；成员的 agent 在其中读写文件、跑命令（受 exec tier + command policy 硬底线约束）
4. **任务/目标/后台工作**：kanban（复用 teams 任务存储）、goals/loops、AI 团队开在项目 scope 下；按 §5.5 以项目身份运行、进展推送全体成员（R5）

### 6.3 名册操作

裁决形态照 qm ProjectStore：`ok / not_found / forbidden / invalid_member`。创建（创建者即 owner）、加/移成员（owner 或 admin）、改名、归档。owner 变更由 admin 执行；v1 不做邀请-接受握手（组织内同事，加了就加了）。

### 6.4 Panel 项目管理 UI（信息架构）

```
侧栏「项目」→ 项目列表（我参与的）
  → 项目页
      ├─ 群聊（默认 tab，多人 + agent）
      ├─ 任务看板（kanban + goals/loops 进度）
      ├─ 工作区（文件浏览，复用现有 directory_browser）
      ├─ 记忆（项目 MEMORY.md + notes 浏览）
      └─ 设置（名册管理 / 工作区绑定 / 归档）
```

## 7. 组织治理 (Org Governance — 刻意薄)

- 组织元数据进 `config.toml [org]`
- Panel Settings 增加**用户管理面**（admin 专属：发配对票邀请、停用/复活、改角色、查看身份链接）
- providers / channels / 集群等既有设置面自然落进 admin 闸
- agent 人格目录 v1 保持全局、admin 治理；member 自建人格、技能的 scope 级共享（qm 的 grant/promotion）后置

## 8. 分期路线 (Phasing)

每期独立可交付、可验证，任何一期停下来都是完整产品：

| 期 | 内容 | 验收标准 |
|---|------|----------|
| **P0 身份地基** | users 表 + devices/pairing 链接 + CallerIdentity + 隐式 owner 迁移 + method_authz 角色闸 | 多用户多设备各自登录；member 调不到 admin RPC；**单用户体验零变化** |
| **P1 数据隔离** | scope 词汇 + 会话 owner/scope + 可见性咽喉 + 记忆 (agent×scope) 组合 + USER.md per-user + 后台工作归属 | 两用户会话/记忆互不可见；隔离守卫测试全绿 |
| **P2 项目房间** | Project 实体升级 + 名册 RPC + 项目群聊（发言人标注）+ 工作区绑定 + Panel 项目 UI（列表/群聊/设置） | 两用户在同一项目群聊协作，项目记忆共享 |
| **P3 项目全形态** | 看板/goals/loops 进项目 + 记忆/工作区浏览面 + 进展推送路由到成员 | 项目页五个 tab 全活 |
| P4+（backlog 不承诺） | 渠道群绑定项目、Grant ACL、guest 角色、OIDC、per-user 密钥视图、多租户升级 | — |

## 9. 测试策略 (Testing)

三类，全部按「断言效果到达而非调用发生」的仓内纪律：

1. **隔离守卫**：每个返回会话/记忆/产物/项目列表的 RPC 配一条「A 建数据，B 看到空」断言
2. **迁移不变量**：单用户 fixture → 升级 → 隐式 owner 收养 → 所有现有查询结果逐字节不变
3. **咽喉防旁路**：handler 注册表扫描——凡返回 scoped 数据的 RPC 必须声明经过可见性咽喉（`gateway/lane.rs::override_for` 守卫的同款形状）

## 10. 边界语义 (Edge Semantics — 定死三条)

- **停用用户**：设备 token 即时拒绝；其 personal scope 后台工作暂停（冻结不删除）。**owner（u-owner）不可停用/降级**：loopback 解析恒 (owner, operator) 且不查 user status——那是恢复路径（等价 root console），停用 owner 只会产生半生效状态（远程设备被踢、本机不受影响），语义不自洽，直接禁止；这也保证系统永远至少有一个 admin。部署机即隐式超管账户——「凭据」是对机器的物理/OS 访问权，设计承认这个物理事实而非假装密码能挡住能直接读 SQLite 的人
- **移出项目成员**：立即失去可见性；**其在项目内发起的后台工作继续跑**——归项目不归人
- **会话 scope 不可变**：会话创建后不迁移 scope（否则记忆 capture 的归属历史说不清）
- 项目群聊多作者并发细节留 P2 实现时敲定，spec 只锁：**成员消息默认 Queue、发起者可 steer 自己的 run**

## 11. 风险与诚实声明 (Risks)

1. **隔离是隐私级——防误看，不防恶意**（D2 的确切边界，适用场景是家庭/小团队：你信任你邀请进来的人）。三条硬边界，全部是刻意接受的取舍而非疏漏：
   - **同一 OS 进程、同一 OS 账户**：所有用户的 agent run 在同一进程内执行工具。member 默认能用 `bash`（`method_authz` 明确对 chat tier 开放），一句 `sqlite3 ~/.aleph/data/sessions.db` 就能读到他人会话——file 工具 denylist 与 command_policy 硬底线防的是误操作和典型攻击模式，不是决心绕过的恶意用户
   - **vault 组织级共享**：member 的 agent 间接使用同一批 provider 凭据，不存在 per-user 密钥视图（P4 backlog）
   - **org scope 记忆按设计共享**：member 写入的 org 层记忆进入所有人的召回——这是特性不是漏洞，但意味着记忆投毒面是组织级的（`content_scanner` + threat-scope 防线在，但同样是防典型模式）
   
   另外服务层强制本身有旁路面：恶意成员遇上一个漏掉可见性咽喉的 handler 就是旁路（§9 咽喉防旁路守卫是对这一点的持续回归）。**升级路径**（本设计不堵死）：接纳不可信用户的正解是 per-user OS 沙箱（D2 否掉的重方案）或一人一部署，不是在应用层继续打补丁；存储层强制（行级/加密）是中间档。**低成本加固**（P1 可做，全是现有旋钮）：member 会话默认 exec tier = `Ask`、按角色收紧 `tool_permissions`
2. **前缀缓存**：user/scope 相关字节进 prompt 时必须按 CLAUDE.md §2.18 纪律分区——per-user 事实在会话内稳定（Stable），别逐轮重印
3. **R6 一致性**：渠道入站消息经 pairing 解析到 User 后进入其 personal scope；未链接的渠道 peer 维持现状（pairing 前置闸）。渠道群聊（多人 IM 群）绑定项目留 P4

## 12. 附录 A：现状接缝清单 (Seam Inventory)

勘察结论（2026-08-04），实现计划的落点参考：

| 接缝 | 位置 | 现状 |
|---|---|---|
| 分区键组合 | `src/memory/project_scope.rs` | 活体、已验证（`base__proj-hash`），默认关 |
| 读取并集 | `read_scope_ids` / `src/tools/result_store.rs::read_scope_keys` | 活体 |
| 主体表（带 role+scopes 列） | `src/gateway/security/store/mod.rs` `devices`/`tokens` | 活体，但所有行 `operator`/`["*"]` |
| 人类身份类型 | `shared/protocol/src/auth.rs`（`Role`/`GuestScope`/`IdentityContext`） | 休眠（消费者 PolicyEngine 已删） |
| 会话身份元数据 | `src/gateway/session_manager/mod.rs` `SessionIdentityMeta` | 休眠，恒 `::owner` |
| 记忆命名空间列 | `src/memory/namespace.rs` + `store/types.rs` | 已接 SQL 过滤器，所有调用点 `Owner`；`NAMESPACE_DESIGN.md` 已标废弃并指向 notes 层重设计 |
| Project 实体 | `src/projects/` + `projects.*` RPC | 活体，无 owner 字段（最近目录名册） |
| per-run 项目 task-local | `src/projects/run_context.rs` | 活体 |
| 渠道人类身份 | `src/gateway/pairing_store.rs` + `SessionKey::DirectMessage{peer_id}` + `DmScope::PerPeer` | 活体，仅渠道派生 |
| 名册形状 | `src/teams/types.rs` `Team`/`TeamMember` | 活体，纯 AI 成员 |
| Caller task-local | `src/gateway/caller_identity.rs` | 活体——「每个连接都是隐式 operator」，多用户最硬的钉子 |

**最硬的单用户钉子**：`caller_identity.rs` 隐式 operator、loopback 零配置规则、单一 `~/.aleph` 根、每 agent 单数的 `USER.md`。

## 13. 附录 B：与 qm 的差异对照 (Divergence from qm)

| 维度 | qm | Aleph（本设计） | 理由 |
|---|---|---|---|
| Scope kinds | 5 种（personal/channel/team/org/group） | 3 种（org/personal/project） | 项目覆盖房间类需求，YAGNI |
| Agent | 单 agent 按 scope 切视图 | 多人格共享 + (agent×scope) 实例 | Aleph 是 agent 中心架构（人格/身份密钥/teams 已建） |
| 项目群聊召回 | visible 合成（含个人） | org + project，不含个人 | 隐私保守：防个人记忆经 agent 泄入共享房间 |
| ACL | Grant 表（逐资源授权） | 成员制即授权 | 短期零消费者；词汇兼容留演化通道 |
| 沙箱 | per-scope 持久沙箱 | 复用 exec tier + sandbox 硬底线 | D2 信任模型为隐私级，非安全级 |
| 身份源 | Slack/web 外部身份 | 设备 token + 渠道 pairing 链接 | Aleph 无外部 IdP 依赖，多渠道对等（R6） |
| 多租户 | 单 org（配置常量） | 同 | 两边一致的关键简化 |
