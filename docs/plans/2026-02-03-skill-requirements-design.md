# Skill Requirements & CLI Wrapper 架构设计

> 日期: 2026-02-03
> 状态: Draft
> 作者: Claude + User

## 概述

本设计为 Aether Skills 系统引入三项增强能力：

1. **元数据扩展** — 依赖声明、安装指令、UI 元数据
2. **健康检查机制** — Pre-flight Check，主动安装工作流
3. **CLI Wrapper Skill** — 安全封装 CLI 工具，复用 exec 审批流

### 设计目标

- **向后兼容** — 现有 SKILL.md 文件无需修改
- **主动修复** — Agent 检测缺失依赖后主动提示安装
- **安全优先** — CLI 执行复用现有 exec 审批机制
- **跨平台** — 支持 macOS (brew)、Linux (apt)、Windows (winget)

### 参考对比

| 特性 | Aether (现有) | OpenClaw | Aether (本设计) |
|------|--------------|----------|----------------|
| 依赖声明 | ❌ | ✅ bins | ✅ requirements.binaries |
| 安装指令 | ❌ | ✅ install | ✅ requirements.install |
| UI 元数据 | ❌ | ✅ emoji | ✅ emoji, category |
| CLI 封装 | ❌ | ✅ 直接调用 | ✅ CLI Wrapper + exec 审批 |
| Skill 进化 | ✅ | ❌ | ✅ 保持 |
| 权限控制 | ✅ allowed-tools | ❌ | ✅ 增强 |

---

## 第一部分：扩展 SkillFrontmatter 元数据

### 新增类型定义

```rust
// core/src/skills/types.rs (新增文件)

use serde::{Deserialize, Serialize};

/// 包管理器类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    Brew,     // macOS Homebrew
    Apt,      // Debian/Ubuntu
    Winget,   // Windows
    Cargo,    // Rust (可选扩展)
    Pip,      // Python (可选扩展)
}

/// 单条安装指令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallCommand {
    pub manager: PackageManager,
    pub package: String,                    // e.g., "gh", "ffmpeg"
    #[serde(default)]
    pub args: Option<String>,               // e.g., "--cask" for brew
}

/// Skill 依赖声明
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// 必需的二进制文件 ["gh", "git"]
    #[serde(default)]
    pub binaries: Vec<String>,
    /// 支持的平台 ["macos", "linux", "windows"]
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    /// 安装指令（按包管理器分类）
    #[serde(default)]
    pub install: Vec<InstallCommand>,
}

/// Skill 健康状态
#[derive(Debug, Clone, PartialEq)]
pub enum SkillHealth {
    /// 所有依赖就绪
    Healthy,
    /// 部分依赖缺失
    Degraded { missing: Vec<String> },
    /// 当前平台不支持
    Unsupported,
}
```

### 扩展 SkillFrontmatter

```rust
// 修改 core/src/skills/mod.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(rename = "allowed-tools", default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<String>,

    // === 新增字段 ===
    /// UI 图标 "🐙"
    #[serde(default)]
    pub emoji: Option<String>,
    /// 依赖声明
    #[serde(default)]
    pub requirements: Option<SkillRequirements>,
    /// 分类标签 "developer", "media", "productivity"
    #[serde(default)]
    pub category: Option<String>,
    /// 是否为 CLI Wrapper Skill
    #[serde(rename = "cli-wrapper", default)]
    pub cli_wrapper: bool,
}
```

### SKILL.md 示例

```yaml
---
name: github
description: GitHub CLI operations - manage repos, PRs, issues
emoji: "🐙"
category: developer
cli-wrapper: true
allowed-tools: []
triggers:
  - github
  - gh
  - pull request
requirements:
  binaries: ["gh"]
  platforms: ["macos", "linux"]
  install:
    - manager: brew
      package: gh
    - manager: apt
      package: gh
---

# GitHub CLI Skill

Instructions here...
```

---

## 第二部分：健康检查机制

### HealthChecker 组件

```rust
// core/src/skills/health.rs (新增文件)

use std::process::Command;
use crate::skills::{Skill, SkillHealth, SkillRequirements};

pub struct HealthChecker;

impl HealthChecker {
    /// 检查单个二进制是否存在于 PATH
    pub fn check_binary(name: &str) -> bool {
        #[cfg(unix)]
        let result = Command::new("which")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        #[cfg(windows)]
        let result = Command::new("where")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        result
    }

    /// 检查当前平台是否在支持列表中
    pub fn check_platform(platforms: &Option<Vec<String>>) -> bool {
        let current = std::env::consts::OS; // "macos", "linux", "windows"
        platforms.as_ref()
            .map(|p| p.iter().any(|s| s == current))
            .unwrap_or(true) // 未声明平台 = 全平台支持
    }

    /// 综合检查 Skill 健康状态
    pub fn check_skill(skill: &Skill) -> SkillHealth {
        let Some(req) = &skill.frontmatter.requirements else {
            return SkillHealth::Healthy; // 无依赖声明 = 默认健康
        };

        // 1. 平台检查
        if !Self::check_platform(&req.platforms) {
            return SkillHealth::Unsupported;
        }

        // 2. 二进制检查
        let missing: Vec<String> = req.binaries.iter()
            .filter(|bin| !Self::check_binary(bin))
            .cloned()
            .collect();

        if missing.is_empty() {
            SkillHealth::Healthy
        } else {
            SkillHealth::Degraded { missing }
        }
    }

    /// 批量检查多个 Skills
    pub fn check_skills(skills: &[Skill]) -> Vec<(Skill, SkillHealth)> {
        skills.iter()
            .map(|s| (s.clone(), Self::check_skill(s)))
            .collect()
    }
}
```

### 集成到 SkillsRegistry

```rust
// 修改 core/src/skills/registry.rs

/// Skill 及其健康状态
#[derive(Debug, Clone)]
pub struct SkillWithHealth {
    pub skill: Skill,
    pub health: SkillHealth,
}

impl SkillsRegistry {
    /// 加载所有 Skills 并附带健康状态
    pub fn load_all_with_health(&self) -> Vec<SkillWithHealth> {
        self.load_all()
            .into_iter()
            .map(|skill| {
                let health = HealthChecker::check_skill(&skill);
                SkillWithHealth { skill, health }
            })
            .collect()
    }

    /// 仅返回健康的 Skills（用于系统提示注入）
    pub fn list_healthy_skills(&self) -> Vec<SkillMetadata> {
        self.load_all_with_health()
            .into_iter()
            .filter(|s| s.health == SkillHealth::Healthy)
            .map(|s| s.skill.into())
            .collect()
    }

    /// 返回需要修复的 Skills（用于主动提示）
    pub fn list_degraded_skills(&self) -> Vec<(Skill, Vec<String>)> {
        self.load_all_with_health()
            .into_iter()
            .filter_map(|s| match s.health {
                SkillHealth::Degraded { missing } => Some((s.skill, missing)),
                _ => None,
            })
            .collect()
    }
}
```

### 安装建议生成器

```rust
// 扩展 core/src/skills/installer.rs

impl SkillsInstaller {
    /// 根据当前平台生成安装命令
    pub fn suggest_install_command(req: &SkillRequirements) -> Option<String> {
        let os = std::env::consts::OS;

        req.install.iter()
            .find(|cmd| match (&cmd.manager, os) {
                (PackageManager::Brew, "macos") => true,
                (PackageManager::Apt, "linux") => true,
                (PackageManager::Winget, "windows") => true,
                _ => false,
            })
            .map(|cmd| {
                let base = match cmd.manager {
                    PackageManager::Brew => "brew install",
                    PackageManager::Apt => "sudo apt install -y",
                    PackageManager::Winget => "winget install",
                    _ => return None,
                };
                let args = cmd.args.as_deref().unwrap_or("");
                Some(format!("{} {} {}", base, args, cmd.package).trim().to_string())
            })
            .flatten()
    }

    /// 为缺失的依赖生成完整安装方案
    pub fn suggest_install_plan(skill: &Skill, missing: &[String]) -> Vec<String> {
        let Some(req) = &skill.frontmatter.requirements else {
            return vec![];
        };

        missing.iter()
            .filter_map(|bin| {
                req.install.iter()
                    .find(|cmd| cmd.package == *bin || req.binaries.contains(bin))
                    .and_then(|_| Self::suggest_install_command(req))
            })
            .collect()
    }
}
```

### 主动安装工作流

```
┌─────────────────┐
│  Skill 激活请求  │
└────────┬────────┘
         │
         ▼
┌─────────────────┐     Healthy
│ HealthChecker   │──────────────▶ 正常执行
└────────┬────────┘
         │ Degraded
         ▼
┌─────────────────┐
│ 生成安装建议     │  InstallSuggestion
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Clarification   │  "需要安装 gh 才能使用 GitHub Skill，
│ 询问用户        │   是否允许运行 brew install gh？"
└────────┬────────┘
         │ 用户同意
         ▼
┌─────────────────┐
│ exec 审批流      │  复用现有安全机制
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 重新检查健康状态 │
└────────┬────────┘
         │ Healthy
         ▼
      正常执行
```

---

## 第三部分：CLI Wrapper Skill

### 设计原则

1. **声明式限制** — Skill 只能执行其 `requirements.binaries` 声明的命令
2. **复用安全机制** — 所有命令通过现有 exec 审批流
3. **可信任列表** — 用户可配置 `skill_allowlist` 自动批准特定 Skill

### CliWrapperExecutor 组件

```rust
// core/src/skills/cli_wrapper.rs (新增文件)

use crate::exec::{ExecRequest, ExecApproval, SecurityPolicy};
use crate::skills::Skill;
use thiserror::Error;

/// CLI Wrapper 执行器
pub struct CliWrapperExecutor {
    policy: SecurityPolicy,
}

#[derive(Debug, Error)]
pub enum CliWrapperError {
    #[error("Skill is not a CLI wrapper")]
    NotCliWrapper,
    #[error("Empty command")]
    EmptyCommand,
    #[error("Unauthorized binary '{attempted}', allowed: {allowed:?}")]
    UnauthorizedBinary { attempted: String, allowed: Vec<String> },
    #[error("Command needs user approval: {reason}")]
    NeedsApproval { reason: String },
    #[error("Command denied: {reason}")]
    Denied { reason: String },
    #[error("Execution failed: {0}")]
    ExecError(#[from] crate::exec::ExecError),
}

impl CliWrapperExecutor {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }

    /// 验证命令是否符合 Skill 的 binary 限制
    pub fn validate_command(&self, skill: &Skill, command: &str) -> Result<(), CliWrapperError> {
        // 检查是否为 CLI Wrapper Skill
        if !skill.frontmatter.cli_wrapper {
            return Err(CliWrapperError::NotCliWrapper);
        }

        let Some(req) = &skill.frontmatter.requirements else {
            return Err(CliWrapperError::NotCliWrapper);
        };

        // 提取命令的第一个 token（binary name）
        let binary = command.split_whitespace()
            .next()
            .ok_or(CliWrapperError::EmptyCommand)?;

        // 检查是否在声明的 binaries 列表中
        if !req.binaries.contains(&binary.to_string()) {
            return Err(CliWrapperError::UnauthorizedBinary {
                attempted: binary.to_string(),
                allowed: req.binaries.clone(),
            });
        }

        Ok(())
    }

    /// 执行 CLI 命令（通过 exec 审批流）
    pub async fn execute(
        &self,
        skill: &Skill,
        command: &str,
        working_dir: Option<&str>,
    ) -> Result<ExecResult, CliWrapperError> {
        // 1. 验证命令合法性
        self.validate_command(skill, command)?;

        // 2. 构造 ExecRequest，附带 Skill 上下文
        let request = ExecRequest {
            command: command.to_string(),
            working_dir: working_dir.map(String::from),
            env: Default::default(),
            context: ExecContext::CliWrapperSkill {
                skill_id: skill.id.clone(),
                skill_name: skill.frontmatter.name.clone(),
            },
        };

        // 3. 提交到 exec 审批流
        let approval = self.policy.check(&request).await;

        match approval {
            ExecApproval::Allowed => {
                crate::exec::run_command(&request).await.map_err(Into::into)
            }
            ExecApproval::NeedsConfirmation { reason } => {
                Err(CliWrapperError::NeedsApproval { reason })
            }
            ExecApproval::Denied { reason } => {
                Err(CliWrapperError::Denied { reason })
            }
        }
    }
}
```

### 与 exec 安全系统集成

```rust
// 扩展 core/src/exec/policy.rs

/// Exec 上下文，用于细粒度权限控制
#[derive(Debug, Clone)]
pub enum ExecContext {
    /// 用户直接请求
    UserRequest,
    /// Agent 自主决策
    AgentAutonomous,
    /// CLI Wrapper Skill 触发
    CliWrapperSkill {
        skill_id: String,
        skill_name: String,
    },
}

impl Default for ExecContext {
    fn default() -> Self {
        Self::AgentAutonomous
    }
}

/// Exec 审批结果
#[derive(Debug, Clone)]
pub enum ExecApproval {
    Allowed,
    NeedsConfirmation { reason: String },
    Denied { reason: String },
}

impl SecurityPolicy {
    /// 检查 CLI Wrapper Skill 是否在 allowlist 中
    pub fn is_skill_allowed(&self, skill_id: &str) -> bool {
        self.config.skill_allowlist
            .as_ref()
            .map(|list| list.contains(&skill_id.to_string()))
            .unwrap_or(false)
    }

    pub async fn check(&self, request: &ExecRequest) -> ExecApproval {
        match &request.context {
            ExecContext::CliWrapperSkill { skill_id, skill_name } => {
                if self.is_skill_allowed(skill_id) {
                    // Skill 在 allowlist 中，自动批准
                    ExecApproval::Allowed
                } else {
                    // 需要用户确认
                    ExecApproval::NeedsConfirmation {
                        reason: format!(
                            "CLI Wrapper Skill '{}' wants to execute: {}",
                            skill_name, request.command
                        ),
                    }
                }
            }
            ExecContext::UserRequest => {
                // 用户直接请求，通常允许
                ExecApproval::Allowed
            }
            ExecContext::AgentAutonomous => {
                // Agent 自主决策，走标准审批流
                self.check_standard(request).await
            }
        }
    }
}
```

### 用户配置

```yaml
# ~/.aether/config.yaml

exec:
  # Skill allowlist - 这些 Skill 的 CLI 命令自动批准
  skill_allowlist:
    - github      # 信任 github skill 的所有 gh 命令
    - ffmpeg      # 信任 ffmpeg skill 的所有 ffmpeg 命令
    - git         # 信任 git skill 的所有 git 命令
```

### 执行流程

```
┌──────────────────┐
│ Agent 决定使用    │
│ github skill     │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ CliWrapperExecutor│
│ validate_command │
└────────┬─────────┘
         │ ✓ binary = "gh" ∈ requirements.binaries
         ▼
┌──────────────────┐
│ SecurityPolicy   │
│ .check()         │
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
 Allowlist  Not in list
    │         │
    ▼         ▼
 自动执行   Clarification
            "GitHub Skill 想执行:
             gh pr create --title '...'
             是否允许？"
                 │
                 ▼ 用户同意
              执行命令
```

---

## 文件变更清单

### 新增文件

| 文件 | 描述 |
|------|------|
| `core/src/skills/types.rs` | 新类型定义 (PackageManager, SkillRequirements, SkillHealth) |
| `core/src/skills/health.rs` | HealthChecker 组件 |
| `core/src/skills/cli_wrapper.rs` | CliWrapperExecutor 组件 |

### 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/skills/mod.rs` | 扩展 SkillFrontmatter，导出新模块 |
| `core/src/skills/registry.rs` | 添加 load_all_with_health, list_healthy_skills 等方法 |
| `core/src/skills/installer.rs` | 添加 suggest_install_command, suggest_install_plan |
| `core/src/exec/policy.rs` | 添加 ExecContext, 扩展 SecurityPolicy |
| `core/src/exec/mod.rs` | 导出新类型 |
| `core/src/config/types/exec.rs` | 添加 skill_allowlist 配置项 |

---

## 兼容性说明

### 向后兼容

- 所有新增的 frontmatter 字段都是 `Option` 或带 `#[serde(default)]`
- 现有 SKILL.md 文件无需任何修改即可继续工作
- 未声明 requirements 的 Skill 默认视为 Healthy

### 渐进式采用

1. **Phase 1**: 仅使用元数据扩展（emoji, category）用于 UI 展示
2. **Phase 2**: 启用健康检查，但仅警告不阻止
3. **Phase 3**: 完整启用健康检查 + 主动安装
4. **Phase 4**: 支持 CLI Wrapper Skills

---

## 后续工作

1. **RPC 扩展** — 添加 `skills.check_health`, `skills.install_dependencies` 方法
2. **Gateway 集成** — 在 Skill 激活时触发健康检查流程
3. **UI 支持** — Halo 界面展示 Skill 健康状态和安装按钮
4. **Skill Evolution 集成** — 自动生成的 Skill 包含依赖信息
