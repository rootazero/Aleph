# Stage E Bounded Executable-Code Loading Run / Stage E 有界可执行代码加载运行记录

- Date / 日期: 2026-10-28
- Branch / 分支: `stage-e-code-loading`
- Baseline / 基线: actual clean `main` and `origin/main` at `8c07dca9f5ccaa33a64ec8105c82c60e84faca49`; the plan handoff value `ee6e65885` was stale.
- Code commit / 代码提交: `7c31a5fa6716dabf23cff12ab7a14c8df6175271` (`feat(extension): prove bounded wasm code loading`)
- Host / 主机: Darwin 27.0.0 arm64

## Scope / 范围

This slice changes only the existing Extism WASM loading boundary. `std::fs::read` makes the guest-byte read observable, then `Wasm::data` feeds the existing `PluginBuilder`; the existing loader, provider generation, admission, `WasmMountIdentity`/`WasmBinding`, disposer, and unmount paths remain authoritative. Test-only receipts are owned by `PluginLoader` and attached to the exact `LoadedWasmPlugin` incarnation.

本阶段只改变现有 Extism WASM 加载边界：用 `std::fs::read` 观察真实 guest bytes，再通过 `Wasm::data` 交给既有 `PluginBuilder`；loader、provider generation、admission、`WasmMountIdentity`/`WasmBinding`、disposer 与 unmount 仍是唯一权威。测试 receipt 由 `PluginLoader` 持有，并绑定到具体 `LoadedWasmPlugin` incarnation。

## Behavioral evidence / 行为证据

- Cold discovery/list/descriptor/projection: zero guest-byte reads, zero instance creation, zero drops, inactive WASM runtime, empty tool projection and empty canonical publication.
- Activation: real fixture guest payload A is executed through the existing canonical tool path; one byte read and one instance creation are observed.
- Replacement: real reload rewrites the guest to payload B; the old canonical entry and `WasmBinding` reject stale use. The retained old owner/entry/binding holders are released before the old exact instance receipt is observed as dropped. A new instance is then created for B and the canonical call returns B.
- Failed build: invalid WASM bytes are read, `PluginBuilder::build` fails, and no instance is created or dropped.
- Drop ordering: `LoadedWasmPlugin::drop` explicitly takes and drops the `extism::Plugin` before recording the test-only receipt. Probe mutex poisoning is recovered in test code.

- 冷态发现、列表、descriptor 与 projection：guest-byte read、instance create、drop 均为零，WASM runtime 未激活，tool projection 与 canonical publication 均为空。
- 激活：通过既有 canonical tool 路径执行真实 fixture guest payload A，观察到一次 byte read 与一次 instance create。
- 替换：真实 reload 将 guest 改为 payload B；旧 canonical entry 与 `WasmBinding` 拒绝 stale 使用。先释放旧 owner/entry/binding 的 retained holders，再观察旧 exact instance drop；随后为 B 创建新 instance，canonical 调用返回 B。
- 失败构建：读取 invalid WASM bytes 后 `PluginBuilder::build` 失败，不创建也不 drop instance。
- Drop 顺序：`LoadedWasmPlugin::drop` 显式取出并 drop `extism::Plugin`，之后才记录 test-only receipt；测试 probe 的 mutex poison 路径可恢复。

## Verification ledger / 验证记录

Every Cargo command was immediately preceded by a separate `python qa/browser_live/check_memory.py` and returned `ALLOW`.

每个 Cargo 命令之前都单独执行了 `python qa/browser_live/check_memory.py`，结果为 `ALLOW`。

| Command / 命令 | Result / 结果 |
|---|---:|
| `cargo test -p alephcore --lib extension::lifecycle::tests::stage_e_ -- --nocapture` | 3/3 passed, repeated three serial runs after repair |
| `cargo test -p alephcore --lib extension::lifecycle::tests:: -- --nocapture` | 62/62 passed |
| `cargo test -p alephcore --lib extension::runtime::wasm:: -- --nocapture` | 60/60 passed |
| `cargo test -p alephcore --lib extension::lifecycle::tests::b3_ -- --nocapture` | 12/12 passed |
| `cargo test -p alephcore --test wasm_capability_test -- --nocapture` | compiled; 0 tests collected, so no behavioral evidence |
| `cargo test -p alephcore --lib gateway::channel_registry::resource_matrix:: -- --nocapture` | 6/6 passed |
| `cargo test -p alephcore --test sms_delivery_policy_test -- --nocapture` | 18/18 passed |
| `cargo test -p alephcore --lib gateway::channel_registry::tests:: -- --nocapture` | 63/63 passed |
| `cargo test -p alephcore --lib gateway::channel_registry::binding::tests:: -- --nocapture` | 3/3 passed |
| `cargo test -p alephcore --lib gateway::webhook_receiver::tests:: -- --nocapture` | 22/22 passed |
| `cargo test -p alephcore --lib gateway::delivery_queue::tests:: -- --nocapture` | 52/52 passed |
| `cargo test -p alephcore --lib gateway::channel_health_monitor::tests:: -- --nocapture` | 14/14 passed |
| `cargo test -p alephcore --lib tools::registry::tests:: -- --nocapture` | 54/54 passed |
| `cargo test -p alephcore --lib gateway::interfaces::webhook:: -- --nocapture` | 51/51 passed |
| `cargo test -p alephcore --lib capability:: -- --nocapture` | 222/222 passed; panic text is expected recovery-test output |
| `cargo check -p alephcore` | passed |
| `cargo check -p alephcore --tests` | passed |
| `cargo clippy -p alephcore --tests -- -D warnings` | passed |
| `cargo fmt --all --check` | passed |
| `git diff --check` | passed |

The recurring `__eh_frame section too large (max 16MB)` macOS linker message was non-fatal and did not affect test results. An earlier invalid Cargo invocation with two test filters was recorded as a command error, not test evidence.

反复出现的 `__eh_frame section too large (max 16MB)` 是 macOS linker 的非致命 warning，不影响测试结果。早先把两个 test filter 传给同一条 Cargo 命令的调用是参数错误，已记录为命令错误，不计入测试证据。

## Review / 审阅

A first independent gstack Pi review found lifecycle-proof gaps around receipt ordering, cold proof coupling, and probe poison handling. Those findings were repaired. A second independent read-only gstack Pi review after the repair returned **Ready: Yes** with no Critical, Important, or Minor findings.

第一次独立 gstack Pi review 指出 receipt 顺序、cold 证据耦合与 probe poison handling 的缺口，已全部修复。修复后的第二次独立只读 gstack Pi review 返回 **Ready: Yes**，无 Critical、Important 或 Minor finding。两次 ACP reviewer 尝试因模型/平台路由没有产生可用结果，未被当作审阅通过。

## Boundaries / 边界

The receipts prove one real `LoadedWasmPlugin`/Extism `Plugin` owner boundary and its exact test-observed destruction. They do not prove engine-wide JIT code unloading, anonymous mapping reclamation, OS executable-page reclamation, thread shutdown, or RSS reduction. No native dylib loading/unloading, `unsafe`, `libloading`, new runtime/dependency, production channel dynamic code loading, Gateway/Feishu/R2/Terminal/Memory migration, or second lifecycle authority was added.

receipt 只证明一个真实 `LoadedWasmPlugin`/Extism `Plugin` owner boundary 及其测试可观察的精确销毁；不能证明 engine-wide JIT code unloading、anonymous mapping 回收、OS executable page 回收、线程退出或 RSS 降低。没有新增 native dylib 加载/卸载、`unsafe`、`libloading`、runtime/dependency、生产 channel 动态代码加载、Gateway/Feishu/R2/Terminal/Memory migration，也没有新增第二套 lifecycle authority。

The bulk `PluginLoader::shutdown` path remains separately owned and is not converted into an engine/RSS claim. The `wasm_capability_test` integration target compiled but collected zero tests in this checkout. Full unfiltered workspace tests, production channel dynamic loading, native unload, engine/JIT telemetry, and RSS measurement remain deferred.

bulk `PluginLoader::shutdown` 路径仍由既有 owner 单独负责，没有被扩大解释为 engine/RSS 证据。本检出中的 `wasm_capability_test` integration target 编译成功但收集到 0 个测试。全量未过滤 workspace tests、生产 channel 动态加载、native unload、engine/JIT telemetry 与 RSS measurement 仍 deferred。
