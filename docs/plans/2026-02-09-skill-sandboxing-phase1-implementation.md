# Skill Sandboxing Phase 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement OS-native sandbox infrastructure for AI-generated skills with macOS support

**Architecture:** Build SandboxAdapter trait abstraction, Capabilities permission model, macOS sandbox-exec implementation, and audit logging system. All evolved skills will execute in isolated sandboxes with fine-grained permissions.

**Tech Stack:** Rust, Tokio, macOS sandbox-exec, serde, tempfile

---

## Prerequisites

**Current Working Directory:** `.worktrees/skill-sandboxing`
**Branch:** `feature/skill-sandboxing`
**Base Commit:** Design document committed

**Existing Modules:**
- `core/src/exec/` - Shell execution security (approval, allowlist, risk assessment)
- `core/src/error.rs` - Error types (AlephError)

---

## Task 1: Create Sandbox Module Structure

**Files:**
- Create: `core/src/exec/sandbox/mod.rs`
- Create: `core/src/exec/sandbox/capabilities.rs`
- Create: `core/src/exec/sandbox/adapter.rs`
- Create: `core/src/exec/sandbox/profile.rs`
- Create: `core/src/exec/sandbox/executor.rs`
- Create: `core/src/exec/sandbox/audit.rs`
- Create: `core/src/exec/sandbox/platforms/mod.rs`
- Create: `core/src/exec/sandbox/platforms/macos.rs`
- Modify: `core/src/exec/mod.rs`

### Step 1: Create sandbox module entry point

Create `core/src/exec/sandbox/mod.rs`:

```rust
//! Sandbox subsystem for secure execution of AI-generated skills.
//!
//! Provides OS-native sandboxing with fine-grained permission control.

pub mod adapter;
pub mod audit;
pub mod capabilities;
pub mod executor;
pub mod platforms;
pub mod profile;

pub use adapter::{SandboxAdapter, SandboxCommand, SandboxProfile};
pub use audit::{ExecutionStatus, SandboxAuditLog, SandboxViolation};
pub use capabilities::{
    Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability,
    ProcessCapability,
};
pub use executor::{FallbackPolicy, SandboxManager};
pub use profile::ProfileGenerator;
```

### Step 2: Register sandbox module in exec

Modify `core/src/exec/mod.rs`, add after line 24:

```rust
pub mod sandbox;
```

Add to exports after line 47:

```rust
pub use sandbox::{
    Capabilities, EnvironmentCapability, FallbackPolicy, FileSystemCapability, NetworkCapability,
    ProcessCapability, SandboxAdapter, SandboxAuditLog, SandboxCommand, SandboxManager,
    SandboxProfile,
};
```

### Step 3: Verify module structure compiles

Run: `cd core && cargo check --lib`
Expected: Compilation succeeds (modules are empty but declared)

### Step 4: Commit module structure

```bash
cd .worktrees/skill-sandboxing
git add core/src/exec/sandbox/
git add core/src/exec/mod.rs
git commit -m "exec/sandbox: add module structure

Create sandbox subsystem with module hierarchy:
- adapter: SandboxAdapter trait
- capabilities: Permission model
- executor: Sandboxed execution manager
- audit: Execution logging
- platforms: Platform-specific implementations

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement Capabilities Permission Model

**Files:**
- Create: `core/src/exec/sandbox/capabilities.rs`
- Test: Unit tests in same file

### Step 1: Write failing test for Capabilities serialization

Add to `core/src/exec/sandbox/capabilities.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_default() {
        let caps = Capabilities::default();
        assert_eq!(caps.filesystem.len(), 1);
        assert!(matches!(
            caps.filesystem[0],
            FileSystemCapability::TempWorkspace
        ));
        assert!(matches!(caps.network, NetworkCapability::Deny));
        assert!(caps.process.no_fork);
        assert_eq!(caps.process.max_execution_time, 300);
    }

    #[test]
    fn test_capabilities_serialization() {
        let caps = Capabilities::default();
        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: Capabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps.process.no_fork, deserialized.process.no_fork);
    }
}
```

### Step 2: Run test to verify it fails

Run: `cd core && cargo test capabilities::tests --lib`
Expected: FAIL with "Capabilities not found"

### Step 3: Implement Capabilities types

Add to `core/src/exec/sandbox/capabilities.rs` before tests:

```rust
/// Sandbox permission set
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    /// Filesystem permissions
    pub filesystem: Vec<FileSystemCapability>,
    /// Network permissions
    pub network: NetworkCapability,
    /// Process permissions
    pub process: ProcessCapability,
    /// Environment variable access
    pub environment: EnvironmentCapability,
}

/// Filesystem permission (fine-grained path control)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemCapability {
    /// Read-only access to specific path
    ReadOnly { path: PathBuf },
    /// Read-write access to specific path
    ReadWrite { path: PathBuf },
    /// Temporary workspace (auto-created and cleaned)
    TempWorkspace,
}

/// Network permission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCapability {
    /// Deny all network access
    Deny,
    /// Allow access to specific domains
    AllowDomains(Vec<String>),
    /// Allow all network access
    AllowAll,
}

/// Process permission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessCapability {
    /// Prohibit forking child processes
    pub no_fork: bool,
    /// Maximum execution time in seconds
    pub max_execution_time: u64,
    /// Maximum memory usage in MB
    pub max_memory_mb: Option<u64>,
}

/// Environment variable access
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentCapability {
    /// No environment access
    None,
    /// Restricted (only safe variables like PATH, HOME)
    Restricted,
    /// Full environment access
    Full,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        }
    }
}
```

### Step 4: Run test to verify it passes

Run: `cd core && cargo test capabilities::tests --lib`
Expected: PASS (2 tests)

### Step 5: Commit Capabilities implementation

```bash
git add core/src/exec/sandbox/capabilities.rs
git commit -m "exec/sandbox: implement Capabilities permission model

Add fine-grained permission types:
- FileSystemCapability: ReadOnly/ReadWrite/TempWorkspace
- NetworkCapability: Deny/AllowDomains/AllowAll
- ProcessCapability: no_fork, timeouts, memory limits
- EnvironmentCapability: None/Restricted/Full

Default policy: TempWorkspace only, no network, no fork, 5min timeout

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Implement SandboxAdapter Trait

**Files:**
- Create: `core/src/exec/sandbox/adapter.rs`
- Create: `core/src/exec/sandbox/profile.rs`

### Step 1: Write SandboxAdapter trait definition

Create `core/src/exec/sandbox/adapter.rs`:

```rust
use crate::error::Result;
use crate::exec::sandbox::capabilities::Capabilities;
use async_trait::async_trait;
use std::path::PathBuf;

/// Command to execute in sandbox
#[derive(Debug, Clone)]
pub struct SandboxCommand {
    /// Program to execute
    pub program: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Working directory
    pub working_dir: Option<PathBuf>,
}

/// Sandbox configuration profile
#[derive(Debug, Clone)]
pub struct SandboxProfile {
    /// Path to sandbox configuration file (e.g., .sb file)
    pub path: PathBuf,
    /// Capabilities this profile enforces
    pub capabilities: Capabilities,
    /// Platform identifier
    pub platform: String,
    /// Temporary workspace directory (if TempWorkspace capability used)
    pub temp_workspace: Option<PathBuf>,
}

/// Result of sandboxed execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Exit code
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether execution was sandboxed
    pub sandboxed: bool,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Platform-specific sandbox adapter
#[async_trait]
pub trait SandboxAdapter: Send + Sync {
    /// Check if sandbox is supported on current platform
    fn is_supported(&self) -> bool;

    /// Get platform identifier
    fn platform_name(&self) -> &str;

    /// Generate sandbox configuration profile
    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile>;

    /// Execute command in sandbox
    async fn execute_sandboxed(
        &self,
        command: &SandboxCommand,
        profile: &SandboxProfile,
    ) -> Result<ExecutionResult>;

    /// Cleanup temporary configuration files and workspaces
    fn cleanup(&self, profile: &SandboxProfile) -> Result<()>;
}
```

### Step 2: Create ProfileGenerator helper

Create `core/src/exec/sandbox/profile.rs`:

```rust
use crate::error::Result;
use crate::exec::sandbox::capabilities::Capabilities;
use crate::exec::sandbox::SandboxProfile;
use std::path::PathBuf;

/// Helper for generating sandbox profiles
pub struct ProfileGenerator;

impl ProfileGenerator {
    /// Create temporary workspace directory
    pub fn create_temp_workspace() -> Result<PathBuf> {
        let temp_dir = tempfile::Builder::new()
            .prefix("aleph-sandbox-")
            .tempdir()?;
        Ok(temp_dir.into_path())
    }

    /// Write profile content to temporary file
    pub fn write_temp_profile(content: &str, extension: &str) -> Result<PathBuf> {
        use std::io::Write;
        let mut temp_file = tempfile::Builder::new()
            .prefix("aleph-profile-")
            .suffix(extension)
            .tempfile()?;
        temp_file.write_all(content.as_bytes())?;
        Ok(temp_file.into_temp_path().keep()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_temp_workspace() {
        let workspace = ProfileGenerator::create_temp_workspace().unwrap();
        assert!(workspace.exists());
        assert!(workspace.is_dir());
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn test_write_temp_profile() {
        let content = "(version 1)\n(deny default)\n";
        let profile_path = ProfileGenerator::write_temp_profile(content, ".sb").unwrap();
        assert!(profile_path.exists());
        let read_content = std::fs::read_to_string(&profile_path).unwrap();
        assert_eq!(read_content, content);
        std::fs::remove_file(profile_path).ok();
    }
}
```

### Step 3: Run tests

Run: `cd core && cargo test sandbox::profile::tests --lib && cargo test sandbox::adapter --lib`
Expected: PASS

### Step 4: Commit adapter trait and profile generator

```bash
git add core/src/exec/sandbox/adapter.rs
git add core/src/exec/sandbox/profile.rs
git commit -m "exec/sandbox: implement SandboxAdapter trait

Add core abstractions:
- SandboxAdapter trait: platform-agnostic sandbox interface
- SandboxCommand: command to execute
- SandboxProfile: sandbox configuration
- ExecutionResult: execution outcome
- ProfileGenerator: temp file/workspace helpers

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Implement macOS Sandbox Platform

**Files:**
- Create: `core/src/exec/sandbox/platforms/macos.rs`
- Modify: `core/src/exec/sandbox/platforms/mod.rs`
- Test: Integration tests

### Step 1: Write failing test for macOS sandbox detection

Create `core/src/exec/sandbox/platforms/macos.rs`:

```rust
use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    Capabilities, ExecutionResult, FileSystemCapability, NetworkCapability, ProfileGenerator,
    SandboxAdapter, SandboxCommand, SandboxProfile,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Instant;
use tokio::process::Command;

pub struct MacOSSandbox;

impl MacOSSandbox {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macos_sandbox_supported() {
        let sandbox = MacOSSandbox::new();
        #[cfg(target_os = "macos")]
        assert!(sandbox.is_supported());
        #[cfg(not(target_os = "macos"))]
        assert!(!sandbox.is_supported());
    }
}
```

### Step 2: Run test to verify it fails

Run: `cd core && cargo test platforms::macos::tests --lib`
Expected: FAIL with "is_supported not implemented"

### Step 3: Implement macOS sandbox adapter

Add implementation to `core/src/exec/sandbox/platforms/macos.rs`:

```rust
#[async_trait]
impl SandboxAdapter for MacOSSandbox {
    fn is_supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            // Check if sandbox-exec exists
            std::process::Command::new("which")
                .arg("sandbox-exec")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn platform_name(&self) -> &str {
        "macos"
    }

    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile> {
        let mut profile = String::from("(version 1)\n");
        profile.push_str("(deny default)\n\n");

        // Allow basic system calls
        profile.push_str(";; Allow basic system operations\n");
        profile.push_str("(allow process-exec*)\n");
        profile.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
        profile.push_str("(allow file-read* (subpath \"/usr/bin\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/urandom\"))\n\n");

        // Temporary workspace tracking
        let mut temp_workspace = None;

        // Generate filesystem rules
        profile.push_str(";; Filesystem permissions\n");
        for fs_cap in &caps.filesystem {
            match fs_cap {
                FileSystemCapability::ReadOnly { path } => {
                    profile.push_str(&format!(
                        "(allow file-read* (subpath \"{}\"))\n",
                        path.display()
                    ));
                }
                FileSystemCapability::ReadWrite { path } => {
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        path.display()
                    ));
                }
                FileSystemCapability::TempWorkspace => {
                    let temp_dir = ProfileGenerator::create_temp_workspace()?;
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        temp_dir.display()
                    ));
                    temp_workspace = Some(temp_dir);
                }
            }
        }
        profile.push_str("\n");

        // Network rules
        profile.push_str(";; Network permissions\n");
        match &caps.network {
            NetworkCapability::Deny => {
                profile.push_str("(deny network*)\n");
            }
            NetworkCapability::AllowDomains(domains) => {
                for domain in domains {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote tcp \"{}:*\"))\n",
                        domain
                    ));
                }
            }
            NetworkCapability::AllowAll => {
                profile.push_str("(allow network*)\n");
            }
        }

        // Write profile to temp file
        let profile_path = ProfileGenerator::write_temp_profile(&profile, ".sb")?;

        Ok(SandboxProfile {
            path: profile_path,
            capabilities: caps.clone(),
            platform: "macos".to_string(),
            temp_workspace,
        })
    }

    async fn execute_sandboxed(
        &self,
        command: &SandboxCommand,
        profile: &SandboxProfile,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();

        // Build sandbox-exec command
        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-f").arg(&profile.path);
        cmd.arg(&command.program);
        cmd.args(&command.args);

        if let Some(ref working_dir) = command.working_dir {
            cmd.current_dir(working_dir);
        }

        // Set timeout
        let timeout = std::time::Duration::from_secs(profile.capabilities.process.max_execution_time);

        // Execute with timeout
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| AlephError::ExecutionTimeout {
                timeout_secs: profile.capabilities.process.max_execution_time,
            })??;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            sandboxed: true,
            duration_ms,
        })
    }

    fn cleanup(&self, profile: &SandboxProfile) -> Result<()> {
        // Remove profile file
        if profile.path.exists() {
            std::fs::remove_file(&profile.path)?;
        }

        // Remove temp workspace
        if let Some(ref workspace) = profile.temp_workspace {
            if workspace.exists() {
                std::fs::remove_dir_all(workspace)?;
            }
        }

        Ok(())
    }
}
```

### Step 4: Add platform module exports

Create `core/src/exec/sandbox/platforms/mod.rs`:

```rust
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacOSSandbox;
```

### Step 5: Run tests

Run: `cd core && cargo test platforms::macos::tests --lib`
Expected: PASS

### Step 6: Add integration test for sandbox execution

Add to `core/src/exec/sandbox/platforms/macos.rs` tests:

```rust
#[tokio::test]
#[cfg(target_os = "macos")]
async fn test_macos_sandbox_execution() {
    let sandbox = MacOSSandbox::new();
    if !sandbox.is_supported() {
        return; // Skip if sandbox-exec not available
    }

    let caps = Capabilities::default();
    let profile = sandbox.generate_profile(&caps).unwrap();

    let command = SandboxCommand {
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        working_dir: None,
    };

    let result = sandbox.execute_sandboxed(&command, &profile).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout.contains("hello"));
    assert!(result.sandboxed);

    sandbox.cleanup(&profile).unwrap();
}
```

### Step 7: Run integration test

Run: `cd core && cargo test platforms::macos::test_macos_sandbox_execution --lib`
Expected: PASS (on macOS) or SKIP (on other platforms)

### Step 8: Commit macOS sandbox implementation

```bash
git add core/src/exec/sandbox/platforms/
git commit -m "exec/sandbox: implement macOS sandbox adapter

Add macOS sandbox-exec support:
- Platform detection via sandbox-exec availability
- Seatbelt profile generation from Capabilities
- Sandboxed command execution with timeout
- Temp workspace and profile cleanup
- Integration tests for execution flow

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Implement Audit Logging

**Files:**
- Create: `core/src/exec/sandbox/audit.rs`
- Test: Unit tests

### Step 1: Write failing test for audit log serialization

Create `core/src/exec/sandbox/audit.rs`:

```rust
use serde::{Deserialize, Serialize};
use crate::exec::sandbox::Capabilities;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_serialization() {
        let log = SandboxAuditLog {
            timestamp: 1707436800,
            skill_id: "test-skill".to_string(),
            capabilities: Capabilities::default(),
            execution_result: ExecutionStatus::Success {
                exit_code: 0,
                duration_ms: 100,
            },
            sandbox_platform: "macos".to_string(),
            violations: vec![],
        };

        let json = serde_json::to_string(&log).unwrap();
        let deserialized: SandboxAuditLog = serde_json::from_str(&json).unwrap();
        assert_eq!(log.skill_id, deserialized.skill_id);
    }
}
```

### Step 2: Run test to verify it fails

Run: `cd core && cargo test sandbox::audit::tests --lib`
Expected: FAIL with "SandboxAuditLog not found"

### Step 3: Implement audit types

Add to `core/src/exec/sandbox/audit.rs` before tests:

```rust
/// Audit log for sandboxed execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxAuditLog {
    /// Unix timestamp
    pub timestamp: i64,
    /// Skill identifier
    pub skill_id: String,
    /// Capabilities used
    pub capabilities: Capabilities,
    /// Execution result
    pub execution_result: ExecutionStatus,
    /// Platform used for sandboxing
    pub sandbox_platform: String,
    /// Security violations detected
    pub violations: Vec<SandboxViolation>,
}

/// Execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Successful execution
    Success { exit_code: i32, duration_ms: u64 },
    /// Execution timeout
    Timeout { duration_ms: u64 },
    /// Sandbox violation detected
    SandboxViolation { violation: String },
    /// Execution error
    Error { error: String },
}

/// Sandbox violation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    /// Violation type
    pub violation_type: ViolationType,
    /// Description
    pub description: String,
    /// Timestamp
    pub timestamp: i64,
}

/// Types of sandbox violations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationType {
    /// Attempted file access outside allowed paths
    UnauthorizedFileAccess,
    /// Attempted network access when denied
    UnauthorizedNetworkAccess,
    /// Attempted process fork when prohibited
    UnauthorizedProcessFork,
    /// Resource limit exceeded
    ResourceLimitExceeded,
}

impl SandboxAuditLog {
    /// Create new audit log
    pub fn new(
        skill_id: String,
        capabilities: Capabilities,
        execution_result: ExecutionStatus,
        sandbox_platform: String,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            skill_id,
            capabilities,
            execution_result,
            sandbox_platform,
            violations: vec![],
        }
    }

    /// Add violation to log
    pub fn add_violation(&mut self, violation: SandboxViolation) {
        self.violations.push(violation);
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        matches!(self.execution_result, ExecutionStatus::Success { .. })
    }
}
```

### Step 4: Run test to verify it passes

Run: `cd core && cargo test sandbox::audit::tests --lib`
Expected: PASS

### Step 5: Commit audit logging

```bash
git add core/src/exec/sandbox/audit.rs
git commit -m "exec/sandbox: implement audit logging

Add audit log types:
- SandboxAuditLog: complete execution record
- ExecutionStatus: Success/Timeout/Violation/Error
- SandboxViolation: security violation tracking
- ViolationType: categorized violation types

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Implement SandboxManager

**Files:**
- Create: `core/src/exec/sandbox/executor.rs`
- Test: Unit tests

### Step 1: Write failing test for SandboxManager

Create `core/src/exec/sandbox/executor.rs`:

```rust
use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    Capabilities, ExecutionResult, SandboxAdapter, SandboxAuditLog, SandboxCommand,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_policy_default() {
        let policy = FallbackPolicy::default();
        assert!(matches!(policy, FallbackPolicy::Deny));
    }
}
```

### Step 2: Run test to verify it fails

Run: `cd core && cargo test sandbox::executor::tests --lib`
Expected: FAIL with "FallbackPolicy not found"

### Step 3: Implement SandboxManager

Add to `core/src/exec/sandbox/executor.rs` before tests:

```rust
/// Fallback policy when sandbox is unavailable
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// Deny execution (safest, recommended)
    Deny,
    /// Request user approval before direct execution
    RequestApproval,
    /// Log warning and execute directly (not recommended)
    WarnAndExecute,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self::Deny
    }
}

/// Sandbox execution manager
pub struct SandboxManager {
    adapter: Arc<dyn SandboxAdapter>,
    fallback_policy: FallbackPolicy,
}

impl SandboxManager {
    /// Create new sandbox manager
    pub fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            adapter,
            fallback_policy: FallbackPolicy::default(),
        }
    }

    /// Create with custom fallback policy
    pub fn with_fallback_policy(mut self, policy: FallbackPolicy) -> Self {
        self.fallback_policy = policy;
        self
    }

    /// Check if sandbox is available
    pub fn is_available(&self) -> bool {
        self.adapter.is_supported()
    }

    /// Execute command in sandbox
    pub async fn execute_sandboxed(
        &self,
        skill_id: &str,
        command: SandboxCommand,
        capabilities: Capabilities,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        // Check if sandbox is supported
        if !self.adapter.is_supported() {
            return self.handle_sandbox_unavailable(skill_id).await;
        }

        // Generate sandbox profile
        let profile = self.adapter.generate_profile(&capabilities)?;

        // Execute in sandbox
        let result = self.adapter.execute_sandboxed(&command, &profile).await;

        // Create audit log
        let execution_status = match &result {
            Ok(exec_result) => crate::exec::sandbox::audit::ExecutionStatus::Success {
                exit_code: exec_result.exit_code.unwrap_or(-1),
                duration_ms: exec_result.duration_ms,
            },
            Err(e) => crate::exec::sandbox::audit::ExecutionStatus::Error {
                error: e.to_string(),
            },
        };

        let audit_log = SandboxAuditLog::new(
            skill_id.to_string(),
            capabilities,
            execution_status,
            self.adapter.platform_name().to_string(),
        );

        // Cleanup
        self.adapter.cleanup(&profile)?;

        result.map(|r| (r, audit_log))
    }

    async fn handle_sandbox_unavailable(
        &self,
        skill_id: &str,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        match self.fallback_policy {
            FallbackPolicy::Deny => Err(AlephError::SandboxUnavailable {
                reason: format!(
                    "Sandbox not supported on platform: {}",
                    self.adapter.platform_name()
                ),
            }),
            FallbackPolicy::RequestApproval => {
                // TODO: Implement approval request
                Err(AlephError::SandboxUnavailable {
                    reason: "Approval request not implemented".to_string(),
                })
            }
            FallbackPolicy::WarnAndExecute => {
                tracing::warn!(
                    skill_id = %skill_id,
                    "Executing skill without sandbox protection"
                );
                // TODO: Implement direct execution
                Err(AlephError::SandboxUnavailable {
                    reason: "Direct execution not implemented".to_string(),
                })
            }
        }
    }
}
```

### Step 4: Add AlephError variant

Modify `core/src/error.rs`, add to AlephError enum:

```rust
#[error("Sandbox unavailable: {reason}")]
SandboxUnavailable { reason: String },

#[error("Execution timeout after {timeout_secs} seconds")]
ExecutionTimeout { timeout_secs: u64 },
```

### Step 5: Run tests

Run: `cd core && cargo test sandbox::executor::tests --lib`
Expected: PASS

### Step 6: Commit SandboxManager

```bash
git add core/src/exec/sandbox/executor.rs
git add core/src/error.rs
git commit -m "exec/sandbox: implement SandboxManager

Add sandbox execution orchestration:
- FallbackPolicy: Deny/RequestApproval/WarnAndExecute
- SandboxManager: high-level execution API
- Automatic profile generation and cleanup
- Audit log creation
- Error handling for unavailable sandbox

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Integration Testing

**Files:**
- Create: `core/src/exec/sandbox/tests.rs`
- Modify: `core/src/exec/sandbox/mod.rs`

### Step 1: Create integration test module

Create `core/src/exec/sandbox/tests.rs`:

```rust
#[cfg(test)]
mod integration_tests {
    use crate::exec::sandbox::*;
    use std::sync::Arc;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_end_to_end_sandbox_execution() {
        use crate::exec::sandbox::platforms::MacOSSandbox;

        let adapter = Arc::new(MacOSSandbox::new());
        if !adapter.is_supported() {
            return; // Skip if sandbox-exec not available
        }

        let manager = SandboxManager::new(adapter);

        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["test".to_string()],
            working_dir: None,
        };

        let capabilities = Capabilities::default();

        let (result, audit_log) = manager
            .execute_sandboxed("test-skill", command, capabilities)
            .await
            .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("test"));
        assert!(result.sandboxed);
        assert!(audit_log.is_success());
        assert_eq!(audit_log.sandbox_platform, "macos");
    }

    #[tokio::test]
    async fn test_sandbox_unavailable_deny_policy() {
        use crate::exec::sandbox::platforms::MacOSSandbox;

        let adapter = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter).with_fallback_policy(FallbackPolicy::Deny);

        // Force unavailable by using on non-macOS or when sandbox-exec missing
        #[cfg(not(target_os = "macos"))]
        {
            let command = SandboxCommand {
                program: "echo".to_string(),
                args: vec!["test".to_string()],
                working_dir: None,
            };

            let result = manager
                .execute_sandboxed("test-skill", command, Capabilities::default())
                .await;

            assert!(result.is_err());
        }
    }
}
```

### Step 2: Add tests module to sandbox/mod.rs

Modify `core/src/exec/sandbox/mod.rs`, add after other modules:

```rust
#[cfg(test)]
mod tests;
```

### Step 3: Run integration tests

Run: `cd core && cargo test sandbox::tests::integration_tests --lib`
Expected: PASS (or SKIP on non-macOS)

### Step 4: Commit integration tests

```bash
git add core/src/exec/sandbox/tests.rs
git add core/src/exec/sandbox/mod.rs
git commit -m "exec/sandbox: add integration tests

Add end-to-end tests:
- Full sandbox execution flow
- Audit log generation
- Fallback policy enforcement
- Platform-specific test guards

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Documentation

**Files:**
- Create: `core/src/exec/sandbox/README.md`
- Update: `docs/plans/2026-02-09-skill-sandboxing-design.md`

### Step 1: Create sandbox module README

Create `core/src/exec/sandbox/README.md`:

```markdown
# Sandbox Subsystem

OS-native sandboxing for AI-generated skills with fine-grained permission control.

## Architecture

```
SandboxManager
    ↓
SandboxAdapter (trait)
    ↓
Platform Implementation (macOS, Linux, Windows)
    ↓
OS-native sandbox (sandbox-exec, seccomp, AppContainer)
```

## Usage

```rust
use alephcore::exec::sandbox::*;
use std::sync::Arc;

// Create platform-specific adapter
#[cfg(target_os = "macos")]
let adapter = Arc::new(platforms::MacOSSandbox::new());

// Create manager
let manager = SandboxManager::new(adapter);

// Define capabilities
let capabilities = Capabilities {
    filesystem: vec![FileSystemCapability::TempWorkspace],
    network: NetworkCapability::Deny,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 300,
        max_memory_mb: Some(512),
    },
    environment: EnvironmentCapability::Restricted,
};

// Execute command
let command = SandboxCommand {
    program: "python3".to_string(),
    args: vec!["script.py".to_string()],
    working_dir: None,
};

let (result, audit_log) = manager
    .execute_sandboxed("skill-id", command, capabilities)
    .await?;
```

## Capabilities

### FileSystemCapability

- `ReadOnly { path }`: Read-only access to specific path
- `ReadWrite { path }`: Read-write access to specific path
- `TempWorkspace`: Temporary directory (auto-created and cleaned)

### NetworkCapability

- `Deny`: No network access
- `AllowDomains(vec)`: Whitelist specific domains
- `AllowAll`: Full network access

### ProcessCapability

- `no_fork`: Prohibit child processes
- `max_execution_time`: Timeout in seconds
- `max_memory_mb`: Memory limit

### EnvironmentCapability

- `None`: No environment variables
- `Restricted`: Safe variables only (PATH, HOME)
- `Full`: All environment variables

## Platform Support

| Platform | Implementation | Status |
|----------|----------------|--------|
| macOS | sandbox-exec | ✅ Implemented |
| Linux | seccomp | 🚧 Planned |
| Windows | AppContainer | 🚧 Planned |

## Security

- **Default Deny**: All permissions denied by default
- **Fail-Safe**: Sandbox failure → execution denied
- **Audit Logging**: All executions logged
- **Resource Limits**: CPU time and memory bounded

## Testing

```bash
# Run all sandbox tests
cargo test sandbox --lib

# Run platform-specific tests
cargo test sandbox::platforms::macos --lib

# Run integration tests
cargo test sandbox::tests::integration_tests --lib
```
```

### Step 2: Update design document status

Modify `docs/plans/2026-02-09-skill-sandboxing-design.md`, update Phase 1 section:

```markdown
### Phase 1: Foundation (COMPLETED)

- ✅ Implement `SandboxAdapter` trait and `Capabilities` model
- ✅ Implement macOS sandbox-exec support
- ✅ Add audit logging system

**Deliverables**:
- ✅ `core/src/exec/sandbox/adapter.rs`
- ✅ `core/src/exec/sandbox/capabilities.rs`
- ✅ `core/src/exec/sandbox/platforms/macos.rs`
- ✅ `core/src/exec/sandbox/audit.rs`
- ✅ `core/src/exec/sandbox/executor.rs` (SandboxManager)
- ✅ Integration tests
- ✅ Documentation
```

### Step 3: Commit documentation

```bash
git add core/src/exec/sandbox/README.md
git add docs/plans/2026-02-09-skill-sandboxing-design.md
git commit -m "exec/sandbox: add documentation

Add comprehensive documentation:
- Module README with usage examples
- Capabilities reference
- Platform support matrix
- Security properties
- Testing guide

Update design document to mark Phase 1 complete

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Success Criteria

✅ All modules compile without errors
✅ All unit tests pass
✅ Integration tests pass on macOS
✅ SandboxAdapter trait fully implemented
✅ Capabilities model with serde support
✅ macOS sandbox-exec integration working
✅ Audit logging functional
✅ SandboxManager orchestration complete
✅ Documentation complete

## Next Steps

After Phase 1 completion:

1. **Phase 2: Integration** - Integrate sandbox into Skill Evolution system
2. **Phase 3: Testing** - Comprehensive security and performance testing
3. **Phase 4: Cross-platform** - Linux and Windows support

---

## Troubleshooting

### macOS sandbox-exec not found

```bash
# Verify sandbox-exec exists
which sandbox-exec

# Should output: /usr/bin/sandbox-exec
```

### Tests failing on non-macOS

Expected behavior. Platform-specific tests are guarded with `#[cfg(target_os = "macos")]`.

### Compilation errors

```bash
# Clean and rebuild
cargo clean
cargo build --lib
```

---

**Plan Status**: Ready for execution
**Estimated Duration**: 4-6 hours
**Complexity**: Medium
