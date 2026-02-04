# Unified Initialization Module Design

**Date**: 2026-01-19
**Status**: Approved
**Author**: Claude (Brainstorming Session)

## Overview

Refactor the initialization module to consolidate all first-launch tasks into a unified, blocking initialization flow. This includes runtime environments, embedding models, memory database, and built-in skills.

## Goals

1. **Unified Experience**: Single blocking progress window on first launch
2. **Complete Installation**: All components downloaded before app is usable
3. **Clean Architecture**: Single `InitializationCoordinator` managing all phases
4. **Atomic Operations**: Any failure triggers full rollback
5. **Code Cleanup**: Remove all scattered initialization logic

## Directory Structure

```
~/.aleph/
├── config.toml           # Main configuration
├── memory.db             # SQLite vector database
├── logs/                 # Log files
├── cache/                # Temporary cache
├── skills/               # User skills directory
│   └── builtin/          # Built-in skills (copied from bundle)
├── models/               # Model directory (NEW location)
│   └── bge-small-zh-v1.5/
│       ├── model.onnx
│       ├── tokenizer.json
│       └── config.json
└── runtimes/             # Runtime environments
    ├── manifest.json     # Version metadata
    ├── ffmpeg/
    │   └── ffmpeg
    ├── yt-dlp/
    │   └── yt-dlp
    ├── uv/
    │   ├── uv
    │   └── python/
    └── fnm/
        ├── fnm
        └── node-vXX/
```

## Architecture

### InitializationCoordinator

```rust
pub struct InitializationCoordinator {
    progress_handler: Option<Arc<dyn InitProgressHandler>>,
    config_dir: PathBuf,
}

pub struct InitializationResult {
    pub success: bool,
    pub completed_steps: Vec<String>,
    pub error_step: Option<String>,
    pub error_message: Option<String>,
}

pub trait InitProgressHandler: Send + Sync {
    fn on_phase_started(&self, phase_name: String, current: u32, total: u32);
    fn on_phase_progress(&self, phase_name: String, progress: f64, message: String);
    fn on_phase_completed(&self, phase_name: String);
    fn on_download_progress(&self, item: String, downloaded: u64, total: u64);
    fn on_error(&self, step: String, message: String, is_retryable: bool);
}
```

### Initialization Phases

| Phase | Name | Description |
|-------|------|-------------|
| 1 | Directories | Create all required directories |
| 2 | Config | Generate default config.toml |
| 3 | EmbeddingModel | Download bge-small-zh-v1.5 to models/ |
| 4 | Database | Initialize memory.db schema |
| 5 | Runtimes | Parallel download: ffmpeg, yt-dlp, uv, fnm |
| 6 | Skills | Copy built-in skills from app bundle |

### Phase 5: Parallel Runtime Download

```rust
async fn download_runtimes(&self) -> Result<(), InitError> {
    let runtimes = ["ffmpeg", "yt-dlp", "uv", "fnm"];

    let handles: Vec<_> = runtimes.iter().map(|id| {
        tokio::spawn(async move {
            registry.install(id, handler).await
        })
    }).collect();

    let results = futures::future::join_all(handles).await;
    // Any failure returns error
}
```

## FFmpeg Runtime Manager

New file: `core/src/runtimes/ffmpeg.rs`

### Download Sources

| Platform | Source |
|----------|--------|
| macOS | https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip |
| Windows | https://github.com/BtbN/FFmpeg-Builds/releases |
| Linux | https://johnvansickle.com/ffmpeg/releases |

### Implementation

```rust
impl RuntimeManager for FfmpegManager {
    fn id(&self) -> &str { "ffmpeg" }
    fn display_name(&self) -> &str { "FFmpeg" }
    fn is_installed(&self) -> bool;
    fn get_version(&self) -> Option<String>;
    fn get_binary_path(&self) -> Option<PathBuf>;
    async fn install(&self) -> Result<()>;
    async fn check_update(&self) -> Result<Option<String>>;
}
```

## FFI Interface

New file: `core/src/ffi/initialization.rs`

```rust
#[uniffi::export]
pub fn needs_first_time_init() -> bool;

#[uniffi::export]
pub fn run_first_time_init(
    handler: Arc<dyn InitProgressHandler>
) -> InitializationResult;
```

## Swift Integration

### Simplified AppDelegate

```swift
func applicationDidFinishLaunching(_ notification: Notification) {
    setupBasicUI()

    guard checkPermissions() else {
        showPermissionGate()
        return
    }

    if needsFirstTimeInit() {
        showInitializationWindow()
    } else {
        initializeAppComponents()
    }
}
```

### InitializationWindow

- NSPanel, 480x320, non-closable
- Shows progress for all 6 phases
- Displays download progress for large files
- Error view with retry button on failure

## Rollback Mechanism

On any phase failure:
1. Stop execution immediately
2. Report error to handler
3. Delete created files/directories in reverse order
4. Return failure result with error details

```rust
async fn rollback(&self, completed_steps: &[String]) -> Result<(), InitError> {
    for step in completed_steps.iter().rev() {
        match step.as_str() {
            "directories" => fs::remove_dir_all(&self.config_dir)?,
            "config" => fs::remove_file(config_path)?,
            "embedding_model" => fs::remove_dir_all(models_dir)?,
            "database" => fs::remove_file(db_path)?,
            "runtimes" => fs::remove_dir_all(runtimes_dir)?,
            "skills" => fs::remove_dir_all(skills_dir)?,
            _ => {}
        }
    }
    Ok(())
}
```

## Files to Change

### New Files (5)

| File | Purpose |
|------|---------|
| `core/src/initialization/mod.rs` | Module exports |
| `core/src/initialization/coordinator.rs` | Main coordinator |
| `core/src/initialization/steps.rs` | Phase implementations |
| `core/src/runtimes/ffmpeg.rs` | FFmpeg runtime manager |
| `core/src/ffi/initialization.rs` | FFI exports |

### Modified Files (5)

| File | Changes |
|------|---------|
| `core/src/runtimes/mod.rs` | Export ffmpeg module |
| `core/src/runtimes/registry.rs` | Register FfmpegManager |
| `core/src/ffi/mod.rs` | Remove old init functions |
| `core/src/utils/paths.rs` | Add get_models_dir() |
| `AppDelegate.swift` | Simplify to call new FFI |

### Deleted Files (1)

| File | Reason |
|------|--------|
| `core/src/initialization.rs` | Replaced by initialization/ module |

### Rewritten Files (2)

| File | Changes |
|------|---------|
| `InitializationWindow.swift` | New blocking panel implementation |
| `InitializationProgressView.swift` | Renamed to InitializationView.swift |

## Code to Remove

| Location | Code | Reason |
|----------|------|--------|
| `core/src/initialization.rs` | Entire file | Replaced |
| `core/src/ffi/mod.rs` | `is_fresh_install()` | Moved to coordinator |
| `core/src/ffi/mod.rs` | `run_first_time_init()` old impl | New FFI |
| `core/src/ffi/mod.rs` | `check_embedding_model_exists()` | Moved to coordinator |
| `AppDelegate.swift` | `checkAndRunFirstTimeInit()` | Simplified |
| `AppDelegate.swift` | `isFreshInstall()` | Rust handles this |
| `AppDelegate.swift` | Background DispatchQueue init code | Now blocking |

## UI Design

### Progress Window (480x320)

```
┌─────────────────────────────────────────┐
│              [App Icon]                 │
│                                         │
│         正在初始化 Aleph               │
│                                         │
│           安装运行时环境                │
│         正在安装: FFmpeg                │
│                                         │
│  ████████████████░░░░░░░░░  65%        │
│            步骤 5/6                     │
│                                         │
│  下载: ffmpeg              45.2 MB     │
│  ████████████░░░░░░░░░░░░  52%         │
│                                         │
│  首次启动需要下载必要组件，请保持网络连接 │
└─────────────────────────────────────────┘
```

### Error View

```
┌─────────────────────────────────────────┐
│              ⚠️                         │
│                                         │
│           初始化失败                    │
│                                         │
│      失败步骤: embedding_model          │
│      网络连接超时                       │
│                                         │
│           [ 重试 ]                      │
│                                         │
│      请检查网络连接后重试               │
└─────────────────────────────────────────┘
```

## Implementation Order

1. Create `core/src/initialization/` module structure
2. Implement `InitializationCoordinator` with all phases
3. Add `FfmpegManager` to runtimes
4. Create new FFI exports
5. Update Swift UI components
6. Delete old initialization code
7. Test full flow

## Testing Checklist

- [ ] Fresh install triggers initialization window
- [ ] All 6 phases complete successfully
- [ ] Progress UI updates correctly
- [ ] Download progress shows for large files
- [ ] Network failure triggers rollback
- [ ] Retry after failure works
- [ ] Existing installation skips initialization
- [ ] All runtimes installed and functional
- [ ] Embedding model in correct location
- [ ] Memory database initialized with schema
