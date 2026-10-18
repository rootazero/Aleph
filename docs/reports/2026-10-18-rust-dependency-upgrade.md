# Rust 与依赖升级 / Rust and dependency upgrade

日期 / Date: 2026-10-18

状态 / Status: 迁移与验证已完成（Linux 验收） / Migration and verification complete for the Linux validation host.

## 范围 / Scope

将 Aleph 工作区固定工具链由 Rust 1.96.0 升级至已安装的稳定版 1.99.1，并审查根工作区、桌面、Panel、共享客户端、插件和模块化示例的直接依赖。支持基线统一为 Rust 1.99。`Cargo.lock` 中的 `0.9.0` 等数字属于各个库，不能用来判断 Rust 编译器版本。

Upgrade the pinned workspace toolchain from Rust 1.96.0 to stable Rust 1.99.1, with a Rust 1.99 support baseline. Review dependencies across the workspace, desktop, Panel, shared client, plugins, and modularization examples. Dependency versions in `Cargo.lock` are independent of the compiler version.

升级保留已有用户数据、凭据服务标识、网络协议、权限与错误语义。平台原生库按所属框架的 ABI 配套迁移；无法安全提升的依赖需要列明证据。依赖升级提供上游能力与修复，并不自动意味着新增 Aleph 产品功能或已获得性能提升。

Preserve existing data, credential service identifiers, wire formats, authorization, and error behavior. Match native library versions to their framework ABI, and document concrete constraints on any retained dependency. Upstream features and fixes do not, by themselves, demonstrate new Aleph product behavior or a measured performance gain.

## 升级前状态 / Before the upgrade

- `e8ef2e342`: 提交原生欢迎引导、配对说明及对应 Panel 资源 / Commit native welcome and pairing guidance, including Panel assets.
- `250e4800a`: 合入远端 `main` 的四笔提交，保留本地修复；本地只有 `main` 和一个 worktree，无其他开发分支 / Merge four remote commits while preserving local fixes; one local branch and worktree, with no other development branches.
- 升级前 Panel 单元测试 / Pre-upgrade Panel unit tests: **1601 passed, 0 failed**.
- 合并冲突处理后 `git diff --check`、暂存区检查与四个冲突文件的 rustfmt 检查均通过 / Merge resolution passed whitespace and rustfmt checks for all four conflicted Rust files.
- 合并后审查 **196** 个直接依赖名称；版本比较排除预发布与撤回版本，并正确处理 `+spec-1.1.0` 等 build metadata / Audit 196 direct dependency names after merging; exclude prereleases and yanked versions and compare build metadata correctly.

Panel 产物体积对照 / Panel artifact footprint comparison:

| Artifact | Before bytes | After bytes | Change |
|---|---:|---:|---:|
| `aleph_panel.js` | 118909 | 121507 | +2.18% |
| `aleph_panel.js.br` | 14369 | 14444 | +0.52% |
| `aleph_panel_bg.wasm` | 24582235 | 72599748 | +195.33% |
| `aleph_panel_bg.wasm.br` | 3849166 | 3793780 | -1.44% |

未压缩 WASM 的增长几乎都来自 `name` 自定义段：11538677 → 59554293 字节；剔除自定义段内容后，模块约为 13.04 MB，变化约 1.9 KB。现有 `wasm-release` 和两次 wasm-opt 的策略故意保留 Rust 符号，用于浏览器崩溃诊断。本次保留这一策略，因此未压缩分发体积明显增加，而 Brotli 文件略小；不能据此推断运行速度或内存改善。
Nearly all raw WASM growth comes from the retained `name` custom section: 11538677 → 59554293 bytes. Excluding custom-section contents leaves about 13.04 MB, changing by roughly 1.9 KB. The existing wasm-release profile and both wasm-opt passes deliberately retain Rust symbols for browser crash diagnostics. That policy is preserved: raw distribution size grows substantially while the Brotli file is slightly smaller. These sizes do not establish runtime speed or memory improvements.

## 工具链 / Toolchain

`rust-toolchain.toml` 固定 `1.99.1`，同时声明 `rustfmt`、`clippy` 和 `wasm32-unknown-unknown`。本机实际输出为 `rustc 1.99.1 (58cf8f9c3 2026-10-13)`。中英文 README、工作区最低版本和 Panel 声明同步维护。

The toolchain file pins Rust 1.99.1 together with rustfmt, clippy, and the WASM target. The installed compiler reports `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Keep both READMEs, workspace/core MSRV declarations, and the Panel declaration consistent.

本机 `wasm-bindgen-cli` 已从 `0.2.122` 升至 `0.2.129`，通过 Cargo 安装并验证实际版本，退出码为 0。其上游锁文件在安装时报告 `yoke-derive 0.8.3` 已撤回，以及 `buf_redux 0.8.4`、`multipart 0.18.0` 的未来 Rust 兼容警告；这是 CLI 的构建依赖，须与 Aleph 工作区的锁文件和警告区分。

The managed local `wasm-bindgen-cli` was upgraded from 0.2.122 to 0.2.129 through Cargo, with exit code 0 and the executable version verified. Its upstream installation lockfile reported the yanked `yoke-derive 0.8.3` and future-incompatibility warnings for `buf_redux 0.8.4` and `multipart 0.18.0`. These belong to the CLI build, and are separate from Aleph's workspace dependency graph and diagnostics.

## 迁移与验证结果 / Migration and verification results

相对合并基线 `250e4800a`，根锁文件中 **555 个包名的最高解析版本提高，606 个包名的版本集合发生变化**；此计数包含间接依赖和各平台分支。当前锁文件 SHA256 为 `3c5a9ad61c7711f593cac0b20bfa3663284b13006d479da1f83f43de00a8c18d`。23 个验收阶段全部通过，聚合器统计 26387 次通过、0 次失败、25 次既有 ignored 执行（集成 no-run 不计运行）。
Compared with merged baseline `250e4800a`, **555 package names have a higher maximum resolved version, and 606 have changed version sets**, including transitive and platform dependencies. The lockfile SHA256 is shown above. All 23 validation stages passed: the collector counted 26387 passed, zero failed, and 25 pre-existing ignored executions (the integration no-run stage is not counted as runtime execution).

| Direct dependency / 直接依赖 | Before / 升级前 | After / 升级后 |
|---|---|---|
| Tokio | 1.52.3 | 1.53.3 |
| Primary Reqwest / 主 HTTP 调用链 | 0.12.28 | 0.13.5 |
| Primary Tokio Tungstenite / 主 WebSocket 调用链 | 0.26.2 | 0.30.0 |
| JSON Schema | 0.29.1 | 0.59.0 |
| TOML / TOML Edit | 0.8 / 0.22 | 1.1.8 / 0.25.17 |
| Base64 | 0.22.1 | 0.23.1 |
| SHA2 / HMAC / HKDF | 0.10 / 0.12 / 0.12 | 0.11 / 0.13 / 0.13 |
| K256 / CBC | 0.13 / 0.1 | 0.14 / 0.2 |
| ZIP / Git2 | 2.4.2 / 0.19 | 9.0.5 / 0.21 |
| Notify / full debouncer | 6.1.1 / 0.3.2 | 8.2.0 / 0.7.0 |
| Ratatui / Crossterm | 0.29 / 0.28 | 0.30.2 / 0.29 |
| Textarea implementation / 文本编辑组件 | tui-textarea 0.7 | ratatui-textarea 0.9.3 |
| Leptos / wasm-bindgen | 0.8.19 / 0.2.122 | 0.8.22 / 0.2.129 |
| Keyring | 3.6.3 | 4.2.0 |
| Tauri / tauri-build | 2.11.2 / 2.6.2 | 2.12.4 / 2.7.3 |
| Xcap / Enigo | 0.8.3 / 0.3 | 0.9.8 / 0.6.1 |
| Matrix SDK | 0.16.0 fork | 0.18.0 |
| Window Vibrancy / serial_test | 0.5.3 / 3.5.0 | 0.8.1 / 4.0.1 |
| Syn / proc-macro2 (dev-only) | 2.0.117 / 1.0.106 | 3.0.9 / 1.0.107 |

迁移包括新密码学 trait、TOML/JSON Schema 接口、ZIP 文件名错误传播、Nostr 密钥类型和 Notify 缓存 API。Notify 使用平台推荐的 `RecommendedCache` 与 `Debouncer::watch`，继续保持原有去抖、递归、过滤及释放语义。Rust 1.99 的原子更新方法改名同步到 Core 和 Panel 的现有并发守卫，原有顺序与溢出边界保持不变。移除未引用的 Core `user-idle` 和 `pem` 直接依赖声明；`user_idle_time` 仍通过原有系统 Bridge 提供。
Compatibility work covers crypto traits, TOML/JSON Schema APIs, ZIP filename-error propagation, Nostr key types, and Notify's cache API. Watchers use `RecommendedCache` and `Debouncer::watch` while retaining debounce, recursion, filtering, and lifetime behavior. Core and Panel atomic guards follow Rust 1.99's method rename while preserving ordering and overflow limits. Unreferenced direct `user-idle` and `pem` dependencies were removed; the existing system Bridge still provides `user_idle_time`.

新版 `toml::Value::from_str` 从文档解析改为单值解析。插件清单、相应生成清单测试以及 `[sandbox].deny_read_globs` 的完整文档读取已改用显式 `toml::from_str::<toml::Value>`；`Table::from_str` 与 `Value` 的 Display 实现经前后版本源码核对，无需更改。
The new `toml::Value::from_str` parses a standalone value rather than a document. Plugin manifests, the generated-manifest test, and the document reader for `[sandbox].deny_read_globs` now use explicit document deserialization. Source comparison confirmed that `Table::from_str` and `Value` Display retain their previous semantics.

首轮 Core 运行发现 VTE 0.15 默认启用 `std`，内部 OSC 原始数据由有界 `ArrayVec` 改成动态 `Vec`。Core 现显式关闭该默认特性；完整解析图确认没有其他消费者重新启用 VTE 0.15 的 `std`，原有越界效果断言已在新二进制定点通过。PTY 日志测试使用输入门控，先观察 Running/pid 再允许合成子进程退出，已通过同一轮定点，避免立即退出导致的快照竞争。
The first full Core run exposed VTE 0.15 enabling `std` by default, changing its internal OSC buffer from bounded ArrayVec storage to a dynamic Vec. Core disables that default, feature unification confirms it remains off, and the original bound assertion passed in a fresh binary. The PTY journal test also passed after gating synthetic child exit until Running/pid is observed.

卸载测试的最终超时涉及两层夹具隔离：发现器独立读取 `~/.agents`，而进程共享技能系统还会累计此前测试注册的基础目录。GDB 在真实超时点捕获到技能伴随文件安全扫描；单独以私有 HOME 运行同一二进制可在 0.10 秒通过，但完整测试中此前注册的共享目录仍会触发扫描。进一步的单线程顺序对照确认：先跑 Claude-cache 加载再跑卸载，在普通初始 HOME 下为 1 passed/1 failed（38.57 秒），私有初始 HOME 下为 2 passed/0 failed（0.16 秒）。
The final unmount timeout exposed two fixture-isolation gaps: discovery reads the independent ~/.agents root, and the process-wide skill system retains base directories registered by earlier tests. GDB captured actual companion-file security scanning at the timeout. A private home made the identical test pass in 0.10 seconds alone, but did not clear directories already registered during a full run. A serial order control reproduced this: Claude-cache loading followed by unmount yielded one pass and one failure with the normal initial home (38.57 seconds), versus two passes with a private initial home (0.16 seconds).

夹具现使用既有双 HOME RAII guard 和固定锁序，断言 agents 根位于临时 HOME，并为该管理器注入独立的真实 `SkillSystem`。`ALEPH_HOME` 使用 `aleph-state` 子目录，保留默认配置路径的保存守卫。真实 guest、扫描实现、5 秒边界、Disabled、拒绝调用、直接 guest 调用和最终卸载断言全部保留；生产扫描与共享实例逻辑未改。临时诊断已移除。最终夹具修正后的三个定点用例已通过：3 passed、0 failed、0 ignored，运行 0.37 秒（含编译命令 537.83 秒）。新增同进程顺序回归也已通过：2 passed、0 failed、0 ignored，运行 26.99 秒；执行数与前序先于目标的顺序校验均通过。完整复验现已通过：21570 passed、0 failed、19 项既有 ignored，1153.62 秒测试运行时间（命令 1154.90 秒）。汇总核对覆盖全部 21589 个注册用例，锁文件未变。
The fixture now uses the existing dual-home RAII guard and lock order, verifies that its agents root belongs to the temporary home, and gives this manager an independent real SkillSystem. Its aleph-state directory preserves the default-config save guard. The real guest and scanner, five-second bounds, Disabled status, denied calls, direct guest call and final disposal assertions remain intact; production discovery and shared-instance behavior are unchanged. Temporary diagnostics are removed. The final fixture correction passed all three focused regressions in 0.37 seconds (537.83 seconds including compilation). The new same-process serial order regression passed both cases in 26.99 seconds, with exact execution counts and predecessor ordering verified. Full validation also passed: 21570 passed, zero failed and 19 existing ignored cases in 1153.62 seconds (1154.90 seconds for the command). The summary accounts for all 21589 registered cases, with the lockfile unchanged.

Rust 1.99 的机械修正保持固定分块丢弃余数的语义、控制流和流返回类型不变。预构造 RPC/HTTP 响应错误和 fake 事件注入的标记结果保留既有接口，在具体函数上使用带原因的 `expect`，没有全局降低 lint。Criterion benchmark 改用标准库 `std::hint::black_box`，测量主体不变。
Fixed-size chunk loops continue to discard the remainder. Other mechanical Rust 1.99 changes preserve control flow and stream return types. Prepared RPC/HTTP response errors and fake event-injection marker results retain their contracts through function-scoped lint expectations with explicit reasons; no global lint level is reduced. Criterion benchmarks use the standard library's black box without changing the measurement body.

验证状态 / Verification status:

| Check | Result |
|---|---|
| Workspace `cargo fmt --all -- --check` | Fresh final-lock exit 0 |
| Final `cargo test --locked -p aleph-panel --lib` | 1601 passed, 0 failed, 0 ignored |
| Final `cargo test --locked -p aleph-client --lib` | 48 passed, 0 failed, 0 ignored |
| Shared UI logic, pure and default-feature shapes | 223 passed in each shape |
| Canonical `just test-shared` | All commands exit 0; existing ignored cases retained |
| CDP and agent-detect unit/integration/doc tests | All exit 0, no compiler warnings |
| Shared desktop unit tests in an isolated headless session | 284 passed, 0 failed, 0 ignored |
| Linux-host checks for desktop-linux/windows/macos packages | Exit 0; native Windows/macOS target code remains unverified |
| Shell full/default check and unit tests | Exit 0; 167 passed, 0 failed, 0 ignored |
| Shell lightweight check and unit tests | Exit 0; 167 passed, 0 failed, 0 ignored |
| Shell `qa-owned-root` configuration check | Exit 0; this is a compile check, not a native GUI run |
| Three focused Core regressions | Final fixture: exit 0, 3 passed/0 failed/0 ignored (0.37 s); exact count and lock checks passed |
| Same-process predecessor → unmount regression | Exit 0, 2 passed/0 failed/0 ignored (26.99 s); exact count and predecessor order checked |
| Full Core `cargo test --lib` | Exit 0: 21570 passed/0 failed/19 existing ignored (1153.62 s); all 21589 registered cases accounted for, lock unchanged |
| Workspace/all-targets Clippy `-D warnings` | Fresh `--locked --workspace --all-targets --keep-going -- -D warnings`: exit 0 (6.36 s); upstream Cargo future-compatibility notice retained |
| Final Panel `just wasm` | Exit 0: optimized WASM, matching 0.2.129 glue, WebView baseline, Brotli and dist validation (41 references / 44 exports) |
| Fresh release server smoke | Exit 0: release server ready 200/health 200; launch-to-ready 4.0203 s; embedded `index.html`, JS and WASM bytes exactly matched final dist; all owned processes cleaned |
| Fresh Playwright Panel smoke | Exit 0: native welcome, shared-token rejection/input clearing, acknowledgement without secret, reload suppression, plain-browser suppression, no page errors; timings 1149.18/987.57/1056.61 ms; resource responses 200 |
| Windows crate lib tests on Linux host | Fresh exit 0: 50 passed, 0 failed, 0 ignored; does not exercise a native Windows hook |
| Affected Core test-helper regression subset | Fresh exit 0: 86 passed, 0 failed, 0 ignored (9.23 s runtime); exact execution-count check passed |
| Core `cargo test --bins` | Fresh final-lock exit 0: 118 passed, 0 failed, 0 ignored |
| Core integration targets with `test-helpers --test '*' --no-run` | Fresh final-lock exit 0: 157 executables compiled (1187.60 s); this stage does not execute them |
| `modularization_import_guards` | Fresh final-lock exit 0: 7 passed/0 failed/0 ignored (0.46 s runtime); guarded metadata has 1527 resolved nodes |
| Resolved-package MSRV metadata | 1527 packages inspected; 1052 declare a minimum Rust version, none above 1.99; missing metadata is not a cross-platform build proof |
| Both Rust plugins: WASM release build and Clippy `-D warnings` | All exit 0; local plugins commit `0aaf615` |
| Five modularization example lockfiles | Regenerated successfully, all exit 0 |
| `cargo check --all-targets --locked --manifest-path examples/modularization/<kind>/Cargo.toml` | All five passed, with no compiler warnings |
| `cargo test --lib --locked --manifest-path examples/modularization/tool/Cargo.toml` | 1 passed, 0 failed, 0 ignored |
| Matrix 0.16.0 fork → 0.18 SQLite state migration fixture | Exit 0; typed sync token and filter preserved, updated, and reopened |
| AES-GCM / HKDF / SHA2 old ↔ new module fixture | Exit 0; 8 cases in both directions, 32 tamper and 32 wrong-master rejections |
| Linux Secret Service Keyring 3.6 ↔ 4.2 compatibility fixture | Exit 0; old/new roundtrips passed and QA entries confirmed removed |

Core 全量通过后，严格 Clippy 又发现 4 个测试文件内的 6 项机械 lint；这些修正不改生产代码、断言条件或加密字节语义。受影响的 Chromium、TOML 序列化、原子写入与 WeChat AES 测试共 86 项已重编执行，86 passed / 0 failed / 0 ignored（9.23 秒），精确执行数判据通过，完成全量结果之后的定点刷新。
After the full Core pass, strict Clippy found six mechanical lints in four test files. The fixes retain production code, assertion conditions, and encryption byte semantics. All 86 affected Chromium, TOML serialization, atomic I/O, and WeChat AES tests were rebuilt and passed (9.23 seconds), with no failed or ignored cases and an exact execution-count check, refreshing the affected portion of that full result.

`<kind>` 为 `ai`、`browser`、`channel`、`search`、`tool`。示例验证使用独立 target 目录，避免干扰主工作区的构建锁。
The five kinds are `ai`, `browser`, `channel`, `search`, and `tool`. Verification uses a separate target directory to avoid contending for the main workspace build lock.

Core 库和二进制测试构建仍显示上游 `proc-macro-error2 2.0.1` 的 Cargo future-incompatibility 提示。该提示已保留；当前测试通过不等于该依赖的未来 Rust 兼容性已经解决。
Core library and binary test builds retain Cargo's future-incompatibility notice for upstream `proc-macro-error2 2.0.1`. Passing the current tests does not resolve that dependency's future Rust compatibility.

如通讯库构建需要重新生成 protobuf，已安装官方 `protoc 36.2`，实际 `--version` 为 `libprotoc 36.2`。Linux x86_64 release archive 已与官方 GitHub asset 的 SHA256 `121f6c7afe1d4d0e3ea6aab9432038599250134cbf4474cb1167d2c7decd4278` 核对一致。二进制、标准 include 文件与来源记录保存在 `~/.local/share/aleph-build-tools/protoc-36.2/`，`~/.local/bin/protoc` 为指向该版本的符号链接；复制后再次比对二进制哈希。没有修改系统包，后续工具更新需维护这一本地版本。
Official protoc 36.2 is installed for protobuf generation and reports `libprotoc 36.2`. The Linux x86_64 archive matches the official asset SHA256 shown above. The versioned user directory contains the binary, standard include files, and source record; a symlink in `~/.local/bin` exposes it on the existing PATH. Binary hashes were compared after copying. No system packages were changed; subsequent updates must maintain this user-installed version.

本机一次 Core `cargo check --lib` 的 rustc 进程曾达到约 9.7 GiB RSS。因此重型验证按顺序执行，并使用 `CARGO_BUILD_JOBS=1`、`CARGO_PROFILE_DEV_DEBUG=0` 和 `CARGO_PROFILE_TEST_DEBUG=0` 限制资源占用。这是构建资源观测，不是 Aleph 运行性能测试，也不代表 4 GiB 的最低门禁能保证整个 Core 构建完成。

A rustc process for one local Core `cargo check --lib` reached about 9.7 GiB RSS. Heavy validation ran sequentially, using `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`. This is a build-resource observation rather than an Aleph runtime benchmark; passing the 4 GiB minimum memory gate does not guarantee enough memory to complete a Core build.

Core 完整单测编译早前受本机可用内存阻塞。实际物理内存为 `20401528 kB`（约 19.46 GiB），与既有桌面和服务共享；原有约 0.5 GiB swap 已用尽。启用增量编译时单个 rustc 达到约 14.6 GiB 峰值 RSS、可用内存低于 0.5 GiB；关闭增量编译并保留单任务、无 debug、Core 测试 `codegen-units=256` 后，仍达到约 15.1 GiB 峰值，可用内存低至 `86120 kB`。两次均主动终止自有编译，未计作通过，没有删减测试范围或断言。随后核实本机已有无交互管理员权限与 swap 管理能力，新增了独立的临时 8 GiB 构建 swap，原有 swap 条目不变；该资源条件已使 Core 测试编译及完整执行结束，失败项已修复并复验通过。验证结束后已运行 `python /tmp/aleph-upgrade-temporary-swap.py disable`，状态记录为 `cleaned=true`，临时目录已删除，`/proc/swaps` 只剩原有 `/swap/swapfile`。
Both owned compilers were stopped deliberately and neither was counted as completed; test scope and assertions were preserved. Existing noninteractive administrator and swap-management permissions were then confirmed, and a separate temporary 8 GiB build swap was enabled without changing the existing swap entry. This made the Core test compilation and full execution possible; the failures were fixed and reverified. After validation, `python /tmp/aleph-upgrade-temporary-swap.py disable` completed with `cleaned=true`, the temporary directory was removed, and `/proc/swaps` contains only the original `/swap/swapfile`.
Earlier full Core unit-test compilation was constrained by available host memory. The machine has about 19.46 GiB physical RAM shared with existing desktop/services and an exhausted 0.5 GiB swap. The compiler reached about 14.6 GiB peak RSS with incremental compilation; the nonincremental single-job/no-debug retry with 256 codegen units reached about 15.1 GiB and left only 86120 kB available. Both owned compilers were stopped deliberately and neither is counted as completed; test scope and assertions were preserved. Existing noninteractive administrator and swap-management permissions were then confirmed, and a separate temporary 8 GiB build swap was enabled without changing the existing swap entry. The Core test binary compiled and completed its first full execution with these resources; the fixture and focused regressions were then corrected and reverified. After validation, `python /tmp/aleph-upgrade-temporary-swap.py disable` completed with `cleaned=true`; the temporary directory was removed and the original swap entry remained unchanged. Task 6's cleanup requirement is complete.

临时 swap 已使首轮完整编译可行，后续 Core lib 与三个定点用例重新启用增量缓存，仍保持单构建任务、无 debug 和 256 codegen units；全量测试使用四个测试线程。增量缓存会增加本地磁盘和内存占用，目的是避免每次小范围测试修正都重新生成整个 Core，而非改变产品运行参数。
With temporary swap available, Core lib and the three focused regressions reuse incremental compilation while retaining one build job, no debug data, and 256 codegen units; the full test runtime uses four test threads. Incremental cache trades additional local disk and memory for smaller subsequent recompilations and does not change product runtime settings.

### 第一轮依赖图审查 / Initial dependency graph audit

对第一轮重新解析的根锁文件通过 OSV 批量查询 **1553** 个 registry 包版本，所有查询成功，标记 **28** 个包版本。去重并获取详情后，共 **30** 条 RustSec 通告，其中按通告摘要区分为 **15** 条漏洞通告和 **15** 条停止维护通告。锁文件包含构建、开发、可选及各平台依赖；这些数量不等同于生产进程可利用漏洞数量。迁移完成后须重新检查最终锁文件。

An OSV batch query of the initial regenerated lockfile checked 1553 registry package versions, with no failed batches and 28 flagged package versions. Deduplicating and retrieving the RustSec entries produced 30 advisories: 15 describing vulnerabilities and 15 describing discontinued maintenance, classified by advisory summary. The lockfile includes build, development, optional, and platform dependencies; these counts do not establish exploitability in production. Recheck the final lockfile after migration.

已将具体路径反馈给迁移负责人 / Dependency paths provided to the migration workers:

- `lru 0.12.5`: Core 与旧版 Ratatui 引入；[RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html) 修复版本为 `0.18.2` / Introduced by the core and old Ratatui, with the fix in 0.18.2.
- `rustls 0.22.4`: `serenity 0.12.5 → tokio-tungstenite 0.21`；需检查上游版本或受支持的 TLS backend / Review the upstream version or a supported TLS backend.
- `h2 0.3.27`: `readability 0.3 → reqwest 0.11.27`；需检查 HTML 提取是否实际调用该网络链 / Check whether HTML extraction invokes this network path.
- `imbl-sized-chunks 0.1.3`: `matrix-sdk 0.16 → imbl 6.1`；有 panic 时 double-free 通告，不能因主包已是最新小版本就忽略 / A panic-related double-free advisory remains relevant even at the latest patch in that dependency line.
- `zip 2.6.1` / `zip 4.6.1`: CLI、Panel 的 ICU 构建工具与 Tauri updater 分别引入；需区分直接依赖、构建工具和上游固定版本 / Distinguish the CLI dependency, Panel ICU build tooling, and the version pinned by Tauri updater.
- `glib 0.18.5`: 与 GTK3/WebKit 类型和 ABI 绑定，不能用单独跨代提升解决 / Tied to GTK3/WebKit types and ABI, preventing an isolated major-version bump.

### 最终锁图审查 / Final dependency graph audit

对上述 SHA256 的锁文件查询 **1513 个 registry 包版本**，**17 个包版本**匹配通告；19 个原始通告 ID 经别名合并为 **18 条通告：6 条安全或健全性通告、12 条停止维护通告**。批量查询及详情读取均无错误。此范围包含构建、开发、可选和跨平台依赖，尚未逐项评估生产可利用性。
The final lockfile's **1513 registry package versions** produced **17 flagged versions**. Alias deduplication merged 19 raw IDs into **18 advisories: six security or soundness entries and 12 discontinued-maintenance entries**. Queries and detail retrieval completed without errors. This covers build, development, optional, and platform dependencies, rather than a production exploitability assessment.

| Remaining security/soundness family / 剩余安全或健全性依赖 | Constraint / 约束 |
|---|---|
| `glib 0.18.5` — RUSTSEC-2024-0429 | GTK3/WebKit native type compatibility / GTK3 与 WebKit 原生类型配套 |
| `bitmaps 3.2.1`, `imbl-sized-chunks 0.1.3` — RUSTSEC-2025-0167, RUSTSEC-2026-0292 | `matrix-sdk 0.18 → imbl 6.1` dependency chain / Matrix SDK 内部集合库依赖链 |
| `zip 2.6.1`, `zip 4.6.1` — RUSTSEC-2026-0367, RUSTSEC-2026-0368, RUSTSEC-2026-0369 | ICU build tooling and Tauri updater retain these branches; direct Aleph ZIP callers use 9.0.5 / ICU 构建工具和 Tauri updater 保留旧分支，Aleph 直接调用已使用 9.0.5 |

本地原始结果位于 `target/upgrade-validation/osv-final.json` 与 `osv-final-classified.json`。剩余维护通告包括 bincode、YAML、宏辅助库及 D-Bus 绑定；本次没有通过新增依赖 fork 来隐藏这些约束。
Local raw results are stored under `target/upgrade-validation/`. Maintenance findings include bincode, YAML, macro helpers, and D-Bus bindings. These constraints remain explicit without introducing new dependency forks.

## SQLite 与 TLS 兼容性 / SQLite and TLS compatibility

已核查的无自建 fork 兼容组合是 `matrix-sdk 0.18.0`、WhatsApp 库族 `0.5.0` 和 `rusqlite 0.37.x`，共同使用 `libsqlite3-sys 0.35`。Matrix 0.19.1 要求 `libsqlite3-sys 0.38`，WhatsApp 0.7 要求 `0.37`，而一个进程不能链接两个声明 `links = "sqlite3"` 的 crate 版本。单独提升根 `rusqlite` 无法消除这个冲突。当前声明已迁移至 Matrix 0.18，临时数据库迁移验证、统一编译和 release 构建均已通过。

The verified dependency-compatible combination without a new fork is Matrix SDK 0.18.0, the WhatsApp 0.5.0 family, and rusqlite 0.37.x, all sharing libsqlite3-sys 0.35. Matrix 0.19.1 requires the 0.38 line and WhatsApp 0.7 the 0.37 line; Cargo cannot link both versions with `links = "sqlite3"`. Updating only the root rusqlite dependency cannot resolve this conflict. The declarations now use Matrix 0.18; the generated database-fixture check, combined compilation, and release build all passed.

真实数据库验证使用升级前锁定的 Git revision `3b511d3e01e95469266dc8acce21771c5e9ccaa1`（Matrix 0.16.0 fork）写入带类型的同步令牌和过滤器。Matrix 0.18 随后成功读取、更新并重开同一临时 SQLite 数据库，退出码为 0。测试只使用生成的数据，没有打开用户的 Matrix 存储。此结果覆盖测试中的状态数据，不代表所有历史账户与加密状态都已迁移验证。

The database fixture used the exact previously locked Git revision, `3b511d3e01e95469266dc8acce21771c5e9ccaa1` (the Matrix 0.16.0 fork), to write typed sync-token and filter data. Matrix 0.18 read and updated the values and successfully reopened the same temporary SQLite database, with exit code 0. The test used generated data without opening any user's Matrix store. It verifies the tested state data rather than every historical account or encryption-state migration.

Matrix 0.18 移除了自己的 `native-tls` feature，但其原生 HTTP builder 使用 `reqwest::Client::builder()`，没有强制选择 Rustls。本项目直接启用 `reqwest 0.13` 的 `native-tls` feature；在未启用 HTTP/3 时，Reqwest 0.13.5 的默认 backend 优先选择 Native TLS。因此可以保留系统证书和代理路径，并继续使用 SDK 自己的超时与 user-agent 设置。此结论来自已发布源代码；不等同于已完成真实 Matrix 服务器握手验证。

Matrix 0.18 removed its own native-tls feature, but its native HTTP builder uses reqwest's normal client builder without forcing Rustls. Aleph directly enables reqwest 0.13's native-tls feature; Reqwest 0.13.5 chooses Native TLS by default when that feature is enabled and HTTP/3 is disabled. This preserves the system-certificate and proxy path while retaining the SDK's timeout and user-agent settings. This conclusion follows from the published source, rather than a live Matrix-server handshake test.

- [Matrix SDK 0.18 dependencies](https://crates.io/api/v1/crates/matrix-sdk/0.18.0/dependencies)
- [Matrix SQLite 0.18 dependencies](https://crates.io/api/v1/crates/matrix-sdk-sqlite/0.18.0/dependencies)
- [Matrix SQLite 0.19.1 dependencies](https://crates.io/api/v1/crates/matrix-sdk-sqlite/0.19.1/dependencies)
- [WhatsApp SQLite 0.7 dependencies](https://crates.io/api/v1/crates/whatsapp-rust-sqlite-storage/0.7.0/dependencies)

## 历史密文字节兼容性 / Historical ciphertext compatibility

临时探针以 `250e4800a` 的实际 `src/secrets/crypto.rs` 为旧实现，仅改三处 crate 导入名，并直接包含当前实际源文件作为新实现。旧密码学依赖固定为 AES-GCM 0.10.3、HKDF 0.12.4、SHA2 0.10.9；新版本为 0.11.1、0.13.0、0.11.1。8 组空值、1/16/17 字节、Unicode、NUL 和长字符串均完成旧写新读、新写旧读；32 次篡改密文与 32 次错误主密钥检查全部返回拒绝，退出码 0。没有读取真实 vault 或凭据。
The fixture uses the actual pre-upgrade crypto module with only three import aliases, and directly includes the live upgraded module. All eight synthetic boundary cases passed both directions, with 32 tampered-data and 32 wrong-master cases rejected. The old cryptographic trio is pinned to the original lockfile versions. No real vault or credential data is read.

此验证只证明测试中的密文字节与拒绝行为；局部错误枚举只包含实际模块所需的三个变体，错误显示文本不在证明范围内。RNG 和 zeroize 使用双方声明兼容的当前 patch，因此不是完整旧锁文件的重建。原始源码与导入替换关系经过脚本逐字节核对；结果记录于 `target/upgrade-validation/crypto-probe.json` 和 `.log`。
This establishes the tested byte compatibility and rejection behavior, using a local minimal error enum rather than verifying error-display text. Compatible current RNG and zeroize patches are shared, so it is not a reconstruction of the entire old dependency graph. A provenance script verifies the original source bytes and the import-only transformation; results are stored under `target/upgrade-validation/`.

## 原生凭据兼容性 / Native credential compatibility

Linux Secret Service 的真实后端验证已通过，退出码为 0。探针使用 `DeviceCredentialStore::qa_service_for_nonce` 生成随机隔离服务名，先确认条目不存在，再由旧 Keyring 3.6 写入版本 1 的测试 JSON、新 4.2 读取；反向的新写旧读也成功。最后分别验证删除效果，最终输出 `old_new_roundtrip=true`、`new_old_roundtrip=true`、`clean=true`。所有值均为无实际权限的合成凭据，没有读取或修改用户的生产条目。此结果覆盖本机后端及该负载格式，不代替真实配对票兑换、外部 Gateway 认证或其他操作系统验证。

The real Linux Secret Service backend passed the compatibility fixture with exit code 0. A random service generated by `DeviceCredentialStore::qa_service_for_nonce` isolated the test. After confirming the entry was absent, Keyring 3.6 wrote a synthetic version-1 JSON payload that 4.2 read successfully; the reverse new-write/old-read roundtrip also passed. Deletion was checked from both sides, ending with `old_new_roundtrip=true`, `new_old_roundtrip=true`, and `clean=true`. No production entries were read or modified. This covers the local backend and tested payload format rather than live pairing-ticket exchange, external Gateway authentication, or other operating systems.

## 桌面原生绑定 / Desktop native bindings

实际锁文件中的 Wry 0.57.0 使用 WebView2 0.39.1 和 `windows-core 0.62.2`，Shell 的 Windows 直接依赖已同步为 `webview2-com = "=0.39.1"`、`windows-core = "0.62"`，使 COM 类型来自同一代绑定。macOS 在 Wry 与 Window Vibrancy 都接受的范围内统一为 `objc2 0.6.5`、Foundation 0.3.2、Block2 0.6.2；Linux WebKitGTK 绑定保持 2.0.2。原生版本按真实依赖区间选择，原有锁定版本不作为补丁升级上限。

The resolved Wry 0.57.0 uses WebView2 0.39.1 and windows-core 0.62.2. The Shell's Windows dependencies now follow that ABI with `webview2-com = "=0.39.1"` and `windows-core = "0.62"`. macOS uses the mutually accepted objc2 0.6.5, Foundation 0.3.2, and Block2 0.6.2; Linux WebKitGTK bindings remain at 2.0.2. Native patch versions follow actual dependency ranges rather than treating an older lockfile as a version ceiling.

桌面共享库最初在真实 Wayland 会话中运行，有四项截图／剪贴板测试等待外部响应超过 60 秒，故终止了该轮。其中既有剪贴板测试实际启动了本机 `wl-copy` 并传入合成文本 `hello`，可能覆盖当时的剪贴板；没有读取或恢复原内容。随后为单测提供独立的临时 runtime 目录、不可连接的显示和 D-Bus 地址，并加 120 秒进程限时；全部 284 项通过。此结果覆盖纯逻辑和无显示时的错误分支，不代表真实截图／剪贴板往返已验证。没有为了通过测试修改生产权限判断或将这些测试标为忽略。
The initial desktop unit-test run waited more than 60 seconds in four screenshot/clipboard cases against the real Wayland session and was interrupted. An existing clipboard test launched the installed wl-copy with the synthetic text hello, potentially replacing the clipboard; its previous contents were neither read nor restored. A rerun used a private temporary runtime directory, unreachable display and D-Bus endpoints, and a 120-second process limit; all 284 tests passed. This verifies pure logic and no-display error paths rather than a real screenshot/clipboard roundtrip. Production authorization was preserved, and no tests were newly ignored.

## 其他兼容性约束 / Other compatibility constraints

- Linux Shell 限定 `rust-ini = "=0.21.1"`：当前稳定版 `tauri-plugin-deep-link 2.6.2` 构造两字段 `ParseOption`，0.21.3 新增必填字段会令上游插件编译失败。0.21.1 是已发布的最高兼容版本，待插件支持新 API 后解除此约束。 / Linux Shell pins the highest compatible rust-ini release until the stable deep-link plugin supports the additional ParseOption fields.

- `reqwest_legacy 0.12` 为 `reqwest-eventsource 0.6` 和 Teloxide 0.17 提供类型兼容；主 HTTP 调用链使用 0.13，并保留 JSON、表单、query、字符集、HTTP/2、代理和 Native TLS 能力。 / The compatibility alias preserves SSE and Teloxide types while the primary HTTP path uses 0.13 with its existing capabilities.
- `textwrap 0.16.2` 保留 Unicode 换行。0.16.3/0.16.4 的 `unicode-linebreak` 需要 ICU Segmenter `^2.3.0`，而 Leptos i18n 构建链的 ICU 2.2 固定 `~2.2.0`；真实精准更新尝试已确认冲突。 / Retain Unicode wrapping at 0.16.2 because the newer default feature conflicts with the ICU 2.2 build chain.
- `bincode 1.3` 的持久化格式保留。 / Preserve the existing bincode 1.3 persistence format.
- `psm 0.1.24` 由 Pest 2.9.4 精确要求；旧构建链的 `toml 0.8.2`、`toml_edit 0.20.2` 和 `toml_datetime 0.6.3` 由 `proc-macro-crate 2.0.2` 的精确约束共同决定。根配置解析器已迁移至 TOML 1.1。 / These older transitive branches follow exact upstream pins; the root configuration parser uses TOML 1.1.

最终 release 二进制已在独立临时 HOME、随机 loopback 端口和禁用外部 provider/channel 的配置中启动；`/ready` 与 `/health` 均返回 200，服务返回的 `index.html`、JS 和 WASM 与最终 dist 逐字节一致，且生成、服务和 Node 进程均已清理。Playwright 使用真实 Chromium headless 对这份 fresh server 完成 native welcome、合成共享 token 拒绝、输入清空、无秘密 acknowledgement、reload 抑制、普通浏览器抑制和无 page error 验证。native IPC 是测试替身，没有接触 OS keyring 或真实 WebKit。浏览器时间是单次本机合成样本，不是性能基准。

The final release binary was started with an isolated temporary HOME, random loopback ports, and configuration disabling external providers and channels. `/ready` and `/health` both returned 200; the served `index.html`, JS, and WASM matched the final dist byte-for-byte, and generation, server, and Node processes were cleaned up. Real headless Chromium then verified the native welcome, synthetic shared-token rejection, input clearing, secret-free acknowledgement, reload suppression, plain-browser suppression, and absence of page errors against that fresh server. Native IPC was a test stub; OS keyring and native WebKit were not touched. Browser timings are single local synthetic samples, not a performance benchmark.

## 验证边界 / Verification boundaries

本次环境为 Linux x86_64，系统已提供 GTK3、WebKitGTK 4.1、libsoup3、D-Bus、OpenSSL、X11、XCB 和 Wayland 开发库。macOS、Windows 以及 Ubuntu/Debian 实机结果须与本机编译结果分开记录。除非另有明确运行证据，不声称已部署、已验证真实凭据读写或取得性能增益。

The verification host is Linux x86_64 with GTK3, WebKitGTK 4.1, libsoup3, D-Bus, OpenSSL, X11, XCB, and Wayland development libraries. Record macOS, Windows, and Ubuntu/Debian machine validation separately. Do not claim deployment, real credential-store validation, or performance improvements without corresponding evidence.
