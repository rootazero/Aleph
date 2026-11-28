# Linux Shell stdin/EOF Implementation Plan / Linux 通用 stdin/EOF 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for the serial implementation. Use a fresh independent reviewer for the final implementation review. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal / 目标:** Allow an authorized, attended Agent to write bounded UTF-8 and send EOF to the same supervised `bash` job, on a verified Linux configuration only. / 让有人值守、权限可复验的 Agent 在通过验证的 Linux 上向同一 `bash` 作业写入有限 UTF-8 文本并发送 EOF。

**Architecture / 架构:** Prove eligibility before adding schema or a pipe sender. Reuse scoped Tool dispatch, exec approval, workspace admission, and the existing Shell lifecycle; the Linux driver owns the pipe and confinement. / 先验证准入，再加公开 schema 和写入端。复用 scoped dispatch、exec approval、workspace admission 和现有生命周期，Linux driver 保有 pipe 和隔离资源。

**Tech Stack / 技术栈:** Rust / Tokio, Linux Bubblewrap and cgroup v2, existing Aleph Capability and signed approval ledger. / Rust / Tokio、Linux Bubblewrap 和 cgroup v2、现有 Capability 与签名审批台账。

**Spec / 设计:** [Approved design / 已批准设计](../specs/2026-11-26-shell-capability-stdin-design.md), commit `b071ac62f`; product baseline / 产品基线 `854505aa6`.

## Global Constraints / 全局约束

- Write only in branch `shell-capability-stdin`; no main, push, new dependency, host permission change, or replacement executor. / 仅在隔离分支写入；不改 main、不推送、不加依赖、不改宿主权限、不另造执行器。
- One existing `bash` Capability. `keep_stdin_open: true` requires `background: true` or `yield_ms`; refuse conflict with internally prepared startup stdin. `write_stdin` takes 1–4096 UTF-8 bytes verbatim; `close_stdin` takes no input. / 一个已有 Capability；显式可续作业且不得叠用启动 stdin；写入按 UTF-8 **字节**计、不补换行，EOF 不收 payload。
- No downgrade: Linux only, read-only persistent workspace, bounded private scratch, no external network or elevation, enforced process/CPU/memory limits, and proven whole-group cleanup. Keep every applicable rule as defined today, including exact text semantics. No general stdin on an unproved profile. / Linux、持久工作区只读、私有有界临时空间、无外网和提权，实效资源限制与整组回收；不改变现有规则的语义，不在未证明的配置开放。
- Preserve noninteractive behavior and budgets. No raw input in durable events, restored messages, traces, receipts, journals, or approval records; no automatic retry. / 保留非交互路径和时间预算。原始输入不进持久或广播记录、回执、journal、审批，也不自动重试。

## Review Focus / 审阅重点

| Case / 情形 | Required result / 必须结果 | Gate / 门禁 |
| --- | --- | --- |
| New or custom text block / 新增或自定义文本硬规则 | Cannot inherit eligibility from a previous ruleset; no spawn / 不继承旧准入，未 spawn | T1, T2 |
| Approval returns after revoke, stop, or exit / 撤权、停止或退出后才收到同意 | Pipe receives zero bytes; another grant or stale job cannot be used / 写入零字节，不能换用旧作业或其他授权 | T3, T5 |
| A sink stops reading / 接收端不读 | Bounded backpressure, no stalled output collection or stop, no automatic retry / 背压有界，不阻塞排空和停止，不自动重试 | T2, T5 |
| Invalid or rejected write reaches a projection / 无效或拒绝请求进入投影 | No raw input in durable events, replay or trace; IDs still pair / 原始文本不落盘或重放，保留调用配对 | T4, T5 |
| Direct child exits before a descendant / 直接子进程先于后代退出 | No terminal cleanup claim until the group is verified empty / 整组确认清空前不报告已收尾 | T2, T5 |

---

### T1. Prove an eligible policy and route / 先证明至少一个可能的策略组合和入口

**Inspect / 核对:** `src/sandbox/factory.rs`, `src/sandbox/command_policy/{mod,rules}.rs`, `src/sandbox/security_kernel_hook.rs`; `src/tools/scoped/{mod,dispatch}.rs`, the authority that updates the selected route's grants, and `src/gateway/handlers/config.rs`; `src/sandbox/{cgroup_v2.rs,platforms/linux/bwrap.rs}`. **Deliverable / 交付:** `docs/superpowers/runs/2026-11-26-shell-capability-stdin-verification.md`, eligibility table and observed evidence, not a public interface. / 策略及证据表，不开放接口。

- [ ] Enumerate every effective hard rule for one existing policy configuration from both mounted hooks (including the always-installed floor). For each, record rule identity, obligation, enforcement source and a benign falsifier. If any applicable text rule cannot be preserved with an arbitrary interpreter, record `ineligible`; do not enact a policy exception or substitute a weaker predicate. / 逐项核对两个 hook 的有效规则及始终启用的底线；不能保留的文本约束直接拒绝，不改规则、不伪造等价。
- [ ] Trace one authenticated attended route to the canonical `bash` lease and the actual mutation authority for its principal/grant, policy and workspace. Identify the same serialization boundary used by the writer and source of revocation. A resolved lease, task-local role or a second snapshot read is not sufficient; if no shared boundary can be wired without a new authority or widening scope, record a blocker. / 找到实际撤权者以及权限更新与效果共用的判定边界；只有 lease、task-local 或快照重读不算。
- [ ] If the policy and route remain eligible, probe a disposable unprivileged Linux scope. Record whether the daemon can create its own per-job cgroup, use all required controllers, attach a benign child before execution, verify limits and observe empty-group cleanup. Do not rewrite the current session/daemon cgroup or require sudo. / 前两项仍成立才探测独立非特权 Linux scope，记录控制器、有效限制、执行前归组、清空证据，不改常驻服务或当前会话。
- [ ] **Go/no-go:** record an actual eligible route and scope, or terminate implementation here, commit only the evidence, and leave the stdin/EOF request and final implementation review explicitly incomplete. No preparatory product code on a no-go. / 要么提供可验证的正例，要么只提交阻断证据；无正例不写预备产品代码、不把需求或终审标完成。

### T2. Verify Linux isolation and a private writer / 验证 Linux 隔离与私有写入端

**Only after T1 passes / T1 通过才开始。 Files / 文件:** `src/sandbox/{cgroup_v2,command,shell_lifecycle}.rs`, `src/sandbox/platforms/{common,linux/bwrap}.rs`, `src/sandbox/workspace/mod.rs`, and existing Linux init code as needed. **Interfaces / 契约:** extend the existing controlled launch with an internal closed-by-default vs. keep-open mode; `ShellLifecycle::apply_input(&self, input: ShellInput<'_>) -> Result<ShellInputOutcome, ShellInputError>` is a single nonblocking operation, called only inside T3's authority boundary. `ShellInput::{Write(&str), Close}` carries data only in memory; outcomes distinguish written byte count, closed/already closed, and errors including not ready, not writable, and no delivery proof. / 沿用 controlled launch 做内部分型和一次非阻塞投递；不引入队列或对外入口。

- [ ] Add native regressions `interactive_profile_rejects_before_spawn_without_confinement`, `interactive_scope_reaps_after_direct_child_exit`, and `interactive_full_pipe_does_not_block_stop` against actual job effects. Expect no start on unmet prerequisites, verified group empty before settlement on exit, and output/drain plus stop progressing under backpressure. / 先红：检查未启动、整组收尾、pipe 满时停止仍生效。
- [ ] Reuse existing bwrap/Landlock/seccomp and per-job scope; require measured isolation rather than soft cgroup fallback. Arm readiness only after confinement is confirmed; sandbox driver owns pipe and group through completion. A nonblocking bounded write requires the native pipe's atomic capacity; if it cannot prove delivery, return an error and never retry after a possible effect. / 保留已有隔离，实测受控后才 ready；内核未证明容量或投递时不假装成功，可能产生效果时不重试。
- [ ] Run focused Linux cgroup, common runner and workspace tests, then exercise the real delegated fixture for natural exit, explicit stop/timeout and shutdown. Rerun ordinary shell behavior. Commit verified native ownership only with its consumer tests. / 重跑 Linux、共用 runner 与 workspace，并在原生委派环境验收退出、停止、超时、停机；复测非交互，无正例不提交空实现。

### T3. Prove revocation and one-time approval at the effect / 验证撤权与一次性审批落到实际效果

**Files / 文件:** selected production authority and `src/tools/scoped/dispatch.rs`, `src/capability/` lease validation as consumed today, `src/sandbox/exec_approval/{action,gate}.rs`, `src/builtin_tools/process_registry.rs`, `src/sandbox/shell_lifecycle.rs`. **Interfaces / 契约:** the existing dispatch must supply an unforgeable live grant for the canonical Shell request; it binds `SessionId`, principal and grant source, canonical workspace, effective policy, Capability lease and live control. Final operation returns T2's `ShellInputOutcome` without awaiting under a grant or process lock. / scoped gate 提供可复验绑定；最终效果使用 T2 的类型，持锁期间不 await。

- [ ] Add `revocation_after_approval_writes_zero_bytes`, `stale_process_after_approval_writes_zero_bytes`, `standing_grant_cannot_approve_input`, and `unattended_and_raw_dispatch_do_not_request_input_approval`. Assert against a real pipe observer, including before and after a queued decision; not just return labels. / 在实测 pipe 断言零字节，不只测响应字段。
- [ ] Bind the pending action once in memory, offer only `AllowOnce` / `Deny`, and show a redacted description through the existing Gate. Require `ApprovalOutcome::Approved` exactly (not `is_approved()`) from this invocation's live human decision, not a cached standing grant. Existing decision clamping may narrow an explicitly clicked wider approval; it must never create or consult a standing grant on this path. After readiness/approval, recheck current authority and the same live control at its synchronization boundary; serialize only the final nonblocking effect against stop/EOF. A repeated close must authenticate before reporting already closed, without a duplicate approval or effect. / 只接受一次同意，复验实际权威与控制身份；首次关管不可逆，已授权的重复关管无新效果或审批。
- [ ] Run targeted scoped dispatch, approval and registry tests with a denial, a successful write and recorded signed verdicts. Verify the effect did not reach a mock or stale authority on the successful mounted route. / 回归 scoped、审批、registry，正反例涵盖真实接线与台账。

### T4. Close projection paths, then wire public `bash` arguments / 先封投影，再接公开参数

**Files / 文件:** `src/session/projection.rs`, `src/harness/agent/{think,act,prompt}.rs` (reuse existing projection ownership), `src/builtin_tools/{bash_exec,code_exec,process_registry}.rs`, and the existing catalog declaration. **Interfaces / 契约:** `BashExecArgs::{keep_stdin_open: bool, input: Option<String>}` with default `false` and absent input, plus `process_action` values `write_stdin`, `close_stdin`. Route only to T3's established authority and T2's lifecycle; preserve the old `bash` descriptor/key. / 默认不开放；只给已有入口新增参数和动作，复用前置门禁。

- [ ] Add `shell_input_is_redacted_before_assistant_and_effective_call_persist` using a synthetic canary. Check assistant raw blocks, effective-input events (`src/harness/agent/act.rs`), durable/recovery replay, live observable trace, returns and approval records for accepted, denied and invalid calls; preserve the original tool-use/result ID. / 用合成标记验证所有投影不含原文且 ID 仍配对。
- [ ] Redact before each first durable or broadcast projection through existing owners, without altering the request-local payload; never log it on validation failure. Add action validation tests: byte lengths 0, 1, 4096, 4097 (including multibyte); EOF carrying input and initial-stdin conflicts fail; absent flag preserves prior semantics. / 测量真正字节边界，拒绝 EOF 附数据和启动 stdin 冲突；旧参数语义不变。
- [ ] Wire parse → controlled launch, ownership → T3, and write/close outcome projection; ensure no bypass from legacy entrypoints and no retries. Run Bash, Tool catalog, scoped service, session projection and harness replay tests. Commit T3/T4 only together with passing integration of authority, projection and dispatch. / 绑定现有链路，旧入口不能绕过，权限、脱敏与接口一起经验证后提交。

### T5. Verify production wiring and obtain independent review / 真机验收并取得独立终审

**Files / 文件:** `docs/reference/{FEATURE_LOCATOR,SANDBOX,PROCESS_MANAGEMENT,TOOL_SYSTEM}.md`, the T1 evidence report and isolated QA fixtures. / 同步当前文档并补齐验收证据。

- [ ] Through the actual Gateway → scoped dispatch → signed approval → Workspace → Linux driver, use a deterministic benign model fixture to feed multiple writes, verify the exact bytes including no inserted newline, close stdin twice as the authorized caller, wait for child exit, continue the existing cursor, and confirm descendant cleanup. Negative cases require no spawn or no bytes to the receiving pipe, not merely an error result. / 真实链路核对字节、EOF、游标和子孙退出；反例以效果为准。
- [ ] Recheck ordinary `bash` and T1–T4 tests, run `just check-exec-wiring`, `just check-executor-wiring`, `just check-tool-wiring`, `just check-seam-wiring`, `just check-gateway-wiring`, `just check-doc-wiring`, and compare any failure to baseline `854505aa6`. Run `git diff --check`, record exact test counts and skipped platforms. / 重跑相关门禁、逐项对照基线，不把基线失败说成通过。
- [ ] On committed candidate, ask a fresh independent Sol/Kimi reviewer to inspect committed diff, reconciliation of policy/authority, native artifacts and durable projections. Fix and re-verify material findings; request confirmation on the final reviewed revision. If the independent host fails at launch, report it and do not substitute self-review. / 对最终提交发起独立实现审阅；修复、回归、复验，宿主故障不冒充终审。
- [ ] Commit bilingual reference/evidence and report the final commit, verified scope, unresolved items and untouched main; no merge or push. / 提交双语文档和证据，报告提交、验证边界、未完成项；不合并、不推送。
