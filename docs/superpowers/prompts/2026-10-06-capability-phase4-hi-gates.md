# Phase 4 Gate H（ACP 入站最小 server）/ I（Effect claim 安全恢复）— 续作

工作目录：`/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`
Worktree 基线：`947e1895130bfc84abca7f1dd7733b07e30519fb`（capability-phase4-follow-up 分支，未合入 main）。
main HEAD：`7f4bdfb8bade0861f92478d103822e8866b3a72d`（**历史观测值，非永恒权威**，本期不 merge，禁 main 写入）。

## 1. 标题与使命

本期承接 Phase 4，Gate H = **ACP 入站最小 server**（实时投影仅是其一组成，非全部），Gate I = Effect claim 安全恢复（I0 / I1 / I2 三层），不回到 Phase 5/6 整体整合，H/I 独立成门、互相不绑。

本 prompt 的启动范围是**只读分析与方案提议**，不预支设计、书面 spec、plan 或代码实施的批准。已有 ZahirFacade 是 library host，尚未接入生产 runtime。

工作流（强顺序）：architectural intent → 2–3 方案 trade-off → 用户对 design 书面批准 → spec 书面批准 → plan 书面批准 + 执行方法 → code。H 先推荐推进，I 仅 audit / design 并行，可 no code / no SafeReplay。

已落但仍 library only：`src/capability/zahir_facade.rs:164 impl Zahir`、`:45 new`、`:230 subscribe`；ToolBackendAdapter；BackendLease descriptor + owner_generation；ResolveError 闭集；OwnershipTree per-binding generation + atomic register_if_absent（保留 claims / incumbent / public-register 替换）；schema SHA 唯一到 descriptor。

**前置门（Gate G 未完部分）**：runtime / 持续订阅是 H 的前置，属 Gate G 未完成内容。当前 worktree **尚无 runtime 挂载**，故必须先 **H-pre 独立批准**（approved minislice），再进 H；不要因果倒置成「runtime 满足时先 pre」。不声称 Gate H 范围已被批准。

## 2. Baseline verify（执行前必跑）

- `ls /Volumes/TBU4/Workspace/Aleph` 与本 worktree 路径分清；main 权威在 Aleph，本期基点在 follow-up worktree。**从含 follow-up 的新 worktree 确认起点，禁 main 写入**。
- `git status --short`：`docs/superpowers/prompts/2026-10-06-capability-phase4-hi-gates.md` 是 untracked 的本期 prompt（**不假设其未来位置 / 状态永不改变**）；旧 `phase4-follow-up-gates.md` prompt 与 untracked plan 保留，不 `git add -f`。
- `git log --oneline -5` 与 `git reflog` 验证 HEAD 与 worktree ancestry；如有未审变化以 `git diff HEAD` 实际为准，禁自动 merge / cherry-pick 到 main，新 feature worktree 由用户确认起点。
- **VCS 检查纪律**：`git diff` 只能覆盖 tracked 差异，**看不到 untracked 内容**；不得宣称「git diff check 能查 untracked」。校验 = tracked diff + 实际 read。
- **引用清单（全文读，按现存状态）**：执行 worktree 的 `AGENTS.md` / `CLAUDE.md`；root `Everything-externally-composable-is-a-Capability.md`；`docs/reference/FEATURE_LOCATOR.md` §3.5d + 验证附录；以及已存在的 `docs/superpowers/prompts/2026-10-06-capability-phase4-follow-up-gates.md`、`docs/superpowers/specs/2026-10-06-capability-phase4-follow-up-gates-design.md`、`docs/superpowers/plans/2026-10-06-capability-phase4-follow-up-gates.md`。**AGENTS 不是总路线图引用权威**，事实一律以现存文件为准，不猜。
- 命中 read：SUBSYSTEM_ROUTING、REDLINES、HARNESS_PHILOSOPHY、TOOL_SYSTEM、GATEWAY / SECURITY / session 按触及按需。
- 旧 prompt / spec / plan 与代码矛盾，**记录并升级审批，不私自取舍**。状态：spec 已 tracked、plan untracked；ignored ledger 是单独的 `.superpowers/sdd/2026-10-06-capability-phase4-follow-up-gates/progress.md`，**不要把 spec / plan 说成 ignored ledger**。

## 3. Pi 真实对照（gap 表，6 行；Pi 历史观测 HEAD `428a12bc7`，root `/Volumes/TBU4/Github/pi`）

| # | pattern | 真实 path（已审） | Aleph 现状 / 证据 | 映射 / 注意点 |
|---|---|---|---|---|
| 1 | durable `CommittedStateSource` snapshot{cursor,value} | `packages/durable/src/session/observation.ts:24,37,48` | 是**内存 cursor，非跨崩溃 durable observer 事实**；模式仅参考 | 不直接搬 universal scheduler 进 harness |
| 2 | `CommittedWatch` + bounded buffer（`MAX_PENDING_WATCH_FRAMES=100`） + cancellation | `packages/durable/src/session/observation.ts:155,204,221,224,241,276` | **100 bound 仅约束 pending，不是 all-queues bounded / 无 quiescence proof** | closed ≠ quiescence 未证明 |
| 3 | 进程内单 client 序列化（`runForClient`） | `packages/server/src/session-router.ts:146` | **session-router 是 per-client 非 per-session** | client-lock 不替代 session gate |
| 4 | crash 后 key 去重 | `packages/durable/test/harness-tasks-recovery.test.ts:47-87,111-146` | service.calls=2 / applied=1 是**外部 mock idempotency，非 framework exactly-once** | typedTaskState + checkpoint 仅参考 |
| 5 | 自有 server 协议与会话路由 | `packages/server/` + `packages/protocol/`（本次所审子系统） | Pi 的 Chord / CBOR 服务不是 ACP server | 借鉴生命周期模式；ACP 标准、版本、wire conformance 必须独立验证 |
| 6 | policy memo / fence / retry budget 的能力对齐 | 本次未完整审计这些链路 | **证据不足，需复核，不能判定缺失** | 不把通用 memo 当 approval 授权，不预设 Pi 或 Aleph 全仓缺少相关能力 |

**口径纪律**：所审范围有限——未读完 pi agent / coding-agent 全部 + Aleph providers；不得断言 Pi 内部「无 X」，也不得作全 pi / Aleph 否定 claim。ACP 不复制。

## 4. GateH-pre（runtime minislice，独立批准）

保留 CapabilityChangeStream 的纯值状态机职责。它当前没有实现 `Stream<Item>`；registry broadcast 容量为 256，且不持久化。持续订阅句柄需要单独设计，不能假设已有。

**当前接口边界**：`ZahirFacade::subscribe` 是**同步返回 cursor + changes 值**；**没有 snapshot payload / receiver / live handle**；caller 传入的 cursor 被忽略；既有测试**未证明 returned snapshot 的送达**。若新增持续订阅句柄，应与纯值状态机分离，并通过独立设计审批。

H-pre 仅做：

- **真实 runtime** 持 **同一个 ToolHandlerRegistry + `Arc<OwnershipTree>`**，由**真实生产 caller projection** 消费；**不以 GlobalBus 通知或 test consumer 冒充 production delivery**。tests 验证另行处理，不混入 H-pre 定义。
- 初始 snapshot + cursor：消费者 attach 后可见历史 snapshot；post-subscribe changes 走 registry broadcast。
- 三个 domain 计数不混：registry cursor / owner generation / session seq，各为独立源。
- owner revoke / bump 对真实 consumer 可见；缺局部 ownership notification 则提交独立 design 批准，不污染 harness。

H-pre 独立批准通过后回 H；H-pre 不达 = 不进 H。

## 5. GateH（ACP inbound 最小 server）

- 协议 / 版本 / transport（stdio vs 其余）先选；本机 acpx 若存在适配验证（不是 conformance 替代）；Pi ACP 对比不替 Aleph ACP conformance。
- **入口先普查** `bin/ cli/ gateway/ tests/` inbound 入口（不能以 no-TCP 证明无 ACP server；stdio local 是双向 / outbound AcpAdapter 已是 client）。
- `ExistingSession / Run` mutation path **复用**，approval 权限沿原线；request route 快照（不重发 / 不漏发）；session 并行 admission / cancel / drain / quiescence，**不**用 client-lock 替代 session gate。
- `invoke` 仍走 canonical dispatch，**不另设 handler registry**。
- **wire 契约**：wire 共享当前所选 ACP 标准 types；**不要求把 descriptor / generation / cursor / session seq 全部塞进 ACP wire**。内部 binding 与 cursor 域跟 wire projection 区分，防 cross-crate mismatch；字段扩展须 approved 且 standard compatible。
- **接口 pure IO**：避免 ACP handler 直接做业务 / `emit_batch`（即入站 run 实现）。明确链路：**入站 command → 已有 Session / Run 入口 → Core 业务 → durable event → outgoing projection**。
- 两 truth：capability live projection 源 registry/ownership；durable session 观察源 `SessionEventStore::load_events_range` / `emit_batch`。registry 通知 ≠ session event committed；startup continuous cap resync ≠ durable replay；session 断线 restore 按 committed seq；退役不可热接新 incarnation 误用旧 cursor。

## 6. GateI（三层独立门禁）

- **I0 — audit / design 路径审批**：仅设计，不动代码；audit 现有真实 path，列出 producer 接入点、VerifyOnly 边界、cache / 持久化边界。
- **I1 — Gate I-pre（补 Gate F 的真实 producer 前置）**：不是直接完成 Gate I。单一路径，真实 producer 接驳；VerifyOnly 按**现存判据**（不虚构 `VerifyOnly::effective_input` 字段）；claim budget + owner fence 与 durable transaction **同一边界**。若接口无法满足，**停止该 slice** 并说明最小契约设计——不造 backend、不 fake 同事务。durable call input 边界 = post guardrail / cache 实际 dispatch；`Prepared / Claimed / Invoking` 状态**不机械改 Unknown**；effect certainty 未知时 VerifyOnly / refuse，**不可自动 replay**。
- **I2 — SafeReplay**：**单独 spec + plan 批准**（在 I1 真实 durable fact 证明后），不许与 I1 同 commit 推进，**不得自动推进**。

Crash matrix（必跑 fault injection，不靠纸面）：before intent / after prepared / claimed / invoking / after external before local terminal / after terminal / window budget。

- 合法 reducer 状态：`Prepared / Claimed / Invoking`；禁止机械改 Unknown。
- 外部不可验证时保留 `Unknown`（effect certainty ≠ state），state vs effect evidence 分开。
- single-use permit + fence 只是 local，**不证 external exactly-once**。
- provider idempotency key：真实 semantics（retention / timeout / same-key query proof）；不支持时只能 at-least-once / unknown，**不**用日志 duplicate suppression 当承诺。
- Policy memo fingerprint：字段沿 **existing approved contract**；新字段须 design 审批；memo ≠ approval 授权。
- 推理 retry 策略不进 harness（R7/R9/R10，harness 薄）。

## 7. 非目标

- 其它 kind backends、universal scheduler、main merge、whole harness 改造。
- 全量无关 baseline 修复（clippy 历史项不混入新 gate commit）。
- 未同步审批的 H 与 I 一个巨 commit。
- Lease dispatch `check → lookup → invoke` TOCTOU 不得靠 validate 先调用表面绿；如必要需 spec 解释线性化。

## 8. 工程双线

- 源实现 + 验证 doc 互不踩文件 / 全局 cargo。**并行资源纪律：全球 cargo 至多 1（跨所有 worktree）**。
- **内存门槛公式**（不能把 grep 输出直接当 GiB）：`sum_pages × page_size / (1024^3)`。
  - macOS：`sysctl -n hw.pagesize` 取实际页面大小，×（free + inactive + speculative）页数算出 GiB，≥ 4 GiB。
  - Linux：`MemAvailable`（KiB）/ 1024^2 ≥ 4；Windows：`FreePhysicalMemory`（KiB）换算 ≥ 4。
  - 低于门槛每轮重测 await，**不** `rm -rf target` 假腾。
- Main agent 编排 implementation / 大量读取 / testing agent。子代理模型池（显式，勿写不存在的 gpt5.6）：`deepseek/deepseek-v4-pro`、`minimax-cn/MiniMax-M3`、`zetaapi/gpt-5.6-luna`。分工：架构 / plan / 跨 crate 合约 → deepseek/deepseek-v4-pro；机械 codemod / 批量 / test-gen → MiniMax-M3 或 zetaapi/gpt-5.6-luna；前端 / 视觉 / 文档 → MiniMax-M3；最终 review + 集成 → deepseek/deepseek-v4-pro；分类 / 路由 / 简单判 → Jev。先建任务，再按子代理 owns 精确 paths；**合理 budget，不硬性 8–15 turns limit**；成功 resume 需 diff 实证（靠 git / log / 验证恢复，不是 turn 数）。
- 2–3 任务集中 check：RED + GREEN 不省；scoped review per task + final whole-branch 一波 fix + scoped review 残留 blocked（不 docs waive）。

## 9. Effect 到达验收（prodcaller / consumer 实测）

- 真实 registry changes projection：buffer 溢出后 consumer 收到 **replacement snapshot + cursor**（不是另 describe 计数）。**actual buffer 容量是生产策略，可设计；不硬写 all-watch-256**。旧 registry 256 可溢出 → 用 256+ tests 示例证明 replacement snapshot 送达。
- **验收 scope 不写模糊「全三维」**：明确 kind / namespace / visibility 共享 predicate + owner invalidated。
- cursor / coldstart vs durable seq renew incarnation；**stale cursor 来自不同 incarnation 不得当 durable 恢复**。
- attach / close / cancel isolated + quiescence：**cancel 发出 ≠ 静止，需 real completion drain proof**。
- 入站 wire → real run → approval / memo / event → egress。
- 多 client 同 session 竞争；断线 events holes / retired。
- I1 crash matrix：fault injection + reopen + producer 写真实 event；**actual event write 与 effect observed 分开记录**；external mock calls 与 effects 分开；terminal 禁重复消费；unknown fail-closed；**negative control 要实测，不只断言**。
- Barriers no sleep proof / mutation negative 敏感性；不为跨 crate 测试造新 crate（若有现成测试足够则复用）。

## 10. Validation（fresh commands）

```
cargo check --package alephcore --all-targets
cargo test  --package alephcore --lib capability:: --no-fail-fast
cargo test  --package alephcore --lib --no-fail-fast
cargo clippy --package alephcore --all-targets -- -D warnings
```

- **定向测试**：按 SUBSYSTEM_ROUTING 触及 ACP / session / gateway 的定向测试 + 真实 wire QA。
- **最终**：`cargo check --all-targets` + `cargo test --lib`（全）+ `cargo clippy -- -D warnings` 以真实 exit code 为准。

历史基线（仅参考，不作 ≥ 阈值；**基线与 followup 分开记，不合成一个 hash**）：

- followup worktree：`112cap pass / check pass / lib 20718 pass 23 fail 20 ignored`。
- base（`7f4bdfb8…`）lib：`20691 pass`；另列 23 fail / 20 ignored 同 23 names 真实复现。
- clippy 6 baseline（绝对 repo 相对路径 + 行列）：`src/gateway/mcp_face/http.rs:133:6,189:70,208:6`（result_large_err）、`src/gateway/mcp_face/protocol.rs:59:1`（large_enum_variant）、`src/session/reduction.rs:1171:17,1191:17`（manual_contains）。

**同 names 不证所有语义安全**；新 gate 以同 config + 同样本比对 failure identities + messages + new failure 差集，不以 passed 数 ≥ 历史作 regression 判据；warning ≠ pass；红 ≠ ready to merge / menu / merge / push。scoped accepted ≠ global green。

日志：exit 真实、no pipeline swallow；format 变化与 hunks 基线差异分开记；不全量 `cargo fmt` 掩盖改动。

## 11. Deliverables

- Gap 表（§3）+ scoped spec + scoped plan + 分 Gate task 文件 + deps。**commit 策略：小任务独立 commit；一个 Gate 可多个 commit（staged-tree check + commit 后 `git status` 干净），不硬性每 Gate 一 commit；不混 H/I 巨 commit**。
- proof tests actual logs（不是描述性叙事）。
- Review + rulings + reason + cost + negative（不豁免 docs blocked 项）。
- AGENTS / CLAUDE sync；FEATURE_LOCATOR reference docs 更新实际消费者 path / status / deferred；禁只标 host completed 无 caller。
- 新会话首轮 read-only 5 actions 后 stop、ask design（具体列表）：
  1. baseline 确认（worktree / git status / log / reflog）；
  2. 全文读引用 docs（AGENTS / CLAUDE / Everything-externally-composable / FEATURE_LOCATOR §3.5d / 旧 prompt / spec / plan）；
  3. Pi + 代码 gap 复核（§3 表 + 真实 path read）；
  4. 产出 2–3 trade-offs；
  5. 等待用户对 design 的**书面批准**再继续。
- **负面报告**：本 prompt 未做任何代码、未 cargo、未 merge / push。

## 红线（recap）

- R1 大脑与四肢分离 / R2 UI 唯一源 / R3 核心轻量 / R4 Interface 只 I/O / R5 AI 主动到达 / R6 一核多端 / R7 LLM 主权 / R8 工具即一切 / R9 智慧在 Prompt / R10 薄 Harness 笨循环（12 文件 / 5 不 / 3 问 / 棘轮）。
- 所有“已有 / 缺失 / 已完成”结论都应指向真实源码与效果证据；未审范围、失败验证和 deferred 内容必须明确列出。
- 不用未审 API 名当已存在；命名仅在 spec approval 时确定。

— end —
