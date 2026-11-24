# Phase8 Entry Inventory / Phase8 入口库存

**日期 / Date:** 2026-11-15
**基线 / Baseline:** local `main` only
**范围 / Scope:** Phase7 accepted handoff → Phase8 Integration & Security planning entry

## 结论 / Result

当前本地 `main` 保留了 Phase7 交接基线，未发现需要因会话重启而重跑 Phase7 的源代码漂移。178 条可移植 Rust source manifest entries 与两个补充 JavaScript inputs 的 SHA-256 均匹配。Phase7 的 bounded acceptance 继续有效；Phase8 仍只进入 specification/planning，NSH 是独立轨道。此文件不授权 Phase8 实施、NSH 依赖迁移、Tauri 删除、merge、push 或 release。

## Git handoff / Git 交接

| 项目 / Item | 实测值 / Observed value |
|---|---|
| Repository root | `<repo-root>` |
| Branch | `main` |
| HEAD | `e08af3789dbf194f02f08461c215eb3c7701557f` |
| HEAD subject | `Merge remote-tracking branch 'origin/main'` |
| Worktree | `<repo-root>` only |
| Worktree state | clean; `## main...origin/main` |
| Phase7 handoff ancestor | `2295330e51c77b20aaa9a6ce55f6fca666a810fc` → `HEAD` exit 0 |
| Handoff parent baseline | `d5e835d18ff6329ee31629548661224d33d579c9` |

没有执行 reset、pull、branch/worktree cleanup 或无关远程合入。受保护的 `.claude/settings.json`、根 `config.lock` 和根 `config.toml` 未读取。

## Source integrity / 源码完整性

| Evidence | Expected | Current result |
|---|---:|---|
| Portable v26 Rust manifest | 178 paths; manifest SHA `0aca3c9595789c8a295edc48d901daa2b4d2dc77497a787025bba2fe9164c36c` | 178/178 match; changed 0; missing 0 |
| `interfaces/webchat/scripts/build-panel-tabs.mjs` | `8737677cf2fc1f568c845edb5202fbe78c10018b5a155cb56430338b27b30fde` | match |
| `interfaces/webchat/tests/panel-build.test.mjs` | `4d08a89dfeb766b34077fd0b7f9625211f73ac8c952527b3918d8283e993679d` | match |
| Prior runtime binary | `56427f1a9b300dc20b8dfd1e17915abfbaaffc5ea54afef5c1611d65eb52a9d8` | retained Phase7 evidence; no new binary claimed |

The portable manifest is the authoritative path/hash list at `docs/superpowers/runs/2026-11-15-phase7-closeout/source-manifest.json`; this inventory records the aggregate verification rather than duplicating 178 rows.

## Retained Phase7 gates / 保留的 Phase7 闸门

The accepted bounded scope remains **PASS WITH RECORDED LIMITS**. Retained evidence is:

- strict lint, workspace checks, formal server build and fresh all-target/workspace verification exited naturally with code 0;
- 27,649 outer test passes, 0 failures, 56 ignored; 213 outer libtest footers; the 3 nested probes and 9 custom harnesses remain separate;
- corrected production line coverage: 66 executable production files, 14,311/16,193 = 88.37769406533687%, missing executable files 0, unchanged 80% threshold;
- 16 encoded production resources decoded to matching hashes;
- real PTY attach, input to shell output, resize, close, and late-resize rejection;
- same-binary I3: 25/25 listed scenarios;
- policy and emitter controls: baseline pass → actual compiled RED → exact restoration GREEN; renderer evidence is retained built-mutant replay, not a fresh mutant build;
- Node checks: 44 pass, 0 fail, 1 optional generated-glue skip;
- benchmark evidence: warm lookup 438.67 ns, invalidated cold 7.3915 μs, fresh resolve 7.0745 μs.

Ignored, skipped, unexecuted platform/backend cases and the prior defective coverage parser are not counted as new passes.

## Reachability and Contract A / 可达性与合同 A

Graphify orientation was completed before source inspection. The current simulated/no-provider startup path is server-reachable:

- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569` constructs `SimpleExecutionEngine`;
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1572` erases it as `ExecutionAdapter`;
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1618` installs it in the fallback `GatewayContext`.

`src/gateway/execution_engine/simple.rs` emits `SessionEvent::UserMessage` and `SessionEvent::AssistantMessage` through the installed global SessionService when one exists, but ignores those emit failures. When the service is absent it logs a dropped message. This is a reachable simulated-transcript persistence P2, not evidence that the engine is library-only and not a failure of the approved Contract A.

Contract A remains narrower: each successful addressed-key real `chat.send`/`agent.run` response commits canonical metadata before success; valid no-run, later queue/spend rejection, cancellation, timeout or caller disconnect may leave a row; one storage latency is accepted. Contract A does not promise a run slot, renderer readiness or durable simulated transcript persistence.

Other retained P2s are per-store/process metadata serialization only, PTY error presentation, and the defective historical coverage parser. Any promotion of these to a Phase8 required fix needs a concrete observable failure contract and terminal resource-cleanup evidence.

## Unexecuted scope / 尚未证明

This inventory does not prove or authorize all capability kinds/backends, external-effect exactly-once, Safe Replay, a universal scheduler, native NSH 0.5 execution, Linux/Windows native parity, remote deployment, same-port candidate routing, Tauri/WebChat removal, or release/push.

## Inputs for Task2 / Task2 输入

Task2 must use the current canonical descriptor, lease, ownership, policy, event and lifecycle authorities, then map five failure classes to real ingress and observable effects: foreign owner/scope; stale lease/provider/connection; queued cancellation; reconnect/withdrawal; cleanup after disable. Renderer display ACK/input is a separate contract from PTY attach/input/resize/close replies. No second registry, ready map, session store, ownership table or speculative universal scheduler may be introduced by the design.
