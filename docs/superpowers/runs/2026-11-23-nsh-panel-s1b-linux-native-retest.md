# NSH Panel S1-B — Linux 原生复验 / Linux native retest

> **证据层 / Evidence snapshot (2026-11-23).** 本回执补齐[首次 Linux 验证](2026-11-23-nsh-panel-s1b-linux-native-verification.md)的阻塞项；不替代其他平台证据。 / This retest closes the outstanding Linux gates documented in the first receipt; it is not evidence for other platforms.

**结果 / Result: PASS.** NSH 独立构建、真实 Gateway + NSH 双进程探针、独立异 UID 检查均通过。 / Standalone NSH build, real Gateway + NSH smoke, and separate-UID checks all passed.

## 基线与修复 / Baseline and fix

- 修复提交 / Fix commit: `e7e351f66778bdf8926dd2a3a874c9f465c8cb84`; tree `e0f41491c06862fe31e7293cc980e77d2e89ad55`. 构建及测试时的内容与该提交相同；相对于上次源码，只改了 `desktop/nsh-shell/src/main.rs`。 / The tested sources match this commit. The only product diff is in the NSH host entry point.
- `Cargo.lock` SHA-256: `a608c2a5187db760c47458f0f0e8d7d9efd599aa1ffa941436cf7d7485466083`; NSH remains pinned to `=0.6.0`.
- 同一 Arch Linux rolling 原生 QEMU VM，kernel `7.2.9-arch1-1`，`x86_64-unknown-linux-gnu`；Rust `1.99.1`、Cargo `1.99.1`、`libprotoc 36.2`、Python `3.14.9`、util-linux `2.42.4`。 / Same native Arch Linux VM and toolchain; only the alternate test peer entered a temporary Linux user namespace.
- 修复前用同一测试命令复现 `E0308`：Linux NSH 的 GTK 入口返回 GLib 退出码。修复在 Linux 宿主边界无损转换为 `std::process::ExitCode`，QA 关闭事件和 `main` 返回值使用同一标准类型。不改协议、权限规则、依赖或其他平台入口。 / On the unchanged baseline the same test command reproduced `E0308`. Linux now converts GLib's exit status once, preserving its numeric value for both the QA event and `main`.

## 执行记录 / Command results

以非 root 调用方运行，使用隔离 `HOME`、`ALEPH_HOME`，`CARGO_TARGET_DIR="$PWD/target"`、`CARGO_BUILD_JOBS=2`。两条 NSH Cargo 命令串行，每次先运行 `python3 qa/browser_live/check_memory.py`，分别在可用 `17,747,128,320` 和 `17,525,923,840` 字节时通过 `4 GiB` 闸门。 / The NSH commands ran serially under the non-root owner, with isolated homes and the repository's memory preflight check.

| 命令 / Command | 状态 / Result |
|---|---|
| `cargo test --locked -p aleph-nsh-shell`（未修复 / before fix） | FAIL · `101` · `E0308` at `desktop/nsh-shell/src/main.rs:297:57` and `:303:5` |
| `cargo test --locked -p aleph-nsh-shell`（修复后 / after fix） | PASS · `0` · 20 unit + 8 integration tests, 0 failed; 0 doctests |
| `cargo build --locked -p aleph-nsh-shell` | PASS · `0` · 新的 Linux NSH 可执行文件 / New Linux NSH executable |
| `python3 qa/nsh_desktop_ipc_smoke.py "$CARGO_TARGET_DIR/debug/aleph-server" "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence/native-server.log"` | PASS · `0` · 真实双进程 / Real binaries |
| `python3 "$evidence/mounted-lifecycle.py" "$CARGO_TARGET_DIR/debug/aleph-server" "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence/paired-lifecycle"` | PASS · `0` · 两次真实进程启动 / Two real launches |
| `python3 "$evidence/cross-uid.py" "$CARGO_TARGET_DIR/debug/aleph-server" "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence/cross-uid"` | PASS · `0` · 所有者正对照、文件系统拒绝及双向对端 euid 拒绝 / Positive owner control, filesystem refusal, and both peer-euid refusals |

Gateway 使用首次 Linux 回执中成功构建的同一 SHA-256 制品：修复仅触及 NSH，不是 Gateway 的构建输入。一次额外 Gateway 重建触发重编译并由执行器在 `130 s` 停止；没有替换原制品，也不计作一次成功构建。客户端、Gateway 和 UI ingress 的回归计数仍归首次回执，没有当作本轮结果重报。 / The already-built native Gateway artifact is byte-identical to the first receipt. An optional redundant rebuild timed out at the harness and did not replace it. Prior Gateway, client, and ingress regressions are not counted as fresh tests here.

## 运行观察 / Runtime observations

- **双进程 / Built binaries.** 已启动的真实 Gateway 通过 caller-owned `0700` 目录内的显式绝对 UDS 向 NSH 返回 `status=healthy` 和时间戳；NSH 探针退出后 Gateway 仍存活。缺端点、混用 `--connect` 和 QA 参数均拒绝；停服后同一路径失败关闭。 / With the actual Gateway already running, the real NSH status command received health and a timestamp via the private UDS and did not stop it. Missing endpoints, mixed modes and QA options were refused. The retired path failed closed. The host status path has no TCP/WS selection; this run did not trace syscalls.
- **退场 / Lifecycle.** 两次探针在 `127.0.0.1:57041`、`127.0.0.1:57887` 启动；健康时间分别为 `2026-11-23T09:58:38.797269+00:00`、`2026-11-23T09:58:42.259367+00:00`。无效数组 ID 和普通 EOF 均到达清理；两次 SIGTERM 均退出 `0`，已接纳连接读到 EOF，原 socket inode 保留且拒连；第二次使用新绝对路径。滚动 trace 分别新增 4 条与 2 条 IPC 清理记录。 / Both launches returned timestamped health and exited cleanly on SIGTERM with an admitted stream. Invalid IDs and ordinary EOF reached cleanup; retired socket inodes survived, refused new connects, and the second launch used a fresh path.
- **异 UID / Different UID.** 调用方未使用 sudo；系统现有 subordinate UID 映射只用于独立测试子进程，没有新建账号、改目录权限或改系统配置。普通异 UID 访问私有端点收到 `EACCES`。为了单独验证 IPC 的对端检查，探针只在临时子进程中控制测试夹具的文件系统身份，保留不同的内核 euid。异 UID 能建立传输但拿不到真实 Gateway 的 hello 响应；同 UID 的真实 NSH 随后仍获健康响应（Gateway 监听 `127.0.0.1:51597`）。反向检查中，实际 NSH 拨到 inode 所属者正确、但 peer euid 不同的拒绝专用 UDS listener，退出 `1`，对端看到 EOF、零 hello 字节。 / No sudo, new account or permission change was used. The ordinary alternate identity hit `EACCES`; a separate fixture reached the peer check without changing the private directory. The actual Gateway returned no hello to the mismatched peer and remained healthy for the owner. The actual NSH exited `1` against the mismatched listener, which read EOF without hello bytes. That listener was only a negative control, not a substitute for the real Gateway smoke.

## 制品与限界 / Artifacts and limits

| 制品 / Artifact | SHA-256 |
|---|---|
| `target/debug/aleph-server` | `de9dd5075807c724fb249b1d5533aee022f2f5380a449e4acd44b25aba6c7b1e` |
| `target/debug/aleph-nsh-shell` | `768f4eb750501bd0332c13d58b20341e554149a31ea7af49e14f6fa8ccca1dba` |
| `sanitized-evidence.txt` | `0304a12b1b2aa5f711f2bc927e05090577b5963c855f7345a500f279a158ad04` |
| `mounted-lifecycle.py`（私有补充夹具 / private fixture） | `90e4c63833cb40f23151bc5d25c3ae4438ad639ddcd47aee9baa70347d27309a` |
| `cross-uid.py`（私有补充夹具 / private fixture） | `862e10d66da4587590de4c7740bd8e9402cda9452aa907f793b2258a79be0c97` |

原始日志和临时脚本留在本机 `0700` 证据目录。NSH 构建仍报告一条既有的 Linux `unused_mut` 警告（macOS QA 分支才修改该变量），不是编译错误。未运行全工作区测试、NSH GUI、打包、其他平台或切换门禁；不改变 `main`，不推送、不合并。 / Raw logs and temporary fixtures remain private. An existing Linux `unused_mut` warning remains in the macOS-specific QA event path. No workspace-wide test run, GUI, packaging, other-platform validation, or cutover was performed; `main` was not changed, pushed, or merged.
