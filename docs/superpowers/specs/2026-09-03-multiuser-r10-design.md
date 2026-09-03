# Multi-user Round 10 — 谓词是对的，装在每张脸上了吗，注释里那份副本还说的是真话吗

- **日期**：2026-09-03
- **分支**：`worktree-multiuser-r10`（worktree `.claude/worktrees/multiuser-r10`，基于 main `b8d95edf9`）
- **状态**：设计已定（自主会话；用户不在线，所有裁定见 §6 并逐条标注）
- **承接**：FEATURE_LOCATOR §5.22 多用户线的第十轮。前九轮的裁定不重做，逐条见 §1。
- **参考项目**：`T:\Github\qm`（TypeScript）。本轮是 qm 第四次被真正走查（round-4 / round-6 / round-9 之后）。

---

## 1. 背景与定位

§5.22 这条线从 P0（2026-08-04，users/devices 两张表 + `CALLER_USER`/`CALLER_ROLE` task-local + `method_admin` 闸）走到 round-9（2026-08-28…30，频道群绑进项目房间）。本轮**不重做**前九轮已经得出结论的那些区域，因为账本里写着它们已经被走过并裁定过：

| 已裁定的区域（ledger `qm_done`） | 结论 | 本轮姿态 |
|---|---|---|
| qm 的逐资源 grant 表 `acl_grants` + admin 也是一行 grant | **不移植**（round-2 首裁，round-4 重读实现后「结论不变且更强」）：Aleph 的「成员制即授权」用一行 roster 原子地回答四个问题；qm 的 no-transitive-reshare 与 `audience.every(entitled)` 是为收拾 grant 模型自己造出来的复杂度 | 不重开 |
| `composeSecurityPosture`（org floor + per-scope 只能收紧） | **单调性已采纳**，落在 round-2 ④ 的 `ExecTier::most_restrictive`；posture enum + LLM inbound screener 违 R7/R10 不移植 | 不重开（T22 的组合规则正是这条单调性，但它被推迟——见 A2） |
| `personKey` / `recordDirectorySync` | **不移植**（round-6）：Aleph 的 id 是服务端铸的 `u-<uuid v4>`，那一族缺陷结构上不存在；目录同步在 Aleph 零消费者 | 不重开 |
| `rate-limiter.ts` 按 principalId 计数 | **已采纳**，round-6 最大的正确性修复（`rate_limit_identity()`：loopback → `127.0.0.1`、已认证 → `user:<id>`） | 本轮只补**操作面**（T20），不动键的推导 |
| `budget.ts` 人 + org 双层美元窗口 | **round-7 整轮落地**（`[policies.spend]` 日历周期双层上限 + 账本 + CLI） | 本轮只补**归人**（T12 T13 让 cron/heartbeat 的花费第一次可归人） |
| qm 每个 admin 动作一条 audit 行 | **已采纳**，round-5 ⑦ 落地 `AuditEventType::AuthorityChange`（一个变体而非八个）+ `install_global`，round-6 ② 才给它接上读者 | 本轮只补**还沉默的那张脸**（T04：`project_manage` 的名册增删零 audit） |
| qm 一条耐久墓碑、从不级联、每个谓词使用时重问存活 | **qm 领先，round-4 采纳了它的判据不是它的实现**；round-4 ①⑬ 补第二第三条腿，round-5 ④ 落地 fire-time 形态 | 本轮补**第四条腿**（T14 heartbeat）与**票**（T15 T16） |
| 用户创建 / principal 来源 · 会话转录分区 · 角色模型与实时撤销 · 硬删除留存 | **Aleph 领先或平手**，逐条有记录 | 不重开 |
| `audienceEgressFloor` · `audienceIsAllInternal`（受众轴） | **刻意不做**（OI-31，round-6 ② 首裁、round-9 §9 重申）：Aleph 的 egress 配置是装机级、没有 per-principal 轴，移植等于先造一个子系统再给它一条规则 | 见 §5 |
| qm 参与者窗口（join-from-now / leave-freezes-history） | **记 backlog 不实施**（OI-67）：房间维持「加入即见全史」的组织内同事语义 | 见 §5 |
| `syncChannelMembers` | **不做**（D2）：会给名册加第二个写者，破坏「名册变更 owner-only ⇒ 转授结构上不可能」 | 见 §5 |
| qm Stage B/C 的 LLM judge · 5 种 scope kind · 一次解析产出全部（`Resolution`） | 分别为违 R7/R10、YAGNI（3 种 scope 够用）、记为未来候选未验证收益比 | 不重开 |

所以本轮走的不是「qm 还有什么我们没有」，而是**已经采纳的那些判据，今天真的装在每一张脸上了吗**。

---

## 2. 扫描摘要（对照表原文）

| 维度 | qm 的机制 | Aleph 今天的机制 | 差距 / 领先 | 本轮动作 |
|---|---|---|---|---|
| **身份模型** | 无 users 表，Principal 由 Slack/IdP 断言按需算出；`personKey` 把含 `@` 的 id 小写归一 | 服务端铸 `u-<uuid v4>`，users/devices 两张表，`CALLER_USER`/`CALLER_ROLE` task-local，loopback 恒 `u-owner` | **Aleph 领先**（无 IdP 时一行显式记录才是唯一可授予/撤销/枚举的东西；`personKey` 那族缺陷结构上不存在） | 刻意不做 —— round-6 已裁定 `personKey` / `recordDirectorySync` 不移植 |
| **角色** | admin 是一行 grant（单变体、scope 强制 org、`canAdminister` 丢弃 target） | `UserRole{Admin,Member}` + `method_admin` 前缀闸 + carve-outs；`caller_role` 字符串在 run 里承载 | **一处落后**：channel 面的 `caller_role` 由**频道 tier**推导（`Config => "operator"`），没有 `member` 拼写 ⇒ config 频道上的 member 跑成 operator，且降级永远够不到这根轴 | **T22**（组合两根轴取最严；Chat 面的 `"guest"` 一个字节都不许动） |
| **隔离（数据）** | 逐资源 grant 表 + `activePrincipal` 使用时重问 | `visibility.rs` 单一 chokepoint（`owner_and_scope_visible_to` 一个谓词体服务四张脸）+ 记忆分区族 | **Aleph 领先**（拒绝形状无存在性预言机），但**工具面仍有洞**：`note_manage.agent_id` 只校验路径穿越；委派子 run 丢 `AUTHOR_USER_KEY` ⇒ `ambient_actor()` 回落成房主；query filer 把成员私料写进 org 分区 `main` | **T06 T07 T08 T09**；grant 表刻意不移植（round-2/4/8/9 四次裁定：一行名册原子回答四个问题） |
| **房间 / scope** | 5 种 scope kind + per-scope 持久沙箱 + 参与者窗口（join-from-now） | 3 种（org/personal/project），名册即可见性谓词，`request_scope` 单一推导，`rescope_attribution` 是 §10 不可变性的唯一例外 | 大体持平；**残留缺陷**：`ProjectStore::remove` 的闸只看 `current_session_key`（频道绑定的房间永远没有它）⇒ 孤儿 binding 永久答 `room_claiming`；`find_by_path_for` 的 `COALESCE(o, ?2)=?2` 对 NULL-owner 行恒真 ⇒ 遗留行匹配每个调用者 | **T05**（两条都在 store 里，且都是「立法当天的谓词」形状）；参与者窗口 **刻意不做**（组织内同事语义，OI-67） |
| **受众 / 到达** | `audienceIsAllInternal` 能拒掉一个非 DM 回合并闸住凭据物化；`audienceEgressFloor` 取名册交集 | channel adapter 从不枚举群成员；出网配置是装机级、无 per-principal 轴 | **qm 领先，但不可移植**——移植等于先造一个子系统再给它一条规则 | **刻意不做**（OI-31，round-6 ② 首裁、round-9 §9 重申）；房间→群主动推送同样不做（OI-46，要回答「谁触发/发给谁/谁看得到」三个新问题） |
| **限流 / 花费** | `rate-limiter.ts` 按 principalId 计数；`budget.ts` 人+org 双层美元窗口 | round-6 已把限流键改成 `user:<id>`；round-7 落地 `[policies.spend]` 日历周期双层上限 + 账本 + CLI | **能力持平，操作面落后**：`RateLimitConfig` 每个生产构造点都是 `::default()`，per-person 配额没有任何操作者句柄；cron/heartbeat 的花费全落 `@unattributed` | **T20**（`[gateway.rate_limit]`，注意 `Config` 无 gateway 字段是设计，第二个 parse root 是判据 #1）；**T12 T13** 补 principal ⇒ 花费第一次可归人 |
| **审计** | 每个 admin 动作经一个 `audit(deps, {principalId, action, resource, scopeLabel})` helper，~40 模块调它 | `AuditEventType::AuthorityChange` 单变体 + `AUTHORITY_PRODUCERS` 双向普查 + `security.audit.query` + `aleph audit` | **仍有一张脸沉默**：`project_manage` 的名册增删零 audit（`grep -c audit` = 0），而普查两个方向都以「文件里已经有 `authority_change(`」为键 ⇒ 结构性看不见缺席；且 `remove_member` 丢弃行数 ⇒ 为没发生的撤销写行 | **T04**（工具面补行 + 普查加 `(tool)` 行 + `remove_member -> Result<bool>`） |
| **管理员可见性 / 停用** | 一条耐久墓碑、从不级联，每个谓词**使用时**重问存活 | 写入时列举式扫描三条腿（devices / channel senders / goal+loop+cron）+ cron 的 fire-time `walled_owner_reason` | **列举法只覆盖立法当天**：heartbeat 整个子系统无 owner 列、无冻结腿、无 fire-time 检查，而回执 `FrozenBackgroundWork{goals,loops,crons}` 读起来像完整清单；bootstrap ticket 铸造有两张脸、取消一张都没有 | **T13 T14**（第四条腿 + 回执字段 + CLI 渲染器）、**T15 T16**（停用连带烧票 + list/revoke，且同一提交里给客户端） |
| **共享 / 转授** | grant 行可转授，需 `no-transitive-reshare` 规则收拾 | 名册变更 owner-only ⇒ 转授结构上不可能；`syncChannelMembers` 刻意不做（会给名册加第二个写者） | **Aleph 领先（设计上）**，但**两张脸不同答案**：RPC 面拒绝移除房主并说明理由（名册即可见性谓词），工具面没有那句比较 ⇒ 一次调用可造出没人（连 org admin 都不）看得见的房间；`member_add` 同样缺 `require_known_user` | **T03**（两条规则下沉进 `projects::authz`，两张脸同一推导、同一话术） |
| **客户端（面数）** | 单一 HTTP API + Slack adapter | Panel / CLI / TUI / channels / tool face（R8），协议在 `aleph_protocol` | **三处「两端完整、中间没线」**：`EventVisibilityIndex::forget_session` 零调用者 ⇒ bind 之后房间直播面对其他成员永久静默；devices.list 两端各手抄一份形状（`filter_map` 静默丢行，缺键渲染成「离线」）；Panel 的 `ProjectInfo` 是 `ProjectRow` 的削弱镜像；Panel 的 `is_owner` 比服务端窄且**无 fallback**（缺控件读起来像缺能力） | **T11 T19 T18 T17**；`.get("project")` 信封 **刻意不做**（Ruling BD / OI-49，11 处跨两个 crate 是一个任务不是一个 nit） |
| **真机 QA 装置** | — | 22 个 `qa/*/run.sh`；多用户线是 multiuser_audit / teamchat_rooms / rooms_channel_bind / spend_budget | **量具本身红着**：teamchat_rooms 冷跑必死（`qa_build` 调用早于 `source` 20 行），rooms_channel_bind 的构建失败被 `tail` 吞掉；spend_budget 在任何索引里都没有条目 | **T01 T02** |

> ⚠️ **上表逐字保留扫描当时的措辞，其中一句后来被证伪，这里不改表、只在下面标出**：最后一行的「rooms_channel_bind 的构建失败被 `tail` 吞掉」**不成立**——那个文件 `:66` 的 `set -uo pipefail` 在两处构建之前，守卫真的会开火（现场演示与 git 溯源见 §5.1 第一条）。T01 仍然做那次迁移，但理由是「一个夹具一个构建入口」，不是「守卫不存在」。

---

## 3. 组织判据（本轮的那一问）

前几轮各有一问：round-2 是「**哪一张脸从来没被问过**」，round-3 是「**被交给谓词的那个 actor 是谁**」，round-4 是「**一个人有几份凭据、一个动词有几条腿、停用覆盖了哪些**」，round-6 是「**写者都在，读者呢**」，round-9 是「**这个缝上有几个读者在各自重新推导同一件事**」。

本轮这一问是这三问的合取，因为本轮找到的东西全部落在同一个形状里：

> **谓词是对的——但它装在这个动词的每一张脸上了吗？而注释里那份副本，今天还说的是真话吗？**

三个子问，逐一对应一批任务：

1. **每一张脸**（判据 #9）——`project_manage` 的工具面没有 RPC 面那两条名册规则（T03）、`cron.create` 的 RPC 面从不 stamp scope 而工具面 stamp（T12）、heartbeat 的两张创建脸都不 stamp（T13）、铸票有两张脸而取消一张都没有（T16）、`note_manage` 的模型输入面没有 `memory_search` 那道分区闸（T08）。**共用判据也要共用推导**：不是两处各写一遍同一个结论，是两处调同一个谓词。
2. **注释里那份副本**（判据 #1，且最贵的那一份在注释里）——`team_admits` 的 doc 否认 team 有 `scope_id` 列而它下面三十行就在读那一列（T31）、`rescope_attribution` 的 doc 说它的普查行「还没加」而那行就在同一个文件里（T31）、A2A 桥的 sentinel 注释描述了一个代码里根本不存在的 actor（T21）、`workspace_path` 写者的序数在三处各写了一个不同的数（T30）、`caller_identity` 一整段 24 行工作目录散文被 rustdoc 发布在了 agent 授权谓词上（T30）、`MEMBER_WITHHELD_KEYS` 引用的那个守卫**从来没有存在过**（T26，`git log --all -S` 返回零个提交）。
3. **两端完整而中间没线**（判据 #7）——`EventVisibilityIndex::forget_session` 零调用者，于是 bind 之后房间直播面对其他成员永久静默（T11）；delegated 子 run 从不写 `AUTHOR_USER_KEY`，于是 `ambient_actor()` 第一条臂交出房主而不是说话人（T06）；`memory_timeline` 把一个 agent id 跟一个 `EventActor` 列比较，于是每一次回合内调用都错（T10）。

**并且量具自己先要能红**（判据 #18）：`qa/teamchat_rooms` 冷跑必死（`qa_build` 调用早于 `source` 20 行），`qa/multiuser_audit` 在这台没有真 python3 的机器上到不了第一条断言，`qa/spend_budget` 在任何索引里都没有条目。所以 T01 / T32 / T02 排在最前面——**先修尺子，再量东西**。

---

## 4. finding → task 映射

> 「判据」列指根 CLAUDE.md §工程判据的**形状号**。「qm 参照」只在该 finding 真的有一条 qm 行时才填。

| 任务 | 严重度 | 判据 | 一句话机制 | qm 参照 |
|---|---|---|---|---|
| **T01** | P1 | #18 #2 | `qa_build` 在 `source build.sh` 之前 20 行被调用 ⇒ 每次冷跑在第一条断言之前死掉；只有 `SKIP_BUILD=1` 活下来，而它量的是共享 target dir 里那个不知道多旧的二进制 | — |
| **T32** | orchestrator | #11 #18 | 五条 python3 腿在本机是 Microsoft Store stub：`python3 - <<EOF` **静默什么都不做并退出 0** ⇒ 三个任务点名的真机阶段在这台机器上跑不了 | — |
| **T03** | P0 / P1 | #9 #16 | `project_manage.rs:4-9` 说两张脸都走 `projects::authz`，对**恰好两条规则**为假：房主移除保护只在 RPC 面（tree-wide 一处命中、无测试），`require_known_user` 同样只在 RPC 面 ⇒ 一次工具调用能造出没人（连 org admin 都不）看得见的房间 | `admin-service.ts:126-133` 最后一个 admin 的锁出保护，同一形状高一层 |
| **T04** | P1 / P2 | #11 #3 #17 | `ProjectStore::remove_member` 丢弃 DELETE 的影响行数恒 `Ok(())` ⇒ 为一次没发生的撤销写审计行并向旁观者推 `affected_user`；工具面**零** audit，而普查两个方向都以「文件里已经有 `authority_change(`」为键 ⇒ 结构性看不见缺席 | `shared/routes.ts:13` 一个 helper、~40 模块调它 |
| **T05** | P2 | #5 #2 | `remove` 的闸只看 `current_session_key`（频道绑定的房间永远没有它）⇒ 孤儿 binding 永久答 `room_claiming`、在任何面上都不可列；`find_by_path_for` 的 `COALESCE(o, ?2) = ?2` 对 NULL-owner 行恒真 ⇒ 遗留行匹配每一个调用者 | — |
| **T06** | P0 | #7 #6 | `build_sub_metadata` 从不写 `AUTHOR_USER_KEY`（该常量在文件里一次都没出现）⇒ `scope::room_author` 回落 `attr.owner_user_id`，每个成员的 delegated 子 run 的 `ambient_actor()` 都是**房主**；四个孩子 metadata 构造器里两个已经做对了 | `capability-token.ts:21` / `orchestrator.ts:1162` 把 `actorId` 显式带进每次委派 |
| **T08** | P0 | #8 #5 | `note_manage.agent_id` 是 JsonSchema 暴露的模型输入且直接成为分区键，只校验路径穿越 ⇒ `agent_id: "main__u-alice"` 逐字节寻址另一个 principal 的 vault；`partition_visible_to(_, None)` 返回 **true**，今天咬得住纯属巧合 | `scope-membership.ts:136` / `acl-store.ts:185` 工具面与 HTTP 面同一推导 |
| **T09** | P1 | #16 #3 | query filer 拿的是 `acting_agent_id` 的 **base** persona id ⇒ 综合了某成员私有记忆的 note 落进 org 分区 `main`，而它被 union 进**每一个** principal 的读集；读侧组合是对的 ⇒ 只有写侧错。孪生 `flag_user_correction` 已修过 | `types.ts:105 scopeLabel` / `context-filter.ts:18` |
| **T10** | P1 | #2 #1 | `WHERE fact_id = ?1 AND (?2 = '' OR actor = ?2)`，而 `actor` 列的词表是 `{agent,user,system,decay,migration}`；回合内 `ScopedToolService` 恒 scope ⇒ 比较恒为 `actor = 'main'`，零行被读成「这个 fact 没有历史」。三处散文各说各话 | — |
| **T11** | P1 | #7 #1 | `forget_session` / `forget_team` **零调用者**；`session_admits` 的 `(owner, scope)` 缓存 fill-on-miss、进程生命期、FIFO 4096、无 TTL、一个 `Arc` 共享给每条连接，而 `rescope_attribution` 恰恰重写 scope_id ⇒ bind 之后其余名册成员的直播帧永久被拒。两处注释点名的四个动词都改不动这对列，能改的那个不在名单上 | `postgres-grant-store.ts:30-56` 用 DB 侧 statement-trigger 版本号，让「忘记失效」结构上不可能 |
| **T31** | P3 | #1 #16 | `team_admits` 的 doc 说「没有 `scope_id` 列、没有用户名册」，而它下面三十行就在读那一列、并把 `project:` scope 派给 `projects::roster::is_member`；`rescope_attribution` 的 doc 说它的普查行「在同一个提交里还没加」，那行就在同文件 `:1411`。RPC 面的孪生带着**正确**表述，只有投递面那份没被扫到 | — |
| **T12** | P0 | #6 #5 | `handlers/cron/real.rs:288` 建 `CronJob` 时既不写 owner 也不写 scope（整个文件 grep 无命中）⇒ Panel / CLI 建的每个 job 都无主体，四个读者在 NULL 上短路：冻结当它不属人、`walled_owner_reason` 当它是 legacy（与真正的旧 job 同形）、run 无 scope 执行、花费落 `@unattributed`。已经有三份同样四行的复制品 | `control-service.ts:451-472` 建时 stamp、`runTrigger` 重读 |
| **T13** | P1 | #5 #16 | `HeartbeatTask` 没有 owner 也没有 scope（九个文件 `rg` 零命中），而它自称的孪生 cron 两者都有；`heartbeat_run_metadata` 只塞 agent id + `UNATTENDED_KEY` ⇒ 每一拍的 L2 run 无 scope 执行、花费落 `@unattributed`。服务默认启用 | `run-trigger.ts:163-186` 每一种 trigger 走同一个 owner 重分类 |
| **T14** | P1 | #5 #17 #8 | `freeze_owned_background_work` 恰好三条腿，而 `FrozenBackgroundWork{goals,loops,crons}` 的 doc 字面写着「the three background legs a deactivation freezes」⇒ 读起来像一份完整清单，而被停用的第二个 admin 的 heartbeat 继续触发、继续投递 | 同上 |
| **T15** | P1 | #15 #14 | `exchange_bootstrap_ticket` 不做任何用户状态检查（状态闸只在**铸**的两处）⇒ 铸票 → 停用 → 兑换，在停用扫描三条腿跑完**之后**造出一台全新的、未被吊销的设备，而复活回执还在断言「设备仍被吊销——请重新铸票」 | `auth-broker.ts:12` 把一次性 claim 当可列可花的耐久状态，宁可拒绝降级也不静默发不可撤销的凭据 |
| **T16** | P1 | #9 #14 | 铸一份设备凭据有两张脸（Panel RPC 与 `aleph-server pair`），取消它**一张都没有**；UNBOUND 票兑换成 u-owner/operator、TTL 由调用者选到 24h，连自称「revoke all remotes」的 `handle_token_rotate` 都留着未兑换的票完全可兑 | 同上 |
| **T28** | P3 | #2 | 两条 CLI 测试是恒真谓词：一条把 `let role: Option<&str> = None` 绑死再 `if let Some(r) = role`（静态死支），另一条字面是 `if let Some(r) = Some("admin")`；两条都不提 `create` / `UserCreateParams` / `client.call` ⇒ 对 `create()`、参数类型、wire 的任何改动都不能让它们变红 | — |
| **T17** | P1 | #1 #17 | Panel 的 `is_owner` 是服务端 `authz::is_owner` 的**严格更窄**的第二份拼写（不认 org admin、不解析 NULL owner），而五个受闸点里有两个是**没有 fallback** 的 `<Show>` ⇒ 受影响的人看不到拒绝、看不到解释、连一个禁用控件都没有：整段消失，读起来像「这个房间不能归档」而不是「你不可以」 | `listScopeResources` 从服务端带回 `manageable: bool`，与执法同一个谓词派生 |
| **T18** | P2 | #1 | Panel 的 `ProjectInfo` 自称镜像 `ProjectView`，而后者只是 `pub type ProjectView = aleph_protocol::projects::ProjectRow` ⇒ 那条注释是第三份表述；镜像本身是严格子集且给四个字段加了 `#[serde(default)]` ⇒ 一次服务端改名把 Panel 降级成 `""`/`[]`，而 CLI 解析同一个方法会硬报错 | — |
| **T19** | P2 | #10 #8 | `gateway.devices.list` 是手写 json! 字面量，被 Panel 一个手卷 `filter_map` 读，`?` 静默丢行、`unwrap_or(false)` 把缺键渲染成「离线」——那是一句断言，不是一个未知。这是操作者看见并因此吊销一份已配对凭据的**唯一**界面，而同族凭据家族已经吃过一模一样的 bug | — |
| **T26** | P3 | #1 #6 | 同一事实两个拼写、跨两个 crate：`config.rs` 的 `MEMBER_WITHHELD_KEYS`（行为读它）vs `shared/protocol` 的 `OPERATOR_ONLY_KEYS`（两个守卫读它）。**漂移不可能静默发货**（键集等值断言双向），所以活着的只有那条注释——而它引用的守卫名 `git log --all -S` 返回**零个**提交，从来没存在过 | — |
| **T21** | P2 | #1 #7 | `a2a_peer` 这个「稳定哨兵」全树只出现在那一条注释里；`TurnContext` 没有 actor 字段给它住，`SessionKey::task("main", …)` 让 `ambient_actor()` 把字面 AGENT id `"main"` 交给比较 user id 的谓词 ⇒ 这条 run 在各处**被静默拒绝**。同一条注释第二句「保持今天行为不变」也是假的 | — |
| **T23** | P2 | #1 | `flush_partitions` 按 `{base}__` 前缀在**全盘** id 上枚举 ⇒ 关掉一个成员的会话会跨每一个其他 principal 的待处理行跑最多 8 轮 LLM 压缩，off the books 且绕开同名 RPC 的 admin 闸。这是**刻意的**并且有测试钉着——缺的是把它说出口 | — |
| **T02** | P3 | #3 #17 | `qa/spend_budget` 在任何 reference doc 与路由表里都没有条目，CLAUDE.md 根本没有 `src/spend/` 行 ⇒ 一个改花费的人被路由到无文档无夹具；新的 hygiene 规则必须**走 `qa/` 目录**得出被检查集合，不是在测试里写一张清单 | — |
| **T20** | P2 | #1 #14 | round-6 把限流键改成 per-principal 之后，每一个生产构造点都是 `RateLimitConfig::default()`：没有配置节、没有 setter、操作者读不到也改不了上限——而默认值自己的注释就记着这些数字已经需要改过一次 | `rate-limiter.ts:6` 每个构造点从注入的 options 取窗口，per-principal budget 是一等配置 |
| **T25** | P3 | #1 #16 #8 #9 | 名册动词**只能**按不透明 id 寻址，而模型对名册的唯一视图是**只有名字没有 id** 的 `speaker_label`；映射的两半坐在墙的两侧，`DESCRIPTION` 从不说 user id 从哪来 ⇒ 按名字移除一个已经在册的人都做不到。全服务端没有任何 name→principal 解析器，所以这是造第一个家不是第二个 | — |
| **T24** | P2/P3 | #15 #8 #1 #6 | 没有 per-principal 的 admin 读：四个注册方法就是 me/list/create/update，`UserView` 四个字段，而一个 principal 的设备 + 花费 + 被冻结的后台作业**唯一**一次被 join 是在 `UserUpdateResult` 里——也就是**不可逆的状态写之后**。handler 自己的注释就承认了这个前提 | qm 的 admin 面把 principal 的持有物在动词之前composed |
| **T29** | P3 | #1 #3 | 四个 Node QA 驱动各持一份逐字节相同的帧信封读法（四段 hash 相同），而**已经腐烂的是注释**：一份带完整三形状清单加它的事故记录，一份是丢了清单与事故的缩写版，两份完全没有——一份出生起就是另一份的削弱版 | — |
| **T30** | P3 | #1 #6 #3 | 四个实例同一形状：一个数字或名册被抄进它旁边那个所有者的散文里，然后被数过去。`workspace_path` 的写者序数在三处各写一个（且 SECURITY.md 自相矛盾）、`caller_identity` 一整段被 rustdoc 挂在错的函数上且其中一半是那份普查的第三份过期副本、`stamp_attribution` 的调用点roster 少数一个、`qa/README` 的 rewind floor 写 5 而 run.sh 写 11 | — |
| **T07** *(推迟)* | P0 | #12 | `carry_policy_metadata` 携带四个键、丢掉 `AUTHOR_USER_KEY` ⇒ 房间内的 goal/loop 续跑把 `ambient_actor()` 解析成房主。**加一行会同时移动三个读者**，需要用户裁定——见 A1 | 同 T06 |
| **T22** *(推迟)* | P0 | #9 #14 | `caller_role_str` 是两值词表（`Config => "operator"`、`Chat => "guest"`），`channel_run_identity` 只收频道 id ⇒ `permission_level="config"` 的频道给**每一个**被接纳的发送者盖 operator，包括 users 行写着 Member 的已配对主体；这还是降级永远够不到的那根轴。**需要用户裁定**——见 A2 | `server.ts:184-191` 每次请求按**当前**名册重分类并重授权，界面从不提供角色 |

---

## 5. 刻意不做（附理由；**不要在下一轮把它们当新发现重新提出来**）

### 5.1 被对抗性复核**证伪**的发现（11 条）

这些不是「以后再做」，是**机制被证明为假**。列在这里是为了它们不再被重新提出。

| 发现 | 为什么被证伪 |
|---|---|
| `reachability-qa-bind-swallowed-build`（P1）——「`rooms_channel_bind` 复制了 `qa_build` 要防的那个吞掉构建的管道」 | 整条机制是「管道的退出码是 `tail` 的、恒 0」，在这个文件里**不成立**：`:66` 的 `set -uo pipefail` 在两处构建（`:91` `:93`）之前，子 shell 继承外层 `set -o`，所以 `|| exit 1` 真的会开火（现场演示：`bash -c 'set -uo pipefail; (false 2>&1 \| tail -3) \|\| echo GUARD FIRED'` 打印 GUARD FIRED，去掉 pipefail 则不打印）。`git log --diff-filter=A` 显示这个夹具**出生时就带着 pipefail**，从来没有守卫失效的窗口。两条次要断言也不准：`webview_compat/run.sh` 同样不 source build.sh，而这个文件 `:80-88` 写明了它不 source 的理由。**T01 仍然做那个迁移**，但理由是「一个夹具一个构建入口」，不是「守卫不存在」 |
| `reachability-banner-methods-with-no-client`（P2）——「三个 banner 广告过的 `projects.*` 方法在每个界面上零客户端」 | 一个已发货的**通用** CLI 客户端就够得到全部三个：`aleph gateway call projects.remove '{"id":"p-x"}'`（`gateway_cmd.rs:9-32` 是无 allowlist 无方法过滤的原样透传），走 loopback 解析成隐式 owner as operator——正是被提议的修复想要发明的那个姿态 |
| `reachability-goal-loop-have-no-write-face`（P2） | **观察准确**（`goal.*`/`loop.*` 各只有一个 `list` RPC，两个客户端调用点都在 Kanban，无 CLI 面），但那是对 OI-44 的忠实重新推导，不是新发现；且提议的修复前提为假——它说复用「停用扫描已经在调的那些 store 动词」，而那些动词是 **bulk** 的（`pause_all_owned_by`），两个 store 都没有任何 per-id 的 set_status/pause/resume |
| `reachability-archived-room-still-routes`（P2）与 `isolation-13`（P2，同一件事的两个 id） | 行为是真的（归档的房间仍在认领回合），但那是一条**记录在案、QA 验证过的裁定**，而且提议的修复把它**反转**成一个先前裁定明令禁止的破坏性形状。`store.rs:572-574` 就在被引用的那个文件里写着：`archive` 是房间的「忘记」动词，它翻 status、**保留名册**，因此**对话保持可达**；`store.rs:1678-1681` 的测试钉着它，SECURITY.md:2765 同一句。所谓「写路径认为归档要紧」的前提也错了种类：`bind_conversation` 的 `status='active'` 是**建时**的目录闸 |
| `reachability-member-withholding-guard-absent`（P2） | 证据扫描停在 `~:1071`，从没走到那个 `mod tests` 块的后半（`:942-1542`）。被提议的修复**逐字已经存在**：`a_member_does_not_receive_the_server_global_policy_axes`（`:1449-1458`）连同它 `:1396-1413` 的 harness。**T26 只保留这条发现里为真的那一小半**：注释 `:748` 引用的那个名字从来没有存在过 |
| `qm-capabilities-cron-fire-time-roster`（P1）——「fire-time cron 闸只问 owner 是否 Active，不问他还在不在这个 scope 命名的房间名册上」 | 代码陈述准确，但它描述的是一条**人工裁定钉死**的行为，而提议的修复**反转**了那条裁定——它正是 P0 spec 定死三条边缘语义之一 |
| `qm-capabilities-spend-self-view` | 机制断言「qm 把 `{spentUsd, limitUsd}` 交给被拒的调用者……Aleph 按主体执法却按机器报告」在具体行上为假：`src/spend/mod.rs:183-185` 的 `Limit::PerUser { spent, limit }` doc 写着「两个数都是他自己的花费，所以都能安全告诉他」，`i18n.rs:465-470` 逐字渲染它们**外加重置时刻**——严格多于 qm 的参照点，且有两条既有测试钉着。修复还会重新起诉 OI-35 ③ |
| `entropy-author-key-census-names-two-of-four` | 计数对，但类别与机制都错：它说「四个写者里有两个是在普查写完**之后**才落地的」，git 说反了——普查建于 2026-08-28，两个 teams 写者分别是 08-13 与 08-18，**早 10–15 天**，普查作者是四个都摆在面前只点了两个。而且「origin site」不是「writer」的同义词。这一半**已经在 T06 的 goal 里逐字包含** |
| `entropy-memory-arms-census-literal` | **T09 的重复**（T09 的 goal 已逐字写明要加 `memory_reflect` 到 `MEMORY_ARMS_THAT_MUST_COMPOSE` 并带一条先红后绿的测试）；其 `memory_timeline` 那一半**认错了普查的主体**——那条 dispatch 臂什么 agent id 都不交 |
| `entropy-freeze-legs-triplicated` | P6 三次法则要三次重复，这里**只有两次**：goals 腿与 crons 腿共用 `match/count/warn`，而 loops 腿**没有 match、没有 Result、没有错误臂**（`LoopRegistry::pause_all_owned_by` 返回裸 `usize`），另外两条还在 async 与锁上再分歧两轴。提议的签名会让一个真实的不对称更糟，而且那个 helper 会去拥有一份 **T14 正要改**的腿清单 |

### 5.2 被重新确认的既往裁定（账本项，本轮**不重开**）

- **受众轴 / `audienceEgressFloor` / `audienceIsAllInternal`**（OI-31，round-6 ② 首裁、round-9 §9 重申）：Aleph 的 egress 配置是装机级、没有 per-principal 轴，移植等于**先造一个子系统再给它一条规则**。同理**房间 → 群的主动推送**（OI-46）不做：它要先回答「谁触发 / 发给谁 / 谁看得到」三个新问题。
- **逐资源 grant 表**（round-2 / round-4 / round-8 / round-9 四次裁定）：一行名册原子地回答四个问题；grant 模型的 no-transitive-reshare 是它自己造出来的复杂度。**转授在 Aleph 结构上不可能**，这是要保住的性质，不是要补的洞。
- **参与者窗口**（join-from-now / leave-freezes-history，OI-67）：房间维持「加入即见全史」的组织内同事语义，**记 backlog 不实施**。
- **`syncChannelMembers`**（D2）：会给名册加第二个写者；且列不全群成员的频道会静默少人，「少了一个」与「他本来就不在」在日志里同形。
- **花费自视图**（见 §5.1 最后一组）：per-user 的 spent/limit **已经**在拒绝里交给调用者，且带重置时刻。
- **OI-63**：`users.*` 保持 **CLI-only**（admin-gated + loopback ⇒ 无 carve-out、无新授权概念）。T24 遵守它：admin-gated、loopback、**没有 Panel 面**。
- **OI-2**（2026-08-07 人工裁定）：后台工作（loop / goal / cron / 群聊会话）owner-only 是**产品边界**不是未完成的谓词；`scope_id` 持久化且**刻意不被** `stamped_owner_visible` 读——T13 要把这句话写进新字段的 doc，免得下一个读者把它当断线接上。
- **Ruling BD / OI-49**：单项目 RPC 响应的 typed 信封（服务端 6 处 `json!({"project": …})` 对客户端 5 处 `.get("project")`）**推迟**——11 处跨两个 crate 是一个任务不是一个 nit。T18 明确不许扩到它。
- **SECURITY.md:2775 的 rowboat 裁定**：对有 claimed session 的房间，采纳的形状是「让 `remove` 不可用并指向 `archive`」，**不是**显式级联删除。T05 因此只加闸不加 cascade（见 A3）。

### 5.3 本轮范围之外，已记录

- `RunStarted` 之外的 schema 级 per-agent 记忆事件分区（T10 恢复的通配符**重新打开一个全局 fact-id 预言机**，那要单独立项）。
- 服务端派生 `manageable: bool` 给 Panel（T17 真正的解，要改 `render_project` 的 8 个生产者并撞上 Ruling BD 的信封）。
- 机器 principal 身份种类 + `ambient_actor()` 新臂（T21；round-9 §9 已裁「零消费者，R10 撤回模式」）。
- cron 的 fire-time 兜底 `walled_owner_reason` **没有** heartbeat 对应物——T14 之后孪生仍未闭合，这是记录在案的欠账。
- `qa/spend_budget` 的 Node 移植（A4）。
- 传输层 metadata 键（`channel_id` / `sender_id` / `conversation_id` / `platform` / `locale` / `caller_role` / `project_root` / `resume` / `run_id`）的常量化——T27 明确不扫它们：那是大得多的改动，且背后没有权限闸。

---

## 6. 未经用户裁定而做出的决定（用户不在线；每一条都可被推翻）

| # | 裁定 | 理由 |
|---|---|---|
| **A1** | **T07 推迟**（`carry_policy_metadata` 的第五个键不加） | 加一行会**同时移动三个读者**：`spend::principal_from_metadata`（`spend/mod.rs:589`）在 scope owner 之前读 `AUTHOR_USER_KEY` ⇒ 续跑的花费从房主改记到最后发言人（毗邻 OI-2 与 OI-38）；`ExecutionEngine::for_shared_room`（`execution_engine/mod.rs:291`）今天看到 `(None, Some) => false`，会把一些 steer/cancel 碰撞翻成同作者。**并且要纠正原写法的一个前提**：T07 的 goal 说「run_loop binds TURN_ORIGINATOR off the same key」——**它不是**。`run_loop/mod.rs:570` 把 `TURN_ORIGINATOR` 绑在裸字面量 `"originator_user_id"` 上，`AUTHOR_USER_KEY` 是在 `:257` 被读进 `CURRENT_ROOM_AUTHOR` 的。两个键，两份推导。所以**审批卡改道那条后果不成立**，需要用户裁定的是**三个**读者不是四个。（这个混同正是 T27 存在的理由，也是把 T27 从 T07 里拆出来单独早做的理由。） |
| **A2** | **T22 推迟**（频道 tier 与主体角色不组合） | 组合会改变一条**已发货的 opt-in**（`permission_level = "config"` ⇒ 每个被接纳的发送者都是 operator，由 `executor.rs` 的 `config_tier_channel_maps_to_operator` 钉着），并且会让一个**已配对的 member 比同一频道上未配对的陌生人更受限**。**Orchestrator 的建议**（记录在此，等用户拍板）：对**已知**主体组合成最严（`Config+Member => member`），`Chat => guest` 逐字节不动（那一句是承重的：`channel_permission_level_from_role` 对 `"member"` 返回 `None`，而 `turn_permissions.rs:186` 从这同一个字符串反推频道 tier，覆盖 Chat 的 `"guest"` 会**删掉**默认界面上的 Full→Auto clamp）；未配对发送者那条残留是**另一个产品决定**。**不在无人裁定的情况下实现。** |
| **A3** | **T05 只加闸，不加 cascade** | 两个复核者意见相左：一个主张「闸 + 级联双保险」，一个主张「只加闸，级联是 SECURITY.md:2775 明确拒绝的形态」。取后者——那条裁定采纳的形状就是「让这个动词对 room 不可用」；而且**加了闸之后那条 DELETE 是一条永远不会触发的臂**（判据 #2）。若用户要双保险，请注意这一点。 |
| **A4** | **新增 T32：把 `qa/multiuser_audit` 移植到 Node；`qa/spend_budget` 记录为本机不可跑的 python-only 夹具** | 本轮有**四个**任务（T14 T15 T16 T24）点名 `qa/multiuser_audit` 作为真机阶段，而它有五条 python3 腿，本机 python3 是 Microsoft Store stub（`python3 - <<EOF` **静默什么都不做并退出 0**，见记忆 `windows-python-heredoc-noop`）⇒ 这台机器上它到不了第一条断言。它的兄弟夹具（`teamchat_rooms` / `rooms_channel_bind`）**已经**用 Node 做了同样的事，所以移植是复用不是发明。`qa/spend_budget` **不移植**：它的 python 表面积大得多（`spend_rpc.py`、`mock_anthropic.py`、浮点比较、jf helper），改为在 `qa/README.md` 加一句老实话说明它需要真的 python3、在这台 Windows 主机上不跑。**移植不许降低任何 FLOOR**——降低了就是一次收窄不是一次移植。 |
| **A5** | **文档 bundle 删掉数字，不重新同步它** | T30 / T31 的每一条，处置都是**删掉那份副本并指向所有者**，而不是把它改对——改对只是把陷阱重新上膛。仓里已经为此裁过两次：根 CLAUDE.md 的 R10「代码是权威，本文件刻意不复制那个数字」，以及 `qa/README.md:254-255`「Deliberately no count here: the first one drifted」。**并且不给它们加源码扫描守卫**（两个复核者都主张的一次刻意拒绝）：扫描器只认得它枚举过的形状（判据 #3），而 `handlers/projects.rs:64` 的普查白纸黑字写着「a count of members is prose and prose has no compiler」。验证改为每个 bundle 的 `tests` 里带一条**指定的 rg 及其期望输出**，改前跑一次（红基线）、改后跑一次。 |
| **A6** | **每个提交以一行 `Claude-Session: https://claude.ai/code/session_01UQ4X6uMhUXQosx1VZKMqCb` 结尾，不加 `Co-Authored-By:`** | 本会话的 harness 归属指令与 2026-09-02 的「无 trailer」裁定相反；未经用户裁定取 harness 一侧（当前、显式、且写明覆盖既往指令），并让全轮 30+ 个提交保持一致——用户若不要，事后一条 filter-branch 统一剥掉即可。上一轮曾有两个 agent 各自把这条冲突报告过一次；本轮写死以免再报。 |
| **A7** | **实现严格串行，单 worktree** | 共享 `CARGO_TARGET_DIR=D:/Workspace/Aleph/target` 会把并发 cargo 串行化，且本机 RAM 撑不住两个 rustc（记忆 `alephcore-build-memory`）——**一次只跑一个 cargo**。加上本轮有七个文件被三到四个任务共同触碰（`project_manage.rs` 四个、`users_cmd.rs` 四个、`shared/protocol/src/users.rs` 四个、`start/mod.rs` 四个、`qa/README.md` 四个、`handlers/projects.rs` 三个、`handlers/users.rs` 三个），并行会把一个语义合并冲突变成常态。**执行顺序就是冲突顺序。** |

> 另有两条**不是裁定、是记录在案的作用域收窄**：① `qm-capabilities-admin-user-dossier` 被一位复核者拆成 P2/P3——他指出那份合成「在客户端四次调用里已经可达」，**真正缺的只有 by-owner 的后台作业读**；T24 仍按完整 dossier 设计（因为四次调用不是一个界面，且 `count_owned_background_work` 无论如何都要建），但严重度按 P2/P3 记。② T21 在复核里只有**一票 CONFIRMED**、另一票记为 PLAUSIBLE——它是零风险的纯注释改动，但计划要求执行者**当场自己重跑一次** `grep -rn a2a_peer --include=*.rs .` 再写提交信息。

---

## 7. 验证纪律

### 7.1 单测 + 变异证伪

每条守卫都要能回答「**在什么情况下这东西会变红**」，并**至少变异一次、记录实际观察到变红的名单**（不是预测的名单）。逐任务的变异对象写在 plan 各任务的 Step 4。三类例外要**明说而不是假装**：

- **纯注释 / 纯文档任务**（T21 T23 T30 T31，以及 T26 的一半）：买不到变异，红基线是**一条指定的 rg 及其期望输出**，改前改后各跑一次，两次输出都进提交正文。
- **纯删除任务**（T28）：「红」是**证明它们永远不会红**——改坏 `create()`、看那两条注定的测试仍然通过、再还原。那个实验就是删除的全部理由。
- **qa 夹具任务**（T01 T02 T29 T32）：红基线是**冷跑的失败输出**；证伪是「故意改坏一个 `.rs`，夹具必须非零退出」或「删掉一条 README 条目，hygiene 规则必须按名字重新变红」。

### 7.2 最小可信验证集（CLAUDE.md 六条，本机口径见 plan §0）

```
cargo test -p alephcore --lib                    # 失败名单与 baseline_failures.txt 按名字比对（17 条，测于 b8d95edf9）
cargo test -p alephcore --bins                   # boot 注册 / method_census 改动必跑（T11 T14 T16 T20 T24）
cargo check -p alephcore --features test-helpers --all-targets   # 替代 --test '*' --no-run（后者在 main 上就红，OI-26）
cargo test -p aleph-panel --lib                  # T17 T18 T19；不是 --no-run
cargo test -p aleph-protocol -p aleph-cli        # T14 T15 T16 T19 T24 T25 T26 T28
just _stage-shell-placeholders && cargo clippy --workspace --all-targets   # 收尾一次
```

外加 `just wasm`（Panel 出厂形态，T17 T18 T19；跑完 `git checkout -- interfaces/webchat/dist`）。**按名字比，不按条数比**——多出来的名字才是本轮引入的。

### 7.3 真机阶段

| 阶段 | 覆盖的任务 | 前置 |
|---|---|---|
| `qa/teamchat_rooms/run.sh`（冷跑） | T03 T04 T06 T25（+ T01 自己） | **T01 先修好它**——今天冷跑必死在 `qa_build: command not found` |
| `qa/rooms_channel_bind/run.sh`（冷跑） | T05 T11（+ T01 自己） | 同上（T01 把它迁到 `qa_build`） |
| `qa/multiuser_audit/run.sh`（冷跑） | T14 T15 T16 T19 T24 | **T32 先把它移植到 Node**——今天在这台主机上到不了第一条断言 |
| `qa/memory_curated/run.sh` | T09 | — |
| `qa/resume_boundary/run.sh` 五阶段 · `qa/agents_viz/run.sh {claims,panel}` | T29（回归：共享 tap 不许收窄） | 五个阶段必须**各自仍达到它的 FLOOR** |
| `qa/spend_budget/run.sh` | — | **本机跑不了**（需要真 python3）；A4 明确记录，不移植 |

`qa/lib/scratch_home.sh::qa_redirect_home` 的判据锚点不变：**必须在 HOME 被重定向之前构建**（cargo 的 registry / git cache / rustup toolchain 都住在真 HOME 下）——这正是 T01 修的那个顺序。

---

## 8. 环境假设

1. 本机 Windows，worktree `D:\Workspace\Aleph\.claude\worktrees\multiuser-r10`，共享 `CARGO_TARGET_DIR=D:/Workspace/Aleph/target`（`check` / `--lib` / clippy 可用；`--test '*'` 需 `-j 1` 且在 main 上就红，见 OI-26 与记忆 `alephcore-integration-tests-need-j1`）。
2. alephcore lib-test 一次完整编译实测约 16m30s，超过 Bash 工具 10 min 上限——**必须**用 plan §0 的分离式启动 + **前台轮询**等待（不要用 Monitor：它要结束本回合才能收到事件，而一结束回合就会被强制交最终报告）。
3. 基线（改动前）`cargo test -p alephcore --lib` 的 **17** 条失败全部是环境/上游，名单在 scratchpad `baseline_failures.txt`，测于基点 `b8d95edf9`。
4. 本机 python3 是 Microsoft Store stub：`python3 - <<EOF` **静默什么都不做并退出 0**（记忆 `windows-python-heredoc-noop`）。所有新写的 QA 装置一律 Node。
5. `just` 必须用 **Bash 工具**跑（PowerShell 的 PATH 缺 cygpath，记忆 `windows-just-cygpath-and-cargo-corruption`）。
6. 全程不碰 `main`；本轮**不合并**，只在分支上提交并报告。
