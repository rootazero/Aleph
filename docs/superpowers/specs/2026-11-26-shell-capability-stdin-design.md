# Linux Shell stdin/EOF 第二阶段设计 / Phase 2 design

基线 / Baseline: `854505aa6`; [第一阶段设计 / Phase 1 design](2026-11-26-shell-capability-rework-design.md) · [验收 / Verification](../runs/2026-11-26-shell-capability-rework-verification.md).

## 目标和取舍 / Intent and approaches

Agent 需要在同一个受控 `bash` 作业中回答提示、驱动解释器或结束输入，不能靠启动一次新命令来替代上下文。普通 `bash` 仍然维持启动时的输入约定。交互版不是人用 PTY、远程控制台、密钥传输通道或第二个执行器。

An agent must be able to answer prompts, drive an interpreter, and end input in the same supervised `bash` job. Ordinary launches keep their current input behavior. This is not a human PTY, a remote console, a channel for secrets, or a second executor.

| 方案 / Approach | 取舍 / Trade-off |
| --- | --- |
| **先做可证明的 Linux 通用交互（用户选定） / Verified Linux general input (selected)** | 接受任意有限 UTF-8，不假定内容只是数据；只在能满足全部适用硬限制的执行配置上开放。首版只读持久工作区，允许有界临时空间；非 Linux 或证据缺失均拒绝。 / Accept bounded UTF-8 regardless of interpretation, but only in configurations that satisfy every applicable hard restriction. The first profile has a read-only persistent workspace and bounded scratch. Other platforms and unproven configurations reject it. |
| 固定数据消费者 / Fixed data-only consumers | 可保留启动内容策略并缩小影响面，但不能驱动任意解释器或未知提示；不符合本次范围。 / Smaller surface and compatible with launch-text policy, but cannot drive an arbitrary interpreter or unknown prompt. |
| 逐块筛查后写普通 pipe / Filter each chunk and write a normal pipe | 不采用：接收程序决定何时、如何解释输入。逐块检查、拼接历史或询问用户都不能替代硬阻断。 / Rejected: the receiver controls interpretation. Chunk filtering, buffering the history, and human approval are not substitutes for a hard block. |

## Linux 启用条件 / Linux eligibility

- **既有启动链路不变。** `bash` → `CodeExecTool` → `WorkspaceSandbox` → Bubblewrap；共享命令校验、审批、工作区绑定和子进程收尾。原有硬规则继续审查启动内容，交互开关不移除、不降级任何规则。提前运行校验、缺少有效权限或隔离证据时，拒绝交互启动；不得偷偷变成普通 `bash` 或 host spawn。
- **One launch pipeline.** Preserve command validation, approvals, workspace binding and teardown. Existing hard rules still check launch content. An interactive request cannot silently spawn with ordinary stdin or on the host when authorization or isolation is missing.
- **按规则语义判定覆盖。** 在当前 `CommandPolicyHook` 及 `SecurityKernelHook` 计算出的有效策略上核对启用条件。只有策略契约本来保护的是某一危险效果，且逐项证明该效果在该 profile 下不可达，才可用原生约束覆盖；不能把现有文本阻断默默改成效果阻断。任何仍要求阻断特定文本、且无法保留该约束的有效规则（含用户自定义）均使该配置不合格。规则增补或策略切换不得默认继承已有结论。逐次审批不是规则豁免。
- **Prove compliance with each rule's semantics.** Check the effective policies used by both existing hooks. Native enforcement is adequate only for rules whose contract already protects against a specified effect and for which the profile demonstrably prevents that effect. Do not silently reinterpret a text prohibition as an effect restriction. An applicable text block that cannot be preserved, including a custom block, makes the configuration ineligible. A changed ruleset does not inherit the old verdict. Approval never overrides a hard block.
- **首版最小 profile。** 无额外能力提升、外部网络、持久写入或可写宿主设备；持久工作区只读。临时可写空间私有且有限。要求实际生效的进程、CPU、内存和临时空间约束，以及原生 backend 可验证的全组清理；同时关闭对子进程、进程环境或已有打开文件描述符的隐式权限继承。复用既有 Bubblewrap、Landlock、seccomp、cgroup v2 能力，不能沿用 cgroup 的软降级。只验证直接子进程退出或只发现 cgroup v2 根目录，不是资格证明。当前开发进程的 cgroup 不可写；本机 systemd 用户服务树虽由用户拥有，但尚未验证可用的委派子树或所需控制器。仅在独立测试 scope 中取正向证据，不调整当前会话、常驻服务或宿主权限。必须证明不可信程序执行前已进入受控组、限制确实生效，并在清理后验证组为空。
- **Minimum initial profile.** No elevated capabilities, external networking, persistent writes, host device writes, or ambient privilege inheritance. Require private bounded scratch, enforced process/CPU/memory/scratch limits, and observable whole-group cleanup. Reuse existing Linux isolation; the soft cgroup fallback and direct-child-only evidence cannot qualify. The current development process cannot write its cgroup directory. Its local systemd user-service tree is user-owned, but usable delegation and required controllers are unproven. Obtain positive evidence in a disposable delegated scope without changing the current session, running services or host policy. Require child attachment before untrusted execution, verify effective limits, and prove group emptiness before claiming cleanup.

## 对外契约和生命周期 / Public contract and lifecycle

- 保持唯一 `bash` Capability。启动传 `keep_stdin_open: true` 且明确选 `background` 或 `yield_ms`；不能与预置脚本 stdin 叠用。`write_stdin` 要求同会话 `process_id` 和 1–4096 字节 UTF-8 `input`，不补换行；`close_stdin` 不接收 `input`，首次批准后不可逆关闭。关闭后写入、未就绪、已退出、跨会话、非交互作业及不支持的平台均无副作用；同一活作业、同一有权限的调用者重复 EOF 不再次写入或发审批。
- Use only the existing `bash` Capability. `keep_stdin_open: true` requires a resumable launch and cannot share stdin with a prebuilt script. `write_stdin` takes 1–4096 UTF-8 bytes verbatim, with no implicit newline. `close_stdin` takes no payload and irreversibly sends EOF on its first approval. An authorized repeat on the same still-live process is harmless. Invalid state, ownership or backend produces no effect. No binary transport, terminal emulation, replay or durable stdin.
- `ShellLifecycle` 和当前 registry 仍是控制与观察入口。Linux driver 拥有子进程、stdin pipe 和隔离资源，现有共用 runner 管期限/取消/排空。一个作业同时最多一个输入效果在途；不靠无界队列承载审批或写入，不在等待用户时持 registry 锁。读取、停止和其他作业不受单个 stdin 的背压影响。任何退出、取消、超时和服务停机终止输入，按第一阶段状态契约和新 profile 清理证明结算。
- Keep the current registry and lifecycle. The driver owns the pipe, child and confinement resources; the common runner owns deadlines, cancellation and output drain. Keep at most one input effect outstanding per job without a registry lock across approval. Input backpressure does not stall output, exit, or unrelated jobs. Termination closes the pipe and must settle only on the required cleanup proof.

## 授权、竞态和保留 / Authority, races, and retention

1. 启动与每次后续效果都走已有 `bash` scoped gate。只接受能提供可信主体与权限来源、完整 `SessionId`、规范化工作区、有效沙箱策略和 live control identity 的已登录、有人值守 dispatch；不从 `TurnContext`、PID 或模型参数推导权限。复验必须读取当前权威而非捕获启动载荷。没有可撤权权威的 dispatch 不开放。
2. 每次写和首次关 stdin 走现有可审计 exec approval 的一次性决策，绑定确认的请求，不能复用启动批准、常驻批准或其他进程的批准。等待审批与 I/O 就绪后，在首个效果前复验权限以及同一个活进程；用现有权威的同步判定边界与单作业的停止/EOF 串行化。只把最终非阻塞效果放在这条边界内，不在持有权限或进程锁时等待审批或 pipe 容量。若无法定义这条边界，直接拒绝。权限撤销或取消先发生则写入零字节。若效果先发生则不假装回滚；部分或不确定写入报告实际已知状态，禁止自动重试。
3. 输入参数是潜在敏感信息。已有 approval 走脱敏的目标和请求概况，原始字节不得进入 shell journal、回执、日志或持久审批。现有 `project_assistant_response` 保留原始 provider blocks，故必须在首个持久或广播投影前处理 `input`，保留 tool-call ID 和可恢复的非敏感参数；拒绝、验证失败和重启路径也要验收。不能仅在 runner 中清理。输入不是专用密钥通道，接收程序输出仍走普通输出保护。

1. Route launch and follow-up effects through the existing scoped `bash` gate. Require an attended authenticated route that can prove the full session, current principal and grant source, canonical workspace, effective policy and exact live control. Neither `TurnContext`, PID nor model parameters confer permission; routes without live revocation checks are ineligible.
2. Use the existing audited exec approval for exactly one requested write or first EOF. Recheck authority and liveness after approval and readiness, before effect, against an authority-backed linearization boundary and the job's own stop/EOF fence. Serialize only the final nonblocking effect there, without holding authority or process locks while awaiting approval or capacity. If the boundary cannot be established, deny. Revocation or cancellation that wins means zero bytes; effects that happened first cannot roll back. Report partial or uncertain delivery without automatic retry.
3. Treat the new argument as potentially sensitive. Use the existing redacted approval view. The current `project_assistant_response` preserves provider blocks as received, so redact `input` before the first durable or broadcast projection while keeping the tool-call ID and non-sensitive replay metadata. Verify denied, invalid and restart paths as well. Never rely on redacting only in the runner. The provider-facing call is not a secret transport, and child output still uses ordinary output protection.

## 验收与终审 / Release proof and final review

先做准入验证，再做公开契约：用无害交互 fixture 证明至少一个现有策略组合可满足规则语义、活权限复验与原生隔离。只要其中一项无法成立，就不新增公开参数或动作；记录失败证据，不把一个始终拒绝的接口当成交付。 / Prove eligibility before adding public actions: demonstrate that one existing policy combination satisfies rule semantics, live authorization and native isolation with a benign fixture. If any prerequisite fails, add no public arguments or actions; record the evidence instead of shipping a permanently denied interface.

- 先证明两个准入端点：在要求的隔离全部生效的真机 profile 上确有可用配置；缺证据或有未覆盖阻断规则时确实拒绝且未 spawn。通过真实 Gateway → scoped approval → Workspace → Bubblewrap 让无害的交互程序收到跨调用文本和 EOF，验证退出、续读及直接进程与后代消失。无原生正向证据就不发布一个永远 fail-closed 的空壳 schema。
- Prove both admission outcomes: a working, natively isolated profile and rejection without spawn when isolation or rule coverage is missing. Drive benign interactive input and EOF through the real Gateway and sandbox. Confirm exit, existing cursor behavior and descendant cleanup. Do not ship a catalog entry that has no verified usable backend.
- 证明审批拒绝、撤权、跨会话、审批中退出或关闭、超时与停止先于写时，pipe 实际收到零字节；验证输入背压和部分/未知写入报告、重复 EOF、持久记录脱敏及旧非交互调用兼容。不在 Windows 或 macOS 发布这个能力。
- Verify zero delivery on denied or stale requests and on termination that wins the race; verify backpressure, partial/uncertain outcomes, repeated EOF, durable redaction and ordinary calls. Non-Linux drivers reject the capability.
- 在隔离分支完成提交后，独立实现 reviewer 审查最终 diff 和真机证据，重点检查限制覆盖、撤权竞态、真实 pipe 效果及清理。设计 builder check 不可代替实现终审；终审若启动失败，记录为未完成而不是通过。不接触 `main` 或远端。
- After implementation and isolated verification, a fresh independent reviewer examines the final diff and native evidence. A design check is not implementation approval; a launch failure is reported as incomplete. No modification of `main` or the remote.
