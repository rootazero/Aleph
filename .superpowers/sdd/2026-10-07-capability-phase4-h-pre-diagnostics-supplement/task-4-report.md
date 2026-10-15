# Capability Phase 4 H-pre Diagnostics Supplement — Task 4 Report

## 范围 / Scope

本报告只覆盖 Task 4：把启动环境变量 `ALEPH_CAPABILITY_DIAGNOSTICS` 以严格的 `== "1"` 判定接入既有诊断控制面。实现位于：

- `src/bin/aleph-server/commands/start/mod.rs`
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`

复用启动阶段已经挂载并完成 readiness 的 canonical `ProjectionHost`，并把同一个 `Arc<OwnershipTree>` 传给既有 `DiagnosticControl::new`。随后将得到的 `Option<Arc<DiagnosticControl>>` 通过 `BuiltinToolConfig::diagnostics_control` 交给 Task 3 的条件注册路径。

This report covers Task 4 only. The startup environment is enabled only when `ALEPH_CAPABILITY_DIAGNOSTICS == "1"`. The implementation reuses the canonical, readiness-confirmed `ProjectionHost` and the same `Arc<OwnershipTree>` through the existing `DiagnosticControl` and Task 3 `BuiltinToolConfig::diagnostics_control` path.

Disabled values—including unset, empty, `0`, `true`, `TRUE`, leading/trailing whitespace, `01`, and `"1\n"`—return `None` before `DiagnosticControl::new`. The repair additionally makes `DiagnosticMachinery` optional and mounts the host with `diagnostics_enabled`, so a disabled host does not retain the hold mutex or `Notify`; this is established by the guarded tests below, not by source inspection alone. No second authority is introduced.

## TDD 结果 / TDD Results

### RED

执行命令（通过 guard，内存门禁为 `PASS`）：

```text
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --bin aleph-server commands::start::tests::diagnostics_requires_exact_startup_env --no-fail-fast
```

在生产 helper 尚未实现时，命令以 `CARGO_EXIT: 101` 失败。关键原始编译错误为：

```text
error[E0425]: cannot find function `startup_diagnostics_control` in this scope
```

该 RED 证明测试先于 startup wiring 存在；之后只增加了最小 helper、readiness 后的启动调用和 agent-init/config 参数传递。

### GREEN

Task 4 的修复已落地：`ProjectionHost::mount_with_diagnostics(..., diagnostics_enabled)` 只在启用时构造并保留 `DiagnosticMachinery`；startup 在 mount 前读取严格 env 值并传入该参数。以下三个 Task 4 定向测试均通过，且每次 guard 内存门禁均为 `PASS`：

```text
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --bin aleph-server commands::start::tests::diagnostics_requires_exact_startup_env --no-fail-fast
# 1 passed; 0 failed

python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --bin aleph-server commands::start::tests::disabled_startup_does_not_construct_diagnostic_control --no-fail-fast
# 1 passed; 0 failed

python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --bin aleph-server commands::start::tests::diagnostics_registration_follows_canonical_mount_readiness --no-fail-fast
# 1 passed; 0 failed
```

完整 aleph-server binary 测试也通过：

```text
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --bin aleph-server --no-fail-fast
# 118 passed; 0 failed; 0 ignored
```

修复后的受保护回归结果：`projection_host` 测试 34 passed；startup 测试 9 passed。上述结果已由 guarded test 执行记录；本次提交不重新运行 Cargo。

## 验证覆盖 / Verification Coverage

- `diagnostics_requires_exact_startup_env` 验证严格 env 值、禁用值不触发错误、`Some("1")` 使用 canonical host/tree 成功，以及不同 owner tree 被 `AuthorityMismatch` 拒绝。
- `disabled_startup_does_not_construct_diagnostic_control` 验证生产代码只作一次 startup decision、读取精确环境变量，并在 agent registration 前传递结果。
- `diagnostics_registration_follows_canonical_mount_readiness` 验证顺序为 `wait_until_ready` → diagnostics decision → `register_agent_handlers`，并验证 host/tree Arc clone 与 agent-init config wiring 的源码存在性。
- 已对两个修改过的 Rust 文件运行 `rustfmt --edition 2021`；随后恢复了 rustfmt 对既有无关代码造成的格式噪声，最终 diff 只包含 Task 4 改动。
- `git diff --check` 通过。
- 没有运行 raw `cargo`；所有 Cargo 命令均通过指定 `cargo-guard.py` 串行执行，未删除 target。

## 未解决项 / Explicit Non-goals and Unresolved Items

- Task 5 的真实启动/黑盒 QA 尚未执行；本报告只记录源码与 binary unit-test 验证，不能替代 QA。
- 已知的 early post-mount `?` drain gap 未处理；本 Task 没有改变 host retention、shutdown funnel 或该阶段的错误路径语义。
- 未修改 QA/docs、Task 1–3、H/I、SafeReplay、harness、session 或 MCP 实现。
- 未 merge、未 push；四个预-existing untracked plan/prompt 文档保持未跟踪且未修改。

Task 4 is therefore GREEN for the tested startup wiring, with Task 5 runtime QA and the known early post-mount `?` drain gap explicitly remaining outside this change.
