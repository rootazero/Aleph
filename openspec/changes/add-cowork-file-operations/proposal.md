# Change: Add Cowork File Operations Executor

## Why

Phase 1 建立了 Cowork 任务编排框架，但只有 NoopExecutor 用于测试。用户无法实际执行任何有用的任务。Phase 2 需要实现真正的文件操作执行器，让用户能够：

1. 读取文件内容进行分析
2. 写入生成的文档或报告
3. 批量移动/重命名文件进行整理
4. 搜索文件系统查找特定内容

这是 Cowork 从"演示框架"到"实用工具"的关键一步。

## What Changes

### Core Implementation

- 新增 `FileOpsExecutor` 实现 `TaskExecutor` trait
- 支持 Read、Write、Move、Copy、Delete、Search 操作
- 实现路径白名单权限控制
- 支持批量操作和并行 IO
- 进度报告集成到 TaskMonitor

### Security

- **BREAKING**: 需要配置 `allowed_paths` 才能执行文件操作
- 敏感路径（~/.ssh, ~/.gnupg）默认禁止
- 高风险操作（Delete）需要额外确认

### Configuration

```toml
[cowork.file_ops]
enabled = true
allowed_paths = ["~/Downloads", "~/Documents"]
denied_paths = ["~/.ssh", "~/.gnupg", "~/.config/aether"]
max_file_size = "100MB"
require_confirmation_for_write = true
require_confirmation_for_delete = true
```

## Impact

- Affected specs: cowork-file-operations (new)
- Affected code:
  - `core/src/cowork/executor/` - 新增 file_ops.rs
  - `core/src/config/types/cowork.rs` - 扩展配置
  - Swift Settings UI - 新增文件操作配置面板
