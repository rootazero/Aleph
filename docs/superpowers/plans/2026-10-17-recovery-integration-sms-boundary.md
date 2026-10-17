# Recovery integration and SMS authority boundary / 恢复整合与短信权限边界

## Objective / 目标

Finish the existing modularization recovery on local main while preserving newer main behavior; validate, commit all retained changes, synchronize remote history and push normally. User authorized the necessary cross-crate SMS boundary refactor after review exposed a semantic conflict.

完成当前 main 的模块化恢复，保留较新 main 的行为；验证后提交保留内容、安全同步远端并普通推送。用户已授权必要的短信跨 crate 边界重构。

## Contracts / 契约

1. `aleph-slot-api` remains pure and never depends on Core. Core channel names re-export the same public API types. Move `OutboundDeliveryPolicy` and serializable `SmsInboundMeta` into the API and add the existing `MessageMeta::Sms` variant there. Core telecom re-exports the DTO.
2. Keep `TelecomPublicContext`, `VoiceIngressProof`, `PrivateSmsLease`, live transport validation and all restricted constructors in Core. Add an opt-in, object-safe `Channel::as_any` method defaulting to `None`, returning an optional reference to `dyn Any + Send + Sync`. It conveys no grant by itself.
3. Expose the concrete `SmsChannel` through that method. Its existing private-send methods become Core inherent methods without changing the checks. A crate-private Core extension trait on all `Channel` implementations downcasts only to the actual `SmsChannel`, delegates the same methods, and denies (`None` / `Invalid`) otherwise. Channel-type strings and arbitrary Any payloads cannot mint authority.
4. Preserve `ExplicitOnly`: no generic retry, no durable plaintext body persistence, no implicit replay. Public-run provenance, expiry, cancellation, relay generation and epoch fences remain unchanged.
5. Registry: reconcile duplicate placeholder and recovered declarations into one set of types, preserving public compatibility signatures. Preserve the declaration catalog across all callable mutations. Failed/no-op transactions must not publish or increment revision; the retained entry budget is the only dispatch budget source. Task60 producer integration remains deferred: the public reconciliation scaffold must explicitly reject an unmounted producer without mutating state, rather than report successful no-op or admit arbitrary caller-built raw entries.

## Trade-offs / 取舍

The optional Any bridge keeps trusted objects in Core and preserves the shared Channel type, but Core-specific SMS calls require an explicit extension trait import. Only concrete Core downcasts may consume it; this is not a serialization or generic capability authority mechanism. Future independent SMS implementations would need a deliberate Core contract extension rather than silently acquiring authority through metadata.

可选 Any 桥让权限对象留在 Core 并保留 Channel 类型身份；代价是短信调用方显式导入 Core 扩展 trait，且未来其他短信实现必须明确扩展 Core 契约，不能通过元数据自动获得权限。

## Execution and verification / 执行与验证

- Back up each original before integration and check status/diff after changes.
- Independently review SMS boundary and registry changes against actual caller paths and negative tests.
- Before every Cargo command, run a fresh real memory gate and require >=4 GiB; unknown is refusal.
- Run API and Hook wire tests, core checks and targeted registry/projection/SMS/security tests; run broader project checks as indicated by results. Report baseline or platform limitations accurately.
- Review staged paths and secrets, commit with hooks limited to the authorized task, fetch/prune and inspect divergence before synchronization, avoid force/reset, and push main only after gates pass.
- Completion audit: retained changes committed, only intended worktrees/branches remain, clean status, successful normal push, main and freshly fetched origin/main identical. Never claim completion from API-only tests.

## Validation repairs / 验证修复

The full Core suite exposed three acceptance failures. The screenshot compressor test now retains the existing single-line passthrough behavior. The no-global-goal-store case runs in its own test process and requires exactly one passing child test, preserving the original zero-promotion assertion without resetting shared global state. Harness remains at the existing 12-file / 5545-line ceiling: two redundant empty-input caller guards were removed because their helpers already perform no work for empty slices. The newer main reply-boundary safety fix is preserved. An empty deferred-call regression also checks that no event is written.

Core 全量测试暴露的三项失败已修复：保留截图单行文本的既有透传行为；以独立测试进程验证 Goal 全局未初始化分支，保留零提升断言并拒绝零测试假绿；删除两处 helper 已覆盖的空输入判断，将 Harness 保持在原有 12 文件 / 5545 行上限内，同时保留较新 main 的回复边界安全修复。空延后调用还验证不会写入事件。代价是 Goal 测试增加一次进程启动，调用方依赖 helper 维持空输入无副作用的契约。

The Darwin memory gate uses `vm_stat` free + speculative + inactive pages with its reported page size; malformed or unknown input still refuses execution. Five standalone API examples and their lockfiles were restored. Their sources passed a separate format check because workspace formatting does not include those independent packages.

Darwin 内存门按 `vm_stat` 的 free、speculative、inactive 页及其报告的页大小估算可用内存，异常或未知输入仍拒绝执行。五个独立 API 示例及锁文件已恢复；这些独立包不属于 workspace 格式检查范围，因此额外验证了其源码格式。

## Current evidence / 当前证据

Fresh final acceptance passed: Core library 21,596 tests (20 ignored); API 388 (2 ignored doctests); Hook wire 11; integration guards 33 / 7 / 1; server binaries 118; Loom 9. Shared tests executed both UI feature combinations and CLI, protocol, API and TUI targets. All integration binaries compiled, five standalone examples passed independent checks, and Panel library tests compiled. Formatting and strict workspace/all-target Clippy passed. The final matrix and a subsequent lint run matched the same 6,784-file source snapshot with no changes. Detailed logs and named-regression evidence are local under `.git/git-housekeeping-audit/recovery/validation/` and `final-acceptance-evidence.json`. This paragraph was updated afterward; executable sources remain identical to that snapshot.

最终本地验收通过：Core 21,596 项（20 忽略），API 388 项（2 个 doctest 忽略），Hook wire 11 项，三个集成 guard 分别 33 / 7 / 1 项，服务端 binaries 118 项，Loom 9 项。共享测试实际覆盖两个 UI feature 组合与 CLI、协议、API、TUI；所有集成测试完成编译，五个独立示例检查及 Panel 测试编译通过。格式与严格 workspace/all-target Clippy 通过；矩阵与随后 lint 对应同一份 6,784 文件快照，期间没有内容变化。本段在验证后更新，可执行源码仍与已验证快照一致。

All original 118 untracked paths are retained. Commit, remote synchronization and push are the remaining Git delivery steps. Platform limits: native Windows runtime QA and hosted CI were not executed; integration binaries other than the three guards were compiled only. The pre-existing contradictory assertions in the Windows pointer lifecycle test are recorded in the local Windows boundary review and remain unresolved.

最初 118 个未跟踪路径全部保留；提交、远端同步及推送为剩余 Git 交付步骤。验证边界：未执行 Windows 原生运行时 QA 或远端 CI；除三个 guard 外，其余集成测试仅完成编译。Windows 指针生命周期测试中既有的矛盾断言已记录在本地边界审阅报告，尚未修复。
