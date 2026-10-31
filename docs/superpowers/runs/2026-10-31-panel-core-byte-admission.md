# Core Panel byte admission execution / Core Panel bytes 准入执行记录

Plan / 计划: [Core byte-admission plan](../plans/2026-10-31-panel-core-byte-admission-plan.md)

Design / 设计: [Approved design](../specs/2026-10-31-panel-core-byte-admission-design.md)

Status / 状态: **Core implementation and scoped acceptance verified; local commit/delivery closure follows below. Broad extension-suite failures remain recorded. / Core 实现与聚焦验收已验证，本地提交收尾见下；全 extension 失败如实保留。**

## Authorization and isolation / 授权与隔离

- User approved the design on 2026-10-31 and requested completing #7. / 用户于 2026-10-31 批准设计并要求完成 #7。
- Branch: `panel-community-live-publication`; implementation base: `208cc8a7d08fa34c8f89157a0a4649833c9903f0`. Worktree is the existing ignored `.worktrees/panel-community-live-publication` checkout. No merge or push. / 沿用隔离 worktree，不 merge 或 push。
- Cargo runs serially from the owning Core package (worktree root), after `python qa/browser_live/check_memory.py`. `CARGO_TARGET_DIR=../../target CARGO_BUILD_JOBS=1` reuses the ignored build cache; Cargo compiles current worktree sources. No generated browser artifact is borrowed. / Cargo 串行、先 memory guard，复用忽略的构建缓存，不借用 browser 产物。
- Per-invocation `git -c core.hooksPath=/dev/null commit` avoids the previously documented unrelated baseline formatter hook; shared Git configuration is unchanged. Changed Rust files are formatted directly and focused tests run before commits. / 单次 commit 绕过既有 baseline formatter hook，不改变共享配置；改动 Rust 文件直接格式化，提交前运行聚焦测试。
- Detailed logs and task briefs remain in `.superpowers/sdd/2026-10-31-panel-core-byte-admission-plan/`. / 详细日志与 task brief 原地保留。

## Task 1 / 任务 1

- RED: `cargo test -p alephcore --lib extension::panel_publication` after the guard failed with `E0432`: missing `crate::extension::panel_publication`. The terminated compiler reported one error. The first detached shell did not persist a numeric exit code; its full failure log is retained. / RED 确认为缺失 Core API 的单一编译错误；首轮 detached shell 未保留数字退出码，完整错误日志仍保留。
- GREEN: same guarded command, exit **0**, **4 passed, 0 failed, 0 ignored**. First successful compilation took **14m 49s**; the cached task-done rerun also passed all four tests. / 首次成功编译耗时 14m 49s，task-done 缓存重跑同样通过。
- Proof: snapshot/byte scope agreement; cross-version and empty-generation withdrawal; lower/equal revision attempts with valid SRI; exact identity, canonical entry and integrity refusal. / 已证明上述 Core store 边界。
- Commit: `563c835d7c46cabbd9cf0a644ec6d4138bac5962` — `refactor(panel): move publication ownership into Core`.
- Moved the actual store implementation and canonical/SRI checks into Core; removed the unmounted Gateway module and boolean authorization request. The store is not yet connected to production lifecycle at this point. Removing the boolean does not prove route-owned HTTP authentication. / 实际迁移 store 与校验，移除未挂载 Gateway 模块及 boolean request；此时尚未接入生产 lifecycle，移除 boolean 不构成 HTTP 认证证明。
- During compilation available RAM briefly fell below the next Cargo's 4 GiB guard threshold. No second Cargo was launched concurrently; this run's preflight had passed. / 编译期间可用 RAM 曾低于下一轮 Cargo 的 4 GiB 阈值，没有并发启动另一轮 Cargo；本轮启动前 guard 已通过。

## Task 2 / 任务 2

- Base: `563c835d7c46cabbd9cf0a644ec6d4138bac5962`.
- RED: same guarded Core test command, exit **101**; only missing `PreparedPanelPublication` and `commit` API errors (`E0432`, `E0599`). / RED 仅出现预期的新 API 缺失错误。
- Added real-file tests for immutable admitted bytes, failed replacement withdrawal, missing/directory/integrity/parent entries, escaping symlink, exact 4 MiB boundary, deterministic 64 MiB generation budget, duplicate keys, changed final policy and revision exhaustion. / 添加上述真实文件与 generation 边界测试。
- Production implementation reuses `derive_panel_snapshot` and existing SRI checks, prepares bounded bytes off-lock, then revalidates declarations against the commit policy. / 实现复用既有声明与 SRI 判据，off-lock 准备 bytes，commit 时重验 policy。
- GREEN: guarded `cargo test -p alephcore --lib extension::panel_publication -- --nocapture`, exit **0**; **10 passed, 0 failed**, compilation **14m 35s**, test runtime **3.76s**. Nine tests exercised their assertions; the symlink test returned early with `SKIP escaping_symlink_is_not_a_publication_source: symlink privilege unavailable`. This machine therefore did **not** prove the escaping-symlink case. / GREEN 退出 0；九项执行断言，symlink 一项因 Windows 权限显式提前返回，本机没有证明 symlink 逃逸拒绝。

- Commit: `afbe21fd4758f8b4ca2eebc83c0100ddfcd1130b` — `feat(panel): admit bounded canonical module bytes`.
- Cached task-done rerun: same ten reported passing tests, **0 failures**; the symlink privilege limitation remains. / task-done 缓存重跑同样报告十项通过，symlink 权限限制不变。

## Task 3 / 任务 3

- Base: `afbe21fd4758f8b4ca2eebc83c0100ddfcd1130b`.
- Six real-manager lifecycle tests live under `src/extension/lifecycle/tests/panel_publication.rs`, a child of the existing lifecycle test module, so they reuse its isolated HOME fixture without exporting test-only production APIs. / 六项真实 manager 测试放在既有 lifecycle test module 的子模块，复用隔离 HOME，不导出测试专用 production API。
- Existing projection regression now writes a real explicit file and matching SRI. / 旧 projection regression 现使用真实文件和匹配 SRI。
- RED: guarded `cargo test -p alephcore --lib core_panel_lifecycle -- --nocapture`, exit **101**, with 35 `E0599` errors exclusively for the missing scoped snapshot/asset manager methods. No production lifecycle code changed before that result. / RED 退出 101，仅缺失预期 manager API，确认后才修改 production。
- Implementation: `ExtensionManager` and `Views` share one publication-state handle; existing `after_transition` prepares off-lock and commits under current policy-read → publication-write locks. Snapshot and asset reads share `visible_to`; zero-context discovery is Global-only. No HTTP authorization is implied. / 复用唯一 trigger 与 state，锁外准备、当前 policy 下 commit，两种读取共用 scope 判据，不构成 HTTP 认证。
- First runtime attempt: same guarded command exited **101**, **1 passed / 5 failed**. The native TOML adapter sanitizes dotted plugin IDs to hyphens, but the fixture's assertions/reload calls used dotted IDs. Thus reload returned `NotFound` and never exercised withdrawal; these failures are not passing lifecycle evidence. The fixture now uses native canonical IDs consistently, without changing production identity rules. / 首轮 1 通过、5 失败；fixture 忽略 native ID sanitation，reload 未命中实际 record。已校正 fixture，不改 production identity，也不把失败当通过。
- Added persisted-outcome assertions for enable/disable. Guarded `cargo test -p alephcore --lib extension:: -- --test-threads=1 --nocapture` compiled in **14m 51s**, then exited **101**: **883 passed / 5 failed**, runtime **150.90s**. All six new lifecycle tests and the repaired real-file projection regression reported `ok`. / 已运行全 extension 回归，883 通过、5 失败；六项新 lifecycle 与真实文件 projection regression 均通过，整套测试不全绿。
- The five failures are listed below. Read-only baseline comparison confirmed the unchanged startup/ownership-guard sources, MCP fixture, pure Panel derivation/validation and Protocol validator. The projection census also reports the same two `mod.rs` calls already present at base lines 713/762. No baseline test binary was separately rebuilt, so this is source-equivalence evidence, not a claimed baseline runtime comparison. The `plugins` submodule is uninitialized at pinned `b35ad47fe8eec3196b2dee70e51eadeacf3a620c`. / 下列失败依据源码等同性与环境检查分类，没有另跑 baseline binary，不伪称运行对比；未初始化的 plugin submodule 也没有借用主检出内容。
  - `extension::hooks::registry::ownership_tests::startup_and_default_constructors_forward_the_single_runtime_tree`: source-string `find(...).unwrap()` on absent startup needle.
  - `extension::mcp_config::tests::bare_and_absolute_commands_are_kept_as_written`: Windows has no `/usr/bin/python3`.
  - `extension::panel::tests::accepts_semver_build_metadata_in_the_canonical_entry`: existing fixture returns `InvalidManifest("panel.invalid_entry")`.
  - `extension::projection::tests::publishing_plugin_projections_has_exactly_one_author`: existing manager wrapper calls conflict with the file-based census; the publication writer remains `after_transition → republish_plugin_projections`.
  - `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`: no manifests in the uninitialized submodule.
- Focused GREEN: guarded `cargo test -p alephcore --lib panel_publication -- --test-threads=1 --nocapture`, exit **0**, **16 passed / 0 failed**, runtime **4.02s**. This covers all six real-manager tests and ten store/admission cases. The Windows symlink case again explicitly returned early; sixteen is the harness-reported count, not proof of symlink escape. / 本次聚焦验收通过；十六为 harness 报告数，symlink 权限限制仍明确保留。
- Snapshot RPC envelope regression: guarded `cargo test -p alephcore --lib gateway::handlers::panel_capabilities`, exit **0**, **2 passed / 0 failed**. This checks the shared response shape, not HTTP asset authentication. / RPC envelope 两项通过，不构成 HTTP asset auth 证明。
- Core compile: guarded `cargo check -p alephcore --lib`, exit **0**, **4m 47s**, five unused/dead-code warnings. / Core 编译通过，保留五条 warning。
- Native Panel: memory guard from the worktree root, then `cargo test --lib --no-default-features --features panel-modular` from `interfaces/webchat`, exit **0**, **1,602 passed / 0 failed**; build **2m 21s**, runtime **1.17s**. Includes discovery 14, loader 15, projection 12, registry 6 and SDK 18 tests, including built-in Terminal descriptor/route projection. This is native state-machine proof, not a generated-WASM/browser test. / native 全套通过，含 65 项上述 Panel 契约测试；不是浏览器证明。
- JS loader: `node --test tests/panel_loader.test.mjs`, exit **0**, **12 passed / 0 failed / 1 skipped**; generated glue absent (`ALEPH_PANEL_BRIDGE_GLUE`). / JS loader 通过，generated glue 一项跳过。
- Documentation: eight changed Markdown files, **101** resolvable local links, **0** missing targets, **0** new machine paths; four pre-existing example targets excluded. Changed-file rustfmt and `git diff --check` passed. / 文档与改动格式检查通过，原有示例链接单独排除。
- Fresh read-only whole-branch review: `del_mvwbhf5a_gg5i` left a complete written assessment, then exited `SIGTERM`; this is written review evidence, not a clean-exit run. It found no definite Critical/Important production-code defect and one Minor concurrent-test evidence gap. The main agent regraded the gap Important for acceptance evidence: `observed > 0` could pass with only the initial version or flake before the reader ran. The test now acknowledges successful exact-byte reads of versions 1.0.0, 2.0.0 and 3.0.0 before stopping; snapshot→asset races may still fail closed. This is a test-only strengthening, not a new production fix or invented RED. The strengthened test passed in both the broad run and focused acceptance. / 书面审查完整但 SIGTERM；并发证据缺口升级为验收 Important，逐版本成功读取确认已在广泛回归及聚焦验收中通过。

### Review rulings / 审查边界裁定

- Runtime/Core/Terminal/browser checks: the reviewer did not run them; the main agent must report actual results separately. / reviewer 不运行，主会话单独报告。
- Symlink escape: unavailable Windows privilege means no runtime proof, retained as an explicit skip. / 权限缺失不能当作通过。
- Concurrent trust setter during real manager file I/O: static lock-order/final-policy review plus the prepare→changed-commit-policy test are the evidence; no controlled real-manager interleaving claim. / 不宣称真实 manager 受控交错已证明。
- HTTP authentication, caller/project proof, origin/TLS/rate limits: accepted as original #2–#5, not #7; internal context is not authorization. / 属后续任务，内部 context 不等于授权。
- Browser credential transport, Gateway→loader→shell, generated WASM: accepted as unimplemented/unverified; fixed-map results are separate. / 保持未实现/未验证。
- Immediate untrust teardown, restart recovery, general hostile concurrent filesystem mutation, installation ecosystem, multi-file/remote loading: outside the approved #7 design; no new guarantee or implementation added. / 设计外，不新增保证。

## Deferred proof / 仍待证明

Core lifecycle acceptance is now backed by Task 3. Production route-owned authentication, trusted HTTP caller/project context, Gateway asset delivery, browser credentials and real HTTP-to-PanelShell integration remain unimplemented/unverified. Full workspace tests, the default legacy-Terminal release artifact, generated WASM/Playwright delivery and privileged Windows symlink execution were not performed here. / Task 3 已证明 Core lifecycle；上述 HTTP/browser 链路仍未实现/验证，未跑 workspace 全套、legacy Terminal release 重建、generated WASM/Playwright delivery 或需提权的 symlink 用例。
