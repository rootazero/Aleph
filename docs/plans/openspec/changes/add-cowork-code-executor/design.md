# Design: Code Execution Executor

## Context

Cowork 需要代码执行能力来处理复杂任务。这涉及重大安全考虑：

- 用户可能执行恶意代码
- AI 生成的代码可能有漏洞
- 需要平衡功能性和安全性

目标用户是开发者和高级用户，他们理解代码执行的风险但需要自动化能力。

## Goals / Non-Goals

**Goals:**
- 安全执行用户指定的代码片段
- 支持常用运行时（Shell、Python、Node.js）
- 提供沙箱隔离和资源限制
- 捕获执行输出供后续任务使用
- 与 FileOpsExecutor 权限系统集成

**Non-Goals:**
- 不支持长时间运行的守护进程
- 不支持交互式命令（需要用户输入）
- 不实现完整的容器化隔离
- 不支持 GUI 应用程序执行

## Decisions

### 1. 沙箱实现方式

**决定**: 使用 macOS sandbox-exec 进行轻量级沙箱

**原因**:
- macOS 原生支持，无需额外依赖
- 可配置文件系统、网络访问权限
- 性能开销小

**替代方案考虑**:
- Docker 容器：太重量级，需要用户安装 Docker
- nsjail/firejail：Linux 专用，不适合 macOS
- 无沙箱：安全风险太高

**实现**:
```rust
// 沙箱配置文件模板
const SANDBOX_PROFILE: &str = r#"
(version 1)
(deny default)
(allow process-fork)
(allow process-exec)
(allow file-read* (subpath "{allowed_paths}"))
(allow file-write* (subpath "{allowed_paths}"))
{network_rule}
"#;
```

### 2. 运行时检测

**决定**: 使用 `which` 命令检测运行时可用性

**原因**:
- 简单可靠
- 尊重用户的 PATH 配置
- 支持自定义解释器路径

**实现**:
```rust
pub struct RuntimeInfo {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub available: bool,
}

impl RuntimeInfo {
    pub async fn detect(runtime: &str) -> Self {
        let path = Command::new("which")
            .arg(runtime)
            .output()
            .await;
        // ...
    }
}
```

### 3. 执行输出处理

**决定**: 分离 stdout 和 stderr，带大小限制

**原因**:
- 便于调试和日志分析
- 防止大量输出耗尽内存
- 符合 Unix 惯例

**限制**:
- stdout: 最大 10MB
- stderr: 最大 1MB
- 超过限制截断并标记

### 4. 超时机制

**决定**: 使用 tokio::time::timeout 包装执行

**原因**:
- 异步友好
- 可取消执行
- 配置灵活

**实现**:
```rust
let result = tokio::time::timeout(
    Duration::from_secs(config.timeout_seconds),
    execute_with_sandbox(command, &sandbox_config),
).await;
```

### 5. 危险命令检测

**决定**: 黑名单 + 模式匹配

**默认黑名单**:
- `rm -rf /`
- `sudo *`
- `chmod 777`
- `:(){ :|:& };:` (fork bomb)
- `> /dev/sda`
- `mkfs.*`

**实现**:
```rust
fn is_dangerous_command(cmd: &str) -> bool {
    DANGEROUS_PATTERNS.iter().any(|p| p.is_match(cmd))
}
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      CodeExecutor                            │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ RuntimeMgr   │  │ SandboxMgr   │  │ OutputCapture    │   │
│  │ (detection)  │  │ (profile)    │  │ (stdout/stderr)  │   │
│  └──────────────┘  └──────────────┘  └──────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ CommandCheck │  │ TimeoutMgr   │  │ ProcessMonitor   │   │
│  │ (blocklist)  │  │ (limits)     │  │ (kill/cleanup)   │   │
│  └──────────────┘  └──────────────┘  └──────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   sandbox-exec (macOS)                       │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Runtime Process (bash/python/node)                   │   │
│  │  - Limited file access                                │   │
│  │  - No network (unless allowed)                        │   │
│  │  - Resource limits                                    │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Data Flow

```
1. Task received with CodeExecution type
   │
   ▼
2. Validate command against blocklist
   │ ✗ → Return error: "Dangerous command blocked"
   │
   ▼
3. Detect runtime availability
   │ ✗ → Return error: "Runtime not available"
   │
   ▼
4. Check file paths against permission system
   │ ✗ → Return error: "Path not allowed"
   │
   ▼
5. Generate sandbox profile
   │
   ▼
6. Execute with sandbox-exec + timeout
   │
   ├── stdout → OutputCapture (with size limit)
   ├── stderr → OutputCapture (with size limit)
   │
   ▼
7. Collect exit code + outputs
   │
   ▼
8. Return CodeExecutionResult
```

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| Sandbox escape | 使用 macOS 原生沙箱，定期更新配置 |
| 资源耗尽 | 强制超时，输出大小限制 |
| 恶意代码 | 命令黑名单，文件访问白名单 |
| 依赖缺失 | 运行时检测，清晰错误消息 |

**Trade-off: 安全 vs 功能**
- 默认禁用代码执行
- 沙箱默认开启
- 网络默认禁止
- 用户需要显式启用和配置

## Migration Plan

1. 实现 CodeExecutor（沙箱模式）
2. 添加配置类型
3. 集成到 CoworkEngine
4. 添加 Swift UI 设置
5. 文档和示例

无需迁移现有数据，纯新增功能。

## Open Questions

1. **Windows 支持**: Windows 没有 sandbox-exec，是否使用 Job Objects？
   - 初始版本仅支持 macOS
   - Windows 可在 Phase 4 添加

2. **虚拟环境**: Python 执行是否支持指定 venv？
   - 可通过 `pass_env` 传递 VIRTUAL_ENV
   - 未来可添加专门的 venv 配置

3. **脚本文件 vs 内联代码**: 是否支持执行文件？
   - 初始支持内联代码
   - 文件执行可通过 FileOps + CodeExec 组合实现
