# Development Guide

> 构建 / 验证 / 工具链 / 分发 / 进程 / 文件工具的工程层操作手册。**Tier 1 只放指针**；详情在这里。

---

## 构建命令

| Command | Description |
|---------|-------------|
| `cargo run --bin aleph-server` · `cargo check -p alephcore` | Start server (debug) · quick compile check（**只验证了仓库的一小半**，见下方验证集） |
| `just dev` · `just build` | Dev server（先重建 WASM）· Release build (WASM + server) |
| `just shell-dev` · `just shell-build` · `just shell-build-lite` | 桌面 App dev · 完整 installers（.dmg/.msi/.deb，内置 server）· Panel 纯壳（无 server，连局域网） |
| `just test-all` · `just clippy` · `just wasm` | 全量测试（core + desktop + proptest + 共享 crate）· Lint · 唯一编译 Panel **出厂形态**的命令 |
| `just verify-build` · `just release YY.M.D` | CI 验证三产物三平台能否构建（不打 tag）· **发版**（需先写 changelog）→ [RELEASE.md](docs/reference/RELEASE.md) |

---

## 最小可信验证集 — 七条命令，不是一条

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins            # alephcore 里唯一真跑而非 --no-run 的一条：--lib 带不到 src/bin/ 下那些测试
                                          # （含钉住 boot 无条件 install_policy/install_ledger 的 census）
cargo test -p alephcore --features test-helpers --test '*' --no-run   # --all-targets 只展开 target 不展开 feature
cargo test -p aleph-panel --lib --no-run  # check 看不见它的 #[cfg(test)]；曾整程编译不过
just test-shared                          # shared-ui-logic（两种 feature 形态）/ aleph-protocol / aleph-tui / aleph-cli 的测试：
                                          # -p alephcore 从不编译依赖的 #[cfg(test)]，共享核的 reducer / G2 只由它和 CI 的 Run shared crate tests 跑
cargo check -p aleph-desktop-{macos,windows,linux}   # 跨平台改动要 check 那个目标的限肢 crate
cargo clippy --workspace --all-targets    # 先 just _stage-shell-placeholders；--all-targets 展开 target、
                                          # --workspace 展开 package（无 default-members ⇒ 默认只 lint 根 crate）
```

- **`cargo check` 不编译 `#[cfg(test)]`**——删 `pub fn` / 字段的同一笔里必须跑 `cargo test --no-run`；改动 `interfaces/tui/` `interfaces/cli/` `shared/ui_logic/` `shared/protocol/` 的同一笔里跑 `just test-shared`（**上面那条 `--workspace` clippy 会 lint 它们，但 lint 不是测试**；CI 的路径过滤不含 `interfaces/tui/**` `interfaces/cli/**`，只改那两处的提交在 CI 上不触发它）。
- **`interfaces/webchat/` 有任何改动（哪怕不是你改的）就跑一次 `cargo test -p aleph-panel --lib`**——这个 crate 的**语义合并冲突是常态形状**（一侧的类型 + 另一侧的调用点，git 不报冲突、两边单独看都完整）。修完**先看警告再看错误**：`unused variable` 说明那半边根本没有调用者，正解是 CUT。只改 Panel 时用 `just wasm`——它是唯一编译**出厂形态**的命令。
- **`cargo check -p aleph-desktop-shell` 前需先 `just _stage-shell-placeholders`**（tauri-build 要求 externalBin 占位文件存在）；**`--workspace` clippy 同样要**。占位路径**别在别处抄一份**——那条 recipe 自己推 triple、Windows 补 `.exe`、`AlephBridge-` 只在 macOS 上建。
- 验证是怎么骗你的（数字 / 仪器 / 扫描边界 / 闸 / 命令陷阱）→ [FEATURE_LOCATOR 附录 C](docs/reference/FEATURE_LOCATOR.md) · 触发器 → 附录 E.10。

---

## 工具链与版本

- **MSRV = 1.95**（由 `sysinfo 0.39` 决定），在 `Cargo.toml` 的 `[workspace.package]` 与 `[package]` 两处 `rust-version` 声明；根 `rust-toolchain.toml` 钉住具体 stable（当前 `1.96.0`），本地与 CI 自动使用同一工具链——无需 `rustup default` 或 `cargo +<ver>`。抬高 MSRV 时同步更新这两处。
- **CalVer `YY.M.D`**（两位年、月/日不补零，如 `26.5.21`；同时是合法 semver 并满足 Windows MSI 约束），每天最多一个版本。**VERSION 文件是唯一版本源**——`build.rs` 读取 → 注入 `ALEPH_VERSION` → 代码用 `env!("ALEPH_VERSION")`；**禁止**硬编码版本号，**禁止** `env!("CARGO_PKG_VERSION")`。Panel System Info、Gateway 版本、MCP/ACP 协议版本、CLI `--version`、release tag 全读它。发版走 `just release YY.M.D` → [RELEASE.md](docs/reference/RELEASE.md)；Windows 构建前置依赖 → [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)。

---

## 会话旋钮 (Session Knobs)

**正交**的会话旋钮：执行档位 / 会话模式 / 推理档 / 记忆模式 / 模型 pin / 繁忙输入。**别在这里维护一个数目**（上一版标题写着「三根」而表里早就不止三行）。除繁忙输入外共用一套机制（值住在 `SessionMetadata.identity_meta.custom[<key>]`，precedence **请求 > 会话 > 全局**，解析在 `src/gateway/execution_engine/turn_*.rs` 的孪生模块里）。表、每根的"谁在拨"、以及**加一根新旋钮要动的每一处**全在 [SESSION_KNOBS.md](docs/reference/SESSION_KNOBS.md)。

`[sandbox.command_policy]` 的硬底线**任何档位都压不下去**。

---

## 分发形态与信任模型

- **三产物**（同一 tag）：完整桌面 App（内置 `aleph-server`，单机零配置）/ Aleph Panel 纯壳 App（连局域网 server）/ 独立 `aleph-server` 二进制 → [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md)
- **信任模型 = 网络边界 + 登录墙**：默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。loopback 免凭据恒 operator；远程须在 `connect` 出示 device token / 一次性配对票 / 共享 token 之一，**过了就是 operator，与本地完全一致——单层，没有 Chat/Config 子层**。协议护栏是 WS Origin 校验 → [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)

---

## Feature Flags / 提交规范 / 进程管理

- 所有生产功能始终编译，无需 feature flags。仅保留测试用：`loom`（并发）、`test-helpers`（集成测试工具）。
- English commit messages，格式 `<scope>: <description>`（例：`gateway: add WebSocket server foundation`）。**单分支开发**：所有工作直接在 main。`EnterWorktree` 会话内只合并不删除（同会话 `git worktree remove` 会损坏 Shell）→ [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)
- Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁。`kill -9` 后可立即重启。doctor 的 `core/duplicate-instance` 是运行时哨兵——**多进程竞争同一 vault → HMAC 失败 → vault 数据丢失** → [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)

**Before restarting Aleph:**
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
```

---

## 内置文件与 Shell 工具

**搜索走 `grep` / `find`，不走 bash**——`grep` 是内容搜索、`find` 是文件名发现，共用 `src/builtin_tools/file_search/walk.rs` 的 `.gitignore`-aware 走树 ＋ deny 闸。多个词写成**一次** alternation（`grep{pattern:"a|b|c"}`）；先 `files_only: true` 拿路径，再 `file_read{offset,limit}` 只读命中附近。非走 shell 不可时用 `rg` 而不是 `grep`。`file_ops(search)` 是**另一张脸**（文件管理：size/type/extension）。**长任务（>3 min build/install）必须 `background: true`**——`WAIT_MAX_TIMEOUT_SECS=170` 是 180s tool budget 的硬约束，**不要**尝试扩展（违 R10）。全部现状 → [FEATURE_LOCATOR §3.4](docs/reference/FEATURE_LOCATOR.md) · [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md)
