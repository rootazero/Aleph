# Memory lifecycle expansion — validation and design record / Memory 生命周期扩展验证与设计记录

**Status / 状态: Validation attempted; lifecycle design approved; implementation plan awaiting review / 已执行扩展验证，生命周期设计已批准，实施计划待审阅。**

## Authorization and baseline / 授权与基线

On 2026-11-01 the user requested actual signal/live-server E2E, workspace/clippy/cross-platform verification and expansion into early-startup-error cleanup and Memory stop. The user then selected Memory stop/re-enable while chat and raw session archival continue, rejecting new Memory work and draining admitted work. / 用户要求扩展验证与启动失败清理、Memory 启停，并选择关闭 Memory 时保留聊天和 raw 会话归档，新工作拒绝、旧工作排空。

Baseline: local `note-maintenance-drain`, `6be5799967013d069394e0da41c93afbe6f912fd`; one checkout, no production edits in this expansion. Local `main`/`origin/main` remain at `c732c2d575d508834cc36090c3f97a89af436ed2`. No push or merge. / 本轮只验证和编写设计，尚无新生产改动，不推送或合并。

Design: [Memory runtime lifecycle](../specs/2026-11-01-memory-runtime-lifecycle-design.md). The explicitly invoked `brainstorming` skill requires design approval before implementation. / 用户显式调用的 brainstorming 要求先批准设计，再进入实施计划和生产改动。

## Execution environment / 执行环境

- macOS / Darwin arm64; Rust 1.99.1. Installed Rust targets include host, `x86_64-unknown-linux-gnu`, `wasm32-unknown-unknown`. Installed target support does not prove a working foreign native linker/sysroot. / 已安装 target 不等于具备完整交叉链接环境。
- Shell placeholders staged using `just _stage-shell-placeholders` before workspace checking. / 使用仓库 recipe，不手工构造平台文件名。
- Every Cargo invocation is preceded by `python qa/browser_live/check_memory.py`, with `ALLOW` and at least 4 GiB available; `CARGO_BUILD_JOBS=1`, serial Cargo. All six expansion Cargo invocations passed the gate; minimum observed availability was 7,206,649,856 bytes (required 4,294,967,296). / 六条 Cargo 均通过逐条内存门，最低约 6.71 GiB，jobs=1 串行。
- Detailed local artifacts: ignored `.superpowers/sdd/2026-11-01-memory-lifecycle-expansion/` (logs, exit files, exact commands, memory gates, pinned server hash and disposable homes). / 详细日志与临时 fixture 位于忽略目录，不提交二进制或临时数据库。

## Real signal/live-server E2E / 真实信号与服务 E2E

The final probe copied and pinned the freshly compiled workspace server before launching it. `cargo test --workspace --features alephcore/test-helpers` had overwritten the original `cargo build` output, so the final tested artifact explicitly has `test-helpers` enabled. / 最终测试固定了 workspace 构建的产物；不能把它误记为之前无 test-helpers 的 build 产物。

Pinned SHA-256: `d2ad74cc2a0ce48a4462663bb8fbe2695a026057344d61f1d5bd3fb4dfb5bef1`.

**Final result: 27 / 27 assertions passed. / 最终 27 项断言全部通过。**

- SIGTERM: actual child exited 0 in approximately 0.143 s. / 真实子进程约 0.143 秒正常退出。
- Restart on the same disposable data, then SIGINT: exited 0 in approximately 0.256 s. / 同一临时数据重启后 SIGINT 约 0.256 秒正常退出。
- Both boots served real HTTP health and WebSocket `tools.invoke` / `note_manage` writes, produced Markdown notes and orientation files, published then removed discovery endpoints, and closed listeners. SQLite `integrity_check` returned `ok`; note rows persisted across restart (one, then two). / 生产 HTTP/WS/工具路径执行真实写入，验证 Markdown、orientation、发现文件、端口、数据库完整性与重启后行数。
- Actual tracing files contained the signal path; no watchdog force-exit indication. Exit 0 alone was not used as proof. / 同时检查 tracing 信号路径和 watchdog，未仅凭 exit 0 判定。
- Temporary `HOME`/`ALEPH_HOME`, loopback-only ports, no usable external provider credential. `RUSTUP_HOME`/`CARGO_HOME` pinned to the existing toolchain. Only the exact children started by the harness received signals. / 临时配置与本地端口，不触碰已安装 daemon 或用户数据库。

Failed harness attempts are preserved: initial scratch HOME omitted toolchain-home pins and both processes required the 15-second external kill guard; it also inspected stdout instead of tracing files. After those corrections, signal behavior passed but the binary fingerprint failed because concurrent workspace compilation had replaced the binary. The final pinned-artifact probe corrected that provenance error. No root-cause claim is made for the initial process delay. / 保留两轮不完整验证，分别修正隔离环境、日志读取与产物指纹；不把首轮延迟推断成已定位的生产缺陷。

This validates real process shutdown with this fixture. It does not prove a pending maintenance drain, all Memory workers stopping, early startup cleanup, global runtime off/on or native resource/RSS reclamation. Deterministic lifecycle evidence remains in the previous [bounded implementation run](2026-11-01-note-maintenance-drain-run.md). / 此 E2E 不替代 pending 工作测试，也不证明尚未实施的全局 Memory 开关。

## Workspace and cross-target checks / Workspace 与交叉目标检查

Commands and final outcomes are recorded below after each command exits. / 下方只记录真实退出的命令结果。

- `cargo metadata --format-version 1 --locked`: exit 0; 18 workspace members. / 成功，18 个 workspace 成员。
- `cargo build -p alephcore --bin aleph-server`: exit 0, approximately 252.3 s. / 构建成功。
- `cargo test --workspace --features alephcore/test-helpers --no-fail-fast -- --test-threads=1`: interrupted (exit `-2`) after 2,662.5 s, not a passing full suite. The root library target stalled in `approval::adapters::tests::falls_back_to_operator_when_channel_missing`; after interrupting that target, other targets continued and later stalled in `parked_gate_integration`. There were 32 visible failed tests. Completed test processes reported **4,399 passed / 9 failed / 45 ignored**; those summaries exclude the interrupted root library's 23 visible failures and its unreported final total. Do not add the visible failures to these totals as though the full suite completed. / 全量未完成，不能报通过；已结束进程的统计不包含被中断 root lib 的最终计数。
- `cargo clippy --workspace --all-targets`: exit 101 after 117.5 s. `src/exec/secret_patterns.rs:138` uses unsupported negative lookahead with `regex::Regex`; `clippy::invalid_regex` is deny-by-default. The file is unchanged relative to local `main` (`c732c2d575d508834cc36090c3f97a89af436ed2`). / clippy 因现有 regex 语法失败，不是单纯风格警告。
- Directly reran the already-built root test binary with exact filter `agents::background_persistence::tests::a_settled_run_stays_addressable_in_the_same_process`: **0 passed / 1 failed / 21,738 filtered**, exit 101 in approximately 0.26 s. Panic at `src/exec/secret_patterns.rs:138:127`: `look-around, including look-ahead and look-behind, is not supported`. This confirms one observed workspace failure shares the clippy blocker; it does not explain every failed/stalled test. No Cargo/rebuild was performed for this diagnostic. / 直接重跑已构建 binary，确认一个失败同源，不推断全部失败同源。
- `cargo check -p aleph-desktop-linux --target x86_64-unknown-linux-gnu`: exit 101 after 29.9 s. `wayland-sys` build script: `pkg-config has not been configured to support cross-compilation`; target sysroot/pkg-config wiring missing. / Linux 交叉检查被环境配置阻塞，不宣称已完成 Linux 验证。
- `cargo check -p aleph-panel --lib --target wasm32-unknown-unknown`: **exit 0**, approximately 124 s. One unused-import warning for `t_string` in `interfaces/webchat/src/app.rs`. This is a WASM type/build check, not browser execution or the complete Panel factory artifact. / WASM 检查通过，仅有一个 unused import 警告；不等同于浏览器执行或完整工厂构建。

Existing compiler warning observed: unread `PluginProjection.panel_snapshot` (`src/extension/projection.rs`). Existing macOS linker unwind warning also appeared. / 记录既有警告，不在本轮验证中顺带修改。

## Resource census and design check / 资源盘点与设计审查

Source census found separate Memory and raw connections to the same SQLite file, direct backend `Arc` captures, compression/Dream/producer/re-embedding detached work, and early fallible startup paths before the maintenance owner is assembled. / 盘点发现共享文件但不同连接、直接 Arc 捕获、多个 detached worker，以及 owner 构造前的启动失败出口。

One native Pi researcher returned a useful source report before SIGTERM. The separate QA researcher timed out after six minutes without a substantive result; the main agent completed the signal fixture investigation directly. These process outcomes are not represented as successful independent QA reviews. / 如实区分可用源码报告与超时无结果的 QA 调研。

A fresh native Pi builder check returned two planning blockers before SIGTERM: (1) Tool-to-Memory Resource lease association and public lease/host changes; (2) retry semantics after cleanup failure. The draft proposed captured dependency leases and retryable cleanup with retained historical error. The user subsequently confirmed the written design on 2026-11-01. The [implementation plan](../plans/2026-11-01-memory-runtime-lifecycle-plan.md) now records the native execution tasks and awaits plan review; no production code changed during planning. / 两个阻塞已转为明确契约，用户随后于 2026-11-01 确认设计；现已编写实施计划待审阅，本阶段仍未改生产代码。

## Not implemented or proven / 未实施与未证明

No new early-startup cleanup, closeable Memory connection, Resource backend, public lease extension, worker ownership migration or runtime stop/re-enable has been implemented. No claim that raw persistence closes while chat continues. / 尚未实施新生命周期生产改动，不宣称 raw 在聊天继续时关闭。

No Linux or Windows native execution, full production Panel factory build, global Memory coldness, external-effect exactly-once, hard stop deadline or RSS result is claimed. The regex blocker was source-compared to the local pre-expansion baseline, but the remaining workspace failures have not been baseline-reproduced or root-caused. The unrelated regex implementation was not changed during this Memory validation/design task. / 不宣称跨平台真机、完整 Panel 工厂产物、全局冷态、硬期限或 RSS。regex 已核对基线源码，其他失败尚未基线复现或定位；本轮未修改无关的 regex 实现。
