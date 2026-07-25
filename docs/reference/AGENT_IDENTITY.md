# AGENT_IDENTITY.md — 每个 Agent 的独立密钥与签名操作账本

> 「带签名日志解决了'发生过什么'，身份权限解决了'谁能做什么'。」
> 本文覆盖前半句的**实现**、后半句的**接线点**，以及两句**都没有覆盖**的边界。
> 代码锚点 `src/identity/`；对照 FEATURE_LOCATOR §5.17。

---

## 1. 一句话

每个 agent 持有自己的 Ed25519 密钥对；它做的每一次**变更类工具调用**、每一次**被拒**、每一次**审批裁决**，都追加进**它自己的**哈希链并由**它自己的密钥签名**。`agent_identity` 工具与 `aleph-server identity` CLI 负责读与验。

## 2. 为什么是这个形状（Gap Analysis vs buzz）

参考项目 **buzz**（`T:/Github/buzz`，Nostr relay 工作区，"same audit trail, a different keypair"）。逐维度对照 —— **改这一层前先看这张表，不必重做对比**：

| 维度 | buzz | Aleph（本轮后） | 取舍 |
|---|---|---|---|
| 身份载体 | secp256k1 keypair 即身份，`Keys::generate()` 单点铸造 | Ed25519 keypair，`AgentKeystore::mint` 单点铸造 | **映射**。曲线不同无实质差异；Ed25519 的原语（`gateway/security/crypto.rs`）Aleph 早已有，只是**零生产消费者**——本轮是它的第一个真实调用方 |
| 私钥托管 | OS keyring / `0600` 文件 / env，best-effort scrub（自认 allocator 可能残留） | 既有 `SecretVault`：AES-256-GCM + 每条 HKDF salt + `Zeroizing` + `VaultIo` fcntl 原子写 | **Aleph 更强**。零新基建 |
| 密钥轮转 | **不存在**（撤销＝不再签发 attestation） | `rotate` 保留旧钥（`retired_at`）以便旧记录仍可验，**不重置链锚** | **超越** |
| 记录结构 | `audit_log`，`(community_id, seq)` 哈希链 | `agent_ledger`，`(agent_id, seq)` 哈希链 | 映射。租户位换成 agent 位 |
| preimage 分帧 | presence tag，**无长度前缀** → 相邻变长字段拼接有歧义（两条不同记录可同哈希） | **每个变长字段 u32 长度前缀** | **修掉参考实现的缺陷**（`hash.rs::length_prefixes_make_adjacent_fields_unambiguous` 钉死） |
| 时间戳 | `to_rfc3339()` 进 preimage —— chrono 按值输出 0/3/6/9 位小数，纳秒值与其微秒截断是**不同字符串**，曾导致**每一条**都验不过 | **整数 epoch-ms** 进 preimage | **绕开整类问题**，不是补丁 |
| 签名 | **无**。只哈希，无密钥、无外部锚 | Ed25519 签在链哈希上 | **超越**：持有 DB 写权限不足以整链重算 |
| 尾部截断 | **结构性失明**（截断后的链内部自洽） | 检出（`agent_identities` 锚 + 下一条 seq 取 `max(锚, 末行)+1`，洞永久留痕） | **超越** |
| 前缀删除 | **结构性失明**（首行只自哈希校验，不与前驱或 genesis 对照） | 检出（首行必须 `seq=1` 且 `prev_hash IS NULL`） | **超越** |
| 整链清空 | 返回 `Ok(false)`，与"没什么可验"同值 | `ChainWiped` fault | **超越** |
| 验证消费者 | **零**（`verify_chain` 无生产调用者；buzz-admin 全文无 "audit" 字样） | R8 工具 + 离线 CLI，**与链同批交付** | **超越**（这是 buzz 最大的实操缺口） |
| 记录丢失可见性 | worker 失败只计 metric，不重试；链对"从未写入"的记录无话可说 | `AgentLedger::lost()` 随每次 `list`/`ledger`/`verify` 一起返回；写入端用 `send().await` **背压**而非 `try_send` 丢弃 | **超越** |
| 身份→权限 | `Scope` 16 条枚举，但生产恒发 `all_known()`（形同虚设）；真正差异在 membership / `MemberRole` / NIP-OA | 既有 `tool_permissions` 三级合并（global→agent→channel，`restrictive_min` 只收紧）× exec tier | **Aleph 领先，刻意不移植** buzz 的 scope 层：再加一套并行权限模型违 P2/P6 |
| agent vs human | 只差**配额**不差权限 | 同（exec tier 与 agent 轴正交） | 平手 |
| 委派/归属 | NIP-OA：owner 签名证明"谁授权了这个 agent"，**作者身份不可改写** | 见 §6 已知边界 | **未移植**（需要 owner 密钥层，超出本轮） |
| git 提交签名 | `git-sign-nostr`（O_NOFOLLOW/fstat/mode 检查/Zeroizing 全套） | 无 | **刻意不移植**：独立二进制 + `gpg.x509.program` 钩子，属生态外挂，按 R3 应做 Skill/MCP 而非进 core |
| Nostr wire / relay 联邦 | 核心 | 无 | **刻意不移植**：Aleph 信任边界是网络边界（loopback + device tier），不是公开 relay |

## 3. 威胁模型 —— 它买到什么，买不到什么

**买到：**
- 任何**已存储记录**的改写、重排、中段删除、跨 agent 搬运、前缀删除、尾部截断，都会被 `verify` 检出并定位到 seq。
- 伪造一条记录需要该 agent 的**私钥**——仅有 DB 写权限不够（buzz 的 keyless 链在这一点上完全无防护：任何能写 DB 的人都能把整条链重算得天衣无缝）。
- 公钥指纹可导出、可离站钉住；此后一段导出的链**可以被不信任本机的人验证**。

**买不到：**
- **不防拥有 `~/.aleph` 的对手**。vault、主密钥、数据库在同一块盘上。这是本地优先 daemon 的固有边界，没有 HSM 或远端公证就无法逾越。文档不假装它能。
- **不防进程内冒充**（见 §6）。
- **对"从未写入"的记录无话可说**。链只能证明它包含的东西。所以 `lost()` 计数与 `ok` 判定**并排返回**——干净的 `ok` 绝不可单独解读为"完整"。

## 4. 架构

```
tools/scoped/dispatch.rs::execute_inner        ← 唯一生产者（全库唯一进工具注册表的路径）
        │  ledger_intent(name, &input)          （tools/scoped/ledger.rs）
        │    ├─ 变更类调用完成 → ToolCall(ok|error)
        │    ├─ 策略/钩子拒绝  → ToolDenied
        │    └─ 审批裁决       → ApprovalGranted|ApprovalDenied
        ▼
identity::record_action(NewRecord)             ← 有界 mpsc(1024)，send().await 背压
        ▼
单写者任务 AgentLedger::append                  ← 定位 → 哈希 → 签名 → 插入 → 推进锚（单事务）
        ▼
security.db : agent_keys / agent_identities / agent_ledger   （schema v12）
        ▲
        ├─ agent_identity 工具（R8，operator 门控）
        └─ aleph-server identity（只读，无 runtime 无锁，daemon 停机亦可）
```

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
| `PrefixMissing{first_seq}` | 链不从 1 开始 | ❌ **失明** |
| `GenesisNotNull{seq}` | 首条带前驱链接（被 re-base） | ❌ |
| `SeqGap{expected,found}` | 中段删除 / 截断后又追加留下的永久洞 | 部分（中段删除✅，截断洞❌） |
| `TailTruncated{anchor,last}` | 末尾被砍 | ❌ **失明** |
| `AnchorMismatch{seq}` | 末行在锚的位置上但不是锚的哈希 | ❌ |
| `ChainWiped{anchor}` | 锚记得有链，行全没了 | ❌（返回 `Ok(false)`，与"无可验"同值） |

`verify` **报告全部** fault 而非首个即停：判断发生了什么需要损伤的**形状**，不只是存在性。

## 6. 已知边界（刻意留下，不要当成漏做）

1. **不是防冒充**。`agent_id` 仍是 `chat.send` 上调用方传入的字符串，`router.rs::route` 原样返回，`AgentRunManager::start_run` 只做**存在性**检查。过了连接层认证的调用方仍可以任意已存在 agent 的身份发起 run，账本会**如实记录它收到的身份**。堵它要把声明的 agent 绑到设备授权范围——那是 RPC 授权模型的改动，不是本子系统的。
2. **子代理的工具调用记在父 agent 名下**。子代理经**父**的 `ScopedToolService` 执行，因而在父的 `TURN_CONTEXT` 下——咽喉处**不存在**子角色信号可记。这是当前执行模型的如实读数，不是用猜测的归属糊过去。要修得先给子代理自己的 scoped 服务。
3. **无 turn context 即不记录**。`approval::audit_identity` 在 turn 外回退字面量 `"main"`——对一行日志是合理默认，对**签名链就是伪造**。所以 `ledger_intent` 返回 `None`。
4. **`revoke` 不是执行闸**。本子系统不拦任何执行。所以被撤销的 agent 若仍在动作，记录**照记**（用其已 retire 的钥签，`AgentKeystore::signing_identity`），而不是拒签。理由：拒签不会阻止行为，只会消灭证据；而"这个 agent 在被撤销 40 分钟后还在动作"恰恰是问责账本最该能证明的事。`revoke` 的真实语义是：标记该身份、retire 其密钥、拒绝 `keygen` 重新启用（要回来必须显式 `rotate`）。
5. **无 owner 层**。buzz 的 NIP-OA（owner 签名证明"谁授权了这个 agent"，作者身份永不改写）没有移植：Aleph 没有 owner 密钥概念，凭空造一个是没有消费者的抽象（YAGNI 撤回规则）。

## 7. 红线合规

- **R10**：`src/harness/` **零改动**。账本挂在 `tools/scoped/`，harness 只经 `Arc<dyn ToolService>` 多态调用，从不点名 `scoped`。棘轮实测仍 5082（＝`budget.rs::CEILING`）。
- **R3 / P6**：**零新依赖**。`ed25519-dalek` / `sha2` / `hex` / `zeroize` 早已是直接依赖，Ed25519 原语早已存在且此前**零生产消费者**。
- **R4**：`src/gateway/security/store/identity.rs` 纯 SQL I/O（对齐 `devices.rs`/`tokens.rs`），全部摘要/哈希/签名在 `src/identity/`。
- **R7**：账本**记录**，不**评分**、不分类、不选恢复策略。"是否变更类"读工具**自己声明**的 `is_idempotent` 元数据（与 exec tier 同一个 `tool_facts` 缝），不猜意图、不查名单。
- **R8**：能力经 `agent_identity` 工具对话式可达，`OPERATOR_TOOLS` 门控。**6 个注册点**（改工具时别漏）：`builtin_tools/mod.rs` · `builtin_registry/definitions.rs`（表项 + `create_tool_boxed`）· `builtin_registry/groups.rs`（**唯一有测试强制的一处**）· `builder/core_tools.rs`（元数据/schema）· `registry/tool_registry_impl.rs`（**真正的执行分派臂**）· `method_authz.rs::OPERATOR_TOOLS`。漏掉后两者中任何一个 ＝ 工具被通告给模型却在调用时报错或越权。
- **Spec C**：走 `SecurityStore`（`open_sqlite_safe`：WAL / busy_timeout），不新开数据库文件。

## 8. 使用

对话（模型或操作员）：
```
agent_identity(action="verify")                    # 验全部链
agent_identity(action="ledger", agent="main", limit=50)
agent_identity(action="show", agent="main")        # 身份 + 全部历史密钥 + 近期记录
agent_identity(action="rotate", agent="main")      # 换钥；历史仍可验，链不重置
```

离线（daemon 停机亦可，这正是重点）：
```
aleph-server identity list
aleph-server identity ledger --agent main --limit 40
aleph-server identity verify          # 有 fault 则非零退出
```

## 9. 熵减（本轮删除）

- `memory_audit_log` 表 + 三个索引：有 `actor` 列、**全库零 INSERT**。与 2026-07-14 被删的审批审计库同型（"operator 跑出来永远是 0，比死代码更坏"）。经 `drop_obsolete_tables` 从既存 DB 一并删除。
- `memory::audit` 的 `AuditEntry` / `AuditActor` / `AuditAction` / `AuditDetails` / `ForgettingExplanation`：零构造者。模块改名 `memory::explain`，只留真正被读的 `FactExplanation` / `ExplainedEvent`（`memory_timeline` 工具与 `TimeTraveler::explain_fact` 在用）。
- `ApprovalSource::Autoconfirm`：零生产者，无任何存储行能带它。同批把 `Trusted` **接上**（会话授权短路），`User` 只留给真人当场裁决。
