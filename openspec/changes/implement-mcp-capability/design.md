# Design: MCP Capability Implementation

## Context

Aether 是一个系统级 AI 中间件，当前已实现：
- **Memory Capability**: 本地 RAG，向量数据库检索历史对话
- **Search Capability**: 6 种搜索提供商，支持 fallback
- **Skills Capability**: Claude Agent Skills 标准，SKILL.md 格式
- **Video Capability**: YouTube 字幕提取

MCP (Model Context Protocol) 是 Anthropic 提出的标准化协议，定义了 AI 应用与外部数据源、工具之间的交互接口。本设计实现 MCP 作为 Aether 的第五个 Capability。

### 核心约束：零外部依赖

**用户永远不应该看到 `Error: npm not found` 这样的错误。**

Aether 是一个面向普通用户的 macOS 桌面应用，要求用户安装 Node.js/Python 会严重破坏用户体验。因此，我们采用**三层服务架构**：

1. **Layer 1 (Builtin)**: Rust 原生实现，编译进 Aether Core
2. **Layer 2 (Bundled)**: 预编译二进制，随 .app 分发
3. **Layer 3 (External)**: 用户自行配置，需自行安装运行时

### 核心约束：低耦合高内聚 (共享基础模块)

为实现**低耦合高内聚**原则，核心能力抽取为独立的**共享基础模块** (`services/`)，供 MCP、Skills 以及未来扩展共同使用：

```
Aether/core/src/services/           # 共享基础服务层
├── mod.rs                          # 模块导出
├── fs/                             # 文件系统服务
│   ├── mod.rs                      # FileOps trait
│   └── local.rs                    # LocalFs impl (tokio::fs)
├── git/                            # Git 服务
│   ├── mod.rs                      # GitOps trait
│   └── repository.rs               # GitRepository impl (git2-rs)
└── system_info/                    # 系统信息服务
    ├── mod.rs                      # SystemInfoProvider trait
    └── macos.rs                    # MacOsSystemInfo impl
```

**设计优势**：
- **低耦合**：MCP 和 Skills 不直接依赖对方，只依赖共享基础层
- **高内聚**：每个服务模块只负责一种能力
- **可测试**：基础模块可独立单元测试
- **可扩展**：新功能可直接复用基础模块

### 现有架构约束

1. **Capability 策略模式**：所有 Capability 实现 `CapabilityStrategy` trait
2. **AgentPayload 中心化**：上下文数据通过 `AgentContext` 字段传递
3. **UniFFI 桥接**：Rust ↔ Swift 通过 UniFFI 自动生成绑定
4. **异步运行时**：使用 tokio，所有 IO 操作必须是 async

## Goals / Non-Goals

### Goals
- G0: 创建共享基础模块 (`services/`)，供 MCP、Skills 及未来扩展复用 ⭐
- G1: 实现内置 MCP 服务（fs、git、system-info、shell），零外部依赖
- G2: 提供 `BuiltinMcpService` trait 作为 MCP 适配器接口
- G3: 支持外部 MCP Server（带运行时检测）
- G4: 与现有 CapabilityStrategy 模式无缝集成
- G5: 提供安全的工具调用机制（权限控制 + 用户确认）

### Non-Goals
- NG1: HTTP/WebSocket 传输层（MVP 不需要）
- NG2: MCP Prompts API 完整支持（低优先级）
- NG3: Layer 2 捆绑服务（Phase 4）
- NG4: MCP Server 开发 SDK
- NG5: 修改 Skills 模块以使用共享基础层（本提案范围外，作为后续提案）

## Detailed Design

### 0. 共享基础模块 (Shared Foundation) ⭐

共享基础模块是整个设计的核心，提供可复用的底层能力。

#### 0.1 模块结构

```
Aether/core/src/services/
├── mod.rs                      # 模块导出
├── fs/
│   ├── mod.rs                  # FileOps trait + DirEntry
│   └── local.rs                # LocalFs impl
├── git/
│   ├── mod.rs                  # GitOps trait + GitStatus/GitCommit
│   └── repository.rs           # GitRepository impl
└── system_info/
    ├── mod.rs                  # SystemInfoProvider trait + SystemInfo
    └── macos.rs                # MacOsSystemInfo impl
```

#### 0.2 FileOps Trait (文件系统操作)

```rust
// Aether/core/src/services/fs/mod.rs

use crate::error::Result;
use async_trait::async_trait;
use std::path::Path;

/// Directory entry information
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: std::path::PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// File system operations trait
///
/// This trait abstracts file system operations for sharing between
/// MCP, Skills, and future extensions.
#[async_trait]
pub trait FileOps: Send + Sync {
    /// Read file contents as string
    async fn read_file(&self, path: &Path) -> Result<String>;

    /// Read file contents as bytes
    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>>;

    /// Write string content to file
    async fn write_file(&self, path: &Path, content: &str) -> Result<()>;

    /// Write bytes to file
    async fn write_file_bytes(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// List directory contents
    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>>;

    /// Check if path exists
    async fn exists(&self, path: &Path) -> Result<bool>;

    /// Check if path is a directory
    async fn is_dir(&self, path: &Path) -> Result<bool>;

    /// Create directory (with parents)
    async fn create_dir(&self, path: &Path) -> Result<()>;

    /// Delete file or directory
    async fn delete(&self, path: &Path) -> Result<()>;

    /// Search files by glob pattern
    async fn search(&self, base: &Path, pattern: &str) -> Result<Vec<DirEntry>>;
}
```

```rust
// Aether/core/src/services/fs/local.rs

use super::{DirEntry, FileOps};
use crate::error::{AetherError, Result};
use async_trait::async_trait;
use glob::glob;
use std::path::Path;
use tokio::fs;

/// Local filesystem implementation using tokio::fs
pub struct LocalFs;

impl LocalFs {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FileOps for LocalFs {
    async fn read_file(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).await
            .map_err(|e| AetherError::io(format!("Failed to read {}: {}", path.display(), e)))
    }

    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        fs::read(path).await
            .map_err(|e| AetherError::io(format!("Failed to read {}: {}", path.display(), e)))
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        fs::write(path, content).await
            .map_err(|e| AetherError::io(format!("Failed to write {}: {}", path.display(), e)))
    }

    async fn write_file_bytes(&self, path: &Path, content: &[u8]) -> Result<()> {
        fs::write(path, content).await
            .map_err(|e| AetherError::io(format!("Failed to write {}: {}", path.display(), e)))
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(path).await
            .map_err(|e| AetherError::io(format!("Failed to list {}: {}", path.display(), e)))?;

        while let Some(entry) = read_dir.next_entry().await
            .map_err(|e| AetherError::io(format!("Failed to read entry: {}", e)))?
        {
            let metadata = entry.metadata().await.ok();
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                is_dir: metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: metadata.and_then(|m| m.modified().ok()),
            });
        }

        Ok(entries)
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        Ok(fs::try_exists(path).await.unwrap_or(false))
    }

    async fn is_dir(&self, path: &Path) -> Result<bool> {
        Ok(fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false))
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).await
            .map_err(|e| AetherError::io(format!("Failed to create dir {}: {}", path.display(), e)))
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        let is_dir = self.is_dir(path).await?;
        if is_dir {
            fs::remove_dir_all(path).await
        } else {
            fs::remove_file(path).await
        }.map_err(|e| AetherError::io(format!("Failed to delete {}: {}", path.display(), e)))
    }

    async fn search(&self, base: &Path, pattern: &str) -> Result<Vec<DirEntry>> {
        // Use spawn_blocking for glob since it's sync
        let full_pattern = base.join(pattern).to_string_lossy().to_string();

        tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            for path in glob(&full_pattern).map_err(|e| AetherError::io(format!("Invalid pattern: {}", e)))? {
                if let Ok(path) = path {
                    let metadata = std::fs::metadata(&path).ok();
                    entries.push(DirEntry {
                        name: path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        path: path.clone(),
                        is_dir: metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                        size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                        modified: metadata.and_then(|m| m.modified().ok()),
                    });
                }
            }
            Ok(entries)
        }).await
            .map_err(|e| AetherError::io(format!("Task join error: {}", e)))?
    }
}
```

#### 0.3 GitOps Trait (Git 操作)

```rust
// Aether/core/src/services/git/mod.rs

use crate::error::Result;
use async_trait::async_trait;
use std::path::Path;

/// Git file status
#[derive(Debug, Clone)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,  // "new", "modified", "deleted", "renamed"
    pub staged: bool,
}

/// Git commit information
#[derive(Debug, Clone)]
pub struct GitCommit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
}

/// Git diff hunk
#[derive(Debug, Clone)]
pub struct GitDiff {
    pub file_path: String,
    pub old_start: u32,
    pub new_start: u32,
    pub content: String,
}

/// Git operations trait
///
/// Abstracts git operations using git2-rs library.
/// All operations are wrapped with spawn_blocking for async compatibility.
#[async_trait]
pub trait GitOps: Send + Sync {
    /// Get repository status (modified/staged files)
    async fn status(&self, repo_path: &Path) -> Result<Vec<GitFileStatus>>;

    /// Get commit history
    async fn log(&self, repo_path: &Path, limit: usize) -> Result<Vec<GitCommit>>;

    /// Get diff of changes
    async fn diff(&self, repo_path: &Path, staged: bool) -> Result<Vec<GitDiff>>;

    /// Get current branch name
    async fn current_branch(&self, repo_path: &Path) -> Result<String>;

    /// Check if path is a git repository
    async fn is_repo(&self, path: &Path) -> Result<bool>;
}
```

```rust
// Aether/core/src/services/git/repository.rs

use super::{GitCommit, GitDiff, GitFileStatus, GitOps};
use crate::error::{AetherError, Result};
use async_trait::async_trait;
use git2::Repository;
use std::path::{Path, PathBuf};

/// Git repository operations using git2-rs
pub struct GitRepository;

impl GitRepository {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GitOps for GitRepository {
    async fn status(&self, repo_path: &Path) -> Result<Vec<GitFileStatus>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Repository::open(&path)
                .map_err(|e| AetherError::mcp(format!("Failed to open repo: {}", e)))?;

            let statuses = repo.statuses(None)
                .map_err(|e| AetherError::mcp(format!("Failed to get status: {}", e)))?;

            let mut files = Vec::new();
            for entry in statuses.iter() {
                if let Some(path) = entry.path() {
                    let status = entry.status();
                    let status_str = if status.is_wt_new() || status.is_index_new() {
                        "new"
                    } else if status.is_wt_modified() || status.is_index_modified() {
                        "modified"
                    } else if status.is_wt_deleted() || status.is_index_deleted() {
                        "deleted"
                    } else if status.is_wt_renamed() || status.is_index_renamed() {
                        "renamed"
                    } else {
                        "unknown"
                    };

                    files.push(GitFileStatus {
                        path: path.to_string(),
                        status: status_str.to_string(),
                        staged: status.is_index_new() || status.is_index_modified()
                            || status.is_index_deleted() || status.is_index_renamed(),
                    });
                }
            }

            Ok(files)
        }).await
            .map_err(|e| AetherError::mcp(format!("Task join error: {}", e)))?
    }

    async fn log(&self, repo_path: &Path, limit: usize) -> Result<Vec<GitCommit>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Repository::open(&path)
                .map_err(|e| AetherError::mcp(format!("Failed to open repo: {}", e)))?;

            let mut revwalk = repo.revwalk()
                .map_err(|e| AetherError::mcp(format!("Failed to create revwalk: {}", e)))?;

            revwalk.push_head()
                .map_err(|e| AetherError::mcp(format!("Failed to push HEAD: {}", e)))?;

            let mut commits = Vec::new();
            for oid in revwalk.take(limit).flatten() {
                let commit = repo.find_commit(oid)
                    .map_err(|e| AetherError::mcp(format!("Failed to find commit: {}", e)))?;

                let author = commit.author();
                commits.push(GitCommit {
                    sha: oid.to_string(),
                    message: commit.message().unwrap_or("").trim().to_string(),
                    author: author.name().unwrap_or("").to_string(),
                    email: author.email().unwrap_or("").to_string(),
                    timestamp: commit.time().seconds(),
                });
            }

            Ok(commits)
        }).await
            .map_err(|e| AetherError::mcp(format!("Task join error: {}", e)))?
    }

    async fn diff(&self, repo_path: &Path, staged: bool) -> Result<Vec<GitDiff>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Repository::open(&path)
                .map_err(|e| AetherError::mcp(format!("Failed to open repo: {}", e)))?;

            let diff = if staged {
                let head_tree = repo.head().ok()
                    .and_then(|r| r.peel_to_tree().ok());
                repo.diff_tree_to_index(head_tree.as_ref(), None, None)
            } else {
                repo.diff_index_to_workdir(None, None)
            }.map_err(|e| AetherError::mcp(format!("Failed to get diff: {}", e)))?;

            let mut diffs = Vec::new();
            diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
                let file_path = delta.new_file().path()
                    .or_else(|| delta.old_file().path())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                let content = std::str::from_utf8(line.content())
                    .unwrap_or("")
                    .to_string();

                diffs.push(GitDiff {
                    file_path,
                    old_start: line.old_lineno().unwrap_or(0),
                    new_start: line.new_lineno().unwrap_or(0),
                    content,
                });
                true
            }).ok();

            Ok(diffs)
        }).await
            .map_err(|e| AetherError::mcp(format!("Task join error: {}", e)))?
    }

    async fn current_branch(&self, repo_path: &Path) -> Result<String> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Repository::open(&path)
                .map_err(|e| AetherError::mcp(format!("Failed to open repo: {}", e)))?;

            let head = repo.head()
                .map_err(|e| AetherError::mcp(format!("Failed to get HEAD: {}", e)))?;

            Ok(head.shorthand().unwrap_or("HEAD").to_string())
        }).await
            .map_err(|e| AetherError::mcp(format!("Task join error: {}", e)))?
    }

    async fn is_repo(&self, path: &Path) -> Result<bool> {
        let path = path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            Ok(Repository::open(&path).is_ok())
        }).await
            .map_err(|e| AetherError::mcp(format!("Task join error: {}", e)))?
    }
}
```

#### 0.4 SystemInfoProvider Trait (系统信息)

```rust
// Aether/core/src/services/system_info/mod.rs

use crate::error::Result;
use async_trait::async_trait;

/// System information
#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub hostname: String,
    pub username: String,
    pub home_dir: String,
    pub cpu_arch: String,
    pub memory_total: u64,     // bytes
    pub memory_available: u64, // bytes
}

/// System information provider trait
///
/// Abstracts system information queries.
/// macOS implementation uses CoreFoundation APIs.
#[async_trait]
pub trait SystemInfoProvider: Send + Sync {
    /// Get comprehensive system information
    async fn get_info(&self) -> Result<SystemInfo>;

    /// Get current active application (frontmost)
    async fn active_application(&self) -> Result<String>;

    /// Get window title of active application
    async fn active_window_title(&self) -> Result<String>;
}
```

```rust
// Aether/core/src/services/system_info/macos.rs

use super::{SystemInfo, SystemInfoProvider};
use crate::error::{AetherError, Result};
use async_trait::async_trait;

/// macOS system information provider
pub struct MacOsSystemInfo;

impl MacOsSystemInfo {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SystemInfoProvider for MacOsSystemInfo {
    async fn get_info(&self) -> Result<SystemInfo> {
        tokio::task::spawn_blocking(|| {
            let os_version = get_macos_version()?;
            let hostname = hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
            let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());

            Ok(SystemInfo {
                os_name: "macOS".to_string(),
                os_version,
                hostname,
                username,
                home_dir,
                cpu_arch: std::env::consts::ARCH.to_string(),
                memory_total: get_total_memory(),
                memory_available: get_available_memory(),
            })
        }).await
            .map_err(|e| AetherError::io(format!("Task join error: {}", e)))?
    }

    async fn active_application(&self) -> Result<String> {
        // Implementation uses NSWorkspace (requires objc bridging)
        // Simplified version returns "unknown"
        Ok("unknown".to_string())
    }

    async fn active_window_title(&self) -> Result<String> {
        Ok("unknown".to_string())
    }
}

fn get_macos_version() -> Result<String> {
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|e| AetherError::io(format!("Failed to get version: {}", e)))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn get_total_memory() -> u64 {
    // Use sysctl for macOS
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok();

    output
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

fn get_available_memory() -> u64 {
    // Simplified - in real implementation, use vm_statistics64
    0
}
```

### 1. 三层服务架构

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│ McpClient (Service Registry & Router)                                             │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────────┐ │
│  │ Layer 1: Builtin Services (MCP Adapters)                                    │ │
│  │ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌─────────────────────┐  │ │
│  │ │ FsService    │ │ GitService   │ │ SystemInfo   │ │ ShellService        │  │ │
│  │ │ (Adapter)    │ │ (Adapter)    │ │ (Adapter)    │ │ (独立实现)          │  │ │
│  │ │              │ │              │ │              │ │                     │  │ │
│  │ │ Tools:       │ │ Tools:       │ │ Tools:       │ │ Tools:              │  │ │
│  │ │ - file_read  │ │ - git_status │ │ - sys_info   │ │ - shell_exec        │  │ │
│  │ │ - file_write │ │ - git_log    │ │ - active_app │ │                     │  │ │
│  │ │ - file_list  │ │ - git_diff   │ │              │ │                     │  │ │
│  │ │ - file_search│ │ - git_branch │ │              │ │                     │  │ │
│  │ └──────┬───────┘ └──────┬───────┘ └──────┬───────┘ └─────────────────────┘  │ │
│  │        │                │                │                                   │ │
│  │        ↓                ↓                ↓                                   │ │
│  │ ┌────────────────────────────────────────────────────────────────────────┐  │ │
│  │ │ Shared Foundation (services/)                                          │  │ │
│  │ │ ┌────────────────┐ ┌────────────────┐ ┌─────────────────────────────┐  │  │ │
│  │ │ │ services::fs   │ │ services::git  │ │ services::system_info       │  │  │ │
│  │ │ │ FileOps trait  │ │ GitOps trait   │ │ SystemInfoProvider trait    │  │  │ │
│  │ │ │ LocalFs impl   │ │ GitRepository  │ │ MacOsSystemInfo impl        │  │  │ │
│  │ │ └────────────────┘ └────────────────┘ └─────────────────────────────┘  │  │ │
│  │ └────────────────────────────────────────────────────────────────────────┘  │ │
│  │ In-process calls, zero latency, zero dependencies                           │ │
│  └─────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────────┐ │
│  │ Layer 2: Bundled Servers (Future - Phase 4)                                 │ │
│  │ Pre-compiled binaries in Aether.app/Contents/Resources/mcp-servers/         │ │
│  │ ┌───────────────────┐ ┌───────────────────┐ ┌───────────────────────────┐   │ │
│  │ │ notion-server     │ │ slack-server      │ │ browser-server            │   │ │
│  │ │ (Bun compiled)    │ │ (Bun compiled)    │ │ (Bun compiled)            │   │ │
│  │ └───────────────────┘ └───────────────────┘ └───────────────────────────┘   │ │
│  │ Subprocess via StdioTransport, still zero user dependencies                 │ │
│  └─────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────────┐ │
│  │ Layer 3: External Servers (Advanced Users)                                  │ │
│  │ ┌─────────────────────────────────────────────────────────────────────────┐ │ │
│  │ │ StdioTransport → JSON-RPC 2.0 → User-configured Server                  │ │ │
│  │ │ Runtime detection: node/python/bun                                      │ │ │
│  │ │ Graceful degradation if runtime missing                                 │ │ │
│  │ └─────────────────────────────────────────────────────────────────────────┘ │ │
│  └─────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                   │
└──────────────────────────────────────────────────────────────────────────────────┘
```

### 2. 数据结构

#### 2.1 MCP Resource & Tool

```rust
// Aether/core/src/mcp/types.rs

/// MCP 资源 - 静态或动态数据源
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// 资源 URI (唯一标识)
    pub uri: String,
    /// 资源名称 (人类可读)
    pub name: String,
    /// 资源描述
    pub description: Option<String>,
    /// MIME 类型
    pub mime_type: Option<String>,
    /// 资源内容 (文本或 Base64)
    pub contents: String,
}

/// MCP 工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// 工具名称 (唯一标识，格式: service:tool_name)
    pub name: String,
    /// 工具描述
    pub description: String,
    /// 输入参数 Schema (JSON Schema 格式)
    pub input_schema: serde_json::Value,
    /// 是否需要用户确认
    pub requires_confirmation: bool,
    /// 所属服务
    pub service: String,
}

/// MCP 工具调用结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// 调用的工具名称
    pub tool_name: String,
    /// 是否成功
    pub success: bool,
    /// 结果内容
    pub content: Option<serde_json::Value>,
    /// 错误信息
    pub error: Option<String>,
}
```

#### 2.2 MCP Context

```rust
// 修改 Aether/core/src/payload/mod.rs

/// MCP 上下文数据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpContext {
    /// 读取的资源列表
    pub resources: Vec<McpResource>,
    /// 工具调用结果列表
    pub tool_results: Vec<McpToolResult>,
    /// 可用工具列表 (供 LLM 参考)
    pub available_tools: Vec<McpTool>,
}
```

### 3. 内置服务 Trait

```rust
// Aether/core/src/mcp/builtin/mod.rs

use async_trait::async_trait;
use crate::error::Result;
use super::types::{McpResource, McpTool, McpToolResult};

/// 内置 MCP 服务 trait
///
/// 实现此 trait 的服务直接在 Rust Core 中执行，无需启动子进程。
#[async_trait]
pub trait BuiltinMcpService: Send + Sync {
    /// 服务名称 (例如: "builtin:fs", "builtin:git")
    fn name(&self) -> &str;

    /// 服务描述
    fn description(&self) -> &str;

    /// 列出服务提供的资源
    async fn list_resources(&self) -> Result<Vec<McpResource>>;

    /// 读取指定资源
    async fn read_resource(&self, uri: &str) -> Result<McpResource>;

    /// 列出服务提供的工具
    fn list_tools(&self) -> Vec<McpTool>;

    /// 调用工具
    async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult>;

    /// 检查工具是否需要用户确认
    fn requires_confirmation(&self, tool_name: &str) -> bool;
}
```

### 4. 内置服务实现 (MCP 适配器)

内置服务作为 **MCP 适配器**，封装共享基础模块 (`services/`) 的功能。

#### 4.1 文件系统服务 (FsService) - MCP 适配器

```rust
// Aether/core/src/mcp/builtin/fs.rs

use super::BuiltinMcpService;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::services::fs::{FileOps, LocalFs};  // 依赖共享基础模块
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// MCP 文件系统服务适配器
///
/// 封装 services::fs::FileOps trait，添加 MCP 协议适配和路径安全检查。
pub struct FsService {
    /// 底层文件操作实现（来自 services::fs）
    fs: Arc<dyn FileOps>,
    /// 允许访问的根目录 (安全限制)
    allowed_roots: Vec<PathBuf>,
}

impl FsService {
    /// 创建使用默认 LocalFs 实现的服务
    pub fn new(allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            fs: Arc::new(LocalFs::new()),
            allowed_roots,
        }
    }

    /// 创建使用自定义 FileOps 实现的服务（便于测试）
    pub fn with_fs(fs: Arc<dyn FileOps>, allowed_roots: Vec<PathBuf>) -> Self {
        Self { fs, allowed_roots }
    }

    fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        self.allowed_roots.iter().any(|root| path.starts_with(root))
    }
}

#[async_trait]
impl BuiltinMcpService for FsService {
    fn name(&self) -> &str { "builtin:fs" }

    fn description(&self) -> &str {
        "File system operations (read, write, list, search)"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        // Return empty - resources are dynamic based on tool calls
        Ok(vec![])
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        let path = uri.strip_prefix("file://")
            .ok_or_else(|| AetherError::mcp("Invalid file URI"))?;

        let path = std::path::Path::new(path);
        if !self.is_path_allowed(path) {
            return Err(AetherError::mcp("Path not allowed"));
        }

        let contents = fs::read_to_string(path).await
            .map_err(|e| AetherError::mcp(format!("Read error: {}", e)))?;

        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(McpResource {
            uri: uri.to_string(),
            name,
            description: None,
            mime_type: Some("text/plain".to_string()),
            contents,
        })
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "file_read".to_string(),
                description: "Read contents of a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" }
                    },
                    "required": ["path"]
                }),
                requires_confirmation: false,
                service: self.name().to_string(),
            },
            McpTool {
                name: "file_write".to_string(),
                description: "Write contents to a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
                requires_confirmation: true, // Dangerous!
                service: self.name().to_string(),
            },
            McpTool {
                name: "file_list".to_string(),
                description: "List files in a directory".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "pattern": { "type": "string", "description": "Glob pattern" }
                    },
                    "required": ["path"]
                }),
                requires_confirmation: false,
                service: self.name().to_string(),
            },
        ]
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult> {
        let args = args.unwrap_or(json!({}));

        match tool_name {
            "file_read" => {
                let path: String = serde_json::from_value(args["path"].clone())
                    .map_err(|_| AetherError::mcp("Missing path"))?;

                let path = std::path::Path::new(&path);
                if !self.is_path_allowed(path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::fs
                match self.fs.read_file(path).await {
                    Ok(content) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: true,
                        content: Some(json!({ "content": content })),
                        error: None,
                    }),
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            "file_write" => {
                let path: String = serde_json::from_value(args["path"].clone())
                    .map_err(|_| AetherError::mcp("Missing path"))?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .map_err(|_| AetherError::mcp("Missing content"))?;

                let path = std::path::Path::new(&path);
                if !self.is_path_allowed(path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::fs
                match self.fs.write_file(path, &content).await {
                    Ok(()) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: true,
                        content: Some(json!({ "written": true })),
                        error: None,
                    }),
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            "file_list" => {
                let path: String = serde_json::from_value(args["path"].clone())
                    .map_err(|_| AetherError::mcp("Missing path"))?;

                let path = std::path::Path::new(&path);
                if !self.is_path_allowed(path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::fs
                match self.fs.list_dir(path).await {
                    Ok(entries) => {
                        let files: Vec<_> = entries.iter().map(|e| json!({
                            "name": e.name,
                            "is_dir": e.is_dir,
                            "size": e.size,
                        })).collect();

                        Ok(McpToolResult {
                            tool_name: tool_name.to_string(),
                            success: true,
                            content: Some(json!({ "entries": files })),
                            error: None,
                        })
                    }
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            _ => Ok(McpToolResult {
                tool_name: tool_name.to_string(),
                success: false,
                content: None,
                error: Some(format!("Unknown tool: {}", tool_name)),
            }),
        }
    }

    fn requires_confirmation(&self, tool_name: &str) -> bool {
        matches!(tool_name, "file_write" | "file_delete")
    }
}
```

#### 4.2 Git 服务 (GitService) - MCP 适配器

```rust
// Aether/core/src/mcp/builtin/git.rs

use super::BuiltinMcpService;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::services::git::{GitOps, GitRepository};  // 依赖共享基础模块
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// MCP Git 服务适配器
///
/// 封装 services::git::GitOps trait，添加 MCP 协议适配和路径安全检查。
pub struct GitService {
    /// 底层 Git 操作实现（来自 services::git）
    git: Arc<dyn GitOps>,
    /// 允许访问的仓库根目录
    allowed_repos: Vec<PathBuf>,
}

impl GitService {
    /// 创建使用默认 GitRepository 实现的服务
    pub fn new(allowed_repos: Vec<PathBuf>) -> Self {
        Self {
            git: Arc::new(GitRepository::new()),
            allowed_repos,
        }
    }

    /// 创建使用自定义 GitOps 实现的服务（便于测试）
    pub fn with_git(git: Arc<dyn GitOps>, allowed_repos: Vec<PathBuf>) -> Self {
        Self { git, allowed_repos }
    }

    fn is_repo_allowed(&self, path: &std::path::Path) -> bool {
        self.allowed_repos.iter().any(|r| path.starts_with(r))
    }
}

#[async_trait]
impl BuiltinMcpService for GitService {
    fn name(&self) -> &str { "builtin:git" }

    fn description(&self) -> &str {
        "Git repository operations using libgit2"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![])
    }

    async fn read_resource(&self, _uri: &str) -> Result<McpResource> {
        Err(AetherError::mcp("Use git tools instead"))
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "git_status".to_string(),
                description: "Get working directory status".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo_path": { "type": "string", "description": "Repository path" }
                    },
                    "required": ["repo_path"]
                }),
                requires_confirmation: false,
                service: self.name().to_string(),
            },
            McpTool {
                name: "git_log".to_string(),
                description: "Get commit history".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo_path": { "type": "string" },
                        "limit": { "type": "integer", "default": 10 }
                    },
                    "required": ["repo_path"]
                }),
                requires_confirmation: false,
                service: self.name().to_string(),
            },
            McpTool {
                name: "git_diff".to_string(),
                description: "Get diff of changes".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo_path": { "type": "string" },
                        "staged": { "type": "boolean", "default": false }
                    },
                    "required": ["repo_path"]
                }),
                requires_confirmation: false,
                service: self.name().to_string(),
            },
        ]
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult> {
        let args = args.unwrap_or(json!({}));

        match tool_name {
            "git_status" => {
                let repo_path: String = serde_json::from_value(args["repo_path"].clone())
                    .map_err(|_| AetherError::mcp("Missing repo_path"))?;

                let repo_path = PathBuf::from(&repo_path);
                if !self.is_repo_allowed(&repo_path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Repository path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::git
                match self.git.status(&repo_path).await {
                    Ok(statuses) => {
                        let files: Vec<_> = statuses.iter().map(|s| json!({
                            "path": s.path,
                            "status": s.status,
                            "staged": s.staged,
                        })).collect();

                        Ok(McpToolResult {
                            tool_name: tool_name.to_string(),
                            success: true,
                            content: Some(json!({ "files": files })),
                            error: None,
                        })
                    }
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            "git_log" => {
                let repo_path: String = serde_json::from_value(args["repo_path"].clone())
                    .map_err(|_| AetherError::mcp("Missing repo_path"))?;
                let limit: usize = args.get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10) as usize;

                let repo_path = PathBuf::from(&repo_path);
                if !self.is_repo_allowed(&repo_path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Repository path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::git
                match self.git.log(&repo_path, limit).await {
                    Ok(commits) => {
                        let commit_list: Vec<_> = commits.iter().map(|c| json!({
                            "sha": c.sha,
                            "message": c.message,
                            "author": c.author,
                            "time": c.timestamp,
                        })).collect();

                        Ok(McpToolResult {
                            tool_name: tool_name.to_string(),
                            success: true,
                            content: Some(json!({ "commits": commit_list })),
                            error: None,
                        })
                    }
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            "git_diff" => {
                let repo_path: String = serde_json::from_value(args["repo_path"].clone())
                    .map_err(|_| AetherError::mcp("Missing repo_path"))?;
                let staged: bool = args.get("staged")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let repo_path = PathBuf::from(&repo_path);
                if !self.is_repo_allowed(&repo_path) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Repository path not allowed".to_string()),
                    });
                }

                // 使用共享基础模块 services::git
                match self.git.diff(&repo_path, staged).await {
                    Ok(diffs) => {
                        let diff_content: String = diffs.iter()
                            .map(|d| format!("{}: {}", d.file_path, d.content))
                            .collect::<Vec<_>>()
                            .join("\n");

                        Ok(McpToolResult {
                            tool_name: tool_name.to_string(),
                            success: true,
                            content: Some(json!({ "diff": diff_content })),
                            error: None,
                        })
                    }
                    Err(e) => Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            _ => Ok(McpToolResult {
                tool_name: tool_name.to_string(),
                success: false,
                content: None,
                error: Some(format!("Unknown tool: {}", tool_name)),
            }),
        }
    }

    fn requires_confirmation(&self, tool_name: &str) -> bool {
        matches!(tool_name, "git_commit" | "git_push" | "git_reset")
    }
}
```

#### 4.3 Shell 服务 (ShellService) - 独立实现

Shell 服务由于安全性要求较高，直接使用 `tokio::process::Command`，不通过共享基础模块。

```rust
// Aether/core/src/mcp/builtin/shell.rs

use super::BuiltinMcpService;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

pub struct ShellService {
    /// 命令执行超时
    timeout: Duration,
    /// 允许执行的命令白名单 (空表示允许所有)
    allowed_commands: Vec<String>,
}

impl ShellService {
    pub fn new(timeout: Duration, allowed_commands: Vec<String>) -> Self {
        Self { timeout, allowed_commands }
    }

    fn is_command_allowed(&self, cmd: &str) -> bool {
        if self.allowed_commands.is_empty() {
            return true;
        }
        let program = cmd.split_whitespace().next().unwrap_or("");
        self.allowed_commands.iter().any(|c| c == program)
    }
}

#[async_trait]
impl BuiltinMcpService for ShellService {
    fn name(&self) -> &str { "builtin:shell" }

    fn description(&self) -> &str {
        "Execute shell commands with timeout and output capture"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![])
    }

    async fn read_resource(&self, _uri: &str) -> Result<McpResource> {
        Err(AetherError::mcp("Use shell tools instead"))
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "shell_exec".to_string(),
                description: "Execute a shell command".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Command to execute" },
                        "cwd": { "type": "string", "description": "Working directory" }
                    },
                    "required": ["command"]
                }),
                requires_confirmation: true, // Always confirm shell commands!
                service: self.name().to_string(),
            },
        ]
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult> {
        let args = args.unwrap_or(json!({}));

        match tool_name {
            "shell_exec" => {
                let command: String = serde_json::from_value(args["command"].clone())
                    .map_err(|_| AetherError::mcp("Missing command"))?;

                if !self.is_command_allowed(&command) {
                    return Ok(McpToolResult {
                        tool_name: tool_name.to_string(),
                        success: false,
                        content: None,
                        error: Some("Command not in whitelist".to_string()),
                    });
                }

                let cwd: Option<String> = args.get("cwd")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mut cmd = Command::new("sh");
                cmd.arg("-c").arg(&command);
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }

                let output = timeout(self.timeout, cmd.output()).await
                    .map_err(|_| AetherError::mcp("Command timed out"))?
                    .map_err(|e| AetherError::mcp(format!("Exec error: {}", e)))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                Ok(McpToolResult {
                    tool_name: tool_name.to_string(),
                    success: output.status.success(),
                    content: Some(json!({
                        "exit_code": output.status.code(),
                        "stdout": stdout,
                        "stderr": stderr,
                    })),
                    error: if output.status.success() { None } else { Some(stderr.to_string()) },
                })
            }
            _ => Ok(McpToolResult {
                tool_name: tool_name.to_string(),
                success: false,
                content: None,
                error: Some(format!("Unknown tool: {}", tool_name)),
            }),
        }
    }

    fn requires_confirmation(&self, _tool_name: &str) -> bool {
        true // All shell commands require confirmation
    }
}
```

### 5. McpClient (服务注册与路由)

```rust
// Aether/core/src/mcp/client.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::error::{AetherError, Result};
use super::builtin::BuiltinMcpService;
use super::external::McpServerConnection;
use super::types::{McpResource, McpTool, McpToolResult};
use super::config::McpConfig;

/// MCP 客户端 - 管理内置服务和外部服务器
pub struct McpClient {
    /// 内置服务 (Layer 1)
    builtin_services: HashMap<String, Arc<dyn BuiltinMcpService>>,

    /// 外部服务器连接 (Layer 3)
    external_servers: Arc<RwLock<HashMap<String, McpServerConnection>>>,

    /// 配置
    config: McpConfig,
}

impl McpClient {
    pub fn new(config: McpConfig) -> Self {
        Self {
            builtin_services: HashMap::new(),
            external_servers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// 注册内置服务
    pub fn register_builtin(&mut self, service: Arc<dyn BuiltinMcpService>) {
        self.builtin_services.insert(service.name().to_string(), service);
    }

    /// 启动外部服务器
    pub async fn start_external_servers(&self) -> Result<()> {
        let mut servers = self.external_servers.write().await;

        for (name, server_config) in &self.config.servers {
            // Check runtime availability
            if let Some(runtime) = &server_config.requires_runtime {
                if !Self::check_runtime(runtime) {
                    tracing::warn!(
                        server = %name,
                        runtime = %runtime,
                        "Required runtime not found, skipping server"
                    );
                    continue;
                }
            }

            match McpServerConnection::connect(server_config).await {
                Ok(conn) => {
                    tracing::info!(server = %name, "External MCP server started");
                    servers.insert(name.clone(), conn);
                }
                Err(e) => {
                    tracing::error!(server = %name, error = %e, "Failed to start external server");
                }
            }
        }

        Ok(())
    }

    /// 检查运行时是否可用
    fn check_runtime(runtime: &str) -> bool {
        let cmd = match runtime {
            "node" => "node",
            "python" => "python3",
            "bun" => "bun",
            _ => return false,
        };

        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// 列出所有可用工具 (聚合所有服务)
    pub async fn list_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();

        // Builtin services
        for service in self.builtin_services.values() {
            tools.extend(service.list_tools());
        }

        // External servers
        let servers = self.external_servers.read().await;
        for server in servers.values() {
            if let Ok(server_tools) = server.list_tools().await {
                tools.extend(server_tools);
            }
        }

        tools
    }

    /// 调用工具 (路由到对应服务)
    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult> {
        // First, try builtin services
        for service in self.builtin_services.values() {
            let service_tools = service.list_tools();
            if service_tools.iter().any(|t| t.name == tool_name) {
                return service.call_tool(tool_name, args).await;
            }
        }

        // Then, try external servers
        let servers = self.external_servers.read().await;
        for server in servers.values() {
            if server.has_tool(tool_name).await {
                return server.call_tool(tool_name, args).await;
            }
        }

        Err(AetherError::mcp(format!("Tool not found: {}", tool_name)))
    }

    /// 检查工具是否需要确认
    pub async fn requires_confirmation(&self, tool_name: &str) -> bool {
        // Check builtin services
        for service in self.builtin_services.values() {
            if service.list_tools().iter().any(|t| t.name == tool_name) {
                return service.requires_confirmation(tool_name);
            }
        }

        // Default: require confirmation for unknown tools
        true
    }

    /// 停止所有外部服务器
    pub async fn stop_all(&self) -> Result<()> {
        let mut servers = self.external_servers.write().await;
        for (name, server) in servers.drain() {
            tracing::info!(server = %name, "Stopping external MCP server");
            let _ = server.close().await;
        }
        Ok(())
    }
}
```

### 6. 配置格式

```toml
# ~/.aether/config.toml

[mcp]
enabled = true

# 内置服务配置
[mcp.builtin]
# 文件系统服务
[mcp.builtin.fs]
enabled = true
allowed_roots = ["~/Documents", "~/Desktop", "~/Downloads"]

# Git 服务
[mcp.builtin.git]
enabled = true
allowed_repos = ["~/projects", "~/work"]

# Shell 服务
[mcp.builtin.shell]
enabled = true
timeout_seconds = 30
allowed_commands = ["ls", "cat", "grep", "find", "wc"]  # 空 = 允许所有

# 外部服务器 (高级用户)
[mcp.servers.notion]
transport = "stdio"
command = "/path/to/bundled/notion-server"  # 预编译二进制
# 或者用户自行配置:
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-notion"]
# requires_runtime = "node"

# 权限设置
[mcp.permissions]
dangerous_tools = ["file_write", "file_delete", "shell_exec", "git_push"]
require_confirmation = true
```

```rust
// Aether/core/src/mcp/config.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub builtin: BuiltinConfig,

    #[serde(default)]
    pub servers: HashMap<String, ExternalServerConfig>,

    #[serde(default)]
    pub permissions: McpPermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuiltinConfig {
    #[serde(default)]
    pub fs: FsConfig,

    #[serde(default)]
    pub git: GitConfig,

    #[serde(default)]
    pub shell: ShellConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub allowed_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub allowed_repos: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    #[serde(default)]
    pub enabled: bool,  // Default false for safety

    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalServerConfig {
    pub transport: String,
    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Required runtime (node/python/bun)
    /// If specified, server will be skipped if runtime is not available
    pub requires_runtime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpPermissions {
    #[serde(default)]
    pub dangerous_tools: Vec<String>,

    #[serde(default = "default_true")]
    pub require_confirmation: bool,
}

fn default_true() -> bool { true }
fn default_timeout() -> u64 { 30 }

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_roots: vec![],
        }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_repos: vec![],
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            timeout_seconds: 30,
            allowed_commands: vec![],
        }
    }
}
```

## Risks / Trade-offs

### R1: git2-rs 库体积
- **风险**: `git2` crate 依赖 libgit2，会增加二进制体积约 2-3MB
- **缓解**: 可接受的代价，换取零运行时依赖

### R2: Shell 服务安全性
- **风险**: Shell 命令执行有潜在安全风险
- **缓解**:
  - 默认禁用 Shell 服务
  - 命令白名单机制
  - 强制用户确认
  - 执行超时保护

### R3: 外部服务器运行时检测
- **风险**: 用户可能绕过运行时检测
- **缓解**: 这是用户自行配置的高级功能，风险由用户承担

### R4: 路径安全
- **风险**: 文件系统/Git 服务可能访问敏感路径
- **缓解**: `allowed_roots`/`allowed_repos` 配置限制

## Migration Plan

### Phase 1: 向后兼容
- 空 `[mcp]` 配置下行为不变
- `AgentContext.mcp_resources` 默认为 None
- `McpStrategy.is_available()` 返回 false

### Phase 2: 内置服务启用
- 用户配置 `[mcp.builtin]` 后启用内置服务
- 默认只启用 fs 和 git 服务
- Shell 服务默认禁用

### Phase 3: 外部服务器
- 高级用户可配置 `[mcp.servers]`
- 运行时检测 + 友好提示

## Open Questions

### Q1: 是否需要支持更多内置服务？
- **当前决策**: MVP 只实现 fs、git、shell
- **未来考虑**: 根据用户反馈添加 clipboard、system-info 等

### Q2: Layer 2 捆绑服务何时实现？
- **当前决策**: Phase 4，视需求评估
- **考虑因素**: 每个捆绑服务增加 50-60MB 体积

### Q3: 是否需要工具调用历史/审计日志？
- **当前决策**: 不在 MVP 范围
- **未来考虑**: 安全审计需求
