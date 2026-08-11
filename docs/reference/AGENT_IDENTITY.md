# AGENT_IDENTITY.md — 每个 Agent 的独立密钥与签名操作账本

> 「带签名日志解决了'发生过什么'，身份权限解决了'谁能做什么'。」
> 本文覆盖前半句的**实现**、后半句的**接线点**，以及两句**都没有覆盖**的边界。
> 代码锚点 `src/identity/`；对照 FEATURE_LOCATOR §5.17。

---

## 1. 一句话

每个 agent 持有自己的 Ed25519 密钥对；它做的每一次**变更类工具调用**、每一次**被拒**、每一次**审批裁决**，都追加进**它自己的**哈希链并由**它自己的密钥签名**。`agent_identity` 工具与 `aleph-server identity` CLI 负责读与验。

## 2. 为什么是这个形状（Gap Analysis vs buzz）

参考项目 **buzz**（`T:/Github/buzz` / macOS `/Volumes/TBU4/Github/buzz`，Nostr relay 工作区，"same audit trail, a different keypair"）。逐维度对照 —— **改这一层前先看这张表，不必重做对比**：

| 维度 | buzz | Aleph（第四轮后，2026-08-04） | 取舍 |
|---|---|---|---|
| 身份载体 | secp256k1 keypair 即身份，`Keys::generate()` 单点铸造 | Ed25519 keypair，`AgentKeystore::mint_key` 单点铸造 | **映射**。曲线不同无实质差异；Ed25519 的原语（`gateway/security/crypto.rs`）Aleph 早已有，只是**零生产消费者**——本轮是它的第一个真实调用方 |
| 私钥托管 | OS keyring / `0600` 文件 / env，best-effort scrub（自认 allocator 可能残留） | 既有 `SecretVault`：AES-256-GCM + 每条 HKDF salt + `Zeroizing` + `VaultIo` fcntl 原子写 | **Aleph 更强**。零新基建 |
| 密钥轮转 | **不存在**（撤销＝不再签发 attestation） | `rotate_identity` 保留旧钥（`retired_at`）以便旧记录仍可验，**不重置链锚** | **超越** |
| 轮转的原子性 | 无对位 | 铸钥 → 上链声明 → 绑定为活跃钥，**整个序列在单写者里**，调用方 await 结果 | **超越**（第四轮补）。两步协议的第二步一旦丢失就永久破链，而工具已经报了成功 |
| 密钥生命周期的可信度 | 无生命周期 | 轮转/撤销**写进主体自己的链**（`IdentityRotated`/`IdentityRevoked`），链首恒为签名的 `IdentityCreated` | **超越**。`retired_at`/`revoked_at` 是普通可变列，只靠它们＝"能写库就能把撤销抹掉且验链照样过" |
| 签名者归属 | 无签名 | 行声称的密钥必须**属于本链 agent**（`ForeignSigner`），退休钥算自己的 | **超越**。缺此检查时"伪造需要**该** agent 私钥"实为"需要**某个** agent 的私钥" |
| 记录结构 | `audit_log`，`(community_id, seq)` 哈希链 | `agent_ledger`，`(agent_id, seq)` 哈希链 | 映射。租户位换成 agent 位 |
| preimage 分帧 | presence tag，**无长度前缀** → 相邻变长字段拼接有歧义（两条不同记录可同哈希） | **每个变长字段 u32 长度前缀** | **修掉参考实现的缺陷**（`hash.rs::length_prefixes_make_adjacent_fields_unambiguous` 钉死） |
| 时间戳 | `to_rfc3339()` 进 preimage —— chrono 按值输出 0/3/6/9 位小数，纳秒值与其微秒截断是**不同字符串**，曾导致**每一条**都验不过 | **整数 epoch-ms** 进 preimage | **绕开整类问题**，不是补丁 |
| 签名 | **无**。只哈希，无密钥、无外部锚 | Ed25519 签在链哈希上 | **超越**：持有 DB 写权限不足以整链重算 |
| 尾部截断 | **结构性失明**（截断后的链内部自洽） | 检出（`agent_identities` 锚 + 下一条 seq 取 `max(锚, 末行)+1`，洞永久留痕） | **超越** |
| 前缀删除 | **结构性失明**（首行只自哈希校验，不与前驱或 genesis 对照） | 检出（首行必须 `seq=1` 且 `prev_hash IS NULL`） | **超越** |
| 整链清空 | 返回 `Ok(false)`，与"没什么可验"同值 | `ChainWiped` fault | **超越** |
| **身份/锚行被删** | 无对位（锚就是 `(community_id, seq)` 之外的东西，buzz 根本没有锚） | `verify_all` 枚举**身份表 ∪ 链表**，缺身份行 ⇒ `IdentityMissing` fault | **超越**（第三轮补；此前删一行就把整条链移出了验证视野） |
| **密钥来源** | 无（无密钥） | 每个签名钥必须由**链自己**引入（首行 signer，或某条 `IdentityRotated` 的 target），否则 `UndeclaredSigner` | **超越**（第三轮补）。没有它，删掉身份行后下一次 append 会**静默新铸一把钥续链**，全链 ok=true |
| 验证消费者 | **零**（`verify_chain` 无生产调用者；buzz-admin 全文无 "audit" 字样） | R8 工具 + 离线 CLI，**与链同批交付** | **超越**（这是 buzz 最大的实操缺口） |
| **离站验证** | 无（`verify_chain` 读 Postgres，只能在 relay 侧跑） | `export` 自包含文档 + `verify --input` **零依赖**（无 DB / 无 vault / 无 daemon）+ 根指纹钉住 | **超越**（第三轮补）。此前三处文档都声称有，代码里没有 |
| **离站检出尾部截断** | 无（`verify_chain(community, from, to)` 按区间验，对前缀/尾部双盲） | `--expect-head <seq>:<hash>` 钉上一份导出的链头，`HeadPin::{Extends,Truncated,Diverged}` 进 `ok` | **超越**（第四轮补）。此前它被三处文档点名为"唯一能发现截断的东西"却只是被**打印**出来供肉眼比对 |
| 记录丢失可见性 | worker 失败只计 metric，不重试；链对"从未写入"的记录无话可说 | `AgentLedger::lost()` 随每次 `list`/`ledger`/`verify` 一起返回；写入端用 `send().await` **背压**而非 `try_send` 丢弃；计数**落库**（`agent_ledger_health`），故**离线验证器与重启后的 daemon 都看得见** | **超越** |
| 关机时的队列 | 无对位（每条 append 同步过 advisory lock） | `identity::flush()` FIFO 屏障，优雅关机路径有界 await | **超越**（第四轮补）。排队未写＝**既丢记录又丢计数**，因为什么都没"失败" |
| 撤销标记 vs 链 | 无（无生命周期概念） | `revoked_at` 列与链自己的 `IdentityRevoked` 两侧对账（`revoked_per_chain`），不一致就报出来 | **超越**（第四轮补）。"改回 NULL 抹不掉撤销"此前**没有任何代码在做这个比较** |
| 委派身份 | 无子代理概念 | 子代理由 `AllowlistToolService` 就地开 `LEDGER_ACTOR` 作用域 → **自己的密钥、自己的链** | **超越**（参考实现无对位） |
| 身份→权限 | `Scope` 16 条枚举，但生产恒发 `all_known()`（形同虚设）；真正差异在 membership / `MemberRole` / NIP-OA | 既有 `tool_permissions` 三级合并（global→agent→channel，`restrictive_min` 只收紧）× exec tier | **Aleph 领先，刻意不移植** buzz 的 scope 层：再加一套并行权限模型违 P2/P6 |
| **谁可以用哪个 agent** | 无对位（无 agent 概念）——连接的 scope 由 token 决定，调用方无从自选 | 第五轮前：**调用方在 wire 上自己填 `agent_id`，零校验**。现 `AgentDefinition.allowed_users` × `caller_may_act_as_agent`，强制在三个 run-start 入口共用的 `build_run_request`（必填参数，不是 `Option`） | **修掉自己的缺陷**。上一行那套 `tool_permissions` 是**按 agent 分的**，而 agent 免检自选 ⇒ 权限层键在一条自助轴上：被某个 agent 拒了就换一个名字 |
| **记录里的"谁"** | 无（无签名，更无 principal） | `agent_id`（哪个身份在动）+ `principal`（哪个人在驱动），**两者都在 preimage 内** | **超越**（第五轮）。只记 agent 时，非否认对 agent 成立、对**人**不成立——共用一个 agent 的两个人在链上无法区分 |
| agent vs human | 只差**配额**不差权限 | 两条正交轴都在：exec tier（这次调用能做什么）× `allowed_users`（这个人能扮演谁） | **超越**（第五轮起） |
| owner attestation | NIP-OA：owner 签名证明"谁授权了这个 agent"，**作者身份不可改写** | 无 owner 密钥概念；委派的**事实**落在父链的 `ToolCall(target="subagent")` 上 | **刻意不移植**（见 §6 已知边界④：凭空造 owner 层＝零消费者抽象） |
| git 提交签名 | `git-sign-nostr`（O_NOFOLLOW/fstat/mode 检查/Zeroizing 全套） | 无 | **刻意不移植**：独立二进制 + `gpg.x509.program` 钩子，属生态外挂，按 R3 应做 Skill/MCP 而非进 core |
| Nostr wire / relay 联邦 | 核心 | 无 | **刻意不移植**：Aleph 信任边界是网络边界（loopback + device tier），不是公开 relay |

## 3. 威胁模型 —— 它买到什么，买不到什么

**买到：**
- **「谁做的」有两个答案，且都在签名里**：`agent_id` 是哪个身份在动，`principal` 是哪个**人**在驱动它。改写、抹掉或凭空安上一个 `principal`，三种编辑都会让哈希移位 ⇒ `HashMismatch`——它不是一个可以随手 UPDATE 的问责列。`principal` 为空只在真的没有人时出现（cron / heartbeat / 续跑 / A2A / 链自己的开篇记录），以及本列存在之前写下的行；两者对读者是同一句话——**这条链没点名任何人**。
- **一个人只能以他被允许的身份行动**。`agent_id` 仍是调用方给的字符串，但现在要过 `allowed_users`：一个被某个 agent 的 `tool_permissions` 拒掉的人，不能靠改一个名字换一套权限（第五轮前可以，见 §6 ①）。委派面同闸——否则「用允许的 agent 去委派给不允许的 agent」是一条两步都合法、合起来等价的路。
- 任何**已存储记录**的改写、重排、中段删除、跨 agent 搬运、前缀删除、尾部截断，都会被 `verify` 检出并定位到 seq。
- 伪造一条记录需要**该** agent 的私钥——仅有 DB 写权限不够（buzz 的 keyless 链在这一点上完全无防护：任何能写 DB 的人都能把整条链重算得天衣无缝）。「该」字是**执行出来的**：`ForeignSigner` 拒绝任何由别的 agent 的密钥签的行，哪怕签名在算术上完全有效。
- 密钥生命周期本身也在链内：链首是签名的 `IdentityCreated`，轮转与撤销各是主体链上一条签名记录。把 `revoked_at` 列改回 NULL **不能**让撤销消失。
- **换钥必须由链自己交代**。删掉可变的 `agent_identities` 行后，下一次 append 会新铸一把钥继续这条链——每一环有效、每一签名有效、密钥确属本 agent。`UndeclaredSigner` 抓的就是这一条：任何签名钥都必须被链内的 `IdentityCreated`/`IdentityRotated` 引入过。
- **可以交给不信任本机的人验证**——`agent_identity(action="export")` / `aleph-server identity export` 产出自包含文档，`aleph-server identity verify --input` 在**没有 DB、没有 vault、没有 Aleph** 的机器上跑同一套走查。前提是**钉住两个值**（下一节）：`--pin` 钉血统，`--expect-head` 钉链头。
- **换钥这件事本身也不会丢**。轮转是「铸钥 → 上链声明 → 绑定为活跃钥」，整个序列在单写者里跑完，调用方 await 结果。任何一步失败都**停在绑定之前**：出让方仍是活跃钥、仍被链声明，新钥无人指向。撤销反向同理（先上链、后打标），所以两半只能落一半时，活下来的是难以抹掉的那一半。

**买不到：**
- **不防拥有 `~/.aleph` 的对手**。vault、主密钥、数据库在同一块盘上。这是本地优先 daemon 的固有边界，没有 HSM 或远端公证就无法逾越。文档不假装它能。
- **不防进程内冒充**（见 §6）。
- **对"从未写入"的记录无话可说**。链只能证明它包含的东西。所以 `lost()` 计数与 `ok` 判定**并排返回**——干净的 `ok` 绝不可单独解读为"完整"。**注意"排队未写"是第三种结局**：什么都没失败，所以 `lost()` 也不会 +1。优雅关机路径 await `identity::flush()` 把它收窄到「被 `kill -9` 或超过 5 秒的写者」。
- **没钉指纹的导出什么也不证明**。造这份文档的人同时也挑了里面的公钥，所以拥有本机的对手可以现铸一把钥、签一条完全捏造的链，`verify --input` 干干净净地通过。把它变成证据要两个**各抄一次**的离站定值：**根指纹**（链开篇那把钥 —— 钉住后没人能拿另一条血统冒充这个 agent，因为换钥必须有一条**由被换掉那把钥签名**的轮转记录）与**链头**（上一份导出的 `last_seq`/`last_hash` —— 这是**唯一**能发现尾部截断的东西，因为锚是随文档走的，对手改它和改行一样自由）。

## 4. 架构

```
gateway/handlers/agent.rs::build_run_request   ← 「这个人能不能扮演这个 agent」
        │  caller_may_act_as_agent(agent.allowed_users)   （三个 run-start 入口共用的
        │    ├─ 通过 → 继续；顺带把 AUTHOR_USER_KEY 盖进 run metadata      唯一 builder，
        │    └─ 拒绝 → BuildRunError::AgentForbidden ⇒ PERMISSION_DENIED   参数必填不是 Option）
        ▼
agents/allowlist_tool_service.rs               ← 子代理身份注入（identity::as_actor）
        │  （只有它知道正在动作的 AgentDef，且它就在 Act 阶段
        │    per-call spawn 的任务里 —— 作用域必须开在这一层）
        ▼
tools/scoped/dispatch.rs::execute_inner        ← 唯一生产者（全库唯一进工具注册表的路径）
        │  ledger_agent_id()                    scoped actor ?? turn 的 session_key
        │  ledger_principal()                   visibility::ambient_actor()（谁在驱动）
        │  ledger_intent(name)                  （tools/scoped/ledger.rs）
        │    ├─ 变更类调用完成 → ToolCall(ok|error)
        │    ├─ 策略/钩子拒绝  → ToolDenied
        │    ├─ 审批裁决       → ApprovalGranted|ApprovalDenied
        │    └─ 无审批通道的 fail-closed 拒绝 → ApprovalDenied
        │         （record_gate_refusal；operator 闸与确认闸各一条，
        │           它们在 confirm_with_memory 之上返回，此前零记录）
        ▼
identity::record_action(NewRecord)             ← 有界 mpsc(1024)，send().await 背压
        │                                         （LedgerJob::Append，即发即忘）
        ▼
单写者任务 —— 四种 job，都在这一个队列里 FIFO
        │  Append  → AgentLedger::append          空链先落签名 IdentityCreated(seq 1)
        │                                         → 定位 → 哈希 → 签名 → 插入 → 推锚（单事务）
        │  Rotate  → perform_rotate               铸钥 → 上链声明（由新钥签）→ 绑定为活跃钥
        │  Revoke  → perform_revoke               上链声明（由被撤那把钥签）→ 打 revoked_at
        │  Flush   → 屏障，ack 即代表它之前的全部已落盘
        │       ▲
        │       └─ identity::{rotate_identity, revoke_identity, flush}（**await 结果**）
        │          ← builtin_tools/agent_identity.rs（R8 工具，纯 I/O）
        │          ← commands/start/mod.rs 优雅关机（flush，有界 5s）
        ▼
security.db : agent_keys / agent_identities / agent_ledger / agent_ledger_health（schema v13）
        ▲
        ├─ agent_identity 工具（R8，operator 门控）
        ├─ aleph-server identity（只读，无 runtime 无锁，daemon 停机亦可）
        └─ identity::export_chain → ChainExport（JSON，自包含）
                 │
                 ▼
           identity::verify_export（**零本地依赖**）
                 ▲
                 └─ aleph-server identity verify --input --pin
                    （在 open_ledger 之前分派 —— 这条路必须一步都不碰本机状态）
```

**为什么身份注入在 `AllowlistToolService` 而不是别处**：子代理跑在**父**的 `ScopedToolService` 上、继承父的 `TURN_CONTEXT`，而 `SessionKey::Subagent::agent_id()` 又把 `agent_id` 委派给 `parent_key`——三条路都指向父。唯一知道"此刻是谁在动作"的是 spawner 建的 allowlist 包装（它拿着子 `AgentDef`，spawner 早已用同一个 `agent_def.id` 标记 provider 用量与 tool signal）。而作用域**必须**开在它那一层：harness Act 阶段按工具调用 `tokio::spawn`，开在子 harness 的 run 外面对 spawn 出去的任务不可见——和 `TURN_CONTEXT` 由 `ScopedToolService::execute` 就地开、而非由更上层开，是同一条约束。嵌套时**最内层的角色胜出**：真正发起调用的那个角色拥有这条记录。

> **归属的时间语义**：`signer_fp` 是**写入时**的活跃密钥，不是**动作发生时**的。记录异步入队，所以一次发生在轮转之前的调用可能由轮转后的新钥签名。链照样自洽（`signer_fp` 在 preimage 内、且属于本 agent），只是别把它读成"动作时刻的密钥"。要那个语义得同步签名，代价是把签名放上工具热路径——不值。

**为什么单写者**：`(读头 → 哈希 → 签名 → 插入 → 推锚)` 必须不可交错。buzz 用 per-tenant Postgres advisory lock 买这个性质；单进程 daemon 结构上就有。`PRIMARY KEY (agent_id, seq)` 是后备闸——真出现第二个写者时**响亮失败**而非静默分叉。

**为什么背压不丢弃**：兄弟模块 `SecurityAuditLog` 用 `try_send` 满即丢（注释明写 best-effort）。度量流可以丢，问责记录不可以——链看不见从未写入的记录。

### preimage（改一个字节就作废所有既存链）

```
SHA256( "aleph-agent-ledger-v1"
      ‖ lp(agent_id) ‖ seq:i64be ‖ at_ms:i64be
      ‖ lp(action) ‖ lp(outcome) ‖ lp(target)
      ‖ opt_lp(args_fp) ‖ lp(detail) ‖ lp(signer_fp)
      ‖ prev_hash ?? 32×0x00
      ‖ [ 0x01 ‖ lp(principal) ]      ← 仅在 principal 非空时存在，否则一个字节都不发
```
`lp(x)` = u32 大端长度 ‖ 字节。`agent_id` 领头 → 记录搬到别人链上重算即不符。

**为什么 `principal` 排在最后、且空值不发字节**（第五轮）：这是**唯一**能让既存链继续验过的形状。域分隔串不动、`opt_lp` 不用（它的 `None` 会发一个 0x00 标记），于是一条没有 principal 的行与本列存在之前的行**逐字节相同**。改 `DOMAIN` 读起来更整齐，代价是作废磁盘上每一条链——本模块自己的 doc 明令禁止的那件事。分帧不受损：`prev_hash` 是定宽的，所以「字节到此为止」和「后面还有一个存在标记」不会混淆；标记本身仍然把 `Some("")` 和缺席分开。篡改**双向**可检出——从一行里抹掉 principal 会让 preimage 变短，往一条老行上安一个会让它变长，两种都移位摘要，而签名盖不住新的摘要。

### 记录里存什么、不存什么

- **不存原始参数**。存 `args_fp = grant_fingerprint(tool, canonical args)`（与会话授权、拒绝账本**同一指纹**，所以一条记录能和授权它的那次审批对上）。
- **存 `principal`：驱动这次动作的那个人**（`users.user_id`）。它由咽喉从 `visibility::ambient_actor()` 解析，**永不**取自工具参数——与 `agent_id` 同一道溯源栅栏，也是 `NewRecord` 至今不实现 `Deserialize` 的理由。两个被否掉的取法各自对应仓里踩过的坑：`CALLER_USER` 跨 `tokio::spawn` 即死而**每一次工具调用都在 spawn 里**（结果是每一行都记 `None`，一个在多用户真机上静默答「没有人」的列）；`ambient_owner()` 在项目房间里是**创建者**、对每个成员是同一个人（那正是他们共享记忆分区的机制），结果不是答不上来而是**自信地答错，并且签了名**。
- 存一行**已脱敏、已截断**的摘要，走审批卡片的同一单一源 `exec_approval::action.rs::redact_and_cap`（`Authorization: Bearer` 那次泄漏就是因为存在第二份脱敏逻辑；这里不再开第二份）。

## 5. `verify` 能检出什么

| fault | 触发 | 参考实现能否检出 |
|---|---|---|
| `HashMismatch{seq}` | 行内容被改 | ✅ |
| `ChainBroken{seq}` | 未链到前驱哈希 | ✅ |
| `BadSignature{seq}` | 哈希不是该密钥签的（无私钥无法伪造） | ❌ 无签名 |
| `UnknownSigner{seq,fp}` | 行声称的签名者本机从未铸造过 | ❌ |
| `ForeignSigner{seq,fp,owner}` | 密钥存在，但**属于别的 agent**（签名有效也不算数） | ❌ |
| `UndeclaredSigner{seq,fp}` | 密钥确属本 agent，但**链自己从没引入过它**（删身份行后的静默新铸） | ❌ |
| `IdentityMissing` | 有记录、无 `agent_identities` 行（锚没了，agent 从所有身份清单里消失） | ❌ **失明** |
| `PrefixMissing{first_seq}` | 链不从 1 开始 | ❌ **失明** |
| `GenesisNotNull{seq}` | 首条带前驱链接（被 re-base） | ❌ |
| `SeqGap{expected,found}` | 中段删除 / 截断后又追加留下的永久洞 | 部分（中段删除✅，截断洞❌） |
| `TailTruncated{anchor,last}` | 末尾被砍 | ❌ **失明** |
| `AnchorMismatch{seq}` | 末行在锚的位置上但不是锚的哈希 | ❌ |
| `ChainWiped{anchor}` | 锚记得有链，行全没了 | ❌（返回 `Ok(false)`，与"无可验"同值） |

`verify` **报告全部** fault 而非首个即停：判断发生了什么需要损伤的**形状**，不只是存在性。

两个**不是 fault** 但同样进报告的判定（它们各有一个良性成因，报成 fault 会喊狼）：

| 字段 | 含义 | 良性成因 |
|---|---|---|
| `revocation_disagrees()` | `revoked_at` 列与链自己的 `IdentityRevoked` 说法不一 | 一条生命周期记录在写入前就丢了（`failed_appends` 已经计过） |
| `head_pin`（仅离站） | 本文档相对**上一份导出**的关系：`Extends` / `Truncated` / `Diverged` | 无——`Truncated`/`Diverged` 直接进 `ok=false` |

**`UndeclaredSigner` 判的是集合成员，不是相邻关系** —— 记录异步入队，所以一次在轮转**之前**发起的调用完全可能落在轮转记录**之后**、并由新钥签名（同 §4 的"归属的时间语义"）。要求轮转记录必须紧邻它覆盖的第一行，等于把这个竞态当成篡改报出来。判据是：链内出现过的每个 `signer_fp`，都必须是首行的签名钥、或某条 `IdentityCreated`/`IdentityRotated` 的 `target`。

**枚举的是身份表 ∪ 链表**。`verify_all` 若只走身份表，删一行就让那条链**整个退出验证视野**，得到一句"全部链 OK"——它只是不再看那一条了。同理 `verify(agent)` 对"有记录无身份行"报 fault 而不是抛 `UnknownAgent`：只有既无身份又无记录才叫未知 agent。

**公钥每链取一次，不是每行取一次**。一条 N 行的链最多点名几个签名者（每次轮转一个），参考实现（以及本模块的第一版）却按行查库——N 次加锁往返回答 K 个不同的问题，长链上这个开销**数量级地**盖过签名验证本身。`Keyring` 一次 `keys_of(agent)` 载入该 agent 全部历史公钥，链外指纹的回查也记忆化。

## 6. 已知边界（刻意留下，不要当成漏做）

1. ~~**不是防冒充**~~ → **第五轮已闭合，但闭合的是授权那一半，不是「agent_id 不再由调用方给」**。`agent_id` 仍是 `chat.send` / `agent.run` 上调用方传入的字符串，`router.rs::route` 仍原样返回——变的是这个字符串现在**要过一道谓词**：`build_run_request` 拿它实际解析到的 `AgentInstanceConfig`，问 `caller_may_act_as_agent(allowed_users)`。
   - **默认仍然全开**：`allowed_users` 未设或为空 ＝ 所有认证调用方，与 `allowed_links` 同一约定。单用户装机与全部存量配置**行为逐字节不变**——这不是遗漏，是那条零变更保证。所以「没配 `allowed_users` 的部署里，任何认证调用方仍可扮演任意 agent」**依然成立**，只是它现在是一个**可以关掉的默认**而不是一条无法表达的事实。
   - **无 gateway 连接的调用方不受限**（cron / heartbeat / A2A / teams dispatcher / 进程内测试），与仓里每一条同族谓词的第一臂一致。loopback **不属于**这一类：它解析成隐式 owner，所以本机 Panel 带着 `Some(owner)` 到达。
   - **仍然不防的**：一个人在他**被允许的** agent 之内做的事。那不是冒充，是授权——而账本现在会记下是谁做的（`principal`）。
   - **仍然不防的**：把 `agent_id` 绑到**设备**授权范围。那是 RPC 授权模型的改动，本轮没做，因为身份的粒度是**人**（`users.user_id`）而不是设备，且设备已经解析成人。
   - **`agent.resume` 曾经不过这道闸；第七轮（2026-08-10）补上了。** 它不经 `build_run_request`——按会话**已存的**归属重跑一个中断的 run——而它又是 member-open（`method_admin.rs` 的 `MEMBER_CARVE_OUTS` 逐字钉着它，那条注释自己写的是「与启动一个 run 是同一个授权问题」）。于是收紧 `allowed_users` 之后，一个已被移出名单的人仍能恢复他自己那条会话里早先被中断的 run。豁免站在两条腿上，**承重的那条是个 bug**：
     - ~~① 撤销本来就要重启才生效（`[agents]` 不是 live section），所以这条窄缝的前提是「重启之后、那条中断 run 还在」~~ —— 第六轮把 `AgentRegistry::set_allowed_users` 修成真的之后，这句话当天就不成立了，窄缝从「重启之后才可达」变成「**紧跟撤销就可达**」，而 resume 那侧一个字符都没动、一条测试都不会红。**这是「一个缺陷被别处引用成安全论证」的教科书实例**，判据已提到根 CLAUDE.md。
     - ② resume 不接受新输入（`ResumeParams` 只有 `session_key`），所以他**无法操纵**它——想操纵就得 `chat.send`，而那条路过闸。**这条仍然为真，但它一条腿撑不住**：`allowed_users` 守的是 `tool_permissions` 的 agent 轴，而续跑不是重播一份已定的 transcript——它**重新进 harness、继续按那个 agent 的权限调工具**。「那份工作被中断时已经过闸」说的是**已经做完的**那部分，对下一轮模型自己想出来的动作一个字都没说。
   - **现在的形状**：闸在 `resume_named_session` 里，**排在 `session_visible` 之后**（可见性有存在性秘密要守 ⇒ `not_found`；准入闸只在那个秘密已经花掉之后才可达 ⇒ 诚实的 `PERMISSION_DENIED`，措辞与 `BuildRunError::AgentForbidden` 同源）。registry 走**必填参数**而不是第二个 `global_*` 句柄——新面漏传是编译错误，全局漏设是沉默（`build_run_request` 的 `agent` 参数不是 `Option` 也是这个理由）。`None` = 这台 server 根本没有 registry（Simulated 构建），它也不跑工具，所以那不是洞；`/v1/admin` 传 `None` 是因为那条路没有 `CALLER_USER`，谓词在那里**按构造**恒真，塞一个进去只会看起来像一道从不生效的闸。
   - **仍然不问的**：run 依旧按会话**已存**的归属重入，从不按调用者的。resume 不是「以我的身份跑点什么」的路子，这道闸只决定你有没有资格说「把它接上」。
2. **无 turn context 即不记录**。`approval::audit_identity` 在 turn 外回退字面量 `"main"`——对一行日志是合理默认，对**签名链就是伪造**。所以 `ledger_agent_id` 返回 `None`。（注意：**只有** turn 缺失才返回 `None`；子代理的角色注入是在有 turn 的前提下**替换**归属，不是新增一条无 turn 的路径。）
3. **`revoke` 不是执行闸**。本子系统不拦任何执行。所以被撤销的 agent 若仍在动作，记录**照记**（用其已 retire 的钥签，`AgentKeystore::signing_identity`），而不是拒签。理由：拒签不会阻止行为，只会消灭证据；而"这个 agent 在被撤销 40 分钟后还在动作"恰恰是问责账本最该能证明的事。`revoke` 的真实语义是：标记该身份、retire 其密钥、拒绝 `keygen` 重新启用（要回来必须显式 `rotate`），并在其链上留下一条**由被撤销的那把钥自己签的** `IdentityRevoked`。
4. **无 owner 层**。buzz 的 NIP-OA（owner 签名证明"谁授权了这个 agent"，作者身份永不改写）没有移植：Aleph 没有 owner 密钥概念，凭空造一个是没有消费者的抽象（YAGNI 撤回规则）。父子委派的**事实**已经落在父链上（`ToolCall(target="subagent")`，`detail` 带 `agent_type`），再加一个 `Delegation` 变体是零增量信息。
5. **不做启动时验链**。全量验证要读遍每条链的每一行并逐行验签，那不该挂在启动路径上；而且"daemon 自己写的日志里有一行 warning"本来也不是任何人会据以行动的证据。验证属于被问到的时候——以及，对真正要紧的场景，属于**没写这些记录的那个进程**（`aleph-server identity verify`）。

6. **导出的锚是随文档走的**。`ChainExport.anchor_seq` / `anchor_hash` 由产出文档的那台机器写，所以**光靠文档自身检不出尾部截断**——对手把行删掉、把锚一起改小即可，根指纹也钉不住（截断后的链仍开在同一把钥下）。解法是**钉链头**（第四轮落地）：`--expect-head <seq>:<hash>` 要求本文档在那个 seq 上有那一行、那个哈希，`Truncated` / `Diverged` 直接判 `ok=false`。**仍然是外带定值**——它证明的是"这份是我上次见到那份的延长"，而不是"这份完整"；从没导出过第一份的人拿不到这个保证。这条边界因此没有消失，只是从"无法检出"变成"必须有人在链外记住一个值"。
7. **不做增量/分段导出**。导出恒为整链：前缀与那条 `IdentityCreated` 正是"这条链从哪开始、开在哪把钥下"的依据，从中段起的片段两样都证明不了（并且会直接踩 `PrefixMissing`）。

**已解决（勿再按旧结论行事）**：
- ~~工具的 `DESCRIPTION` 随 schema 发给模型~~ → **它没有**（第四轮修）。`BUILTIN_TOOL_DEFINITIONS` 的手写字面量整体遮蔽了常量，而那条字面量连 `export` 都不提；第三轮写进 DESCRIPTION 的整套钉指纹说明因此一个模型都没收到。现指向常量，守卫断言在**目录那一侧**。
- ~~rotate/revoke 之后那条链记录一定会写进去~~ → **它可能丢，且丢了就永久破链**（第四轮修：两半收进单写者并 await）。
- ~~进程退出时队列里的记录至多是"丢了并被计数"~~ → **既没写也没计数**（第四轮修：`flush()` 屏障 + 关机 await）。
- ~~离站验证已经能靠 `--pin` 兜住~~ → `--pin` 只兜血统；尾部截断要 `--expect-head`，而它第四轮才有实现。
- ~~子代理的工具调用记在父 agent 名下~~ → 已修（§4 的身份注入）。团队成员从来不受影响：成员 run 自己拥有一个 turn（`SessionKey::task(agent_id, "team", …)`）。
- ~~`lost()` 只在写入进程内可见~~ → 已落库（`agent_ledger_health`），离线验证器与重启后的 daemon 都读得到。
- ~~删掉 `agent_identities` 一行即可让整条链退出验证~~ → 已修（第三轮：`verify_all` 枚举并集 + `IdentityMissing`）。
- ~~删身份行后 agent 再动作一次即静默换钥续链、验链干净~~ → 已修（第三轮：`UndeclaredSigner`）。
- ~~两条无审批通道的 fail-closed 拒绝零记录~~ → 已修（第三轮：`record_gate_refusal`）。
- ~~"公钥可导出、导出的链可被不信任本机的人验证"三处有声称、零实现~~ → 已修（第三轮：`identity/export.rs` + 工具 `export` + `identity verify --input --pin`）。

## 7. 红线合规

- **第五轮补充**：**R10** `src/harness/` 仍零改动（五轮皆是）；**R3/P6** 零新依赖（五轮皆是）；**Spec C** schema **v16**（`agent_ledger.principal`），迁移用「先探列再 ALTER」——与 v15 同形状同理由：从零建库的实例在更早的臂里已经跑过 `IDENTITY_SCHEMA`，而那份 batch 现在带着 `principal`，无条件 ALTER 会在**每一次首启**上撞 `duplicate column name`；**R8** 工具注册点零变化（`agent_identity` 一个 action 都没加，`agent_update` 只加了一个参数），但 `agent_update` 与 `agent_unbind` **进了 `OPERATOR_TOOLS`**——前者现在写的正是 run-start 闸读的那张表，闸必须覆盖能把闸拿掉的那个动词；**R7** 新增判定没有一个是推理（`agent_admits_user` 是集合成员判断，`principal` 是一次 task-local 读取）。
- **R10**：`src/harness/` **零改动**（三轮都是）。账本挂在 `tools/scoped/`，身份注入挂在 `agents/allowlist_tool_service.rs`；harness 只经 `Arc<dyn ToolService>` 多态调用，从不点名任何一个。棘轮以 `budget.rs::CEILING` 为准，本轮不动它。
- **R3 / P6**：**零新依赖**（三轮都是）。`ed25519-dalek` / `sha2` / `hex` / `zeroize` 早已是直接依赖，Ed25519 原语早已存在且此前**零生产消费者**；导出格式用的 `serde` / `serde_json` 同理。
- **R8**：第三轮只给 `agent_identity` **加了一个 action**（`export`），第四轮一个 action 都没加 —— 所以下面那 6 个注册点两轮都不用动。⚠️ **但第 7 个登记面是第四轮才发现的**：`BUILTIN_TOOL_DEFINITIONS` 的 `description` 字段。写成字面量就**整体遮蔽** `AlephTool::DESCRIPTION`（`agent_init` 只追加目录里没有的名字），于是往 DESCRIPTION 里写的任何东西模型一个字都收不到。判据：**往 `DESCRIPTION` 里写模型必须看到的内容之前，先确认目录条目指向常量**——这条对全仓 156 个工具都成立，见 CLAUDE.md R9 前置条件段。
- **R7**：第四轮新增的判定没有一个是推理：`HeadPin` 是三路哈希比较，`revoked_per_chain` 是取链上最后一条生命周期记录，`revocation_disagrees` 是两个布尔比大小。账本仍然只**记录**，不评分、不分类、不选恢复策略。
- **R4**：`src/gateway/security/store/identity.rs` 纯 SQL I/O（对齐 `devices.rs`/`tokens.rs`），全部摘要/哈希/签名在 `src/identity/`。
- **R7**：账本**记录**，不**评分**、不分类、不选恢复策略。"是否变更类"读工具**自己声明**的 `is_idempotent` 元数据（与 exec tier 同一个 `tool_facts` 缝），不猜意图、不查名单。
- **R8**：能力经 `agent_identity` 工具对话式可达，`OPERATOR_TOOLS` 门控。**6 个注册点**（改工具时别漏）：`builtin_tools/mod.rs` · `builtin_registry/definitions.rs`（表项 + `create_tool_boxed`）· `builtin_registry/groups.rs`（**唯一有测试强制的一处**）· `builder/core_tools.rs`（元数据/schema）· `registry/tool_registry_impl.rs`（**真正的执行分派臂**）· `method_authz.rs::OPERATOR_TOOLS`。漏掉后两者中任何一个 ＝ 工具被通告给模型却在调用时报错或越权。
- **Spec C**：走 `SecurityStore`（`open_sqlite_safe`：WAL / busy_timeout），不新开数据库文件。schema **v13** 的迁移臂直接重跑 `IDENTITY_SCHEMA`（全是 `CREATE ... IF NOT EXISTS`），这样表结构只有**一份**定义，而不是在迁移里放第二份会漂移的副本。

## 8. 使用

对话（模型或操作员）：
```
agent_identity(action="verify")                    # 验全部链
agent_identity(action="ledger", agent="main", limit=50)
agent_identity(action="show", agent="main")        # 身份 + 全部历史密钥 + 近期记录
agent_identity(action="rotate", agent="main")      # 换钥；历史仍可验，链不重置
agent_identity(action="export", agent="main")      # 写出自包含文档，回 path + 该钉的两个值
```

`rotate` / `revoke` **会等**那条生命周期记录真的写进链才返回。写不进就报错——而不是回一句 `ok` 然后让新钥在一条从没声明过它的链上签下去（那会让此后每一行都 `UndeclaredSigner`，永久）。

`export` **写文件、不内联返回**：链是无界的，而这份文档的用途是交给别人，不是给模型读——内联等于把上下文窗口花在这段对话里没人会看的字节上。落点是 `<data_dir>/exports/`，文件名派生自 agent id，**不接受调用方给的路径**（不新增任何文件系统触达面，也没有穿越可写错）。

限制谁能扮演一个 agent（第五轮）——config 里一行，或对话式一句：

```toml
[[agents.list]]
id = "ops"
allowed_users = ["u-alice"]     # 空或不写 = 所有认证调用方（出厂即此，零变更）
```
```
agent_update(agent_id="ops", allowed_users=["u-alice"])   # 清空＝改回所有人：allowed_users=[]
```

`agent_update` 的 **`allowed_users` 下一轮即生效**（第六轮，2026-08-10）：它把新名单装进 `AgentRegistry`——run-start 闸读的正是那个对象——所以一次**撤销**在被拒者的下一回合就绑定。`name` / `description` / `model` **仍然是写 config.toml、重启后才生效**，所以 `takes_effect` 现在**按本次 patch 实际落地的情况分开措辞**，两半的文案仍取自 `ReloadImpact`。**`Live` 只在 registry 写入返回 `true` 时才声称**（与 `live_apply::classify_verified` 同一条降级规则）——说「已经生效」而它没有，正是这个字段存在要防的那件事。**RPC 面 `agents.update` 走同一个 registry 方法**并回 `allowed_users_applied_live`，两张脸不会对「撤销有没有发生」给出两个答案。被拒的调用方拿到的是**诚实的 `PERMISSION_DENIED` 并点名 agent**，不是 `not_found`——`agents.list` 本来就对每个认证调用方返回全部 agent，没有存在性秘密要守；而一道谜语只会把忘了把自己加进列表的 operator 推向「干脆全开」。

`list` 里出现的不只是顶层 agent：**做过变更类动作的子代理角色各有自己的一条链**（身份即 `AgentDef.id`，故同一角色跨多次委派共用一条链——这是想要的，角色就是身份）。密钥在**首次被记录的动作**时铸造，纯只读的角色永远不会铸钥。链首那条 `identity_created` 就是它开始的地方。

离线（daemon 停机亦可，这正是重点）：
```
aleph-server identity list
aleph-server identity ledger --agent main --limit 40
aleph-server identity verify          # 有 fault 则非零退出
aleph-server identity export --agent main --out chain.json
```

**在审计方的机器上**（没有 Aleph、没有 `security.db`、没有 vault —— 这一条在 `open_ledger` 之前分派，所以它是真的零依赖）：

```
aleph-server identity verify --input chain.json \
    --pin <第一次拿到的根指纹> \
    --expect-head <上一份导出报的 expect_head>
```

两个钉都可以省，但输出会**每次都把省掉的那个说出来**：没有 `--pin` 就只证明了内部自洽（造文档的人也挑了里面的公钥），没有 `--expect-head` 就**检不出尾部截断**（锚是随文档走的）。写不出格式的 `--expect-head` 会被**拒绝**而不是当成没给——一个被静默读成"没钉"的钉子看起来和成功一模一样。

## 9. 熵减

### 第五轮（2026-08-10）

本轮的两件事互为前提，**顺序不能反**：`principal` 记的是「谁驱动了这次动作」，而在 agent 可以被免检自选之前，那个人本来就能挑一套权限——先记下来只会得到一份忠实记录的越权。

- **`agent_admits_user` 是一条规则，不是三条**。`allowed_users` 在到达闸之前经过三个类型（`AgentDefinition` → `ResolvedAgent` → `AgentInstanceConfig`），各写一个谓词就是三次分歧机会——`restrictive_min` 对权限、`session_visible` 对会话都是这么收敛的。
- **闸的参数必填，不是 `Option`**。`method_visibility.rs` 那张表存在的理由是「删掉一次调用会变成一条指名道姓的测试失败」；这里删掉它是**编译错误**，更强，所以本轮**没有**给这张表加条目，也**没有**加源码级 pin——加了就是第二个更弱的真源。
- **`agent_update` 的 `takes_effect` 两半都取自 `ReloadImpact`，不是字面量**。第五轮它每个字段都是「写进去了、但要重启」，因为运行时那一半是 no-op；第六轮 `allowed_users` 有了真的运行时半边，于是一次 patch **可能同时跨两个答案**，措辞按**本次调用实际落地了什么**合成。把任一半写成常量会得到第二份表述。⚠️ **`:cleared` 是报告的装饰不是另一个字段**——`is_live_field` 按基名匹配，否则「清空名单」会被描述成需要重启，而 registry 早就改完了（往安全方向撒的谎，最容易活过评审）。
- **目录字节自己付账**。两处 `DESCRIPTION` 新增让 `CATALOG_DESCRIPTION_CEILING_BYTES` 超了 681 B，还的方式是在**同一批描述里**删掉参数 schema 已经发出去的 JSON 字面量与复述它的散文，而不是抬闸。
- **`agent_identity` 的目录守卫改判**「这一行 bullet 介绍了这个 action」而非「描述里出现了 `"action": "x"` 这个 JSON 拼法」。判据落在性质上；落在排版上的守卫会为一次无害的改写变红，而那正是守卫被赶时间的人放松的方式。
- **顺手修好一处先于本轮存在的损坏**（独立提交）：`loop_graph::service` 的 `unicode_line_separators_in_root_body_cannot_forge_a_root_line` 断言自相矛盾（`trim_start()` 后要求没有任何 `根参照` 行，同一个测试两句之后又要求真的根参照必须在第 0 列），自落地起一直红、连带 `cargo test -p alephcore --lib` 一起红。**防御本身是对的**——伪造行确实被缩进了；红的是断言。改成它 ASCII 孪生二十行之上一直用对的那个形状，并补上真正承重的那一句（`lines()` 只按 `\n` 切，所以拿掉 Unicode 映射后伪造内容会藏在**一行之内**、列检查照样通过），用变异证过 RED。

### 第四轮（2026-08-04）

- **`AgentKeystore::rotate` 删除**，替换为它一直藏着的两个原语 `mint_key` / `activate`。理由不是"更小"，而是**这两步之间必须放一条链记录**——合成一个方法就把那条记录变成了调用方事后要补的第二步，而那正是本轮 F1 修的 bug。组合现在只有一处：`AgentLedger::perform_rotate`。
- **`ExportPins` 取代 `&[String]`**，两种钉法一个入口。加第三种钉法时不必再改一次签名。
- **root fingerprint 只推导一次**。CLI 的 `export` 曾自己 `records.first().signer_fp` 算一遍；现在两张脸都读 `verify_export` 的报告。
- **`ExportPins::is_empty` 当轮撤回**（写了、零消费者）。
- **顺手修好两处先于本轮存在的损坏**（独立提交）：`shared/logging` 的 `#[deprecated(since = "2026.08.04")]` 不合 semver，让 `cargo clippy --all-targets` 对**整个 workspace** 失败；`tests/cron_probe/delivery_alert.rs` 引用已删的 `pre_delivery_status`——**第四次**同型（`cf6db395b` / `dc8d32e0d` / `8ee77389b`），这次藏在 `test-helpers` feature 门后面，`cargo check` 与不带 feature 的 `cargo test --test '*'` 都编不到它。

### 第三轮（2026-08-01）

本轮没有可删的死代码（第二轮已清过一遍），熵减体现在**没有生出第二份**：

- **一套链走查，两个公钥来源**。离站验证本可以自成一个"轻量校验器"——那就是第二份安全检查，也正是本模块删掉 `AgentKeystore::verify` 的同一个理由。改为把 `verify_chain` 的循环抽成 `walk_chain(rows, anchor, &mut dyn SignerSource)`，`Keyring`（读 `security.db`）与 `ExportKeyring`（读文档内嵌公钥）各实现一次 `check`。验签原语只在 `check_against` 一处调用，所以"有效"在两条路上是同一个意思。
- **`ExportedRecord` 单独定义，不给 `LedgerRecord` 加 `Deserialize`**。理由与 `NewRecord` 那道溯源栅栏相同：能被追加的类型不该有反序列化入口。附带好处是导出行**不带 `agent_id`**（文档只声明一次 agent，每行按它重建），于是从别人链里搬过来的行在到达时就哈希不上——不需要为它新造一个 fault。
- **顺手修好主分支上先前存在的编译损坏**：`src/config/tests/mod.rs` 引用已删除的 `dispatcher` 模块、`src/config/tests/serialization.rs` 有一段重复粘贴的括号残片（来自 `54a12d89b` 的 cut）。二者让**整个 lib 测试目标编译不过**。⚠️ 仍有 47 个同型错误散在 6 个无关子系统（`config/ui_hints` / `a2a/adapter/client/pool` / `browser/{manager,profile}` / `tool_metadata/registry/tests` / `context/retrieval/content_index` / `agents/swarm/tasks/store/tests` / `tests/tool_scheduling.rs`）——全是**测试代码引用已被裁剪掉的生产 API**。本轮不碰（跨 6 个子系统＝失控重构），但因此**本轮新增测试全部落在 `tests/`**，那里编译且真的跑得起来。

### 第二轮删除

- `AgentKeystore::verify`：本轮之后**零生产消费者**（只剩自己的测试）。它还是个**更弱的第二条验证路径**——按指纹查表验签，不问这把钥是不是本链 agent 的。签名校验现在只有 `verify.rs` 一处，`Keyring` 同时回答"签名对不对"和"这把钥凭什么能签这条链"。
- `KeyError::Crypto(#[from] CryptoError)`：随上一条一起变成**零构造者**变体——正是本子系统第一轮删 `memory_audit_log` 时给出的理由。

### 第一轮删除

- `memory_audit_log` 表 + 三个索引：有 `actor` 列、**全库零 INSERT**。与 2026-07-14 被删的审批审计库同型（"operator 跑出来永远是 0，比死代码更坏"）。经 `drop_obsolete_tables` 从既存 DB 一并删除。
- `memory::audit` 的 `AuditEntry` / `AuditActor` / `AuditAction` / `AuditDetails` / `ForgettingExplanation`：零构造者。模块改名 `memory::explain`，只留真正被读的 `FactExplanation` / `ExplainedEvent`（`memory_timeline` 工具与 `TimeTraveler::explain_fact` 在用）。
- `ApprovalSource::Autoconfirm`：零生产者，无任何存储行能带它。同批把 `Trusted` **接上**（会话授权短路），`User` 只留给真人当场裁决。
