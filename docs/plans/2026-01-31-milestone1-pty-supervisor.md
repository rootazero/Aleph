# PtySupervisor 基础实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 让 Aleph 能启动 Claude Code 并读取其输出，为后续的监管者模式打下基础。

**Architecture:** 创建独立的 `supervisor` 模块，使用 `portable-pty` 库创建虚拟终端，欺骗 Claude Code 让其以为运行在真实终端中。通过 PTY master 端读写实现 stdin/stdout 双向通信。

**Tech Stack:** portable-pty (PTY), strip-ansi-escapes (ANSI 清洗), tokio (异步运行时)

---

## Task 1: 添加依赖到 Cargo.toml

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: 添加 portable-pty 和 strip-ansi-escapes 依赖**

在 `[dependencies]` 部分添加：

```toml
# PTY for process control (PtySupervisor)
portable-pty = "0.8"
# ANSI escape sequence stripping
strip-ansi-escapes = "0.2"
```

**Step 2: 验证依赖可以解析**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check`

Expected: 编译通过，无依赖冲突

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add portable-pty and strip-ansi-escapes for PtySupervisor"
```

---

## Task 2: 创建 supervisor 模块骨架

**Files:**
- Create: `core/src/supervisor/mod.rs`
- Create: `core/src/supervisor/types.rs`
- Modify: `core/src/lib.rs`

**Step 1: 创建 types.rs 定义核心类型**

```rust
//! PtySupervisor type definitions.

use std::path::PathBuf;

/// PTY 终端尺寸配置
#[derive(Debug, Clone)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self { rows: 24, cols: 120 }
    }
}

/// Supervisor 配置
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// 工作目录
    pub workspace: PathBuf,
    /// PTY 终端尺寸
    pub pty_size: PtySize,
    /// 要执行的命令 (默认 "claude")
    pub command: String,
    /// 命令参数
    pub args: Vec<String>,
}

impl SupervisorConfig {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            pty_size: PtySize::default(),
            command: "claude".to_string(),
            args: vec![],
        }
    }

    pub fn with_command(mut self, cmd: impl Into<String>) -> Self {
        self.command = cmd.into();
        self
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_pty_size(mut self, rows: u16, cols: u16) -> Self {
        self.pty_size = PtySize { rows, cols };
        self
    }
}

/// Supervisor 事件类型
#[derive(Debug, Clone)]
pub enum SupervisorEvent {
    /// 收到输出行
    Output(String),
    /// 进程退出
    Exited(i32),
    /// 检测到审批请求
    ApprovalRequest(String),
    /// 检测到上下文窗口满
    ContextOverflow,
    /// 检测到错误
    Error(String),
}

/// Supervisor 错误类型
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("Failed to create PTY: {0}")]
    PtyCreation(String),
    #[error("Failed to spawn command: {0}")]
    SpawnFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Process not running")]
    NotRunning,
    #[error("Write failed: {0}")]
    WriteFailed(String),
}
```

**Step 2: 创建 mod.rs 导出模块**

```rust
//! PtySupervisor module for controlling external CLI tools.
//!
//! This module provides PTY-based process control for tools like Claude Code,
//! allowing Aleph to act as a "supervisor" that can:
//! - Spawn processes in a pseudo-terminal
//! - Read and parse their output in real-time
//! - Inject input (commands, approvals)
//! - Detect semantic events (approval requests, errors)

pub mod types;

pub use types::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};
```

**Step 3: 在 lib.rs 中注册模块**

在 `core/src/lib.rs` 的模块声明部分添加：

```rust
pub mod supervisor; // PTY-based process supervisor for Claude Code control
```

在 re-exports 部分添加：

```rust
// Supervisor exports (PTY-based process control)
pub use crate::supervisor::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};
```

**Step 4: 验证模块结构**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check`

Expected: 编译通过

**Step 5: Commit**

```bash
git add core/src/supervisor/ core/src/lib.rs
git commit -m "feat(supervisor): add module skeleton with types"
```

---

## Task 3: 实现 ClaudeSupervisor 核心结构

**Files:**
- Create: `core/src/supervisor/pty.rs`
- Modify: `core/src/supervisor/mod.rs`

**Step 1: 编写失败测试**

在 `core/src/supervisor/pty.rs` 底部添加测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_supervisor_creation() {
        let config = SupervisorConfig::new("/tmp");
        let supervisor = ClaudeSupervisor::new(config);
        assert!(!supervisor.is_running());
    }
}
```

**Step 2: 运行测试验证失败**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test supervisor::pty::tests::test_supervisor_creation`

Expected: FAIL with "cannot find struct `ClaudeSupervisor`"

**Step 3: 实现 ClaudeSupervisor 结构体**

创建 `core/src/supervisor/pty.rs`：

```rust
//! PTY-based supervisor implementation.

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize as PortablePtySize, PtySystem};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::supervisor::types::{SupervisorConfig, SupervisorError, SupervisorEvent};

/// PTY-based supervisor for controlling Claude Code and similar CLI tools.
///
/// # Example
///
/// ```rust,no_run
/// use alephcore::supervisor::{ClaudeSupervisor, SupervisorConfig};
///
/// let config = SupervisorConfig::new("/path/to/workspace");
/// let mut supervisor = ClaudeSupervisor::new(config);
///
/// // Spawn the process
/// let rx = supervisor.spawn().unwrap();
///
/// // Send input
/// supervisor.write("Hello\n").unwrap();
///
/// // Read events
/// while let Some(event) = rx.blocking_recv() {
///     println!("Event: {:?}", event);
/// }
/// ```
pub struct ClaudeSupervisor {
    config: SupervisorConfig,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    running: Arc<AtomicBool>,
}

impl ClaudeSupervisor {
    /// Create a new supervisor with the given configuration.
    pub fn new(config: SupervisorConfig) -> Self {
        Self {
            config,
            master: None,
            writer: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the supervised process is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Spawn the supervised process and return an event receiver.
    ///
    /// Returns a channel receiver that will emit `SupervisorEvent` as they occur.
    pub fn spawn(&mut self) -> Result<mpsc::UnboundedReceiver<SupervisorEvent>, SupervisorError> {
        let pty_system = native_pty_system();

        // Create PTY pair
        let pair = pty_system
            .openpty(PortablePtySize {
                rows: self.config.pty_size.rows,
                cols: self.config.pty_size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SupervisorError::PtyCreation(e.to_string()))?;

        // Build command
        let mut cmd = CommandBuilder::new(&self.config.command);
        cmd.cwd(&self.config.workspace);
        for arg in &self.config.args {
            cmd.arg(arg);
        }

        // Spawn process
        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

        // Get reader and writer
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SupervisorError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SupervisorError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.running.store(true, Ordering::SeqCst);

        // Create event channel
        let (tx, rx) = mpsc::unbounded_channel();
        let running = self.running.clone();

        // Spawn reader thread
        std::thread::spawn(move || {
            let buf_reader = BufReader::new(reader);
            for line in buf_reader.lines() {
                match line {
                    Ok(text) => {
                        // Strip ANSI escape sequences
                        let clean = strip_ansi(&text);

                        // Detect semantic events
                        let event = detect_event(&clean);
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(SupervisorEvent::Exited(0));
        });

        Ok(rx)
    }

    /// Write input to the supervised process.
    pub fn write(&mut self, input: &str) -> Result<(), SupervisorError> {
        let writer = self.writer.as_mut().ok_or(SupervisorError::NotRunning)?;
        writer
            .write_all(input.as_bytes())
            .map_err(|e| SupervisorError::WriteFailed(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| SupervisorError::WriteFailed(e.to_string()))?;
        Ok(())
    }

    /// Write a line (appends newline) to the supervised process.
    pub fn writeln(&mut self, input: &str) -> Result<(), SupervisorError> {
        self.write(&format!("{}\n", input))
    }
}

/// Strip ANSI escape sequences from text.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    match strip_ansi_escapes::strip(bytes) {
        Ok(stripped) => String::from_utf8_lossy(&stripped).to_string(),
        Err(_) => text.to_string(),
    }
}

/// Detect semantic events from cleaned output text.
fn detect_event(text: &str) -> SupervisorEvent {
    // Approval request detection
    if text.contains("Do you want to run") || text.contains("Allow this command") {
        return SupervisorEvent::ApprovalRequest(text.to_string());
    }

    // Context overflow detection
    if text.contains("Context window") && text.contains("full") {
        return SupervisorEvent::ContextOverflow;
    }

    // Error detection
    if text.starts_with("Error:") || text.contains("error:") {
        return SupervisorEvent::Error(text.to_string());
    }

    // Default: regular output
    SupervisorEvent::Output(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_creation() {
        let config = SupervisorConfig::new("/tmp");
        let supervisor = ClaudeSupervisor::new(config);
        assert!(!supervisor.is_running());
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[31mRed text\x1b[0m";
        let output = strip_ansi(input);
        assert_eq!(output, "Red text");
    }

    #[test]
    fn test_strip_ansi_plain() {
        let input = "Plain text";
        let output = strip_ansi(input);
        assert_eq!(output, "Plain text");
    }

    #[test]
    fn test_detect_approval_request() {
        let text = "Do you want to run this command?";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::ApprovalRequest(_)));
    }

    #[test]
    fn test_detect_context_overflow() {
        let text = "Context window is full. Consider using /compact.";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::ContextOverflow));
    }

    #[test]
    fn test_detect_error() {
        let text = "Error: Command not found";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::Error(_)));
    }

    #[test]
    fn test_detect_output() {
        let text = "Hello, world!";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::Output(_)));
    }
}
```

**Step 4: 更新 mod.rs 导出**

```rust
//! PtySupervisor module for controlling external CLI tools.
//!
//! This module provides PTY-based process control for tools like Claude Code,
//! allowing Aleph to act as a "supervisor" that can:
//! - Spawn processes in a pseudo-terminal
//! - Read and parse their output in real-time
//! - Inject input (commands, approvals)
//! - Detect semantic events (approval requests, errors)

pub mod pty;
pub mod types;

pub use pty::ClaudeSupervisor;
pub use types::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};
```

**Step 5: 更新 lib.rs 导出**

在 re-exports 部分更新：

```rust
// Supervisor exports (PTY-based process control)
pub use crate::supervisor::{
    ClaudeSupervisor, PtySize, SupervisorConfig, SupervisorError, SupervisorEvent,
};
```

**Step 6: 运行测试验证通过**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test supervisor::`

Expected: All tests PASS

**Step 7: Commit**

```bash
git add core/src/supervisor/
git commit -m "feat(supervisor): implement ClaudeSupervisor with PTY and ANSI parsing"
```

---

## Task 4: 添加集成测试 (使用 echo 命令)

**Files:**
- Create: `core/src/supervisor/tests.rs`
- Modify: `core/src/supervisor/mod.rs`

**Step 1: 创建集成测试文件**

```rust
//! Integration tests for PtySupervisor.
//!
//! These tests use real PTY processes to verify the supervisor works correctly.

use crate::supervisor::{ClaudeSupervisor, SupervisorConfig, SupervisorEvent};
use std::time::Duration;

/// Test spawning a simple echo command.
#[test]
fn test_spawn_echo() {
    let config = SupervisorConfig::new("/tmp")
        .with_command("echo")
        .with_args(vec!["Hello from PTY".to_string()]);

    let mut supervisor = ClaudeSupervisor::new(config);
    let rx = supervisor.spawn().expect("Failed to spawn");

    // Collect events with timeout
    let mut outputs = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);

    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Output(text)) => {
                outputs.push(text);
            }
            Ok(SupervisorEvent::Exited(_)) => break,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    // Verify we got the expected output
    let combined = outputs.join("");
    assert!(
        combined.contains("Hello from PTY"),
        "Expected 'Hello from PTY' in output, got: {:?}",
        outputs
    );
}

/// Test writing input to a cat process.
#[test]
fn test_write_to_cat() {
    let config = SupervisorConfig::new("/tmp")
        .with_command("cat");

    let mut supervisor = ClaudeSupervisor::new(config);
    let rx = supervisor.spawn().expect("Failed to spawn");

    // Write input
    supervisor.writeln("Test input line").expect("Failed to write");

    // Collect output
    let mut outputs = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);

    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Output(text)) => {
                outputs.push(text);
                if text.contains("Test input line") {
                    break;
                }
            }
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    let combined = outputs.join("");
    assert!(
        combined.contains("Test input line"),
        "Expected 'Test input line' in output, got: {:?}",
        outputs
    );
}

/// Test that is_running reflects actual state.
#[test]
fn test_is_running_state() {
    let config = SupervisorConfig::new("/tmp")
        .with_command("echo")
        .with_args(vec!["quick".to_string()]);

    let mut supervisor = ClaudeSupervisor::new(config);
    assert!(!supervisor.is_running(), "Should not be running before spawn");

    let rx = supervisor.spawn().expect("Failed to spawn");

    // Give it a moment to start
    std::thread::sleep(Duration::from_millis(50));

    // Wait for exit
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Exited(_)) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    // After echo exits, should eventually show not running
    std::thread::sleep(Duration::from_millis(100));
    assert!(!supervisor.is_running(), "Should not be running after process exits");
}
```

**Step 2: 更新 mod.rs 包含测试模块**

```rust
//! PtySupervisor module for controlling external CLI tools.

pub mod pty;
pub mod types;

#[cfg(test)]
mod tests;

pub use pty::ClaudeSupervisor;
pub use types::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};
```

**Step 3: 运行集成测试**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test supervisor::tests::`

Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/supervisor/tests.rs core/src/supervisor/mod.rs
git commit -m "test(supervisor): add integration tests with echo and cat"
```

---

## Task 5: 添加 Gateway RPC Handler (可选)

**Files:**
- Create: `core/src/gateway/handlers/supervisor.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: 创建 supervisor handler 骨架**

```rust
//! Supervisor RPC handlers.
//!
//! Provides RPC methods for controlling external processes via PtySupervisor.
//!
//! ## Methods
//!
//! - `supervisor.spawn` - Spawn a supervised process
//! - `supervisor.write` - Write input to the process
//! - `supervisor.status` - Get process status

use serde::{Deserialize, Serialize};

/// Parameters for supervisor.spawn
#[derive(Debug, Deserialize)]
pub struct SupervisorSpawnParams {
    /// Working directory
    pub workspace: String,
    /// Command to execute (default: "claude")
    #[serde(default = "default_command")]
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
}

fn default_command() -> String {
    "claude".to_string()
}

/// Result of supervisor.spawn
#[derive(Debug, Serialize)]
pub struct SupervisorSpawnResult {
    /// Unique session ID for this supervisor instance
    pub session_id: String,
    /// Whether spawn was successful
    pub success: bool,
}

/// Parameters for supervisor.write
#[derive(Debug, Deserialize)]
pub struct SupervisorWriteParams {
    /// Session ID from spawn
    pub session_id: String,
    /// Input to write
    pub input: String,
    /// Whether to append newline
    #[serde(default)]
    pub newline: bool,
}

/// Result of supervisor.write
#[derive(Debug, Serialize)]
pub struct SupervisorWriteResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for supervisor.status
#[derive(Debug, Deserialize)]
pub struct SupervisorStatusParams {
    pub session_id: String,
}

/// Result of supervisor.status
#[derive(Debug, Serialize)]
pub struct SupervisorStatusResult {
    pub running: bool,
    pub session_id: String,
}

// TODO: Implement actual handlers in Milestone 2 when integrating with Gateway
```

**Step 2: 注册到 handlers/mod.rs**

在 handlers/mod.rs 中添加：

```rust
pub mod supervisor;
```

**Step 3: 验证编译**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check --features gateway`

Expected: 编译通过

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/supervisor.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add supervisor RPC handler skeleton"
```

---

## Task 6: 最终验证和文档

**Step 1: 运行所有 supervisor 测试**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test supervisor:: --all-features`

Expected: All tests PASS

**Step 2: 运行完整测试套件确保无回归**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test`

Expected: All existing tests still pass

**Step 3: 更新设计文档状态**

在 `docs/plans/2026-01-31-aether-beyond-openclaw-design.md` 的 Milestone 1 部分标记完成：

```markdown
### Milestone 1: PtySupervisor 基础

- [x] 集成 portable-pty crate
- [x] 实现 ClaudeSupervisor::spawn()
- [x] ANSI 清洗层 (strip_ansi_escapes)
- [x] 基础 stdin/stdout 交互测试

**验收**: ✅ Aleph 能启动 Claude Code 并读取输出
```

**Step 4: Final Commit**

```bash
git add docs/plans/
git commit -m "docs: mark Milestone 1 (PtySupervisor) as complete"
```

---

## 验收标准

完成本计划后，应满足以下条件：

1. ✅ `cargo build` 成功编译 supervisor 模块
2. ✅ `cargo test supervisor::` 所有测试通过
3. ✅ 可以通过 `ClaudeSupervisor::spawn()` 启动任意 CLI 程序
4. ✅ 可以通过 `ClaudeSupervisor::write()` 向进程发送输入
5. ✅ 输出自动清洗 ANSI 转义序列
6. ✅ 能检测语义事件（审批请求、上下文溢出、错误）

---

## 依赖关系

```
无前置依赖，可立即开始
```

## 后续 Milestone

完成本 Milestone 后，可以开始：
- **Milestone 2**: SecurityKernel 规则引擎
- **Milestone 3**: Telegram 审批集成（依赖 M2）

---

*生成时间: 2026-01-31*
