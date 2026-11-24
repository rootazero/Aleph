# Phase9 dependency decision — adopt resolver candidate / 采用 resolver 候选

**Decision / 决定：** adopt the Cargo-generated candidate below. The retained graph remains preserved at `2026-11-15-phase9-retained-Cargo.lock`; both graphs and their complete machine-readable deltas are retained for review. / 采用以下真实 Cargo 生成的最小更新候选，retained 原图和完整差异数组保留供审查。

| Graph / 图 | Entries | SHA-256 |
|---|---:|---|
| Frozen Phase8 input | 1537 | `bb1b647bb8f53785467c2dd47b40ed0bd46f2514c9de4e6e94bc29fccc900345` |
| Retained Phase8 graph | 1596 | `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb` |
| Adopted resolver candidate | 1572 | `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5` |

Frozen baseline remains `e08af3789dbf194f02f08461c215eb3c7701557f`; the rebased handoff HEAD is not substituted for it. All seven NSH Git entries remain pinned to `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`. / 原始比较基线不变，七个 NSH Git 条目保持精确 pin。

## Resolver evidence / Resolver 证据

A separate candidate workspace copied the tracked manifests and started from the exact frozen lock. No lock entries were hand edited. Each Cargo invocation had its own successful 4 GiB memory gate and used offline mode. / 候选在独立目录中由真实 Cargo resolver 推导，未手编条目，逐次检查内存，未隐式联网。

1. `cargo update --offline --workspace` failed on frozen `toml 0.8.2` versus NSH's `^0.8.8` requirement.
2. Updating `toml` alone exposed the `proc-macro-crate 2.0.2` exact `toml_datetime =0.6.3` conflict.
3. Joint `cargo update --offline -p toml@0.8.2 -p proc-macro-crate@2.0.2` succeeded: `toml 0.8.23`, `toml_datetime 0.6.11`, `proc-macro-crate 2.0.0`, plus the required NSH closure.

Frozen→candidate identity delta is **+43 / −8**, with **20 changed name-level version/source/checksum sets**. Frozen→retained was +328 / −269 and 285 changed sets. These are explicit counting conventions, not interchangeable package counts. The full arrays are in [minimal candidate delta](2026-11-16-phase9-minimal-candidate-delta.json); retained→candidate is also included there. / 三元 identity 与按名称聚合的 changed 口径分别列示，不混用。

Offline locked metadata passed (19 workspace members). Candidate Core test compilation passed in 15m53s; [build receipt](2026-11-16-phase9-candidate-build.json) records argv, memory gate, exit and lock hash. Candidate full lib execution reached **22,402 passed; 0 failed; 20 ignored; 0 filtered out**, 661.14s test execution, in [final raw receipt](phase9-test-output/20261116T091528.077216Z.json). That receipt binds candidate SHA `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5` and confirms the lock remained unchanged. The prior 1200s command timed out after compilation plus most tests and had a denied cleanup probe; it remains FAIL in [test history](2026-11-16-phase9-candidate-test-receipt.md). The retry used a bounded 2100s command window, including compilation. / 候选全量取得终态，未借用 retained 图证据；旧超时与清理错误不撤回，重试窗口含真实编译开销。

## Related validation and limits / 相关验证与限制

NSH package tests passed 5/5 (four host input cases and the pinned v0.5 Window API case) in [NSH receipt](2026-11-16-phase9-nsh-host-tests-v05.json). Debug lite Tauri tests passed 190/190 in [Tauri receipt](2026-11-16-phase9-tauri-lite-tests-fixed.json); both native binaries built in [shell receipt](2026-11-16-phase9-shell-binaries-fixed.json), followed by the production server in [server receipt](2026-11-16-phase9-gateway-binary-fixed.json). Earlier compiler and QA guard failures remain in their original receipts. / 相关包测试及原生壳、生产 server 编译均取得终态，早先失败不撤回。

This closes the dependency selection gate only. Runtime, parity and independent review remain separate gates. Offline resolution proves the locally available graph; it does not prove every platform/backend or dependency security status. The candidate changes 20 name-level sets from the frozen input; that dependency risk remains explicit. / 本结论仅闭合依赖决策，不代替运行时、性能或独立审查；20 组依赖更新及平台覆盖限制继续明示。

No manifest dependency was broadened and no network fetch was used for Cargo resolution. / 未放宽 manifest 依赖，Cargo resolver 未联网。
