# Skill Sandboxing Architecture Design

**Date**: 2026-02-09
**Status**: Design
**Priority**: Critical
**Author**: Architecture Review

## Executive Summary

This design implements sandboxing for AI-generated skills (Skill Evolution) to ensure safe execution on personal devices. All evolved skills will run in OS-native sandboxes with fine-grained permission control, while built-in tools maintain direct system access.

## Background

### Problem Statement

Aleph's Skill Evolution (Phase 10) enables the system to automatically generate and execute code based on learned patterns. Without proper isolation, this poses significant security risks:

- **System Damage**: AI-generated code could accidentally execute destructive operations (e.g., `rm -rf`)
- **Privacy Breach**: Uncontrolled file system access could expose sensitive data
- **Resource Exhaustion**: Infinite loops or memory leaks could impact system performance
- **Trust Erosion**: Users cannot safely enable self-evolution without protection

### Current State

The existing `SafetyGate` (core/src/skill_evolution/safety.rs) provides static pattern matching for dangerous operations, but:
- Cannot prevent runtime violations
- No execution isolation
- Limited to string pattern detection
- No resource limits enforcement

## Design Principles

### 1. Trust Boundary Layering

```
┌─────────────────────────────────────────┐
│  System Layer (Trusted)                 │
│  - Built-in Tools                       │
│  - Direct system API access             │
│  - Developer audited                    │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Application Layer (Sandboxed)          │
│  - Skill Evolution generated skills     │
│  - OS-native sandbox isolation          │
│  - Fine-grained permissions             │
└─────────────────────────────────────────┘
```

### 2. Zero Runtime Dependencies

- Use OS-native sandbox capabilities (macOS sandbox-exec, Linux seccomp)
- No Docker/Podman requirement
- Maintain "personal device" positioning

### 3. Defensive Design

- Sandbox failure → Deny execution (no unsafe fallback)
- Explicit permission declaration (deny-by-default)
- Complete audit logging

## Architecture

### Module Structure

```
core/src/exec/
├── mod.rs                    # Existing execution manager
├── approval.rs               # Existing approval system
├── sandbox/                  # New: Sandbox subsystem
│   ├── mod.rs               # Sandbox entry point
│   ├── adapter.rs           # SandboxAdapter trait
│   ├── capabilities.rs      # Permission model
│   ├── profile.rs           # Sandbox profile generation
│   ├── executor.rs          # Sandboxed executor
│   ├── platforms/           # Platform-specific implementations
│   │   ├── mod.rs
│   │   ├── macos.rs        # macOS sandbox-exec
│   │   ├── linux.rs        # Linux seccomp/namespaces
│   │   └── windows.rs      # Windows AppContainer
│   └── audit.rs            # Audit logging
```

### Core Abstraction: SandboxAdapter

```rust
// core/src/exec/sandbox/adapter.rs
pub trait SandboxAdapter: Send + Sync {
    /// Check if sandbox is supported on current platform
    fn is_supported(&self) -> bool;

    /// Generate sandbox configuration file (e.g., .sb file)
    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile>;

    /// Execute command in sandbox
    async fn execute_sandboxed(
        &self,
        command: &Command,
        profile: &SandboxProfile,
    ) -> Result<ExecutionResult>;

    /// Cleanup temporary configuration files
    fn cleanup(&self, profile: &SandboxProfile) -> Result<()>;
}
```

## Permission Model

### Capabilities Definition

```rust
// core/src/exec/sandbox/capabilities.rs

/// Sandbox permission set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub filesystem: Vec<FileSystemCapability>,
    pub network: NetworkCapability,
    pub process: ProcessCapability,
    pub environment: EnvironmentCapability,
}

/// Filesystem permission (fine-grained path control)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileSystemCapability {
    /// Read-only access to specific path
    ReadOnly { path: PathBuf },
    /// Read-write access to specific path
    ReadWrite { path: PathBuf },
    /// Temporary workspace (auto-created and cleaned)
    TempWorkspace,
}

/// Network permission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkCapability {
    Deny,
    AllowDomains(Vec<String>),
    AllowAll,
}

/// Process permission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCapability {
    pub no_fork: bool,
    pub max_execution_time: u64,
    pub max_memory_mb: Option<u64>,
}
```

### Default Permission Policy

Safe defaults for Skill Evolution generated skills:

```rust
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,  // 5 minutes
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        }
    }
}
```

## Platform Implementation

### macOS: Sandbox Profile Generation

```rust
// core/src/exec/sandbox/platforms/macos.rs

impl SandboxAdapter for MacOSSandbox {
    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile> {
        let mut profile = String::from("(version 1)\n");
        profile.push_str("(deny default)\n");

        // Allow basic system calls
        profile.push_str("(allow process-exec* (literal \"/usr/bin/python3\"))\n");
        profile.push_str("(allow file-read* (subpath \"/System/Library\"))\n");

        // Generate filesystem rules from Capabilities
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
                    let temp_dir = self.create_temp_workspace()?;
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        temp_dir.display()
                    ));
                }
            }
        }

        // Network rules
        match &caps.network {
            NetworkCapability::Deny => {
                profile.push_str("(deny network*)\n");
            }
            NetworkCapability::AllowAll => {
                profile.push_str("(allow network*)\n");
            }
            _ => {}
        }

        let profile_path = self.write_temp_profile(&profile)?;
        Ok(SandboxProfile { path: profile_path, capabilities: caps.clone() })
    }
}
```

### Execution Flow

```rust
async fn execute_sandboxed(
    &self,
    command: &Command,
    profile: &SandboxProfile,
) -> Result<ExecutionResult> {
    let mut cmd = tokio::process::Command::new("sandbox-exec");
    cmd.arg("-f").arg(&profile.path);
    cmd.arg(command.program);
    cmd.args(&command.args);

    let timeout = Duration::from_secs(
        profile.capabilities.process.max_execution_time
    );

    let output = tokio::time::timeout(timeout, cmd.output()).await??;

    Ok(ExecutionResult {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        sandboxed: true,
    })
}
```

## Integration with Skill Evolution

### Skill Source Identification

```rust
// core/src/skills/types.rs

/// Skill source
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SkillSource {
    /// Built-in skill (developer audited, direct execution)
    Builtin,
    /// User-created skill (trusted, direct execution)
    UserCreated,
    /// Skill Evolution generated (requires sandboxing)
    Evolved {
        generated_at: i64,
        approval_status: ApprovalStatus,
    },
}

impl SkillSource {
    pub fn requires_sandbox(&self) -> bool {
        matches!(self, SkillSource::Evolved { .. })
    }
}
```

### Executor Integration

```rust
// core/src/skills/executor.rs

impl SkillExecutor {
    pub async fn execute_skill(
        &self,
        skill: &Skill,
        context: ExecutionContext,
    ) -> Result<ExecutionResult> {
        if skill.metadata.source.requires_sandbox() {
            let capabilities = self.get_skill_capabilities(skill)?;
            self.sandbox_manager
                .execute_sandboxed(skill, capabilities, context)
                .await
        } else {
            self.execute_direct(skill, context).await
        }
    }
}
```

### Permission Declaration in SKILL.md

```yaml
---
id: evolved-file-organizer
name: File Organizer
source: evolved
generated_at: 1707436800
capabilities:
  filesystem:
    - type: read_write
      path: ~/Documents
  network: deny
  process:
    no_fork: true
    max_execution_time: 300
---
```

## Error Handling and Fallback

### Sandbox Unavailable Handling

```rust
// core/src/exec/sandbox/executor.rs

pub enum FallbackPolicy {
    /// Deny execution (recommended, safest)
    Deny,
    /// Request user approval before direct execution
    RequestApproval,
    /// Log warning and execute directly (not recommended)
    WarnAndExecute,
}

impl SandboxManager {
    async fn handle_sandbox_unavailable(
        &self,
        skill: &Skill,
        context: ExecutionContext,
    ) -> Result<ExecutionResult> {
        match self.fallback_policy {
            FallbackPolicy::Deny => {
                Err(AlephError::SandboxUnavailable {
                    skill_id: skill.id.clone(),
                    reason: "Sandbox not supported on this platform".to_string(),
                })
            }
            FallbackPolicy::RequestApproval => {
                self.request_unsandboxed_approval(skill, context).await
            }
            FallbackPolicy::WarnAndExecute => {
                warn!("Executing evolved skill without sandbox protection");
                self.execute_direct(skill, context).await
            }
        }
    }
}
```

### Audit Logging

```rust
// core/src/exec/sandbox/audit.rs

#[derive(Debug, Serialize)]
pub struct SandboxAuditLog {
    pub timestamp: i64,
    pub skill_id: String,
    pub skill_source: SkillSource,
    pub capabilities: Capabilities,
    pub execution_result: ExecutionStatus,
    pub sandbox_platform: String,
    pub violations: Vec<SandboxViolation>,
}

pub enum ExecutionStatus {
    Success { exit_code: i32, duration_ms: u64 },
    Timeout { duration_ms: u64 },
    SandboxViolation { violation: String },
    Error { error: String },
}
```

## Monitoring and Metrics

```rust
// core/src/exec/sandbox/metrics.rs

pub struct SandboxMetrics {
    pub total_executions: AtomicU64,
    pub successful_executions: AtomicU64,
    pub timeouts: AtomicU64,
    pub violations: AtomicU64,
    pub avg_execution_time_ms: AtomicU64,
}
```

## Implementation Roadmap

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

### Phase 2: Integration (1 week)

- Modify `SkillSource` enum to add source identification
- Integrate into `SkillExecutor`
- Update SKILL.md format to support permission declaration

**Deliverables**:
- Updated `core/src/skills/types.rs`
- Updated `core/src/skills/executor.rs`
- Updated skill manifest parser

### Phase 3: Testing and Optimization (1 week)

- Write unit tests and integration tests
- Test various permission combinations
- Performance optimization (temp file management, config caching)

**Deliverables**:
- Test suite in `core/src/exec/sandbox/tests/`
- Performance benchmarks
- Documentation updates

### Phase 4: Cross-platform Support (Optional, 2-3 weeks)

- Implement Linux seccomp support
- Implement Windows AppContainer support
- Unified test suite

**Deliverables**:
- `core/src/exec/sandbox/platforms/linux.rs`
- `core/src/exec/sandbox/platforms/windows.rs`
- Cross-platform CI tests

## Architecture Guardrails

To prevent future deviation from "personal device" positioning, add architectural guardrails:

```rust
// ARCHITECTURE.md or core/src/lib.rs

/// Architecture Guardrails: Prohibited Dependencies
///
/// The following dependency types indicate deviation from "personal device" positioning:
/// - Distributed databases (etcd, consul, zookeeper)
/// - Container orchestration (kubernetes client)
/// - Multi-tenant auth (oauth2 server, jwt issuer)
/// - Object storage (s3, minio client)
///
/// If these dependencies are needed, architectural review is required.
```

## Security Considerations

### Threat Model

**In Scope**:
- Accidental destructive operations by AI-generated code
- Unintended file system access
- Resource exhaustion (CPU, memory, disk)
- Network access to malicious endpoints

**Out of Scope**:
- Intentional malicious code injection (assumes AI provider is trusted)
- Kernel-level exploits
- Hardware attacks

### Security Properties

1. **Isolation**: Evolved skills cannot access system directories or user data outside declared permissions
2. **Resource Limits**: CPU time and memory usage are bounded
3. **Auditability**: All executions are logged with full context
4. **Fail-Safe**: Sandbox failures result in execution denial, not unsafe fallback

## Performance Considerations

### Overhead Analysis

- **Profile Generation**: ~1-5ms (cached after first generation)
- **Sandbox Startup**: ~10-50ms (OS-dependent)
- **Execution Overhead**: ~5-10% (minimal for I/O-bound tasks)
- **Cleanup**: ~1-2ms (async, non-blocking)

### Optimization Strategies

1. **Profile Caching**: Cache generated profiles for identical capability sets
2. **Temp Workspace Reuse**: Reuse temp directories for same skill across invocations
3. **Async Cleanup**: Cleanup temp files asynchronously after execution

## Testing Strategy

### Unit Tests

- Capability serialization/deserialization
- Profile generation for various permission combinations
- Sandbox adapter platform detection

### Integration Tests

- Execute safe scripts in sandbox (should succeed)
- Execute scripts violating permissions (should fail)
- Timeout enforcement
- Resource limit enforcement

### Security Tests

- Attempt to escape sandbox (should fail)
- Attempt to access prohibited paths (should fail)
- Attempt to fork processes when `no_fork=true` (should fail)

## Success Criteria

✅ All Skill Evolution generated skills execute in sandbox
✅ Zero runtime dependencies (no Docker/Podman)
✅ Sandbox failure results in execution denial
✅ Complete audit logs for all sandboxed executions
✅ Performance overhead < 10% for typical skills
✅ macOS support in Phase 1
✅ Cross-platform support in Phase 4 (optional)

## References

- [Skill Evolution System](../AGENT_SYSTEM.md#skill-evolution)
- [Safety Gate Implementation](../../core/src/skill_evolution/safety.rs)
- [macOS Sandbox Guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/AppSandboxDesignGuide/)
- [Linux seccomp](https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html)

## Appendix: Alternative Approaches Considered

### A. WASM-First Strategy

**Pros**: Cross-platform consistency, strong isolation
**Cons**: Limited ecosystem (Python/Node.js libraries unavailable), compilation complexity

**Decision**: Rejected. OS-native sandbox provides better ecosystem compatibility.

### B. Container-Based Isolation

**Pros**: Strong isolation, familiar tooling
**Cons**: Requires Docker/Podman installation, high resource overhead, conflicts with "personal device" positioning

**Decision**: Rejected. Violates zero-dependency principle.

### C. Lightweight Sandboxing All External Code

**Pros**: Maximum security
**Cons**: High complexity, impacts built-in tools, unnecessary for audited code

**Decision**: Rejected. Trust boundary should be at skill source, not execution runtime.
