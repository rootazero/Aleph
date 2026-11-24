# NSH Panel S1-B — Linux 原生验证 Prompt / Linux native verification handoff

你在 **原生 Linux**（物理机或运行 Linux 内核的 VM，记录发行版、版本、内核和架构）上接手 Aleph NSH Panel 的 S1-B Linux 分项。**只复验已有的、显式启用的只读 Unix Domain Socket，不重写传输或迁移 UI。** macOS 的证据不能代替 Linux；交叉编译、容器单测、WSL 和假 Gateway 不算本任务的原生验收。无须启动桌面界面。

**English scope:** Validate the Linux UDS path implemented on this branch with built Gateway and NSH executables, isolated fixtures, and native regression tests. Report evidence and gaps; do not change the product or promote any other platform.

## 接手与边界 / Starting point and boundaries

1. 使用交接方的本地 `nsh-panel-refactor` 检出或 Git bundle。推荐从 `8c02bb13582d8535b67f04075f58c8a80e68f58b` 或其经确认的后继提交创建独立 worktree/分支，例如 `git worktree add -b s1b-linux-native ../aleph-s1b-linux <verified-ref>`。该 ref 必须包含 S1-B 代码 `06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43`（源码树 `7f13748409ee1664a43c7dab78cdb4d5f1b08214`）。用 `git merge-base --is-ancestor 06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43 HEAD` 验证，记录实际 `git rev-parse HEAD HEAD^{tree}` 和 `git status --short --branch`。不要假定远端 `main` 含本地提交，不 reset/rebase/覆盖已有改动。如果用较新的代码验证，回执绑定实测提交，不将旧 macOS 结果算作新树的实测。
2. 阅读仓库 `AGENTS.md`、[S1-B 计划](../plans/2026-11-23-nsh-panel-s1b-desktop-ipc.md)、[S1-B 原始回执](../runs/2026-11-23-nsh-panel-s1b-desktop-ipc-verification.md)、[Linux Desktop](../../reference/LINUX_DESKTOP.md) 和[验证规范](../../reference/DEVELOPMENT.md)。核对 `shared/client/src/local_ipc.rs`、`src/gateway/server/connection/local_ipc.rs`、`src/gateway/server/connection/local_ipc/tests.rs`、`desktop/nsh-shell/src/ipc_status.rs` 与 [`qa/nsh_desktop_ipc_smoke.py`](../../../qa/nsh_desktop_ipc_smoke.py)。以当前调用点和回执中的最终契约为准，不沿用早期 socket `0600`/自动 unlink 设想。
3. 用非 root 的隔离测试账户或可丢弃环境，独立 `ALEPH_HOME`，仅操作本次启动的进程和短路径私有目录。不访问已安装服务、生产配置/凭据、默认端口或用户真实会话；不安装服务、不变更自启或系统权限。记录工具链、`protoc`、Python、可用内存和目标三元组；沿用仓库锁定依赖，NSH 固定 `=0.6.0`。若系统开发包不足（如 NSH Linux 的 WebKitGTK/GTK 链接依赖），先记录缺失项，不静默升级依赖、降低门槛或安装系统软件。

## 执行 / Run

从已核对的 worktree 根运行。下列命令逐条执行，首次失败保留原日志和退出码。每次 Cargo 都先通过仓库内存闸门，默认最低 4 GiB（大链接建议预留 10 GiB），不并行启动额外 Cargo。普通 Gateway 测试用独立 `ALEPH_HOME`；最后的脚本另外创建隔离 `HOME`/`ALEPH_HOME`，运行传入的两个真实二进制并选择非默认 loopback 端口，不使用实际服务。用 Bash 运行：

```bash
set -euo pipefail
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="$PWD/target"
evidence=$(mktemp -d /tmp/aleph-s1b-linux.XXXXXX)
mkdir -m 700 "$evidence/profile"
export ALEPH_HOME="$evidence/profile"
gated() { python3 qa/browser_live/check_memory.py && cargo "$@"; }
{
  uname -srmo
  rustc -vV
  cargo --version
  protoc --version
  python3 --version
  git rev-parse HEAD 'HEAD^{tree}'
  sha256sum Cargo.lock
  gated test --locked -p aleph-client
  gated clippy --locked -p aleph-client --all-targets -- -D warnings
  gated check --locked -p aleph-cli -p aleph-tui
  gated test --locked -p alephcore --lib gateway::server:: -- --test-threads=1
  gated test --locked -p alephcore --test gateway_ui_ingress
  gated test --locked -p alephcore --bin aleph-server desktop_ipc_is_explicitly_opt_in
  gated build --locked -p alephcore --bin aleph-server
  gated test --locked -p aleph-nsh-shell
  gated build --locked -p aleph-nsh-shell
  python3 qa/nsh_desktop_ipc_smoke.py "$CARGO_TARGET_DIR/debug/aleph-server" \
    "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$evidence/native-server.log"
} 2>&1 | tee "$evidence/verify.log"
```

关键断言必须看到**效果**，而不是只有入口调用或编译成功：

| 验收项 / Gate | 观察 / Observation |
|---|---|
| 私有端点与生命周期 | 调用方拥有的 `0700` 父目录、全新绝对路径；占用路径拒绝、替换项不改动；SIGTERM 正常退出，连接关闭、旧目录项保留且不可连接；下一次换新路径。此处不是 socket inode `0600` 测试。 |
| 账户、首帧和作用域 | 双端使用内核同 euid 判据；只映射活跃 Core owner，无 legacy loopback/已配对设备身份；无/错误 hello 拒绝，`sessions.list` 真实只见自己的夹具行。测试源未证明的异 UID 独立进程检查如实记 `NOT RUN`，不为验收创建系统用户或改权限；有已授权的隔离第二身份时可补记录。 |
| 帧、并发和清理 | 确认 Gateway 日志列出 `desktop_ipc_uses_gateway_and_releases_its_connection`、`desktop_ipc_cleans_up_when_server_future_is_cancelled`、`desktop_ipc_leaves_occupied_and_published_entries_untouched`；确认帧/JSON 边界、错误 ID、并发乱序、重复活跃 ID 拒绝、客户端超时不提前归还已发送槽位，以及取消/EOF 后共享连接行释放均有原生测试证据。 |
| 真二进制与兼容 | `aleph-nsh-shell --desktop-ipc-status` 从预启动的真实 Gateway 得到 `status=healthy` 及时间戳；探针不结束 Gateway；缺端点、混合或 QA 参数均拒绝、无 TCP/WS fallback。旧 `/ws` 首帧仍是 `connect`，由 Gateway 本机测试与 `gateway_ui_ingress` 回归证明。 |

## 交付 / Return

在 `docs/superpowers/runs/` 新建 Linux-only 验证回执：实际提交和 tree、锁文件 SHA-256、OS/架构/工具版本、每条命令和退出码/计数、两二进制及脱敏证据 SHA-256、实际监听与清理结果，逐项填 `PASS` / `FAIL` / `NOT RUN`。原始日志放在私有目录；不要提交临时 profile、机器绝对路径、真实身份或凭据。失败时先保留基线及可复跑证据，不顺手修改产品代码。可本地提交回执，回传短提交号和路径；不推送、不合并、不改变 `main`。

**Do not claim:** Windows Named Pipe、三平台整体验收、NSH GUI/安装包、默认切换、发布或 Tauri 退场。本回执只更新 Linux 分项；macOS 与 Windows 分别保留自己的证据身份。
