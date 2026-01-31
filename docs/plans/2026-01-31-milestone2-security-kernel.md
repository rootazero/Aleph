# SecurityKernel 规则引擎实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现四级安全分类系统 (Blocked/Danger/Caution/Safe)，确保 `rm -rf /` 被绝对禁止，`ls` 被静默放行，并添加 SecretMasker 敏感信息脱敏。

**Architecture:** 在现有 exec 模块基础上，新增 `SecurityKernel` 结构体作为统一入口，使用 Regex 规则引擎进行确定性判断（不依赖 LLM）。现有的 `decide_exec_approval` 函数将调用 SecurityKernel 进行风险评估。

**Tech Stack:** regex crate (已存在), once_cell (懒加载正则)

---

## 现有模块分析

现有 `exec` 模块已实现：
- ✅ `ExecSecurity` 枚举 (Deny/Allowlist/Full) - 三级策略
- ✅ `DEFAULT_SAFE_BINS` - 安全二进制列表
- ✅ `decide_exec_approval` - 审批决策逻辑
- ✅ 命令解析和分析

需要补充：
- ❌ `RiskLevel` 四级枚举 (Blocked/Danger/Caution/Safe)
- ❌ `SecurityKernel` 结构体 (Regex 规则引擎)
- ❌ `SecretMasker` 敏感信息脱敏
- ❌ 危险命令模式 (rm -rf /, fork bomb, dd 等)

---

## Task 1: 添加 RiskLevel 枚举和默认规则

**Files:**
- Create: `core/src/exec/risk.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: 创建 risk.rs 定义四级风险等级**

创建 `/Volumes/TBU4/Workspace/Aether/core/src/exec/risk.rs`：

```rust
//! Risk level assessment for command execution.
//!
//! Four-tier traffic light protocol:
//! - Blocked: Absolutely forbidden (rm -rf /, fork bomb)
//! - Danger: Requires explicit approval (rm, sudo, chmod)
//! - Caution: Allowed but logged (npm install, docker run)
//! - Safe: Silent pass (ls, cat, echo)

use once_cell::sync::Lazy;
use regex::Regex;

/// Risk level for a command
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Safe: Read-only operations, silent pass
    Safe,
    /// Caution: Resource consumption, network requests - allowed but logged
    Caution,
    /// Danger: Destructive operations - requires explicit approval
    Danger,
    /// Blocked: Absolutely forbidden - immediate rejection
    Blocked,
}

impl RiskLevel {
    /// Check if this risk level requires user approval
    pub fn requires_approval(&self) -> bool {
        matches!(self, RiskLevel::Danger)
    }

    /// Check if this risk level should be blocked
    pub fn is_blocked(&self) -> bool {
        matches!(self, RiskLevel::Blocked)
    }

    /// Check if this risk level is safe for auto-execution
    pub fn is_auto_safe(&self) -> bool {
        matches!(self, RiskLevel::Safe | RiskLevel::Caution)
    }
}

/// Blocked command patterns - NEVER execute these
pub static BLOCKED_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // rm -rf / or rm -rf /* (catastrophic delete)
        Regex::new(r"rm\s+(-[a-zA-Z]*[rf][a-zA-Z]*\s+)*(/|/\*)(\s|$)").unwrap(),
        // Fork bomb variations
        Regex::new(r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:").unwrap(),
        // dd to disk devices
        Regex::new(r"dd\s+.*of=/dev/(sd[a-z]|hd[a-z]|nvme\d+n\d+)").unwrap(),
        // mkfs on disk devices without confirmation
        Regex::new(r"mkfs(\.[a-z0-9]+)?\s+/dev/(sd[a-z]|hd[a-z]|nvme)").unwrap(),
        // Overwrite MBR/boot sector
        Regex::new(r"dd\s+.*of=/dev/(sd[a-z]|hd[a-z])\s*$").unwrap(),
        // chmod 777 on root
        Regex::new(r"chmod\s+(-[a-zA-Z]*\s+)*777\s+/\s*$").unwrap(),
        // Recursively delete root with other tools
        Regex::new(r"find\s+/\s+-delete").unwrap(),
    ]
});

/// Danger command patterns - require approval
pub static DANGER_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // rm with force/recursive flags
        Regex::new(r"^rm\s+").unwrap(),
        // sudo anything
        Regex::new(r"^sudo\s+").unwrap(),
        // su command
        Regex::new(r"^su(\s+|$)").unwrap(),
        // chmod/chown
        Regex::new(r"^(chmod|chown)\s+").unwrap(),
        // kill/killall
        Regex::new(r"^(kill|killall|pkill)\s+").unwrap(),
        // System control
        Regex::new(r"^(shutdown|reboot|halt|poweroff)").unwrap(),
        // Disk operations
        Regex::new(r"^(fdisk|parted|mkfs|mount|umount)\s+").unwrap(),
        // Network config
        Regex::new(r"^(iptables|ip6tables|nft|ufw)\s+").unwrap(),
        // Package managers with install/remove
        Regex::new(r"^(apt|apt-get|yum|dnf|pacman|brew)\s+(install|remove|purge)").unwrap(),
        // mv to sensitive locations
        Regex::new(r"^mv\s+.*\s+(/etc/|/usr/|/bin/|/sbin/)").unwrap(),
    ]
});

/// Safe command patterns - auto-allow (read-only)
pub static SAFE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // File listing and info
        Regex::new(r"^(ls|ll|la|dir)(\s+|$)").unwrap(),
        // File content viewing (without modification)
        Regex::new(r"^(cat|head|tail|less|more)(\s+|$)").unwrap(),
        // Text processing (read-only)
        Regex::new(r"^(grep|egrep|fgrep|rg|ag)(\s+|$)").unwrap(),
        Regex::new(r"^(awk|sed|cut|sort|uniq|wc|tr)(\s+|$)").unwrap(),
        // Directory navigation
        Regex::new(r"^(pwd|cd|pushd|popd)(\s+|$)").unwrap(),
        // Information commands
        Regex::new(r"^(echo|printf|date|cal|whoami|hostname|uname)(\s+|$)").unwrap(),
        Regex::new(r"^(which|where|whereis|type|file|stat)(\s+|$)").unwrap(),
        // Git read operations
        Regex::new(r"^git\s+(status|log|diff|show|branch|remote|fetch)(\s+|$)").unwrap(),
        // Environment
        Regex::new(r"^(env|printenv|set)(\s+|$)").unwrap(),
        // Test and comparison
        Regex::new(r"^(test|\[|diff|cmp|comm)(\s+|$)").unwrap(),
        // Path operations
        Regex::new(r"^(basename|dirname|realpath|readlink)(\s+|$)").unwrap(),
    ]
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Safe < RiskLevel::Caution);
        assert!(RiskLevel::Caution < RiskLevel::Danger);
        assert!(RiskLevel::Danger < RiskLevel::Blocked);
    }

    #[test]
    fn test_blocked_rm_rf_root() {
        let cmd = "rm -rf /";
        assert!(BLOCKED_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_blocked_rm_rf_root_star() {
        let cmd = "rm -rf /*";
        assert!(BLOCKED_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_blocked_fork_bomb() {
        let cmd = ":(){ :|:& };:";
        assert!(BLOCKED_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_danger_rm() {
        let cmd = "rm -rf ./temp";
        assert!(DANGER_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_danger_sudo() {
        let cmd = "sudo apt install vim";
        assert!(DANGER_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_safe_ls() {
        let cmd = "ls -la";
        assert!(SAFE_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_safe_git_status() {
        let cmd = "git status";
        assert!(SAFE_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }

    #[test]
    fn test_safe_echo() {
        let cmd = "echo hello";
        assert!(SAFE_PATTERNS.iter().any(|p| p.is_match(cmd)));
    }
}
```

**Step 2: 更新 mod.rs 导出**

在 `/Volumes/TBU4/Workspace/Aether/core/src/exec/mod.rs` 添加：

```rust
pub mod risk;

pub use risk::{RiskLevel, BLOCKED_PATTERNS, DANGER_PATTERNS, SAFE_PATTERNS};
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test exec::risk::tests
```

Expected: All 9 tests PASS

**Step 4: Commit**

```bash
git add core/src/exec/risk.rs core/src/exec/mod.rs
git commit -m "feat(exec): add RiskLevel enum and pattern definitions"
```

---

## Task 2: 实现 SecurityKernel 结构体

**Files:**
- Create: `core/src/exec/kernel.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: 创建 kernel.rs**

创建 `/Volumes/TBU4/Workspace/Aether/core/src/exec/kernel.rs`：

```rust
//! SecurityKernel - Deterministic command risk assessment.
//!
//! Uses regex pattern matching for zero-latency security decisions.
//! Does NOT rely on LLM for security judgments.

use super::risk::{RiskLevel, BLOCKED_PATTERNS, DANGER_PATTERNS, SAFE_PATTERNS};
use regex::Regex;

/// Security kernel for command risk assessment.
///
/// # Example
///
/// ```rust
/// use aethecore::exec::SecurityKernel;
///
/// let kernel = SecurityKernel::default();
///
/// // Blocked command
/// let risk = kernel.assess("rm -rf /");
/// assert!(risk.is_blocked());
///
/// // Safe command
/// let risk = kernel.assess("ls -la");
/// assert_eq!(risk, aethecore::exec::RiskLevel::Safe);
/// ```
#[derive(Debug, Clone)]
pub struct SecurityKernel {
    /// Custom blocked patterns (in addition to defaults)
    custom_blocked: Vec<Regex>,
    /// Custom danger patterns (in addition to defaults)
    custom_danger: Vec<Regex>,
    /// Custom safe patterns (in addition to defaults)
    custom_safe: Vec<Regex>,
}

impl Default for SecurityKernel {
    fn default() -> Self {
        Self {
            custom_blocked: Vec::new(),
            custom_danger: Vec::new(),
            custom_safe: Vec::new(),
        }
    }
}

impl SecurityKernel {
    /// Create a new security kernel with default patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom blocked pattern.
    pub fn add_blocked_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_blocked.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Add a custom danger pattern.
    pub fn add_danger_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_danger.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Add a custom safe pattern.
    pub fn add_safe_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_safe.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Assess the risk level of a command.
    ///
    /// Evaluation order (first match wins):
    /// 1. Blocked patterns → RiskLevel::Blocked
    /// 2. Danger patterns → RiskLevel::Danger
    /// 3. Safe patterns → RiskLevel::Safe
    /// 4. Default → RiskLevel::Caution
    pub fn assess(&self, command: &str) -> RiskLevel {
        let cmd = command.trim();

        // 1. Check blocked patterns (custom first, then defaults)
        for pattern in self.custom_blocked.iter().chain(BLOCKED_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Blocked;
            }
        }

        // 2. Check danger patterns
        for pattern in self.custom_danger.iter().chain(DANGER_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Danger;
            }
        }

        // 3. Check safe patterns
        for pattern in self.custom_safe.iter().chain(SAFE_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Safe;
            }
        }

        // 4. Default: Caution (unknown commands)
        RiskLevel::Caution
    }

    /// Assess a command and return detailed result.
    pub fn assess_detailed(&self, command: &str) -> RiskAssessment {
        let level = self.assess(command);
        let reason = match level {
            RiskLevel::Blocked => "Command matches blocked pattern",
            RiskLevel::Danger => "Command matches danger pattern",
            RiskLevel::Caution => "Command is unknown, requires caution",
            RiskLevel::Safe => "Command matches safe pattern",
        };

        RiskAssessment {
            command: command.to_string(),
            level,
            reason: reason.to_string(),
        }
    }
}

/// Detailed risk assessment result.
#[derive(Debug, Clone)]
pub struct RiskAssessment {
    /// The assessed command
    pub command: String,
    /// Risk level
    pub level: RiskLevel,
    /// Human-readable reason
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assess_blocked() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("rm -rf /"), RiskLevel::Blocked);
        assert_eq!(kernel.assess("rm -rf /*"), RiskLevel::Blocked);
    }

    #[test]
    fn test_assess_danger() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("rm -rf ./temp"), RiskLevel::Danger);
        assert_eq!(kernel.assess("sudo apt install vim"), RiskLevel::Danger);
        assert_eq!(kernel.assess("chmod 755 script.sh"), RiskLevel::Danger);
    }

    #[test]
    fn test_assess_safe() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("ls -la"), RiskLevel::Safe);
        assert_eq!(kernel.assess("echo hello"), RiskLevel::Safe);
        assert_eq!(kernel.assess("git status"), RiskLevel::Safe);
        assert_eq!(kernel.assess("pwd"), RiskLevel::Safe);
        assert_eq!(kernel.assess("cat file.txt"), RiskLevel::Safe);
    }

    #[test]
    fn test_assess_caution() {
        let kernel = SecurityKernel::new();
        // Unknown commands default to Caution
        assert_eq!(kernel.assess("npm install"), RiskLevel::Caution);
        assert_eq!(kernel.assess("cargo build"), RiskLevel::Caution);
        assert_eq!(kernel.assess("docker run nginx"), RiskLevel::Caution);
    }

    #[test]
    fn test_custom_blocked_pattern() {
        let mut kernel = SecurityKernel::new();
        kernel.add_blocked_pattern(r"^danger-cmd").unwrap();
        assert_eq!(kernel.assess("danger-cmd arg"), RiskLevel::Blocked);
    }

    #[test]
    fn test_custom_safe_pattern() {
        let mut kernel = SecurityKernel::new();
        kernel.add_safe_pattern(r"^my-safe-tool").unwrap();
        assert_eq!(kernel.assess("my-safe-tool --help"), RiskLevel::Safe);
    }

    #[test]
    fn test_assess_detailed() {
        let kernel = SecurityKernel::new();
        let result = kernel.assess_detailed("ls -la");
        assert_eq!(result.level, RiskLevel::Safe);
        assert!(result.reason.contains("safe"));
    }

    #[test]
    fn test_blocked_takes_priority() {
        let kernel = SecurityKernel::new();
        // Even if it looks like a safe command, blocked patterns win
        assert_eq!(kernel.assess("rm -rf /"), RiskLevel::Blocked);
    }
}
```

**Step 2: 更新 mod.rs 导出**

在 `/Volumes/TBU4/Workspace/Aether/core/src/exec/mod.rs` 添加：

```rust
pub mod kernel;

pub use kernel::{RiskAssessment, SecurityKernel};
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test exec::kernel::tests
```

Expected: All 9 tests PASS

**Step 4: Commit**

```bash
git add core/src/exec/kernel.rs core/src/exec/mod.rs
git commit -m "feat(exec): implement SecurityKernel with regex rule engine"
```

---

## Task 3: 实现 SecretMasker

**Files:**
- Create: `core/src/exec/masker.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: 创建 masker.rs**

创建 `/Volumes/TBU4/Workspace/Aether/core/src/exec/masker.rs`：

```rust
//! SecretMasker - Redact sensitive information from output.
//!
//! Detects and masks:
//! - API keys (OpenAI, Anthropic, Google, AWS, etc.)
//! - Private keys (SSH, PEM)
//! - Passwords and tokens
//! - Connection strings

use once_cell::sync::Lazy;
use regex::Regex;

/// Secret pattern with replacement
struct SecretPattern {
    regex: Regex,
    replacement: &'static str,
}

/// All secret patterns
static SECRET_PATTERNS: Lazy<Vec<SecretPattern>> = Lazy::new(|| {
    vec![
        // OpenAI API Key (sk-...)
        SecretPattern {
            regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
            replacement: "sk-***REDACTED***",
        },
        // Anthropic API Key (sk-ant-...)
        SecretPattern {
            regex: Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
            replacement: "sk-ant-***REDACTED***",
        },
        // Google API Key
        SecretPattern {
            regex: Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap(),
            replacement: "AIza***REDACTED***",
        },
        // AWS Access Key ID
        SecretPattern {
            regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
            replacement: "AKIA***REDACTED***",
        },
        // AWS Secret Access Key
        SecretPattern {
            regex: Regex::new(r"(?i)(aws_secret_access_key|secret_access_key)\s*[=:]\s*['\"]?([a-zA-Z0-9/+=]{40})['\"]?").unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // GitHub Token
        SecretPattern {
            regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
            replacement: "gh*_***REDACTED***",
        },
        // Generic Bearer Token
        SecretPattern {
            regex: Regex::new(r"(?i)(bearer|token|authorization)\s*[=:]\s*['\"]?([a-zA-Z0-9\-_.]{20,})['\"]?").unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // Private Key Block
        SecretPattern {
            regex: Regex::new(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+ PRIVATE KEY-----").unwrap(),
            replacement: "-----BEGIN PRIVATE KEY-----\n***REDACTED***\n-----END PRIVATE KEY-----",
        },
        // Password in URL
        SecretPattern {
            regex: Regex::new(r"://([^:]+):([^@]+)@").unwrap(),
            replacement: "://$1:***REDACTED***@",
        },
        // Generic password assignment
        SecretPattern {
            regex: Regex::new(r"(?i)(password|passwd|pwd|secret)\s*[=:]\s*['\"]?([^\s'\"]{8,})['\"]?").unwrap(),
            replacement: "$1=***REDACTED***",
        },
        // Slack Token
        SecretPattern {
            regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
            replacement: "xox*-***REDACTED***",
        },
        // Discord Token
        SecretPattern {
            regex: Regex::new(r"[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27}").unwrap(),
            replacement: "***DISCORD_TOKEN_REDACTED***",
        },
    ]
});

/// SecretMasker for redacting sensitive information.
#[derive(Debug, Clone, Default)]
pub struct SecretMasker {
    /// Additional custom patterns
    custom_patterns: Vec<(Regex, String)>,
}

impl SecretMasker {
    /// Create a new secret masker with default patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom pattern with replacement.
    pub fn add_pattern(&mut self, pattern: &str, replacement: &str) -> Result<(), regex::Error> {
        self.custom_patterns
            .push((Regex::new(pattern)?, replacement.to_string()));
        Ok(())
    }

    /// Mask secrets in the given text.
    pub fn mask(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Apply default patterns
        for pattern in SECRET_PATTERNS.iter() {
            result = pattern
                .regex
                .replace_all(&result, pattern.replacement)
                .to_string();
        }

        // Apply custom patterns
        for (regex, replacement) in &self.custom_patterns {
            result = regex.replace_all(&result, replacement.as_str()).to_string();
        }

        result
    }

    /// Check if the text contains any secrets.
    pub fn contains_secrets(&self, text: &str) -> bool {
        // Check default patterns
        for pattern in SECRET_PATTERNS.iter() {
            if pattern.regex.is_match(text) {
                return true;
            }
        }

        // Check custom patterns
        for (regex, _) in &self.custom_patterns {
            if regex.is_match(text) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_openai_key() {
        let masker = SecretMasker::new();
        let input = "API key is sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        let output = masker.mask(input);
        assert!(output.contains("sk-***REDACTED***"));
        assert!(!output.contains("abcdefgh"));
    }

    #[test]
    fn test_mask_anthropic_key() {
        let masker = SecretMasker::new();
        let input = "Key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let output = masker.mask(input);
        assert!(output.contains("sk-ant-***REDACTED***"));
    }

    #[test]
    fn test_mask_aws_key() {
        let masker = SecretMasker::new();
        let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let output = masker.mask(input);
        assert!(output.contains("AKIA***REDACTED***"));
    }

    #[test]
    fn test_mask_github_token() {
        let masker = SecretMasker::new();
        let input = "GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let output = masker.mask(input);
        assert!(output.contains("gh*_***REDACTED***"));
    }

    #[test]
    fn test_mask_private_key() {
        let masker = SecretMasker::new();
        let input = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8DHGP...
-----END RSA PRIVATE KEY-----"#;
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("MIIEpAIBAAKCAQEA"));
    }

    #[test]
    fn test_mask_password_in_url() {
        let masker = SecretMasker::new();
        let input = "postgres://user:secretpassword123@localhost:5432/db";
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("secretpassword123"));
    }

    #[test]
    fn test_mask_generic_password() {
        let masker = SecretMasker::new();
        let input = "DATABASE_PASSWORD=mysupersecretpassword";
        let output = masker.mask(input);
        assert!(output.contains("***REDACTED***"));
        assert!(!output.contains("mysupersecret"));
    }

    #[test]
    fn test_contains_secrets() {
        let masker = SecretMasker::new();
        assert!(masker.contains_secrets("sk-abcdefghijklmnopqrstuvwxyz12345678"));
        assert!(!masker.contains_secrets("This is just normal text"));
    }

    #[test]
    fn test_custom_pattern() {
        let mut masker = SecretMasker::new();
        masker
            .add_pattern(r"CUSTOM_SECRET_\d+", "CUSTOM_***")
            .unwrap();
        let input = "Value: CUSTOM_SECRET_12345";
        let output = masker.mask(input);
        assert!(output.contains("CUSTOM_***"));
    }

    #[test]
    fn test_no_false_positives() {
        let masker = SecretMasker::new();
        // Normal text should not be masked
        let input = "Hello world, this is a normal message";
        let output = masker.mask(input);
        assert_eq!(input, output);
    }
}
```

**Step 2: 更新 mod.rs 导出**

在 `/Volumes/TBU4/Workspace/Aether/core/src/exec/mod.rs` 添加：

```rust
pub mod masker;

pub use masker::SecretMasker;
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test exec::masker::tests
```

Expected: All 10 tests PASS

**Step 4: Commit**

```bash
git add core/src/exec/masker.rs core/src/exec/mod.rs
git commit -m "feat(exec): implement SecretMasker for sensitive data redaction"
```

---

## Task 4: 更新 lib.rs 导出新类型

**Files:**
- Modify: `core/src/lib.rs`

**Step 1: 添加新类型到 lib.rs 的 exec exports**

找到 `core/src/lib.rs` 中的 exec exports 部分（大约在 340-360 行），更新为：

```rust
// Exec security exports (command execution approval)
pub use crate::exec::{
    // Config
    AgentExecConfig, AllowlistEntry, ExecApprovalsFile, ExecAsk, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
    // Analysis
    CommandAnalysis, CommandResolution, CommandSegment,
    // Parser
    analyze_shell_command, tokenize_segment,
    // Allowlist
    match_allowlist,
    // Decision
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
    // Socket
    ApprovalDecisionType, ApprovalRequestPayload, SegmentInfo, SocketMessage,
    // Manager
    ExecApprovalManager, ExecApprovalRecord, PendingApproval,
    // Storage
    ConfigWithHash, ExecApprovalsStorage, StorageError,
    // Risk (NEW)
    RiskLevel, BLOCKED_PATTERNS, DANGER_PATTERNS, SAFE_PATTERNS,
    // Kernel (NEW)
    RiskAssessment, SecurityKernel,
    // Masker (NEW)
    SecretMasker,
};
```

**Step 2: 验证编译**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

Expected: 编译通过

**Step 3: Commit**

```bash
git add core/src/lib.rs
git commit -m "feat(lib): export SecurityKernel, RiskLevel, and SecretMasker"
```

---

## Task 5: 集成 SecurityKernel 到 Supervisor 模块

**Files:**
- Modify: `core/src/supervisor/pty.rs`

**Step 1: 在 ClaudeSupervisor 中集成 SecretMasker**

修改 `/Volumes/TBU4/Workspace/Aether/core/src/supervisor/pty.rs`，在 `strip_ansi` 函数后添加 secret masking：

找到 `strip_ansi` 函数（大约在 368-375 行），将 reader thread 中的处理逻辑更新：

```rust
// 在文件顶部添加 import
use crate::exec::SecretMasker;

// 在 ClaudeSupervisor 结构体中添加 masker 字段
pub struct ClaudeSupervisor {
    config: SupervisorConfig,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    running: Arc<AtomicBool>,
    masker: SecretMasker, // NEW
}

// 更新 new 方法
pub fn new(config: SupervisorConfig) -> Self {
    Self {
        config,
        master: None,
        writer: None,
        running: Arc::new(AtomicBool::new(false)),
        masker: SecretMasker::new(), // NEW
    }
}

// 更新 spawn 方法中的 reader thread
// 在 spawn 方法的 std::thread::spawn 之前，克隆 masker
let masker = self.masker.clone();

// 在 reader thread 中更新处理逻辑
std::thread::spawn(move || {
    let buf_reader = BufReader::new(reader);
    for line in buf_reader.lines() {
        match line {
            Ok(text) => {
                // Strip ANSI escape sequences
                let clean = strip_ansi(&text);
                // Mask secrets (NEW)
                let safe = masker.mask(&clean);

                // Detect semantic events
                let event = detect_event(&safe);
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
```

**Step 2: 添加测试**

在 `pty.rs` 的测试模块中添加：

```rust
#[test]
fn test_secret_masking_in_output() {
    // SecretMasker should be used in supervisor
    let masker = crate::exec::SecretMasker::new();
    let input = "API_KEY=sk-abcdefghijklmnopqrstuvwxyz12345678901234";
    let masked = masker.mask(input);
    assert!(masked.contains("***REDACTED***"));
}
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test supervisor::
```

Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/supervisor/pty.rs
git commit -m "feat(supervisor): integrate SecretMasker for output redaction"
```

---

## Task 6: 最终验证和文档

**Step 1: 运行所有 exec 和 supervisor 测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test exec:: && cargo test supervisor::
```

Expected: All tests PASS

**Step 2: 运行完整测试验证无回归**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib -- --test-threads=4
```

Expected: 现有测试无回归

**Step 3: 更新设计文档状态**

修改 `/Volumes/TBU4/Workspace/Aether/docs/plans/2026-01-31-aether-beyond-openclaw-design.md`。

找到 "### Milestone 2: SecurityKernel 规则引擎" 部分，将其更新为：

```markdown
### Milestone 2: SecurityKernel 规则引擎

- [x] 定义 RiskLevel 四级枚举
- [x] 实现 CommandPolicy (Regex 规则集)
- [x] SecurityKernel::assess() 零延迟判断
- [x] SecretMasker 敏感信息脱敏

**验收**: ✅ rm -rf / 被 Blocked，ls 被 Safe
```

**Step 4: Final Commit**

```bash
git add docs/plans/
git commit -m "docs: mark Milestone 2 (SecurityKernel) as complete"
```

---

## 验收标准

完成本计划后，应满足以下条件：

1. ✅ `RiskLevel` 四级枚举：Safe < Caution < Danger < Blocked
2. ✅ `SecurityKernel::assess("rm -rf /")` 返回 `RiskLevel::Blocked`
3. ✅ `SecurityKernel::assess("ls -la")` 返回 `RiskLevel::Safe`
4. ✅ `SecurityKernel::assess("npm install")` 返回 `RiskLevel::Caution`
5. ✅ `SecretMasker` 能识别并脱敏 OpenAI/Anthropic/AWS 等 API Key
6. ✅ Supervisor 输出自动经过 SecretMasker 处理

---

## 依赖关系

```
Milestone 1 (PtySupervisor) ✅ 完成
    │
    └──► Milestone 2 (SecurityKernel) ← 当前
             │
             └──► Milestone 3 (Telegram 审批集成)
```

---

*生成时间: 2026-01-31*
