# NSH Panel S1-B — Linux 原生验证回执 / Linux native verification receipt

> **证据层 / Evidence snapshot (2026-11-23).** 本回执只记录下列源码树的 Linux 分项，不是当前架构说明，也不替代其他平台证据。 / This receipt covers the Linux gate for the tested tree only; it is not a current architecture reference or evidence for other platforms.

**Linux 标准门禁 / Required Linux gates:** **FAIL** — `aleph-nsh-shell` 在 Linux 编译失败，退出 `101`；实测 Gateway 路径通过，但双二进制门禁 **NOT RUN**。 / The NSH host failed to compile on Linux (exit `101`). The real Gateway path passed; the dual-binary gate was not run.

## 实测基线 / Tested baseline

| 项目 / Item | 实测值 / Observed value |
|---|---|
| Git commit / 源码树 | `8c02bb13582d8535b67f04075f58c8a80e68f58b` / `0d43587d22883bc1d31402da51ad8d379738d7ab` |
| S1-B 祖先 / Ancestor, tree | `git merge-base --is-ancestor 06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43 HEAD` → `0`; ancestor tree `7f13748409ee1664a43c7dab78cdb4d5f1b08214` |
| Worktree before run / 开跑前 | `git status --short --branch` → `## s1b-linux-native`, no changes / 无改动 |
| `Cargo.lock` SHA-256 | `a608c2a5187db760c47458f0f0e8d7d9efd599aa1ffa941436cf7d7485466083` |
| Platform / 平台 | Arch Linux rolling, native QEMU VM (not a container or WSL / 非容器或 WSL), kernel `7.2.9-arch1-1`, `x86_64 GNU/Linux` |
| Toolchain / 工具链 | `x86_64-unknown-linux-gnu`; `rustc 1.99.1 (58cf8f9c3 2026-10-13)` / LLVM `23.1.1`; `cargo 1.99.1 (5f94df478 2026-08-27)`; `libprotoc 36.2`; Python `3.14.9` |
| Native dependencies / 本机依赖 | NSH pinned at `=0.6.0`; GTK4 `4.24.1`, WebKitGTK 6.0 and JavaScriptCoreGTK 6.0 `2.54.2`, PipeWire `1.6.10`, xkbcommon-x11 `1.13.2` |
| Resources / 资源 | RAM `19 GiB`, swap `24 GiB`; no parallel Cargo / 未并行运行 Cargo; `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR="$PWD/target"` |

已核对锁定子模块：`plugins` `b35ad47fe8eec3196b2dee70e51eadeacf3a620c`、`skills` `4f4cc618fdc20b5854de62c8d917abb4d05f6ed4`。使用非 root 用户、私有临时 HOME/ALEPH_HOME 和 `0700` socket 父目录，仅操作自启测试进程。没有触碰已安装服务、默认端口、真实会话或凭据，也未安装系统依赖。 / Pinned submodules were verified. The non-root fixture used isolated homes and caller-owned `0700` directories. No installed service, default port, real session, credentials, or system package changes were involved.

## 执行记录 / Command results

按 Linux 交接顺序运行；每条 Cargo 命令前先执行 `python3 qa/browser_live/check_memory.py`。 / Cargo ran serially, with the repository's `4 GiB` preflight check before each invocation. 8 checks returned `ALLOW`: `17,589,325,824`–`18,371,833,856` bytes available (threshold `4,294,967,296`).

| 命令 / Command | 结果 / Exit and observation |
|---|---|
| `cargo test --locked -p aleph-client` | PASS · `0` · 68 passed, 0 failed; 0 doctests |
| `cargo clippy --locked -p aleph-client --all-targets -- -D warnings` | PASS · `0` |
| `cargo check --locked -p aleph-cli -p aleph-tui` | PASS · `0` |
| `cargo test --locked -p alephcore --lib gateway::server:: -- --test-threads=1` | PASS · `0` · 240 passed, 0 failed, 1 ignored, 22,075 filtered out |
| `cargo test --locked -p alephcore --test gateway_ui_ingress` | PASS · `0` · 1 passed, 0 failed |
| `cargo test --locked -p alephcore --bin aleph-server desktop_ipc_is_explicitly_opt_in` | PASS · `0` · 1 passed, 0 failed, 147 filtered out |
| `cargo build --locked -p alephcore --bin aleph-server` | PASS · `0` · native x86_64 ELF produced |
| `cargo test --locked -p aleph-nsh-shell` | FAIL · `101` · compile error `E0308` at `desktop/nsh-shell/src/main.rs:297:57` and `:303:5`; 0 host tests executed |
| `cargo build --locked -p aleph-nsh-shell` | NOT RUN · stopped at first required failure / 标准序列遇首个失败后停止 |
| `python3 qa/nsh_desktop_ipc_smoke.py "$CARGO_TARGET_DIR/debug/aleph-server" "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence/native-server.log"` | NOT RUN · no NSH executable / 未产出 NSH 二进制 |
| `python3 "$evidence/mounted-lifecycle.py" "$CARGO_TARGET_DIR/debug/aleph-server" "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence"` | NOT RUN · no NSH executable / 未产出 NSH 二进制 |
| `python3 "$evidence/mounted-lifecycle.py" "$CARGO_TARGET_DIR/debug/aleph-server" - "$evidence"` (Gateway-only, first attempt / 仅 Gateway，首次) | FAIL · `1` · supplemental probe looked for tracing in redirected stdout, where the non-TTY startup omits it / 探针误在 stdout 查找 tracing |
| `python3 "$evidence/mounted-lifecycle.py" "$CARGO_TARGET_DIR/debug/aleph-server" - "$evidence/gateway-only-replay"` (Gateway-only, rotated tracing / 仅 Gateway，读取滚动日志) | PASS · `0` · two actual process launches, four admitted streams cleaned up / 两次进程启动，四条已接纳连接清理 |

首个门禁失败可用上表的 NSH 测试命令在同一锁定基线复跑：`run` 返回 `nsh_platform_linux::gtk4::glib::ExitCode`；第 297 行拿它与 `std::process::ExitCode::SUCCESS` 比较，第 303 行把它作为 `main` 的 `std::process::ExitCode` 返回。两处均为 `error[E0308]: mismatched types`。未修改产品或依赖。 / Reproduce with the listed NSH test command on the pinned tree: Linux `run` returns GLib's `ExitCode`, but the comparison at line 297 and `main`'s return at line 303 require `std::process::ExitCode`. Neither the product nor dependencies changed.

Gateway 跳过了已标记 `ignored` 的浏览器测试 `web_socket_requires_hello_and_keeps_member_scope`（需要本地 Chrome 和工作区 Playwright）；未将其计为通过。 / The suite's browser-only test remained ignored; it did not count as a pass.

## 验收观察 / Gate observations

| 验收项 / Gate | 结论与观察 / Status and observations |
|---|---|
| 私有端点与不可覆盖 / Private endpoint, no overwrite | **PASS.** `desktop_ipc_leaves_occupied_and_published_entries_untouched` 拒绝不够私有的父目录与占用的绝对路径，不修改旧目录项或替换项，能另用新路径。检查的是父目录 `0700`，不是 socket inode `0600`。 / The listener rejects unsafe parents and occupied names, leaves existing and replacement entries untouched, and accepts a fresh path. |
| 同 euid、首帧、作用域 / Same euid, first frame, owner scope | **PASS.** 两端走内核同 euid 判据；Gateway 只映射活跃 Core owner。`desktop_ipc_uses_gateway_and_releases_its_connection` 验证缺失或错误 `system.hello` 被拒，停用 owner 后拒绝已接纳连接继续读取；真实 Core `sessions.list` 只返回当前 owner 的夹具会话，不授予 legacy loopback 或配对设备身份。 / The Linux client and Gateway use kernel peer credentials; the Gateway maps only the active Core owner. The real Gateway test rejects missing/wrong hello and denies further reads after owner deactivation. The real Core session store returns only the current owner's fixture rows, without legacy-loopback or paired-device identity. 仅 Gateway 补充探针还从 `SO_PEERCRED` 核对了启动子进程和同 euid。 / The Gateway-only replay additionally verified the child process and same euid via `SO_PEERCRED`. |
| 帧、并发及连接表 / Framing, concurrency, connection rows | **PASS.** 客户端 `frames_split_and_coalesce`、`frames_refuse_invalid_before_body`、`json_shape_limits` 通过。`local_client_uses_hello_and_correlates_out_of_order` 验证乱序关联和两笔已发送请求超时后仍占用槽位，直到回包或连接退休。Gateway 验证乱序、重复活跃 ID 的拒绝，以及取消后共享连接表清理（`desktop_ipc_cleans_up_when_server_future_is_cancelled`）。 / Split/coalesced frames, bounds, invalid UTF-8 and JSON shapes are covered; timed-out sent calls retain their slots until a correlated reply or transport teardown. Native Gateway tests cover out-of-order replies, duplicate active IDs, and shared row removal on cancellation. 真实 Gateway 补充探针验证数组类型 ID 被拒绝、普通对端 EOF 触发清理。 / The standalone Gateway closed an admitted stream on an array ID and logged cleanup after normal client EOF. |
| 真实进程、启动互斥及无 fallback / Built binaries, startup exclusion, no fallback | **NOT RUN.** NSH 构建阻塞；未实测 `--desktop-ipc-status` 的健康时间戳、探针后进程存活，以及缺端点、混合、QA 参数拒绝和 Host 无 TCP/WS fallback。客户端本机测试仍验证本地连接拒绝 WebSocket reconnect；不替代 Host 进程证据。 / Host health, timestamp and survival, option rejection, and no host fallback remain unverified at the executable boundary. Native client tests reject WebSocket reconnect on the local client; this does not substitute for the host smoke. |
| 真实进程退场及换路径 / Live-process shutdown and fresh path | **PASS (Gateway only / 仅 Gateway).** 两次 SIGTERM 都在已接纳连接存在时退出 `0`；对端收到 EOF，Gateway 写出对应清理记录，保持原 socket inode，新的 connect 返回 `ECONNREFUSED`。重启使用另一绝对路径，两旧目录项都保留。 / Both actual Gateway processes exited `0` under SIGTERM with an admitted connection; each stream reached EOF, cleanup was logged, the same socket inode remained, and a new connect returned `ECONNREFUSED`. The restart used a fresh absolute path and left both retired entries intact. |
| 旧 `/ws` 回归 / Legacy `/ws` regression | **PASS.** `test_legacy_ws_first_frame_and_rpc_dispatch` 验证首帧仍为 `connect` 及 RPC 分发；`display_event_and_ack_progress_on_same_authenticated_connection` 验证同一已认证连接的 UI 事件与确认正常推进。 / The first frame is still `connect`, RPC dispatch works, and event/ack traffic progresses on the same authenticated connection. |
| 独立异 UID 对端 / Separate process under another UID | **NOT RUN.** 没有事先授权的隔离第二身份；未创建用户或改权限。 / No authorized second identity was available; no system users or permissions were modified. This is not implied by the same-euid pass. |

补充探针读取隔离 `ALEPH_HOME` 的滚动 tracing，确认 Gateway 实际监听 `127.0.0.1:37089`、`127.0.0.1:43355` 以及两个显式私有 UDS。第一次启动记录 3 条 IPC 清理（无效 ID、客户端正常 EOF、SIGTERM），第二次新增 1 条（SIGTERM）。临时目录由调用方在观察完成后清理。 / Rotated tracing records both non-default loopback listeners and the two private sockets. Launch one produced three IPC cleanup records (invalid ID, normal EOF, SIGTERM); launch two added one (SIGTERM). The caller cleaned up the temporary directory after observation.

## 制品与限界 / Artifacts and limits

| 制品 / Artifact | SHA-256 |
|---|---|
| `target/debug/aleph-server` | `de9dd5075807c724fb249b1d5533aee022f2f5380a449e4acd44b25aba6c7b1e` |
| `target/debug/aleph-nsh-shell` | — · no artifact / 无制品 |
| `sanitized-evidence.txt` | `58ac731d90f8e3b6420c0229c4d560548cd4ce1ac23b9a60bff3bcde5e0024ef` |
| `mounted-lifecycle.py` (local supplemental probe / 本地补充探针) | `90e4c63833cb40f23151bc5d25c3ae4438ad639ddcd47aee9baa70347d27309a` |

原始日志和临时探针留在本机 `0700` 证据目录；摘要只保留白名单中的执行状态、测试名、监听与清理观察和制品摘要。不提交原始日志、绝对路径、临时 profile 或凭据。 / The private evidence directory holds the raw logs and supplemental probe; only allowlisted execution results, test names, listener and cleanup observations, and artifact digests enter the evidence summary. No raw logs, machine-local paths, fixture profiles, or credentials are committed.

**未覆盖 / Not covered:** NSH GUI 与安装包、macOS 或 Windows（含 Named Pipe）、跨平台整体验收、S1-C、默认切换、发布、Tauri 退场。只提交本回执，不修改产品代码，不推送或合并。 / This run does not cover NSH GUI or packaging, macOS or Windows (including Named Pipe), cross-platform acceptance, S1-C, switching defaults, release, or Tauri retirement. The local receipt is the only checkout change; no push or merge.
