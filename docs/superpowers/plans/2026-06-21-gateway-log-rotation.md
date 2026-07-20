# gateway.log 体积治理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `~/.aleph/logs/gateway.log` 停止无界增长——从源头消除它对 `aleph-server.log` 的重复，并给它套上和 aleph-server.log 一致的大小/保留兜底。

**Architecture:** 两处外科改动。(1) `shared/logging` 仅当 stdout 是交互式终端才挂 tracing 的 console 层——daemon 模式下 stdout 已被重定向到 gateway.log，`is_terminal()` 为假，于是不再灌副本。(2) `daemonize()` 打开 gateway.log 前按 5 MiB 滚动成 `gateway.log.YYYY-MM-DD`，并复用既有 `cleanup_old_logs` 做 7 天保留。

**Tech Stack:** Rust、`tracing` / `tracing-subscriber`、`std::io::IsTerminal`、`chrono`、`aleph-logging`（`shared/logging`，`path` 依赖）。

## Global Constraints

- 不引入新依赖（`chrono` / `aleph-logging` / `tracing-subscriber` 均已在用）。
- 兜底备份必须命名为 `gateway.log.YYYY-MM-DD`（日期后缀正好 10 字符、第 4 与第 7 位是 `-`），否则 `cleanup_old_logs(prefix)` 不会清理它。
- 日期取 `chrono::Utc::now().format("%Y-%m-%d")`，与 `daemon.rs` 既有 `chrono::Utc` 及 tracing-appender 的 UTC 按天轮转一致。
- 极度节制 cargo 调用：每个 Task 至多一次目标化 `cargo test` / `cargo check`，不跑全量。
- 外科手术：每行变更可追溯到本需求；不顺手改邻近代码。
- 提交信息英文，格式 `<scope>: <description>`。

---

### Task 1: daemon 模式不再向 gateway.log 灌 tracing 副本

仅当 stdout 是交互式终端时才挂 console 层。这是消除 82MB 重复的根因修复。无新增单元测试——console 层取舍是全局 subscriber 的进程级副作用（`OnceLock` 只能初始化一次、且依赖真实 stdout 是否 TTY），按代码审查为准；验证手段是既有测试仍绿 + 编译通过。

**Files:**
- Modify: `shared/logging/src/file_appender.rs`（import + `setup_logging` 内 console 层，约 74-79 行附近）

**Interfaces:**
- Consumes: 无（独立于 Task 2）。
- Produces: 行为变化——`setup_logging` 在非 TTY 进程不再注册 console 层；公开函数签名不变（`init_component_logging` / `init_file_logging` 原样）。

- [ ] **Step 1: 加 `IsTerminal` trait import**

打开 `shared/logging/src/file_appender.rs`，在文件顶部的 `use` 区加一行（紧跟现有 `use std::sync::OnceLock;` 之后）：

```rust
use std::io::IsTerminal;
```

- [ ] **Step 2: 把 console 层改成 TTY 条件挂载**

将现有这段（约 74-79 行）：

```rust
    let console_layer = fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::pii_filter::PiiScrubbingFormat);
```

替换为：

```rust
    // Console layer only when stdout is an interactive terminal. In daemon
    // mode `daemonize()` redirects stdout to gateway.log BEFORE this runs, so
    // is_terminal() is false there — dropping the console layer stops
    // gateway.log from accumulating a duplicate of the rotating file_layer.
    // `Option<L>` implements `Layer`, so `None` contributes nothing to the
    // subscriber and the registry chain below is unchanged.
    let console_layer = std::io::stdout().is_terminal().then(|| {
        fmt::layer()
            .with_target(true)
            .with_level(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .event_format(crate::pii_filter::PiiScrubbingFormat)
    });
```

`registry().with(env_filter).with(console_layer).with(file_layer).try_init()` 一行不动——`console_layer` 现在是 `Option<fmt::Layer<…>>`，因 `impl<L,S> Layer<S> for Option<L>` 而照常成立。

- [ ] **Step 3: 编译 + 跑既有测试，确认无回归**

Run: `cargo test -p aleph-logging`
Expected: PASS（`test_get_log_directory` 等既有测试全绿；无编译错误/警告）。

- [ ] **Step 4: Commit**

```bash
git add shared/logging/src/file_appender.rs
git commit -m "logging: only attach console layer on an interactive tty"
```

---

### Task 2: 启动时按大小滚动 gateway.log + 7 天保留

给 daemon 的 stdout/stderr 重定向目标加大小兜底。抽出纯函数 `rotate_oversized_log` 做 TDD，再在 `daemonize()` 里接线并复用 `cleanup_old_logs`。

**Files:**
- Modify: `src/bin/aleph-server/daemon.rs`
  - import：`use std::path::PathBuf;` → `use std::path::{Path, PathBuf};`
  - 新增 const `MAX_GATEWAY_LOG_BYTES` 与 fn `rotate_oversized_log`（`#[cfg(unix)]`）
  - `daemonize()` 的 `Some(log_path)` 分支内接线（约 342-351 行）
  - 新增 `#[cfg(all(test, unix))] mod rotation_tests`

**Interfaces:**
- Consumes: `expand_path(&str) -> PathBuf`（同文件，已存在）；`aleph_logging::cleanup_old_logs(&Path, u32, Option<&str>) -> Result<usize, _>`（bin 已用 `aleph_logging::` 路径，见 `commands/start/helpers.rs:82`）；`chrono::Utc::now()`（同文件已用，见 line 262）。
- Produces: `fn rotate_oversized_log(log_path: &Path, max_bytes: u64) -> std::io::Result<()>`，`const MAX_GATEWAY_LOG_BYTES: u64`。

- [ ] **Step 1: 写失败测试**

在 `src/bin/aleph-server/daemon.rs` 末尾追加测试模块：

```rust
#[cfg(all(test, unix))]
mod rotation_tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_file(path: &Path, bytes: usize) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(&vec![b'x'; bytes]).unwrap();
    }

    fn siblings(dir: &Path, except: &str) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != except)
            .collect()
    }

    #[test]
    fn under_cap_is_not_rotated() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        write_file(&log, 100);
        rotate_oversized_log(&log, 1024).unwrap();
        assert!(log.exists());
        assert!(
            siblings(dir.path(), "gateway.log").is_empty(),
            "no rotation expected under cap"
        );
    }

    #[test]
    fn over_cap_is_rotated_to_dated_sibling() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        write_file(&log, 2048);
        rotate_oversized_log(&log, 1024).unwrap();
        // Original name freed for a fresh file; exactly one dated backup exists.
        assert!(!log.exists());
        let rotated = siblings(dir.path(), "gateway.log");
        assert_eq!(rotated.len(), 1, "expected one rotation, got {rotated:?}");
        let suffix = rotated[0].strip_prefix("gateway.log.").expect("gateway.log. prefix");
        assert_eq!(suffix.len(), 10, "date suffix must be YYYY-MM-DD");
        assert_eq!(suffix.as_bytes()[4], b'-');
        assert_eq!(suffix.as_bytes()[7], b'-');
    }

    #[test]
    fn missing_file_is_noop() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        rotate_oversized_log(&log, 1024).unwrap();
        assert!(!log.exists());
    }
}
```

- [ ] **Step 2: 跑测试确认失败（未定义符号）**

Run: `cargo test -p alephcore --bin aleph-server rotation_tests`
Expected: 编译失败 —— `cannot find function rotate_oversized_log in this scope`。

- [ ] **Step 3: 加 import、const、纯函数实现**

把 `daemon.rs` 顶部的 `use std::path::PathBuf;` 改为：

```rust
use std::path::{Path, PathBuf};
```

在 `expand_path` 之上（紧跟顶部 `use` 区之后）插入常量与函数：

```rust
/// Cap on the daemon's stdout/stderr redirect file (`gateway.log`). Beyond
/// this it is rotated out at the next daemon start so it never grows unbounded.
#[cfg(unix)]
const MAX_GATEWAY_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Rotate `log_path` out of the way if it already exceeds `max_bytes`, keeping
/// the daemon's redirect target bounded across restarts. The oversized file is
/// renamed to `<file_name>.YYYY-MM-DD` (UTC) — a name `cleanup_old_logs` can
/// later age out — and the next append starts a fresh file. No-op when the
/// file is absent or within budget. Same-day re-rotation overwrites that day's
/// backup (atomic rename replace), which is acceptable for a catch-all stream.
#[cfg(unix)]
fn rotate_oversized_log(log_path: &Path, max_bytes: u64) -> std::io::Result<()> {
    let metadata = match std::fs::metadata(log_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if metadata.len() <= max_bytes {
        return Ok(());
    }
    let file_name = log_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("gateway.log");
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let rotated = log_path.with_file_name(format!("{file_name}.{date}"));
    std::fs::rename(log_path, rotated)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --bin aleph-server rotation_tests`
Expected: PASS（3 个测试全绿）。

- [ ] **Step 5: 在 daemonize() 里接线（滚动 + 保留）**

定位 `daemonize()` 的 stdout 重定向分支（约 342-351 行）：

```rust
    // Redirect stdout/stderr to log file if specified
    if let Some(log_path) = log_file {
        let log_path = expand_path(&log_path.to_string_lossy());
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
```

把 `if let Some(parent)` 这一小块替换为（在 `create_dir_all` 之后插入滚动 + 保留）：

```rust
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;

            // Keep the redirect target bounded: rotate it out if it has grown
            // past the cap, then age out old rotations with the same 7-day
            // retention aleph-server.log uses. Best-effort — logging hygiene
            // must never block daemon startup.
            if let Err(e) = rotate_oversized_log(&log_path, MAX_GATEWAY_LOG_BYTES) {
                eprintln!("Warning: failed to rotate {}: {e}", log_path.display());
            }
            if let Some(prefix) = log_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".log"))
            {
                // best-effort: cleanup logs internally via tracing (not yet
                // initialized here), so a failure is silently non-fatal.
                let _ = aleph_logging::cleanup_old_logs(parent, 7, Some(prefix));
            }
        }
```

其余（`OpenOptions` 打开、`dup2` 重定向）一行不动。

- [ ] **Step 6: 编译整个 bin，确认接线无误**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: 编译通过，无 warning（注意 `rotate_oversized_log` 与 `MAX_GATEWAY_LOG_BYTES` 现在有真实消费者，不会触发 dead_code）。

- [ ] **Step 7: Commit**

```bash
git add src/bin/aleph-server/daemon.rs
git commit -m "daemon: rotate oversized gateway.log on start with 7-day retention"
```

---

## 部署 / 收尾（实现后手动，非代码任务）

- 改动经 `rust_embed` 链对 WASM 无关，但 daemon 行为变更需重编 `aleph-server` 并替换运行中的 binary 才生效（见 DESKTOP_SHELL.md 刷新链）。
- 存量 82MB `gateway.log`：下次 daemon 重启时 >5MiB 会被自动 rename 成 `gateway.log.<date>`，7 天后被保留策略清掉；也可现在直接 `rm ~/.aleph/logs/gateway.log`（daemon 持 fd，删文件名无需停服）。
- 验证：重启 daemon 后 `tail -f ~/.aleph/logs/aleph-server.log.<今天>` 看实时日志；确认新 `gateway.log` 只剩零星裸输出且体积停在 KB 级。

---

## Self-Review

**Spec coverage:**
- Part 1（停止重复 / 条件 console 层）→ Task 1 ✅
- Part 2（按大小滚动 `gateway.log.YYYY-MM-DD` + `cleanup_old_logs` 7 天保留）→ Task 2 ✅
- retention 命名约束（10 字符日期后缀）→ Task 2 测试 `over_cap_is_rotated_to_dated_sibling` 断言后缀长度与 `-` 位置 ✅
- 5 MiB 阈值 / UTC 日期 → `MAX_GATEWAY_LOG_BYTES` 与 `chrono::Utc` ✅
- 存量 82MB 处理、迁移提示 → 部署/收尾段 ✅
- 已知局限（rotate-on-start 不覆盖单次长跑）→ spec 已声明；本计划不实现后台线程（YAGNI），无需任务。

**Placeholder scan:** 无 TBD/TODO；每个改码步骤均给出完整代码与确切命令/预期。Task 1 无新测试已显式说明理由（全局副作用），非占位。

**Type consistency:** `rotate_oversized_log(&Path, u64) -> std::io::Result<()>` 在定义（Step 3）、测试（Step 1）、调用（Step 5）三处签名一致；`MAX_GATEWAY_LOG_BYTES: u64` 定义与传参一致；`cleanup_old_logs(parent: &Path, 7, Some(prefix): Option<&str>)` 与既有签名一致；`Path` 已在 Step 3 加入 import 供函数签名与测试使用。
