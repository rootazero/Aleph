# Runtime Manager 设计文档

> 统一管理 Aether 外挂运行时（uv、fnm、yt-dlp 等）

**日期**: 2026-01-18
**状态**: 已批准

---

## 1. 概述

Aether 作为 AI Agent 中间件，需要支持 JS/TS 和 Python 运行时来执行本地 MCP server 和脚本。本设计引入统一的 Runtime Manager 模块，采用 trait-based 架构管理所有外挂运行时。

### 核心决策

| 决策点 | 选择 |
|--------|------|
| 管理粒度 | 只管理运行时，不管具体包依赖 |
| 安装时机 | 懒加载，首次使用时下载 |
| 目录结构 | 统一 `runtimes/` 目录 |
| 环境存放 | 自包含在 runtimes 内部 |
| yt-dlp 迁移 | 自动迁移到新结构 |
| 版本策略 | 单版本，保持简单 |
| 更新机制 | 启动时后台检查，提示但不强制 |

---

## 2. 目录结构

```
~/.aether/
├── config.toml
├── memory.db
├── models/
│   └── fastembed/           # 嵌入模型（保持现状）
├── skills/
├── logs/
└── runtimes/
    ├── manifest.json        # 运行时状态元数据
    ├── yt-dlp               # 单文件可执行
    ├── uv/
    │   ├── uv               # uv 二进制
    │   └── envs/
    │       └── default/     # 默认 Python venv
    └── fnm/
        ├── fnm              # fnm 二进制
        └── versions/
            └── default/     # 默认 Node 版本
```

### manifest.json 结构

```json
{
  "version": 1,
  "runtimes": {
    "yt-dlp": {
      "installed_at": "2026-01-15T10:30:00Z",
      "version": "2024.12.23",
      "last_update_check": "2026-01-18T08:00:00Z"
    },
    "uv": {
      "installed_at": "2026-01-16T14:00:00Z",
      "version": "0.5.14",
      "python_version": "3.12.1",
      "last_update_check": "2026-01-18T08:00:00Z"
    }
  }
}
```

---

## 3. Rust 模块结构

```
Aether/core/src/
├── runtimes/
│   ├── mod.rs              # 模块入口，导出公共 API
│   ├── manager.rs          # RuntimeManager trait 定义
│   ├── registry.rs         # 运行时注册表，统一入口
│   ├── manifest.rs         # manifest.json 读写
│   ├── download.rs         # 通用下载工具
│   ├── ytdlp.rs            # yt-dlp 实现
│   ├── uv.rs               # uv 实现
│   └── fnm.rs              # fnm 实现
```

---

## 4. 核心 Trait 定义

```rust
// manager.rs
use crate::error::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub version: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub download_url: String,
}

#[async_trait::async_trait]
pub trait RuntimeManager: Send + Sync {
    /// Runtime identifier (e.g., "yt-dlp", "uv", "fnm")
    fn id(&self) -> &'static str;

    /// Human-readable name
    fn name(&self) -> &'static str;

    /// Check if runtime is installed
    fn is_installed(&self) -> bool;

    /// Get executable path (may not exist if not installed)
    fn executable_path(&self) -> PathBuf;

    /// Install the runtime (lazy, called on first use)
    async fn install(&self) -> Result<()>;

    /// Check for updates (returns None if up-to-date or check fails)
    async fn check_update(&self) -> Option<UpdateInfo>;

    /// Update to latest version
    async fn update(&self) -> Result<()>;

    /// Get runtime info for UI display
    fn info(&self) -> RuntimeInfo;
}
```

---

## 5. RuntimeRegistry 统一入口

```rust
// registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct RuntimeRegistry {
    runtimes: HashMap<&'static str, Arc<dyn RuntimeManager>>,
    manifest: RwLock<Manifest>,
    runtimes_dir: PathBuf,
}

impl RuntimeRegistry {
    /// Initialize registry with all known runtimes
    pub fn new() -> Result<Self> {
        let runtimes_dir = get_runtimes_dir()?;
        let manifest = Manifest::load_or_default(&runtimes_dir)?;

        let mut runtimes = HashMap::new();
        runtimes.insert("yt-dlp", Arc::new(YtDlpRuntime::new(&runtimes_dir)) as _);
        runtimes.insert("uv", Arc::new(UvRuntime::new(&runtimes_dir)) as _);
        runtimes.insert("fnm", Arc::new(FnmRuntime::new(&runtimes_dir)) as _);

        Ok(Self { runtimes, manifest: RwLock::new(manifest), runtimes_dir })
    }

    /// Get runtime by id, auto-install if needed
    pub async fn require(&self, id: &str) -> Result<Arc<dyn RuntimeManager>> {
        let runtime = self.runtimes.get(id)
            .ok_or_else(|| AetherError::config(format!("Unknown runtime: {}", id)))?;

        if !runtime.is_installed() {
            runtime.install().await?;
            self.manifest.write().await.mark_installed(id)?;
        }

        Ok(Arc::clone(runtime))
    }

    /// List all runtimes with their status
    pub fn list(&self) -> Vec<RuntimeInfo> {
        self.runtimes.values().map(|r| r.info()).collect()
    }

    /// Background update check (called on app startup)
    pub async fn check_updates(&self) -> Vec<UpdateInfo> {
        // 检查上次检查时间，避免频繁请求
        // 并行检查所有已安装运行时
    }
}
```

### 使用示例

```rust
// 在 youtube.rs 中使用
let registry = RuntimeRegistry::new()?;
let ytdlp = registry.require("yt-dlp").await?;
let executable = ytdlp.executable_path();

// 在 MCP Python server 启动时使用
let uv = registry.require("uv").await?;
let python = uv.executable_path(); // 返回 venv 中的 python
```

---

## 6. 具体运行时实现

### yt-dlp（单文件类型）

```rust
// ytdlp.rs
pub struct YtDlpRuntime {
    runtimes_dir: PathBuf,
}

impl YtDlpRuntime {
    const DOWNLOAD_URL: &'static str =
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";
    const RELEASES_API: &'static str =
        "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
}

#[async_trait]
impl RuntimeManager for YtDlpRuntime {
    fn id(&self) -> &'static str { "yt-dlp" }
    fn name(&self) -> &'static str { "yt-dlp" }

    fn executable_path(&self) -> PathBuf {
        self.runtimes_dir.join("yt-dlp")
    }

    fn is_installed(&self) -> bool {
        self.executable_path().exists()
    }

    async fn install(&self) -> Result<()> {
        let path = self.executable_path();
        download_file(Self::DOWNLOAD_URL, &path).await?;
        set_executable(&path)?;
        Ok(())
    }

    async fn check_update(&self) -> Option<UpdateInfo> {
        // 调用 GitHub API 获取最新版本
        // 与 manifest 中记录的版本比较
    }
}
```

### uv（带环境管理）

```rust
// uv.rs
pub struct UvRuntime {
    runtimes_dir: PathBuf,
}

impl UvRuntime {
    fn uv_dir(&self) -> PathBuf {
        self.runtimes_dir.join("uv")
    }

    fn uv_binary(&self) -> PathBuf {
        self.uv_dir().join("uv")
    }

    fn default_venv(&self) -> PathBuf {
        self.uv_dir().join("envs").join("default")
    }

    /// Get python executable in default venv
    pub fn python_path(&self) -> PathBuf {
        self.default_venv().join("bin").join("python")
    }
}

#[async_trait]
impl RuntimeManager for UvRuntime {
    async fn install(&self) -> Result<()> {
        // 1. 下载 uv 二进制
        let uv_url = Self::get_download_url()?; // 根据 arch 选择
        download_file(&uv_url, &self.uv_binary()).await?;
        set_executable(&self.uv_binary())?;

        // 2. 创建默认 Python 环境
        Command::new(&self.uv_binary())
            .args(["venv", self.default_venv().to_str().unwrap()])
            .output()?;

        Ok(())
    }

    fn executable_path(&self) -> PathBuf {
        // 返回 Python 路径而非 uv 路径
        self.python_path()
    }
}
```

---

## 7. 迁移逻辑

### yt-dlp 迁移

```rust
impl YtDlpRuntime {
    pub fn migrate_if_needed(&self) -> Result<()> {
        let old_path = get_config_dir()?.join("yt-dlp");
        let new_path = self.executable_path();

        if old_path.exists() && !new_path.exists() {
            fs::create_dir_all(&self.runtimes_dir)?;
            fs::rename(&old_path, &new_path)?;
            tracing::info!("Migrated yt-dlp to new location: {:?}", new_path);
        }

        Ok(())
    }
}
```

### Registry 初始化时触发

```rust
impl RuntimeRegistry {
    pub fn new() -> Result<Self> {
        let runtimes_dir = get_runtimes_dir()?;
        fs::create_dir_all(&runtimes_dir)?;

        // 迁移旧版本文件
        YtDlpRuntime::new(&runtimes_dir).migrate_if_needed()?;

        // ...
    }
}
```

---

## 8. 更新机制

### 启动时后台检查

```rust
impl AetherCore {
    pub async fn init() -> Result<Self> {
        let runtime_registry = RuntimeRegistry::new()?;

        // 后台检查更新（不阻塞启动）
        let registry_clone = runtime_registry.clone();
        tokio::spawn(async move {
            if registry_clone.should_check_updates() {
                let updates = registry_clone.check_updates().await;
                if !updates.is_empty() {
                    notify_updates_available(updates);
                }
            }
        });

        // ...
    }
}
```

### 检查节流（24 小时间隔）

```rust
impl Manifest {
    const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

    pub fn should_check_updates(&self) -> bool {
        match self.last_update_check {
            Some(last) => last.elapsed() > Self::UPDATE_CHECK_INTERVAL,
            None => true,
        }
    }
}
```

---

## 9. 错误处理

```rust
// error.rs 新增
pub enum AetherError {
    #[error("Runtime error: {message}")]
    Runtime {
        message: String,
        runtime_id: String,
        recoverable: bool,
    },
}

impl AetherError {
    pub fn runtime(id: &str, msg: impl Into<String>) -> Self {
        Self::Runtime {
            message: msg.into(),
            runtime_id: id.to_string(),
            recoverable: true,
        }
    }
}
```

---

## 10. UniFFI 导出

```
// aether.udl
interface RuntimeRegistry {
    constructor();

    [Throws=AetherError]
    sequence<RuntimeInfo> list_runtimes();

    [Throws=AetherError]
    boolean is_installed(string runtime_id);

    [Throws=AetherError, Async]
    void install_runtime(string runtime_id);

    [Throws=AetherError, Async]
    sequence<UpdateInfo> check_updates();

    [Throws=AetherError, Async]
    void update_runtime(string runtime_id);
};

dictionary RuntimeInfo {
    string id;
    string name;
    string description;
    string? version;
    boolean installed;
};

dictionary UpdateInfo {
    string runtime_id;
    string current_version;
    string latest_version;
};
```

---

## 11. 实施计划

| 阶段 | 内容 | 依赖 |
|------|------|------|
| **Phase 1** | 创建 `runtimes/` 模块骨架，实现 trait 和 registry | 无 |
| **Phase 2** | 实现 `YtDlpRuntime`，包含迁移逻辑 | Phase 1 |
| **Phase 3** | 改造 `youtube.rs` 使用新 API | Phase 2 |
| **Phase 4** | 实现 `UvRuntime` | Phase 1 |
| **Phase 5** | 实现 `FnmRuntime` | Phase 1 |
| **Phase 6** | UniFFI 导出 + Swift 设置界面 | Phase 1-5 |
| **Phase 7** | 更新检查机制 + UI 提示 | Phase 6 |

---

## 12. 文件变更清单

**新增：**
- `Aether/core/src/runtimes/mod.rs`
- `Aether/core/src/runtimes/manager.rs`
- `Aether/core/src/runtimes/registry.rs`
- `Aether/core/src/runtimes/manifest.rs`
- `Aether/core/src/runtimes/download.rs`
- `Aether/core/src/runtimes/ytdlp.rs`
- `Aether/core/src/runtimes/uv.rs`
- `Aether/core/src/runtimes/fnm.rs`

**修改：**
- `Aether/core/src/lib.rs` - 添加 `mod runtimes`
- `Aether/core/src/video/youtube.rs` - 使用新 API
- `Aether/core/src/aether.udl` - UniFFI 接口
- `Aether/core/src/error.rs` - 新增 Runtime 错误类型
