# Skill Sandboxing Phase 3 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现契约式权限审批系统和透明化审计机制，为 evolved skills 提供用户审批工作流和安全测试

**Architecture:** 扩展现有 ApprovalManager 支持 Capability 审批，实现三阶段信任演进（Draft → Trial → Verified），添加 Adaptive Runtime Escorts 机制，创建 SQLite 审计表，实现 Audit Dashboard CLI 命令

**Tech Stack:** Rust + Tokio + rusqlite + serde + chrono

---

## Task 1: Approval Types Foundation

**Files:**
- Create: `core/src/exec/approval/types.rs`
- Modify: `core/src/exec/approval/mod.rs`
- Test: `core/src/exec/approval/types.rs` (inline tests)

**Step 1: 编写 TrustStage 和 EscalationReason 的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_stage_progression() {
        let draft = TrustStage::Draft;
        assert!(matches!(draft, TrustStage::Draft));

        let trial = TrustStage::Trial;
        assert!(matches!(trial, TrustStage::Trial));

        let verified = TrustStage::Verified;
        assert!(matches!(verified, TrustStage::Verified));
    }

    #[test]
    fn test_escalation_reason_variants() {
        let reasons = vec![
            EscalationReason::PathOutOfScope,
            EscalationReason::SensitiveDirectory,
            EscalationReason::UndeclaredBinding,
            EscalationReason::FirstExecution,
        ];
        assert_eq!(reasons.len(), 4);
    }
}
```

**Step 2: 运行测试确认失败**

Run: `cd core && cargo test --lib exec::approval::types`
Expected: FAIL with "module not found"

**Step 3: 创建 approval 模块和 types.rs**

```rust
// core/src/exec/approval/mod.rs
pub mod types;

// core/src/exec/approval/types.rs
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Trust stage for capability approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustStage {
    /// Tool just generated, waiting for first approval
    Draft,
    /// Approved, waiting for first execution confirmation
    Trial,
    /// Executed multiple times, entered silent mode
    Verified,
}

/// Reason for escalation trigger
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    /// Parameter exceeds custom_paths range
    PathOutOfScope,
    /// Accessing sensitive directory
    SensitiveDirectory,
    /// Using undeclared parameter binding
    UndeclaredBinding,
    /// First execution (Trial stage)
    FirstExecution,
}

/// Escalation trigger information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationTrigger {
    pub reason: EscalationReason,
    pub requested_path: Option<PathBuf>,
    pub approved_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_stage_progression() {
        let draft = TrustStage::Draft;
        assert!(matches!(draft, TrustStage::Draft));

        let trial = TrustStage::Trial;
        assert!(matches!(trial, TrustStage::Trial));

        let verified = TrustStage::Verified;
        assert!(matches!(verified, TrustStage::Verified));
    }

    #[test]
    fn test_escalation_reason_variants() {
        let reasons = vec![
            EscalationReason::PathOutOfScope,
            EscalationReason::SensitiveDirectory,
            EscalationReason::UndeclaredBinding,
            EscalationReason::FirstExecution,
        ];
        assert_eq!(reasons.len(), 4);
    }
}
```

**Step 4: 在 exec/mod.rs 中添加 approval 模块**

```rust
// core/src/exec/mod.rs
pub mod approval;
```

**Step 5: 运行测试确认通过**

Run: `cd core && cargo test --lib exec::approval::types`
Expected: PASS

**Step 6: 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/feature/skill-sandboxing-phase3
git add core/src/exec/approval/
git commit -m "exec/approval: add TrustStage and EscalationReason types

- Add TrustStage enum (Draft/Trial/Verified)
- Add EscalationReason enum (PathOutOfScope/SensitiveDirectory/UndeclaredBinding/FirstExecution)
- Add EscalationTrigger struct
- Add unit tests for type variants

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Capability Approval Request Types

**Files:**
- Modify: `core/src/exec/approval/types.rs`
- Test: `core/src/exec/approval/types.rs` (inline tests)

**Step 1: 编写 CapabilityApprovalRequest 的测试**

```rust
#[test]
fn test_capability_approval_request_creation() {
    use crate::exec::sandbox::capabilities::Capabilities;
    use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

    let required = RequiredCapabilities {
        preset: Some("file_processor".to_string()),
        overrides: None,
        parameter_bindings: None,
    };

    let resolved = Capabilities {
        file_read: vec!["/tmp/*".to_string()],
        file_write: vec!["/tmp/*".to_string()],
        network_access: vec![],
        allow_exec: false,
    };

    let request = CapabilityApprovalRequest {
        tool_name: "test_tool".to_string(),
        tool_description: "A test tool".to_string(),
        required_capabilities: required,
        resolved_capabilities: resolved,
        trust_stage: TrustStage::Draft,
    };

    assert_eq!(request.tool_name, "test_tool");
    assert_eq!(request.trust_stage, TrustStage::Draft);
}
```

**Step 2: 运行测试确认失败**

Run: `cd core && cargo test --lib exec::approval::types::test_capability_approval_request_creation`
Expected: FAIL with "CapabilityApprovalRequest not found"

**Step 3: 实现 CapabilityApprovalRequest**

```rust
use crate::exec::sandbox::capabilities::Capabilities;
use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

/// Capability approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityApprovalRequest {
    pub tool_name: String,
    pub tool_description: String,
    pub required_capabilities: RequiredCapabilities,
    pub resolved_capabilities: Capabilities,
    pub trust_stage: TrustStage,
}

/// Approval request enum (unified)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
    Capability(CapabilityApprovalRequest),
}

/// Command approval request (placeholder for existing type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
}
```

**Step 4: 运行测试确认通过**

Run: `cd core && cargo test --lib exec::approval::types`
Expected: PASS

**Step 5: 提交**

```bash
git add core/src/exec/approval/types.rs
git commit -m "exec/approval: add CapabilityApprovalRequest type

- Add CapabilityApprovalRequest struct
- Add unified ApprovalRequest enum (Command/Capability)
- Add CommandApprovalRequest placeholder
- Add unit test for capability request creation

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Approval Metadata for Tool Definition

**Files:**
- Modify: `core/src/skill_evolution/tool_generator.rs`
- Test: `core/src/skill_evolution/tool_generator.rs` (inline tests)

**Step 1: 编写 ApprovalMetadata 的测试**

```rust
#[test]
fn test_approval_metadata_serialization() {
    let metadata = ApprovalMetadata {
        approved: true,
        approved_at: Some("2026-02-09T10:30:00Z".to_string()),
        approved_by: Some("owner".to_string()),
        approval_scope: Some("permanent".to_string()),
        trust_stage: Some("verified".to_string()),
        execution_count: 42,
        last_executed_at: Some("2026-02-09T15:20:00Z".to_string()),
    };

    let json = serde_json::to_string(&metadata).unwrap();
    assert!(json.contains("\"approved\":true"));
    assert!(json.contains("\"execution_count\":42"));
}
```

**Step 2: 运行测试确认失败**

Run: `cd core && cargo test --lib skill_evolution::tool_generator::test_approval_metadata_serialization`
Expected: FAIL with "ApprovalMetadata not found"

**Step 3: 实现 ApprovalMetadata**

```rust
/// Approval metadata for tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalMetadata {
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_stage: Option<String>,
    #[serde(default)]
    pub execution_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_executed_at: Option<String>,
}

impl Default for ApprovalMetadata {
    fn default() -> Self {
        Self {
            approved: false,
            approved_at: None,
            approved_by: None,
            approval_scope: None,
            trust_stage: Some("draft".to_string()),
            execution_count: 0,
            last_executed_at: None,
        }
    }
}
```

**Step 4: 添加到 GeneratedToolDefinition**

```rust
pub struct GeneratedToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub runtime: String,
    pub entrypoint: String,
    pub self_tested: bool,
    pub requires_confirmation: bool,
    pub required_capabilities: Option<RequiredCapabilities>,
    pub approval_metadata: Option<ApprovalMetadata>,  // NEW
    pub generated: GenerationMetadata,
}
```

**Step 5: 运行测试确认通过**

Run: `cd core && cargo test --lib skill_evolution::tool_generator`
Expected: PASS

**Step 6: 提交**

```bash
git add core/src/skill_evolution/tool_generator.rs
git commit -m "skill_evolution: add ApprovalMetadata to tool definition

- Add ApprovalMetadata struct with approval state
- Add approval_metadata field to GeneratedToolDefinition
- Add serialization test for approval metadata
- Default trust_stage to 'draft'

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 4: SQLite Audit Tables

**Files:**
- Create: `core/src/exec/approval/storage.rs`
- Modify: `core/src/exec/approval/mod.rs`
- Test: `core/src/exec/approval/storage.rs` (inline tests)

**Step 1: 编写审计表创建的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_audit_tables() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Verify tables exist
        let conn = storage.conn.lock().await;
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"capability_approvals".to_string()));
        assert!(tables.contains(&"capability_escalations".to_string()));
    }
}
```

**Step 2: 运行测试确认失败**

Run: `cd core && cargo test --lib exec::approval::storage`
Expected: FAIL with "module not found"

**Step 3: 实现 ApprovalAuditStorage**

```rust
// core/src/exec/approval/storage.rs
use rusqlite::{Connection, Result as SqliteResult};
use std::path::Path;
use tokio::sync::Mutex;

pub struct ApprovalAuditStorage {
    conn: Mutex<Connection>,
}

impl ApprovalAuditStorage {
    pub async fn new(db_path: &Path) -> SqliteResult<Self> {
        let conn = Connection::open(db_path)?;

        // Create capability_approvals table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS capability_approvals (
                id INTEGER PRIMARY KEY,
                tool_name TEXT NOT NULL,
                capabilities_hash TEXT NOT NULL,
                approved BOOLEAN NOT NULL,
                approved_by TEXT NOT NULL,
                approval_scope TEXT NOT NULL,
                approved_at INTEGER NOT NULL,
                reason TEXT
            )",
            [],
        )?;

        // Create capability_escalations table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS capability_escalations (
                id INTEGER PRIMARY KEY,
                tool_name TEXT NOT NULL,
                execution_id TEXT NOT NULL,
                escalation_reason TEXT NOT NULL,
                requested_path TEXT,
                approved_paths TEXT,
                user_decision TEXT,
                decided_at INTEGER NOT NULL
            )",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_audit_tables() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Verify tables exist
        let conn = storage.conn.lock().await;
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"capability_approvals".to_string()));
        assert!(tables.contains(&"capability_escalations".to_string()));
    }
}
```

**Step 4: 在 mod.rs 中添加 storage 模块**

```rust
// core/src/exec/approval/mod.rs
pub mod types;
pub mod storage;
```

**Step 5: 运行测试确认通过**

Run: `cd core && cargo test --lib exec::approval::storage`
Expected: PASS

**Step 6: 提交**

```bash
git add core/src/exec/approval/
git commit -m "exec/approval: add SQLite audit tables

- Create ApprovalAuditStorage with capability_approvals table
- Create capability_escalations table
- Add async initialization with tokio::sync::Mutex
- Add test for table creation

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Runtime Escalation Checker

**Files:**
- Create: `core/src/exec/approval/escalation.rs`
- Modify: `core/src/exec/approval/mod.rs`
- Test: `core/src/exec/approval/escalation.rs` (inline tests)

**Step 1: 编写 escalation 检查的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_path_out_of_scope_detection() {
        let approved_paths = vec!["/tmp/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/etc/passwd".to_string());

        let trigger = check_path_escalation(&params, &approved_paths);
        assert!(trigger.is_some());
        assert_eq!(trigger.unwrap().reason, EscalationReason::PathOutOfScope);
    }

    #[test]
    fn test_sensitive_directory_detection() {
        let path = PathBuf::from("/Users/test/.ssh/id_rsa");
        assert!(is_sensitive_directory(&path));

        let path = PathBuf::from("/Users/test/Documents/file.txt");
        assert!(!is_sensitive_directory(&path));
    }
}
```

**Step 2: 运行测试确认失败**

Run: `cd core && cargo test --lib exec::approval::escalation`
Expected: FAIL with "module not found"

**Step 3: 实现 escalation 检查逻辑**

```rust
// core/src/exec/approval/escalation.rs
use super::types::{EscalationReason, EscalationTrigger};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Check if path escalation is needed
pub fn check_path_escalation(
    params: &HashMap<String, String>,
    approved_paths: &[String],
) -> Option<EscalationTrigger> {
    for (key, value) in params {
        if key.contains("path") || key.contains("file") || key.contains("dir") {
            let path = PathBuf::from(value);

            // Check if path is within approved paths
            let is_approved = approved_paths.iter().any(|approved| {
                // Simple glob matching (simplified)
                if approved.ends_with("/*") {
                    let prefix = approved.trim_end_matches("/*");
                    value.starts_with(prefix)
                } else {
                    value == approved
                }
            });

            if !is_approved {
                return Some(EscalationTrigger {
                    reason: EscalationReason::PathOutOfScope,
                    requested_path: Some(path),
                    approved_paths: approved_paths.to_vec(),
                });
            }

            // Check if sensitive directory
            if is_sensitive_directory(&path) {
                return Some(EscalationTrigger {
                    reason: EscalationReason::SensitiveDirectory,
                    requested_path: Some(path),
                    approved_paths: approved_paths.to_vec(),
                });
            }
        }
    }

    None
}

/// Check if path is in sensitive directory
pub fn is_sensitive_directory(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    let sensitive_patterns = [
        "/.ssh/",
        "/.gnupg/",
        "/Keychain.app/",
        "/.aws/",
        "/.config/gcloud/",
    ];

    sensitive_patterns.iter().any(|pattern| path_str.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_path_out_of_scope_detection() {
        let approved_paths = vec!["/tmp/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/etc/passwd".to_string());

        let trigger = check_path_escalation(&params, &approved_paths);
        assert!(trigger.is_some());
        assert_eq!(trigger.unwrap().reason, EscalationReason::PathOutOfScope);
    }

    #[test]
    fn test_sensitive_directory_detection() {
        let path = PathBuf::from("/Users/test/.ssh/id_rsa");
        assert!(is_sensitive_directory(&path));

        let path = PathBuf::from("/Users/test/Documents/file.txt");
        assert!(!is_sensitive_directory(&path));
    }
}
```

**Step 4: 在 mod.rs 中添加 escalation 模块**

```rust
// core/src/exec/approval/mod.rs
pub mod types;
pub mod storage;
pub mod escalation;
```

**Step 5: 运行测试确认通过**

Run: `cd core && cargo test --lib exec::approval::escalation`
Expected: PASS

**Step 6: 提交**

```bash
git add core/src/exec/approval/
git commit -m "exec/approval: add runtime escalation checker

- Add check_path_escalation() for path scope validation
- Add is_sensitive_directory() for sensitive path detection
- Support glob pattern matching for approved paths
- Add tests for escalation detection

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 6-12: 后续任务

由于篇幅限制，完整的实施计划包含 12 个任务：

- Task 6: Audit Dashboard Data Models
- Task 7: CLI Audit Commands
- Task 8: Security Tests - Path Traversal
- Task 9: Security Tests - Sensitive Directory
- Task 10: Security Tests - Undeclared Binding
- Task 11: Performance Benchmarks
- Task 12: Integration Tests

每个任务都遵循相同的 TDD 模式：
1. 编写失败的测试
2. 运行测试确认失败
3. 实现最小代码使测试通过
4. 运行测试确认通过
5. 提交

---

## 执行策略

**推荐**: 使用 `superpowers:subagent-driven-development` 逐任务执行，每个任务完成后进行代码审查。

**总工期**: 5 周（核心功能）

**成功标准**:
- ✅ 所有测试通过
- ✅ 代码覆盖率 > 80%
- ✅ 性能基准达标
- ✅ 安全测试全部通过
