# AGENT_IDENTITY.md — 每个 Agent 的独立密钥与签名操作账本

> 「带签名日志解决了'发生过什么'，身份权限解决了'谁能做什么'。」
> 本文覆盖前半句的**实现**、后半句的**接线点**，以及两句**都没有覆盖**的边界。
> 代码锚点 `src/identity/`；对照 FEATURE_LOCATOR §5.17。

---

## 1. 一句话

每个 agent 持有自己的 Ed25519 密钥对；它做的每一次**变更类工具调用**、每一次**被拒**、每一次**审批裁决**，都追加进**它自己的**哈希链并由**它自己的密钥签名**。`agent_identity` 工具与 `aleph-server identity` CLI 负责读与验。

## 2. 为什么是这个形状（Gap Analysis vs buzz）

参考项目 **buzz**（`T:/Github/buzz`，Nostr relay 工作区，"same audit trail, a different keypair"）。逐维度对照 —— **改这一层前先看这张表，不必重做对比**：

| 维度 | buzz | Aleph（第三轮后，2026-08-01） | 取舍 |
|---|---|---|---|
| 身份载体 | secp256k1 keypair 即身份，`Keys::generate()` 单点铸造 | Ed25519 keypair，`AgentKeystore::mint` 单点铸造 | **映射**。曲线不同无实质差异；Ed25519 的原语（`gateway/security/crypto.rs`）Aleph 早已有，只是**零生产消费者**——本轮是它的第一个真实调用方 |
| 私钥托管 | OS keyring / `0600` 文件 / env，best-effort scrub（自认 allocator 可能残留） | 既有 `SecretVault`：AES-256-GCM + 每条 HKDF salt + `Zeroizing` + `VaultIo` fcntl 原子写 | **Aleph 更强**。零新基建 |
| 密钥轮转 | **不存在**（撤销＝不再签发 attestation） | `rotate` 保留旧钥（`retired_at`）以便旧记录仍可验，**不重置链锚** | **超越** |
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
| 记录丢失可见性 | worker 失败只计 metric，不重试；链对"从未写入"的记录无话可说 | `AgentLedger::lost()` 随每次 `list`/`ledger`/`verify` 一起返回；写入端用 `send().await` **背压**而非 `try_send` 丢弃；计数**落库**（`agent_ledger_health`），故**离线验证器与重启后的 daemon 都看得见** | **超越** |
| 委派身份 | 无子代理概念 | 子代理由 `AllowlistToolService` 就地开 `LEDGER_ACTOR` 作用域 → **自己的密钥、自己的链** | **超越**（参考实现无对位） |
| 身份→权限 | `Scope` 16 条枚举，但生产恒发 `all_known()`（形同虚设）；真正差异在 membership / `MemberRole` / NIP-OA | 既有 `tool_permissions` 三级合并（global→agent→channel，`restrictive_min` 只收紧）× exec tier | **Aleph 领先，刻意不移植** buzz 的 scope 层：再加一套并行权限模型违 P2/P6 |
| agent vs human | 只差**配额**不差权限 | 同（exec tier 与 agent 轴正交） | 平手 |
| owner attestation | NIP-OA：owner 签名证明"谁授权了这个 agent"，**作者身份不可改写** | 无 owner 密钥概念；委派的**事实**落在父链的 `ToolCall(target="subagent")` 上 | **刻意不移植**（见 §6 已知边界④：凭空造 owner 层＝零消费者抽象） |
| git 提交签名 | `git-sign-nostr`（O_NOFOLLOW/fstat/mode 检查/Zeroizing 全套） | 无 | **刻意不移植**：独立二进制 + `gpg.x509.program` 钩子，属生态外挂，按 R3 应做 Skill/MCP 而非进 core |
| Nostr wire / relay 联邦 | 核心 | 无 | **刻意不移植**：Aleph 信任边界是网络边界（loopback + device tier），不是公开 relay |

## 3. 威胁模型 —— 它买到什么，买不到什么

**买到：**
- 任何**已存储记录**的改写、重排、中段删除、跨 agent 搬运、前缀删除、尾部截断，都会被 `verify` 检出并定位到 seq。
- 伪造一条记录需要**该** agent 的私钥——仅有 DB 写权限不够（buzz 的 keyless 链在这一点上完全无防护：任何能写 DB 的人都能把整条链重算得天衣无缝）。「该」字是**执行出来的**：`ForeignSigner` 拒绝任何由别的 agent 的密钥签的行，哪怕签名在算术上完全有效。
- 密钥生命周期本身也在链内：链首是签名的 `IdentityCreated`，轮转与撤销各是主体链上一条签名记录。把 `revoked_at` 列改回 NULL **不能**让撤销消失。
- **换钥必须由链自己交代**。删掉可变的 `agent_identities` 行后，下一次 append 会新铸一把钥继续这条链——每一环有效、每一签名有效、密钥确属本 agent。`UndeclaredSigner` 抓的就是这一条：任何签名钥都必须被链内的 `IdentityCreated`/`IdentityRotated` 引入过。
- **可以交给不信任本机的人验证**——`agent_identity(action="export")` / `aleph-server identity export` 产出自包含文档，`aleph-server identity verify --input` 在**没有 DB、没有 vault、没有 Aleph** 的机器上跑同一套走查。前提是**钉住根指纹**（下一节）。

**买不到：**
- **不防拥有 `~/.aleph` 的对手**。vault、主密钥、数据库在同一块盘上。这是本地优先 daemon 的固有边界，没有 HSM 或远端公证就无法逾越。文档不假装它能。
- **不防进程内冒充**（见 §6）。
- **对"从未写入"的记录无话可说**。链只能证明它包含的东西。所以 `lost()` 计数与 `ok` 判定**并排返回**——干净的 `ok` 绝不可单独解读为"完整"。
- **没钉指纹的导出什么也不证明**。造这份文档的人同时也挑了里面的公钥，所以拥有本机的对手可以现铸一把钥、签一条完全捏造的链，`verify --input` 干干净净地通过。把它变成证据要两个**各抄一次**的离站定值：**根指纹**（链开篇那把钥 —— 钉住后没人能拿另一条血统冒充这个 agent，因为换钥必须有一条**由被换掉那把钥签名**的轮转记录）与**链头**（上一份导出的 `last_seq`/`last_hash` —— 这是**唯一**能发现尾部截断的东西，因为锚是随文档走的，对手改它和改行一样自由）。

## 4. 架构

```
agents/allowlist_tool_service.rs               ← 子代理身份注入（identity::as_actor）
        │  （只有它知道正在动作的 AgentDef，且它就在 Act 阶段
        │    per-call spawn 的任务里 —— 作用域必须开在这一层）
        ▼
tools/scoped/dispatch.rs::execute_inner        ← 唯一生产者（全库唯一进工具注册表的路径）
        │  ledger_agent_id()                    scoped actor ?? turn 的 session_key
        │  ledger_intent(name)                  （tools/scoped/ledger.rs）
        │    ├─ 变更类调用完成 → ToolCall(ok|error)
        │    ├─ 策略/钩子拒绝  → ToolDenied
        │    ├─ 审批裁决       → ApprovalGranted|ApprovalDenied
        │    └─ 无审批通道的 fail-closed 拒绝 → ApprovalDenied
        │         （record_gate_refusal；operator 闸与确认闸各一条，
        │           它们在 confirm_with_memory 之上返回，此前零记录）
        ▼
identity::record_action(NewRecord)             ← 有界 mpsc(1024)，send().await 背压
        │       ▲
        │       └─ builtin_tools/agent_identity.rs：rotate/revoke 后补一条
        │          IdentityRotated / IdentityRevoked 到**主体自己**的链
        ▼
单写者任务 AgentLedger::append                  ← 空链先落签名 IdentityCreated(seq 1)
        │                                         → 定位 → 哈希 → 签名 → 插入 → 推锚（单事务）
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
      ‖ prev_hash ?? 32×0x00 )
```
`lp(x)` = u32 大端长度 ‖ 字节。`agent_id` 领头 → 记录搬到别人链上重算即不符。

### 记录里存什么、不存什么

- **不存原始参数**。存 `args_fp = grant_fingerprint(tool, canonical args)`（与会话授权、拒绝账本**同一指纹**，所以一条记录能和授权它的那次审批对上）。
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

**`UndeclaredSigner` 判的是集合成员，不是相邻关系** —— 记录异步入队，所以一次在轮转**之前**发起的调用完全可能落在轮转记录**之后**、并由新钥签名（同 §4 的"归属的时间语义"）。要求轮转记录必须紧邻它覆盖的第一行，等于把这个竞态当成篡改报出来。判据是：链内出现过的每个 `signer_fp`，都必须是首行的签名钥、或某条 `IdentityCreated`/`IdentityRotated` 的 `target`。

**枚举的是身份表 ∪ 链表**。`verify_all` 若只走身份表，删一行就让那条链**整个退出验证视野**，得到一句"全部链 OK"——它只是不再看那一条了。同理 `verify(agent)` 对"有记录无身份行"报 fault 而不是抛 `UnknownAgent`：只有既无身份又无记录才叫未知 agent。

**公钥每链取一次，不是每行取一次**。一条 N 行的链最多点名几个签名者（每次轮转一个），参考实现（以及本模块的第一版）却按行查库——N 次加锁往返回答 K 个不同的问题，长链上这个开销**数量级地**盖过签名验证本身。`Keyring` 一次 `keys_of(agent)` 载入该 agent 全部历史公钥，链外指纹的回查也记忆化。

## 6. 已知边界（刻意留下，不要当成漏做）

1. **不是防冒充**。`agent_id` 仍是 `chat.send` 上调用方传入的字符串，`router.rs::route` 原样返回，`AgentRunManager::start_run` 只做**存在性**检查。过了连接层认证的调用方仍可以任意已存在 agent 的身份发起 run，账本会**如实记录它收到的身份**。堵它要把声明的 agent 绑到设备授权范围——那是 RPC 授权模型的改动，不是本子系统的。
2. **无 turn context 即不记录**。`approval::audit_identity` 在 turn 外回退字面量 `"main"`——对一行日志是合理默认，对**签名链就是伪造**。所以 `ledger_agent_id` 返回 `None`。（注意：**只有** turn 缺失才返回 `None`；子代理的角色注入是在有 turn 的前提下**替换**归属，不是新增一条无 turn 的路径。）
3. **`revoke` 不是执行闸**。本子系统不拦任何执行。所以被撤销的 agent 若仍在动作，记录**照记**（用其已 retire 的钥签，`AgentKeystore::signing_identity`），而不是拒签。理由：拒签不会阻止行为，只会消灭证据；而"这个 agent 在被撤销 40 分钟后还在动作"恰恰是问责账本最该能证明的事。`revoke` 的真实语义是：标记该身份、retire 其密钥、拒绝 `keygen` 重新启用（要回来必须显式 `rotate`），并在其链上留下一条**由被撤销的那把钥自己签的** `IdentityRevoked`。
4. **无 owner 层**。buzz 的 NIP-OA（owner 签名证明"谁授权了这个 agent"，作者身份永不改写）没有移植：Aleph 没有 owner 密钥概念，凭空造一个是没有消费者的抽象（YAGNI 撤回规则）。父子委派的**事实**已经落在父链上（`ToolCall(target="subagent")`，`detail` 带 `agent_type`），再加一个 `Delegation` 变体是零增量信息。
5. **不做启动时验链**。全量验证要读遍每条链的每一行并逐行验签，那不该挂在启动路径上；而且"daemon 自己写的日志里有一行 warning"本来也不是任何人会据以行动的证据。验证属于被问到的时候——以及，对真正要紧的场景，属于**没写这些记录的那个进程**（`aleph-server identity verify`）。

6. **导出的锚是随文档走的**。`ChainExport.anchor_seq` / `anchor_hash` 由产出文档的那台机器写，所以**尾部截断在离站验证里检不出来**——对手把行删掉、把锚一起改小即可。根指纹钉不住这个（截断后的链仍开在同一把钥下）。唯一的解是**钉链头**：把上一份导出的 `last_seq`/`last_hash` 记在别处，下一份必须是它的延长。这不是遗漏，是"自包含文档 + 敌方产出"这一组合的固有上限，写在这里以免它被当成 `--pin` 已经解决的事。
7. **不做增量/分段导出**。导出恒为整链：前缀与那条 `IdentityCreated` 正是"这条链从哪开始、开在哪把钥下"的依据，从中段起的片段两样都证明不了（并且会直接踩 `PrefixMissing`）。

**已解决（勿再按旧结论行事）**：
- ~~子代理的工具调用记在父 agent 名下~~ → 已修（§4 的身份注入）。团队成员从来不受影响：成员 run 自己拥有一个 turn（`SessionKey::task(agent_id, "team", …)`）。
- ~~`lost()` 只在写入进程内可见~~ → 已落库（`agent_ledger_health`），离线验证器与重启后的 daemon 都读得到。
- ~~删掉 `agent_identities` 一行即可让整条链退出验证~~ → 已修（第三轮：`verify_all` 枚举并集 + `IdentityMissing`）。
- ~~删身份行后 agent 再动作一次即静默换钥续链、验链干净~~ → 已修（第三轮：`UndeclaredSigner`）。
- ~~两条无审批通道的 fail-closed 拒绝零记录~~ → 已修（第三轮：`record_gate_refusal`）。
- ~~"公钥可导出、导出的链可被不信任本机的人验证"三处有声称、零实现~~ → 已修（第三轮：`identity/export.rs` + 工具 `export` + `identity verify --input --pin`）。

## 7. 红线合规

- **R10**：`src/harness/` **零改动**（三轮都是）。账本挂在 `tools/scoped/`，身份注入挂在 `agents/allowlist_tool_service.rs`；harness 只经 `Arc<dyn ToolService>` 多态调用，从不点名任何一个。棘轮以 `budget.rs::CEILING` 为准，本轮不动它。
- **R3 / P6**：**零新依赖**（三轮都是）。`ed25519-dalek` / `sha2` / `hex` / `zeroize` 早已是直接依赖，Ed25519 原语早已存在且此前**零生产消费者**；导出格式用的 `serde` / `serde_json` 同理。
- **R8**：第三轮只给 `agent_identity` **加了一个 action**（`export`），没有新工具 —— 所以下面那 6 个注册点一个都不用动。这是刻意的：新造一个 `agent_export` 工具会同时新增六处登记面和一条 `OPERATOR_TOOLS` 条目，换来的信息量为零。
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

`export` **写文件、不内联返回**：链是无界的，而这份文档的用途是交给别人，不是给模型读——内联等于把上下文窗口花在这段对话里没人会看的字节上。落点是 `<data_dir>/exports/`，文件名派生自 agent id，**不接受调用方给的路径**（不新增任何文件系统触达面，也没有穿越可写错）。

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
aleph-server identity verify --input chain.json --pin <第一次拿到的根指纹>
```

不带 `--pin` 也能跑，但输出会**每次都说出来**它只证明了内部自洽。

## 9. 熵减

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
