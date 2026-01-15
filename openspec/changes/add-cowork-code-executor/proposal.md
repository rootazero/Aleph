# Change: Add Cowork Code Execution Executor

## Why

Phase 2 实现了文件操作执行器，让用户可以读写文件。但很多复杂任务需要执行代码来处理数据，例如：

1. 运行 Python 脚本分析 CSV 数据
2. 执行 Shell 命令处理文件
3. 运行 Node.js 脚本生成图表
4. 执行数据转换和格式化

没有代码执行能力，Cowork 只能处理简单的文件操作，无法完成复杂的自动化任务。

## What Changes

### Core Implementation

- 新增 `CodeExecutor` 实现 `TaskExecutor` trait
- 支持多种运行时：Shell (bash/zsh)、Python、Node.js
- 实现沙箱执行环境（资源限制）
- 支持执行超时和进程管理
- 捕获 stdout/stderr 输出

### Security Model

- **BREAKING**: 需要显式启用代码执行 `[cowork.code_exec].enabled = true`
- 沙箱模式默认开启，限制：
  - 网络访问（可配置允许）
  - 文件系统访问（继承 file_ops 白名单）
  - 执行时间限制
  - 内存/CPU 限制
- 高风险命令黑名单（rm -rf、sudo 等）
- 每次执行前显示命令预览

### Supported Runtimes

| Runtime | Command | Use Case |
|---------|---------|----------|
| Shell | `bash`, `zsh` | 系统命令、文件处理 |
| Python | `python3` | 数据分析、脚本 |
| Node.js | `node` | Web 相关、JSON 处理 |

### Configuration

```toml
[cowork.code_exec]
# Enable code execution
enabled = false

# Default runtime
default_runtime = "shell"

# Execution timeout in seconds
timeout_seconds = 60

# Enable sandboxed execution
sandbox_enabled = true

# Allowed runtimes (empty = all)
allowed_runtimes = ["shell", "python"]

# Network access in sandbox
allow_network = false

# Working directory for executions
working_directory = "~/Downloads"

# Environment variables to pass
pass_env = ["PATH", "HOME"]

# Blocked commands (always denied)
blocked_commands = ["rm -rf /", "sudo", "chmod 777"]
```

## Impact

- Affected specs: cowork-execution (new)
- Affected code:
  - `core/src/cowork/executor/` - 新增 code_exec.rs
  - `core/src/config/types/cowork.rs` - 扩展配置
  - Swift Settings UI - 新增代码执行配置面板
- Security implications:
  - 代码执行是高风险操作，默认禁用
  - 需要完整的沙箱和权限控制
