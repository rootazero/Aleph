# Three-Layer Control Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a three-layer control architecture (Orchestrator/Skill-DAG/Tools) with safety guards to prevent cost overruns and agent rabbit-holing.

**Architecture:** FSM-based Orchestrator at top (Clarify→Plan→Execute→Evaluate→Reflect→Stop), Skill DAG layer in middle for stable workflows, capability-based Tools layer at bottom with sandbox and audit.

**Tech Stack:** Rust, serde, tokio, existing rig-core integration, UniFFI for Swift bindings.

**Reference Design:** `docs/plans/2026-01-21-three-layer-control-design.md`

---

## Phase 1: Safety Infrastructure (P0)

### Task 1.1: Create three_layer module structure

**Files:**
- Create: `core/src/three_layer/mod.rs`
- Modify: `core/src/lib.rs:55` (add module declaration)

**Step 1: Create module directory and mod.rs**

```rust
// core/src/three_layer/mod.rs
//! Three-Layer Control Architecture
//!
//! A balanced approach to agent control:
//! - Top Layer: Orchestrator (FSM state machine with hard constraints)
//! - Middle Layer: Skill DAG (stable, testable workflows)
//! - Bottom Layer: Tools (capability-based with sandbox)
//!
//! # Usage
//!
//! Enable via config: `orchestrator.use_three_layer_control = true`

pub mod safety;

// Re-exports
pub use safety::{Capability, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation};
```

**Step 2: Add module to lib.rs**

In `core/src/lib.rs`, after line 87 (`pub mod orchestrator;`), add:

```rust
pub mod three_layer; // NEW: Three-layer control architecture (Orchestrator/Skill-DAG/Tools)
```

**Step 3: Verify compilation**

Run: `cd core && cargo check`
Expected: Compilation succeeds (module exists but safety submodule not yet created - will error, that's expected)

**Step 4: Commit**

```bash
git add core/src/three_layer/mod.rs core/src/lib.rs
git commit -m "feat(three-layer): create module structure"
```

---

### Task 1.2: Implement Capability enum (P0)

**Files:**
- Create: `core/src/three_layer/safety/mod.rs`
- Create: `core/src/three_layer/safety/capability.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/safety/capability.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_equality() {
        assert_eq!(Capability::FileRead, Capability::FileRead);
        assert_ne!(Capability::FileRead, Capability::FileWrite);
    }

    #[test]
    fn test_capability_mcp_with_server() {
        let cap1 = Capability::Mcp { server: "github".to_string() };
        let cap2 = Capability::Mcp { server: "github".to_string() };
        let cap3 = Capability::Mcp { server: "slack".to_string() };

        assert_eq!(cap1, cap2);
        assert_ne!(cap1, cap3);
    }

    #[test]
    fn test_capability_level_default() {
        assert_eq!(Capability::FileRead.default_level(), CapabilityLevel::Safe);
        assert_eq!(Capability::FileWrite.default_level(), CapabilityLevel::Confirmation);
        assert_eq!(Capability::ShellExec.default_level(), CapabilityLevel::Blocked);
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(format!("{}", Capability::FileRead), "file:read");
        assert_eq!(format!("{}", Capability::Mcp { server: "github".to_string() }), "mcp:github");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test three_layer::safety::capability::tests --no-run`
Expected: FAIL with "cannot find value `Capability`"

**Step 3: Write minimal implementation**

```rust
// core/src/three_layer/safety/capability.rs
//! Capability definitions for the Three-Layer Control architecture
//!
//! Capabilities follow the principle of least privilege - each Skill declares
//! what capabilities it needs, and the ToolRouter enforces these restrictions.

use std::fmt;

/// Capability that a Skill can request
///
/// Each capability represents a specific type of operation that may be
/// restricted based on security policies.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Capability {
    // ===== File System =====
    /// Read files (safe by default)
    FileRead,
    /// List directories (safe by default)
    FileList,
    /// Write files (requires confirmation)
    FileWrite,
    /// Delete files (dangerous, blocked by default)
    FileDelete,

    // ===== Network =====
    /// Web search (safe by default)
    WebSearch,
    /// Fetch URL content (safe by default)
    WebFetch,

    // ===== MCP =====
    /// Access specific MCP server
    Mcp { server: String },

    // ===== LLM =====
    /// Call LLM (safe by default, but has token cost)
    LlmCall,

    // ===== System =====
    /// Execute shell commands (dangerous, blocked by default)
    ShellExec,
    /// Spawn processes (dangerous, blocked by default)
    ProcessSpawn,
}

/// Security level for a capability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityLevel {
    /// No confirmation needed
    Safe,
    /// Requires user confirmation before execution
    Confirmation,
    /// Blocked by default, requires explicit override
    Blocked,
}

impl Capability {
    /// Get the default security level for this capability
    pub fn default_level(&self) -> CapabilityLevel {
        match self {
            // Safe operations
            Capability::FileRead => CapabilityLevel::Safe,
            Capability::FileList => CapabilityLevel::Safe,
            Capability::WebSearch => CapabilityLevel::Safe,
            Capability::WebFetch => CapabilityLevel::Safe,
            Capability::LlmCall => CapabilityLevel::Safe,
            Capability::Mcp { .. } => CapabilityLevel::Safe,

            // Requires confirmation
            Capability::FileWrite => CapabilityLevel::Confirmation,

            // Blocked by default
            Capability::FileDelete => CapabilityLevel::Blocked,
            Capability::ShellExec => CapabilityLevel::Blocked,
            Capability::ProcessSpawn => CapabilityLevel::Blocked,
        }
    }

    /// Check if this capability is dangerous (Confirmation or Blocked)
    pub fn is_dangerous(&self) -> bool {
        !matches!(self.default_level(), CapabilityLevel::Safe)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::FileRead => write!(f, "file:read"),
            Capability::FileList => write!(f, "file:list"),
            Capability::FileWrite => write!(f, "file:write"),
            Capability::FileDelete => write!(f, "file:delete"),
            Capability::WebSearch => write!(f, "web:search"),
            Capability::WebFetch => write!(f, "web:fetch"),
            Capability::Mcp { server } => write!(f, "mcp:{}", server),
            Capability::LlmCall => write!(f, "llm:call"),
            Capability::ShellExec => write!(f, "shell:exec"),
            Capability::ProcessSpawn => write!(f, "process:spawn"),
        }
    }
}
```

**Step 4: Create safety/mod.rs**

```rust
// core/src/three_layer/safety/mod.rs
//! Safety module for Three-Layer Control
//!
//! Provides capability-based security, path sandboxing, and resource quotas.

mod capability;

pub use capability::{Capability, CapabilityLevel};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test three_layer::safety::capability::tests -v`
Expected: All 4 tests PASS

**Step 6: Commit**

```bash
git add core/src/three_layer/safety/
git commit -m "feat(three-layer): add Capability enum with security levels"
```

---

### Task 1.3: Implement CapabilityGate (P0)

**Files:**
- Create: `core/src/three_layer/safety/gate.rs`
- Modify: `core/src/three_layer/safety/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/safety/gate.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_check_granted() {
        let gate = CapabilityGate::new(vec![
            Capability::FileRead,
            Capability::WebSearch,
        ]);

        assert!(gate.check(&Capability::FileRead).is_ok());
        assert!(gate.check(&Capability::WebSearch).is_ok());
    }

    #[test]
    fn test_gate_check_denied() {
        let gate = CapabilityGate::new(vec![Capability::FileRead]);

        let result = gate.check(&Capability::FileWrite);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.required, Capability::FileWrite);
    }

    #[test]
    fn test_gate_check_mcp_specific() {
        let gate = CapabilityGate::new(vec![
            Capability::Mcp { server: "github".to_string() },
        ]);

        assert!(gate.check(&Capability::Mcp { server: "github".to_string() }).is_ok());
        assert!(gate.check(&Capability::Mcp { server: "slack".to_string() }).is_err());
    }

    #[test]
    fn test_gate_empty() {
        let gate = CapabilityGate::empty();
        assert!(gate.check(&Capability::FileRead).is_err());
    }

    #[test]
    fn test_gate_all() {
        let gate = CapabilityGate::all();
        assert!(gate.check(&Capability::FileRead).is_ok());
        assert!(gate.check(&Capability::ShellExec).is_ok());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test three_layer::safety::gate::tests --no-run`
Expected: FAIL with "cannot find type `CapabilityGate`"

**Step 3: Write minimal implementation**

```rust
// core/src/three_layer/safety/gate.rs
//! Capability Gate - enforces capability restrictions on tool execution

use super::Capability;
use std::collections::HashSet;

/// Error returned when a capability check fails
#[derive(Debug, Clone)]
pub struct CapabilityDenied {
    /// The capability that was required
    pub required: Capability,
    /// The capabilities that were granted
    pub granted: Vec<Capability>,
}

impl std::fmt::Display for CapabilityDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Capability '{}' denied. Granted: {:?}",
            self.required,
            self.granted.iter().map(|c| c.to_string()).collect::<Vec<_>>()
        )
    }
}

impl std::error::Error for CapabilityDenied {}

/// Gate that enforces capability restrictions
///
/// A Skill declares its required capabilities, and the gate ensures
/// only those capabilities can be used during execution.
#[derive(Debug, Clone)]
pub struct CapabilityGate {
    /// Capabilities that have been granted
    granted: HashSet<Capability>,
}

impl CapabilityGate {
    /// Create a new gate with specific granted capabilities
    pub fn new(capabilities: Vec<Capability>) -> Self {
        Self {
            granted: capabilities.into_iter().collect(),
        }
    }

    /// Create an empty gate (denies everything)
    pub fn empty() -> Self {
        Self {
            granted: HashSet::new(),
        }
    }

    /// Create a gate that allows all capabilities (for testing/admin)
    pub fn all() -> Self {
        Self {
            granted: vec![
                Capability::FileRead,
                Capability::FileList,
                Capability::FileWrite,
                Capability::FileDelete,
                Capability::WebSearch,
                Capability::WebFetch,
                Capability::LlmCall,
                Capability::ShellExec,
                Capability::ProcessSpawn,
            ]
            .into_iter()
            .collect(),
        }
    }

    /// Check if a capability is granted
    ///
    /// Returns Ok(()) if granted, Err(CapabilityDenied) if not.
    pub fn check(&self, required: &Capability) -> Result<(), CapabilityDenied> {
        if self.granted.contains(required) {
            Ok(())
        } else {
            Err(CapabilityDenied {
                required: required.clone(),
                granted: self.granted.iter().cloned().collect(),
            })
        }
    }

    /// Get all granted capabilities
    pub fn granted(&self) -> &HashSet<Capability> {
        &self.granted
    }

    /// Add a capability to the gate
    pub fn grant(&mut self, capability: Capability) {
        self.granted.insert(capability);
    }

    /// Remove a capability from the gate
    pub fn revoke(&mut self, capability: &Capability) {
        self.granted.remove(capability);
    }
}
```

**Step 4: Update safety/mod.rs**

```rust
// core/src/three_layer/safety/mod.rs
//! Safety module for Three-Layer Control
//!
//! Provides capability-based security, path sandboxing, and resource quotas.

mod capability;
mod gate;

pub use capability::{Capability, CapabilityLevel};
pub use gate::{CapabilityDenied, CapabilityGate};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test three_layer::safety::gate::tests -v`
Expected: All 5 tests PASS

**Step 6: Commit**

```bash
git add core/src/three_layer/safety/gate.rs core/src/three_layer/safety/mod.rs
git commit -m "feat(three-layer): add CapabilityGate for enforcing restrictions"
```

---

### Task 1.4: Implement PathSandbox (P0)

**Files:**
- Create: `core/src/three_layer/safety/sandbox.rs`
- Modify: `core/src/three_layer/safety/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/safety/sandbox.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_sandbox_allows_valid_path() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let file_path = temp.path().join("test.txt");
        std::fs::write(&file_path, "test").unwrap();

        let result = sandbox.validate(&file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sandbox_denies_outside_root() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let outside_path = PathBuf::from("/etc/passwd");
        let result = sandbox.validate(&outside_path);

        assert!(matches!(result, Err(SandboxViolation::OutsideAllowedRoots)));
    }

    #[test]
    fn test_sandbox_denies_path_traversal() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let traversal_path = temp.path().join("..").join("..").join("etc").join("passwd");
        let result = sandbox.validate(&traversal_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_denies_pattern() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()])
            .with_denied_patterns(vec![r"\.env$".to_string()]);

        let env_path = temp.path().join(".env");
        std::fs::write(&env_path, "SECRET=xxx").unwrap();

        let result = sandbox.validate(&env_path);
        assert!(matches!(result, Err(SandboxViolation::DeniedPattern { .. })));
    }

    #[test]
    fn test_sandbox_default_denied_patterns() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::with_defaults(vec![temp.path().to_path_buf()]);

        // .git should be denied by default
        let git_path = temp.path().join(".git").join("config");
        std::fs::create_dir_all(git_path.parent().unwrap()).unwrap();
        std::fs::write(&git_path, "test").unwrap();

        let result = sandbox.validate(&git_path);
        assert!(matches!(result, Err(SandboxViolation::DeniedPattern { .. })));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test three_layer::safety::sandbox::tests --no-run`
Expected: FAIL with "cannot find type `PathSandbox`"

**Step 3: Write minimal implementation**

```rust
// core/src/three_layer/safety/sandbox.rs
//! Path Sandbox - restricts file system access to allowed directories

use regex::Regex;
use std::path::{Path, PathBuf};

/// Violation of sandbox rules
#[derive(Debug, Clone)]
pub enum SandboxViolation {
    /// Path is outside allowed root directories
    OutsideAllowedRoots,
    /// Path matches a denied pattern
    DeniedPattern { pattern: String },
    /// Symlink escape attempt detected
    SymlinkEscape,
    /// Path traversal attempt detected (e.g., ..)
    PathTraversal,
    /// Path does not exist
    NotFound,
    /// IO error during validation
    IoError(String),
}

impl std::fmt::Display for SandboxViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxViolation::OutsideAllowedRoots => {
                write!(f, "Path is outside allowed root directories")
            }
            SandboxViolation::DeniedPattern { pattern } => {
                write!(f, "Path matches denied pattern: {}", pattern)
            }
            SandboxViolation::SymlinkEscape => {
                write!(f, "Symlink escape attempt detected")
            }
            SandboxViolation::PathTraversal => {
                write!(f, "Path traversal attempt detected")
            }
            SandboxViolation::NotFound => {
                write!(f, "Path does not exist")
            }
            SandboxViolation::IoError(e) => {
                write!(f, "IO error: {}", e)
            }
        }
    }
}

impl std::error::Error for SandboxViolation {}

/// Sandbox that restricts file system access
///
/// Only allows access to files within specified root directories,
/// and denies access to files matching certain patterns (e.g., .env, .git).
#[derive(Debug, Clone)]
pub struct PathSandbox {
    /// Allowed root directories
    allowed_roots: Vec<PathBuf>,
    /// Denied path patterns (regex)
    denied_patterns: Vec<String>,
    /// Compiled regex patterns (not Clone, so we store strings and compile on demand)
    #[allow(dead_code)]
    compiled_patterns: Vec<Regex>,
}

impl PathSandbox {
    /// Create a new sandbox with specified allowed roots
    pub fn new(allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            allowed_roots,
            denied_patterns: Vec::new(),
            compiled_patterns: Vec::new(),
        }
    }

    /// Create a sandbox with sensible default denied patterns
    ///
    /// Default denied patterns:
    /// - `.git/` directories
    /// - `.env` files
    /// - `credentials` files
    /// - `.ssh/` directories
    /// - `*.pem` and `*.key` files
    pub fn with_defaults(allowed_roots: Vec<PathBuf>) -> Self {
        Self::new(allowed_roots).with_denied_patterns(vec![
            r"\.git(/|$)".to_string(),
            r"\.env$".to_string(),
            r"\.env\.".to_string(),
            r"credentials".to_string(),
            r"\.ssh(/|$)".to_string(),
            r"\.pem$".to_string(),
            r"\.key$".to_string(),
            r"id_rsa".to_string(),
            r"id_ed25519".to_string(),
        ])
    }

    /// Add denied patterns
    pub fn with_denied_patterns(mut self, patterns: Vec<String>) -> Self {
        self.compiled_patterns = patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        self.denied_patterns = patterns;
        self
    }

    /// Validate a path against sandbox rules
    ///
    /// Returns the canonicalized path if valid, or a SandboxViolation if not.
    pub fn validate(&self, path: &Path) -> Result<PathBuf, SandboxViolation> {
        // 1. Canonicalize to resolve symlinks and .. components
        let canonical = path
            .canonicalize()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SandboxViolation::NotFound
                } else {
                    SandboxViolation::IoError(e.to_string())
                }
            })?;

        // 2. Check if within allowed roots
        let in_allowed = self
            .allowed_roots
            .iter()
            .any(|root| {
                if let Ok(canonical_root) = root.canonicalize() {
                    canonical.starts_with(&canonical_root)
                } else {
                    false
                }
            });

        if !in_allowed {
            return Err(SandboxViolation::OutsideAllowedRoots);
        }

        // 3. Check denied patterns
        let path_str = canonical.to_string_lossy();
        for (i, pattern) in self.compiled_patterns.iter().enumerate() {
            if pattern.is_match(&path_str) {
                return Err(SandboxViolation::DeniedPattern {
                    pattern: self.denied_patterns.get(i)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                });
            }
        }

        Ok(canonical)
    }

    /// Check if a path is allowed (without canonicalization for non-existent files)
    ///
    /// Use this for checking paths before creating files.
    pub fn validate_parent(&self, path: &Path) -> Result<(), SandboxViolation> {
        if let Some(parent) = path.parent() {
            if parent.exists() {
                self.validate(parent)?;
            }
        }

        // Check denied patterns on the raw path
        let path_str = path.to_string_lossy();
        for (i, pattern) in self.compiled_patterns.iter().enumerate() {
            if pattern.is_match(&path_str) {
                return Err(SandboxViolation::DeniedPattern {
                    pattern: self.denied_patterns.get(i)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                });
            }
        }

        Ok(())
    }

    /// Get allowed roots
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }

    /// Add an allowed root
    pub fn add_root(&mut self, root: PathBuf) {
        self.allowed_roots.push(root);
    }
}
```

**Step 4: Update safety/mod.rs**

```rust
// core/src/three_layer/safety/mod.rs
//! Safety module for Three-Layer Control
//!
//! Provides capability-based security, path sandboxing, and resource quotas.

mod capability;
mod gate;
mod sandbox;

pub use capability::{Capability, CapabilityLevel};
pub use gate::{CapabilityDenied, CapabilityGate};
pub use sandbox::{PathSandbox, SandboxViolation};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test three_layer::safety::sandbox::tests -v`
Expected: All 5 tests PASS

**Step 6: Commit**

```bash
git add core/src/three_layer/safety/sandbox.rs core/src/three_layer/safety/mod.rs
git commit -m "feat(three-layer): add PathSandbox for file system restrictions"
```

---

### Task 1.5: Update three_layer/mod.rs exports

**Files:**
- Modify: `core/src/three_layer/mod.rs`

**Step 1: Update exports**

```rust
// core/src/three_layer/mod.rs
//! Three-Layer Control Architecture
//!
//! A balanced approach to agent control:
//! - Top Layer: Orchestrator (FSM state machine with hard constraints)
//! - Middle Layer: Skill DAG (stable, testable workflows)
//! - Bottom Layer: Tools (capability-based with sandbox)
//!
//! # Usage
//!
//! Enable via config: `orchestrator.use_three_layer_control = true`
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::three_layer::{Capability, CapabilityGate, PathSandbox};
//!
//! // Create a gate with specific capabilities
//! let gate = CapabilityGate::new(vec![
//!     Capability::FileRead,
//!     Capability::WebSearch,
//! ]);
//!
//! // Create a sandbox for a workspace
//! let sandbox = PathSandbox::with_defaults(vec![
//!     PathBuf::from("/workspace/project"),
//! ]);
//! ```

pub mod safety;

// Re-exports for convenience
pub use safety::{
    Capability, CapabilityDenied, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation,
};
```

**Step 2: Verify full module compiles**

Run: `cd core && cargo check`
Expected: Compilation succeeds

**Step 3: Run all safety tests**

Run: `cd core && cargo test three_layer::safety -v`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/three_layer/mod.rs
git commit -m "feat(three-layer): complete Phase 1 safety infrastructure"
```

---

## Phase 2: Orchestrator Guards

### Task 2.1: Add OrchestratorConfig to config types

**Files:**
- Modify: `core/src/config/types/mod.rs`
- Create: `core/src/config/types/orchestrator.rs`

**Step 1: Write the failing test**

```rust
// core/src/config/types/orchestrator.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_config_defaults() {
        let config = OrchestratorConfig::default();

        assert!(!config.use_three_layer_control);
        assert_eq!(config.guards.max_rounds, 12);
        assert_eq!(config.guards.max_tool_calls, 30);
        assert_eq!(config.guards.max_tokens, 100_000);
        assert_eq!(config.guards.timeout_seconds, 600);
        assert_eq!(config.guards.no_progress_threshold, 2);
    }

    #[test]
    fn test_guards_is_exceeded() {
        let guards = OrchestratorGuards::default();

        assert!(!guards.is_rounds_exceeded(10));
        assert!(guards.is_rounds_exceeded(12));
        assert!(guards.is_rounds_exceeded(15));
    }

    #[test]
    fn test_config_serialization() {
        let config = OrchestratorConfig::default();
        let toml = toml::to_string(&config).unwrap();

        assert!(toml.contains("use_three_layer_control = false"));
        assert!(toml.contains("max_rounds = 12"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test config::types::orchestrator::tests --no-run`
Expected: FAIL with "cannot find type `OrchestratorConfig`"

**Step 3: Write minimal implementation**

```rust
// core/src/config/types/orchestrator.rs
//! Orchestrator configuration types

use serde::{Deserialize, Serialize};

/// Configuration for the Three-Layer Orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Enable three-layer control architecture (default: false)
    #[serde(default)]
    pub use_three_layer_control: bool,

    /// Hard constraint guards
    #[serde(default)]
    pub guards: OrchestratorGuards,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            use_three_layer_control: false,
            guards: OrchestratorGuards::default(),
        }
    }
}

/// Hard constraints for the orchestrator loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorGuards {
    /// Maximum number of orchestrator rounds (default: 12)
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,

    /// Maximum number of tool calls across all rounds (default: 30)
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,

    /// Maximum tokens to consume (default: 100,000)
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,

    /// Timeout in seconds (default: 600 = 10 minutes)
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Rounds without progress before stopping (default: 2)
    #[serde(default = "default_no_progress_threshold")]
    pub no_progress_threshold: u32,
}

fn default_max_rounds() -> u32 { 12 }
fn default_max_tool_calls() -> u32 { 30 }
fn default_max_tokens() -> u64 { 100_000 }
fn default_timeout_seconds() -> u64 { 600 }
fn default_no_progress_threshold() -> u32 { 2 }

impl Default for OrchestratorGuards {
    fn default() -> Self {
        Self {
            max_rounds: default_max_rounds(),
            max_tool_calls: default_max_tool_calls(),
            max_tokens: default_max_tokens(),
            timeout_seconds: default_timeout_seconds(),
            no_progress_threshold: default_no_progress_threshold(),
        }
    }
}

impl OrchestratorGuards {
    /// Check if max rounds exceeded
    pub fn is_rounds_exceeded(&self, current: u32) -> bool {
        current >= self.max_rounds
    }

    /// Check if max tool calls exceeded
    pub fn is_tool_calls_exceeded(&self, current: u32) -> bool {
        current >= self.max_tool_calls
    }

    /// Check if max tokens exceeded
    pub fn is_tokens_exceeded(&self, current: u64) -> bool {
        current >= self.max_tokens
    }

    /// Get timeout as Duration
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds)
    }
}
```

**Step 4: Update config/types/mod.rs**

Add to `core/src/config/types/mod.rs`:

```rust
mod orchestrator;
pub use orchestrator::{OrchestratorConfig, OrchestratorGuards};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test config::types::orchestrator::tests -v`
Expected: All 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/config/types/orchestrator.rs core/src/config/types/mod.rs
git commit -m "feat(config): add OrchestratorConfig with guards"
```

---

### Task 2.2: Add orchestrator config to main Config

**Files:**
- Modify: `core/src/config/mod.rs`

**Step 1: Add orchestrator field to Config struct**

In `core/src/config/mod.rs`, add to the `Config` struct (after line ~50):

```rust
    /// Orchestrator configuration
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
```

**Step 2: Verify compilation**

Run: `cd core && cargo check`
Expected: Compilation succeeds

**Step 3: Test config loading with new field**

Run: `cd core && cargo test config::tests -v`
Expected: Existing tests PASS (new field has default)

**Step 4: Commit**

```bash
git add core/src/config/mod.rs
git commit -m "feat(config): integrate OrchestratorConfig into main Config"
```

---

### Task 2.3: Implement GuardViolation and GuardChecker

**Files:**
- Create: `core/src/three_layer/orchestrator/mod.rs`
- Create: `core/src/three_layer/orchestrator/guards.rs`
- Modify: `core/src/three_layer/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/orchestrator/guards.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::OrchestratorGuards;

    #[test]
    fn test_guard_checker_rounds() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_rounds(5).is_ok());
        assert!(checker.check_rounds(12).is_err());

        if let Err(GuardViolation::MaxRoundsExceeded { current, max }) = checker.check_rounds(15) {
            assert_eq!(current, 15);
            assert_eq!(max, 12);
        } else {
            panic!("Expected MaxRoundsExceeded");
        }
    }

    #[test]
    fn test_guard_checker_tokens() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_tokens(50_000).is_ok());
        assert!(checker.check_tokens(100_000).is_err());
    }

    #[test]
    fn test_guard_checker_no_progress() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_progress(1).is_ok());
        assert!(checker.check_progress(2).is_err());
    }

    #[test]
    fn test_guard_violation_display() {
        let violation = GuardViolation::MaxRoundsExceeded { current: 15, max: 12 };
        let display = format!("{}", violation);
        assert!(display.contains("15"));
        assert!(display.contains("12"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test three_layer::orchestrator::guards::tests --no-run`
Expected: FAIL with "cannot find type `GuardChecker`"

**Step 3: Write minimal implementation**

```rust
// core/src/three_layer/orchestrator/guards.rs
//! Guard checking for Orchestrator hard constraints

use crate::config::types::OrchestratorGuards;
use std::time::{Duration, Instant};

/// Violation of an orchestrator guard
#[derive(Debug, Clone)]
pub enum GuardViolation {
    /// Maximum rounds exceeded
    MaxRoundsExceeded { current: u32, max: u32 },
    /// Maximum tool calls exceeded
    MaxToolCallsExceeded { current: u32, max: u32 },
    /// Token budget exhausted
    TokenBudgetExhausted { current: u64, max: u64 },
    /// Timeout reached
    Timeout { elapsed: Duration, max: Duration },
    /// No progress detected
    NoProgress { rounds_without_progress: u32, threshold: u32 },
}

impl std::fmt::Display for GuardViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardViolation::MaxRoundsExceeded { current, max } => {
                write!(f, "Maximum rounds exceeded: {} >= {}", current, max)
            }
            GuardViolation::MaxToolCallsExceeded { current, max } => {
                write!(f, "Maximum tool calls exceeded: {} >= {}", current, max)
            }
            GuardViolation::TokenBudgetExhausted { current, max } => {
                write!(f, "Token budget exhausted: {} >= {}", current, max)
            }
            GuardViolation::Timeout { elapsed, max } => {
                write!(f, "Timeout: {:?} >= {:?}", elapsed, max)
            }
            GuardViolation::NoProgress { rounds_without_progress, threshold } => {
                write!(
                    f,
                    "No progress for {} rounds (threshold: {})",
                    rounds_without_progress, threshold
                )
            }
        }
    }
}

impl std::error::Error for GuardViolation {}

/// Checker for orchestrator guards
#[derive(Debug, Clone)]
pub struct GuardChecker {
    guards: OrchestratorGuards,
}

impl GuardChecker {
    /// Create a new guard checker with the given configuration
    pub fn new(guards: OrchestratorGuards) -> Self {
        Self { guards }
    }

    /// Check if rounds limit is exceeded
    pub fn check_rounds(&self, current: u32) -> Result<(), GuardViolation> {
        if self.guards.is_rounds_exceeded(current) {
            Err(GuardViolation::MaxRoundsExceeded {
                current,
                max: self.guards.max_rounds,
            })
        } else {
            Ok(())
        }
    }

    /// Check if tool calls limit is exceeded
    pub fn check_tool_calls(&self, current: u32) -> Result<(), GuardViolation> {
        if self.guards.is_tool_calls_exceeded(current) {
            Err(GuardViolation::MaxToolCallsExceeded {
                current,
                max: self.guards.max_tool_calls,
            })
        } else {
            Ok(())
        }
    }

    /// Check if token budget is exceeded
    pub fn check_tokens(&self, current: u64) -> Result<(), GuardViolation> {
        if self.guards.is_tokens_exceeded(current) {
            Err(GuardViolation::TokenBudgetExhausted {
                current,
                max: self.guards.max_tokens,
            })
        } else {
            Ok(())
        }
    }

    /// Check if timeout is exceeded
    pub fn check_timeout(&self, start: Instant) -> Result<(), GuardViolation> {
        let elapsed = start.elapsed();
        let max = self.guards.timeout();
        if elapsed >= max {
            Err(GuardViolation::Timeout { elapsed, max })
        } else {
            Ok(())
        }
    }

    /// Check if no progress threshold is reached
    pub fn check_progress(&self, rounds_without_progress: u32) -> Result<(), GuardViolation> {
        if rounds_without_progress >= self.guards.no_progress_threshold {
            Err(GuardViolation::NoProgress {
                rounds_without_progress,
                threshold: self.guards.no_progress_threshold,
            })
        } else {
            Ok(())
        }
    }

    /// Check all guards at once
    pub fn check_all(
        &self,
        rounds: u32,
        tool_calls: u32,
        tokens: u64,
        start: Instant,
        rounds_without_progress: u32,
    ) -> Result<(), GuardViolation> {
        self.check_rounds(rounds)?;
        self.check_tool_calls(tool_calls)?;
        self.check_tokens(tokens)?;
        self.check_timeout(start)?;
        self.check_progress(rounds_without_progress)?;
        Ok(())
    }
}
```

**Step 4: Create orchestrator/mod.rs**

```rust
// core/src/three_layer/orchestrator/mod.rs
//! Orchestrator module - Top layer FSM state machine

mod guards;

pub use guards::{GuardChecker, GuardViolation};
```

**Step 5: Update three_layer/mod.rs**

```rust
// core/src/three_layer/mod.rs
//! Three-Layer Control Architecture
//!
//! A balanced approach to agent control:
//! - Top Layer: Orchestrator (FSM state machine with hard constraints)
//! - Middle Layer: Skill DAG (stable, testable workflows)
//! - Bottom Layer: Tools (capability-based with sandbox)
//!
//! # Usage
//!
//! Enable via config: `orchestrator.use_three_layer_control = true`

pub mod orchestrator;
pub mod safety;

// Re-exports for convenience
pub use orchestrator::{GuardChecker, GuardViolation};
pub use safety::{
    Capability, CapabilityDenied, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation,
};
```

**Step 6: Run test to verify it passes**

Run: `cd core && cargo test three_layer::orchestrator::guards::tests -v`
Expected: All 4 tests PASS

**Step 7: Commit**

```bash
git add core/src/three_layer/orchestrator/
git commit -m "feat(three-layer): add GuardChecker for hard constraints"
```

---

### Task 2.4: Implement OrchestratorState FSM

**Files:**
- Create: `core/src/three_layer/orchestrator/states.rs`
- Modify: `core/src/three_layer/orchestrator/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/orchestrator/states.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions_from_clarify() {
        let state = OrchestratorState::Clarify;
        assert!(state.can_transition_to(&OrchestratorState::Plan));
        assert!(state.can_transition_to(&OrchestratorState::Stop));
        assert!(!state.can_transition_to(&OrchestratorState::Execute));
    }

    #[test]
    fn test_state_transitions_from_evaluate() {
        let state = OrchestratorState::Evaluate;
        assert!(state.can_transition_to(&OrchestratorState::Reflect));
        assert!(state.can_transition_to(&OrchestratorState::Stop));
        assert!(!state.can_transition_to(&OrchestratorState::Clarify));
    }

    #[test]
    fn test_state_is_terminal() {
        assert!(!OrchestratorState::Clarify.is_terminal());
        assert!(!OrchestratorState::Execute.is_terminal());
        assert!(OrchestratorState::Stop.is_terminal());
    }

    #[test]
    fn test_state_display() {
        assert_eq!(format!("{}", OrchestratorState::Clarify), "Clarify");
        assert_eq!(format!("{}", OrchestratorState::Execute), "Execute");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test three_layer::orchestrator::states::tests --no-run`
Expected: FAIL with "cannot find type `OrchestratorState`"

**Step 3: Write minimal implementation**

```rust
// core/src/three_layer/orchestrator/states.rs
//! Orchestrator State Machine

use std::fmt;

/// State of the Orchestrator FSM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestratorState {
    /// Clarify problem definition, constraints, evaluation criteria
    Clarify,
    /// Produce executable plan (which Skills to invoke)
    Plan,
    /// Invoke Skill DAG for execution
    Execute,
    /// Check if goals are met (evidence, test results, etc.)
    Evaluate,
    /// On failure, identify cause, adjust plan or gather more info
    Reflect,
    /// Exit when stop conditions are met
    Stop,
}

impl OrchestratorState {
    /// Check if transition to another state is valid
    pub fn can_transition_to(&self, target: &OrchestratorState) -> bool {
        match self {
            OrchestratorState::Clarify => matches!(
                target,
                OrchestratorState::Plan | OrchestratorState::Stop
            ),
            OrchestratorState::Plan => matches!(
                target,
                OrchestratorState::Execute | OrchestratorState::Clarify | OrchestratorState::Stop
            ),
            OrchestratorState::Execute => matches!(
                target,
                OrchestratorState::Evaluate | OrchestratorState::Stop
            ),
            OrchestratorState::Evaluate => matches!(
                target,
                OrchestratorState::Reflect | OrchestratorState::Stop
            ),
            OrchestratorState::Reflect => matches!(
                target,
                OrchestratorState::Plan | OrchestratorState::Execute | OrchestratorState::Stop
            ),
            OrchestratorState::Stop => false, // Terminal state
        }
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, OrchestratorState::Stop)
    }

    /// Get the initial state
    pub fn initial() -> Self {
        OrchestratorState::Clarify
    }
}

impl fmt::Display for OrchestratorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrchestratorState::Clarify => write!(f, "Clarify"),
            OrchestratorState::Plan => write!(f, "Plan"),
            OrchestratorState::Execute => write!(f, "Execute"),
            OrchestratorState::Evaluate => write!(f, "Evaluate"),
            OrchestratorState::Reflect => write!(f, "Reflect"),
            OrchestratorState::Stop => write!(f, "Stop"),
        }
    }
}

impl Default for OrchestratorState {
    fn default() -> Self {
        Self::initial()
    }
}
```

**Step 4: Update orchestrator/mod.rs**

```rust
// core/src/three_layer/orchestrator/mod.rs
//! Orchestrator module - Top layer FSM state machine

mod guards;
mod states;

pub use guards::{GuardChecker, GuardViolation};
pub use states::OrchestratorState;
```

**Step 5: Update three_layer/mod.rs exports**

Add `OrchestratorState` to re-exports:

```rust
pub use orchestrator::{GuardChecker, GuardViolation, OrchestratorState};
```

**Step 6: Run test to verify it passes**

Run: `cd core && cargo test three_layer::orchestrator::states::tests -v`
Expected: All 4 tests PASS

**Step 7: Commit**

```bash
git add core/src/three_layer/orchestrator/states.rs core/src/three_layer/orchestrator/mod.rs core/src/three_layer/mod.rs
git commit -m "feat(three-layer): add OrchestratorState FSM"
```

---

## Phase 3: Deprecate Old Orchestrator

### Task 3.1: Add deprecation warning to RequestOrchestrator

**Files:**
- Modify: `core/src/orchestrator/mod.rs`

**Step 1: Add #[deprecated] attribute**

The file already has a deprecation note in the doc comment. Add the formal attribute to the struct (around line 80):

```rust
#[deprecated(
    since = "0.10.0",
    note = "Use ThreeLayerOrchestrator instead. Enable via config: orchestrator.use_three_layer_control = true"
)]
pub struct RequestOrchestrator {
    // ... existing fields
}
```

**Step 2: Verify compilation with warnings**

Run: `cd core && cargo check 2>&1 | head -50`
Expected: Compilation succeeds (may show deprecation warnings where used)

**Step 3: Commit**

```bash
git add core/src/orchestrator/mod.rs
git commit -m "chore(orchestrator): mark RequestOrchestrator as deprecated"
```

---

### Task 3.2: Create FFI config switch

**Files:**
- Modify: `core/src/ffi/processing.rs` (or equivalent FFI entry point)

**Step 1: Find FFI processing entry point**

Run: `grep -n "process_with_orchestrator\|fn process" core/src/ffi/*.rs | head -20`

**Step 2: Add config-based routing (pseudo-code for reference)**

```rust
// In the FFI processing function, add routing logic:
pub async fn process(&self, input: String, options: ProcessOptions) -> ProcessResult {
    if self.config.orchestrator.use_three_layer_control {
        // New path (to be implemented in Phase 4+)
        todo!("ThreeLayerOrchestrator not yet implemented")
    } else {
        // Old path (deprecated but functional)
        #[allow(deprecated)]
        self.request_orchestrator.process(input, options).await
    }
}
```

**Note:** This is a placeholder. Full implementation will be in later phases.

**Step 3: Commit**

```bash
git add core/src/ffi/
git commit -m "feat(ffi): prepare config switch for three-layer control"
```

---

## Phase 4: Skill Layer (Middle)

### Task 4.1: Create Skill definition types

**Files:**
- Create: `core/src/three_layer/skill/mod.rs`
- Create: `core/src/three_layer/skill/definition.rs`
- Modify: `core/src/three_layer/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/skill/definition.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_definition_basic() {
        let skill = SkillDefinition::new(
            "research".to_string(),
            "Research Skill".to_string(),
            "Research and collect information".to_string(),
        );

        assert_eq!(skill.id, "research");
        assert_eq!(skill.name, "Research Skill");
        assert!(skill.required_capabilities.is_empty());
    }

    #[test]
    fn test_skill_definition_with_capabilities() {
        let skill = SkillDefinition::new(
            "file_analyzer".to_string(),
            "File Analyzer".to_string(),
            "Analyze files".to_string(),
        )
        .with_capabilities(vec![
            Capability::FileRead,
            Capability::LlmCall,
        ]);

        assert_eq!(skill.required_capabilities.len(), 2);
    }

    #[test]
    fn test_skill_node_types() {
        let tool_node = SkillNode::tool("search", "web_search", serde_json::json!({}));
        assert!(matches!(tool_node.node_type, SkillNodeType::Tool { .. }));

        let llm_node = SkillNode::llm("summarize", "Summarize: {{ input }}");
        assert!(matches!(llm_node.node_type, SkillNodeType::LlmProcess { .. }));
    }
}
```

**Step 2: Write minimal implementation**

```rust
// core/src/three_layer/skill/definition.rs
//! Skill definition types for the Skill DAG layer

use crate::three_layer::safety::Capability;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Definition of a Skill in the middle layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of what this skill does
    pub description: String,
    /// Input JSON schema
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// Output JSON schema
    #[serde(default)]
    pub output_schema: Option<Value>,
    /// Required capabilities
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    /// Cost estimate
    #[serde(default)]
    pub cost_estimate: CostEstimate,
    /// Retry policy
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// DAG nodes
    #[serde(default)]
    pub nodes: Vec<SkillNode>,
    /// DAG edges (from_id, to_id)
    #[serde(default)]
    pub edges: Vec<(String, String)>,
}

impl SkillDefinition {
    /// Create a new skill definition
    pub fn new(id: String, name: String, description: String) -> Self {
        Self {
            id,
            name,
            description,
            input_schema: None,
            output_schema: None,
            required_capabilities: Vec::new(),
            cost_estimate: CostEstimate::default(),
            retry_policy: RetryPolicy::default(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Add required capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.required_capabilities = capabilities;
        self
    }

    /// Add nodes
    pub fn with_nodes(mut self, nodes: Vec<SkillNode>) -> Self {
        self.nodes = nodes;
        self
    }

    /// Add edges
    pub fn with_edges(mut self, edges: Vec<(String, String)>) -> Self {
        self.edges = edges;
        self
    }
}

/// Cost estimate for a skill execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Estimated max tokens
    pub max_tokens: u64,
    /// Estimated max tool calls
    pub max_tool_calls: u32,
}

impl Default for CostEstimate {
    fn default() -> Self {
        Self {
            max_tokens: 10_000,
            max_tool_calls: 10,
        }
    }
}

/// Retry policy for skill execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum retries
    pub max_retries: u32,
    /// Initial backoff duration
    #[serde(with = "humantime_serde")]
    pub initial_backoff: Duration,
    /// Maximum backoff duration
    #[serde(with = "humantime_serde")]
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// A node in the Skill DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillNode {
    /// Node ID (unique within skill)
    pub id: String,
    /// Node type
    pub node_type: SkillNodeType,
}

impl SkillNode {
    /// Create a tool invocation node
    pub fn tool(id: &str, tool_id: &str, args_template: Value) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::Tool {
                tool_id: tool_id.to_string(),
                args_template,
            },
        }
    }

    /// Create an LLM processing node
    pub fn llm(id: &str, prompt_template: &str) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::LlmProcess {
                prompt_template: prompt_template.to_string(),
            },
        }
    }

    /// Create a skill invocation node (nested skill)
    pub fn skill(id: &str, skill_id: &str) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::Skill {
                skill_id: skill_id.to_string(),
            },
        }
    }
}

/// Type of skill node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillNodeType {
    /// Invoke a tool
    Tool {
        tool_id: String,
        args_template: Value,
    },
    /// Invoke another skill
    Skill {
        skill_id: String,
    },
    /// LLM processing
    LlmProcess {
        prompt_template: String,
    },
    /// Conditional branch
    Condition {
        expression: String,
    },
    /// Parallel fan-out
    Parallel {
        branches: Vec<String>,
    },
    /// Aggregate fan-in
    Aggregate {
        strategy: AggregateStrategy,
    },
}

/// Strategy for aggregating parallel results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AggregateStrategy {
    /// Collect all results into array
    CollectAll,
    /// Take first successful result
    FirstSuccess,
    /// Merge objects
    MergeObjects,
    /// Custom aggregation via LLM
    LlmMerge { prompt: String },
}

// Custom serde for Duration (humantime format like "1s", "30s")
mod humantime_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // Simple parser for "Ns" format
        let secs: u64 = s
            .trim_end_matches('s')
            .parse()
            .map_err(serde::de::Error::custom)?;
        Ok(Duration::from_secs(secs))
    }
}
```

**Step 3: Create skill/mod.rs**

```rust
// core/src/three_layer/skill/mod.rs
//! Skill Layer - Middle layer with stable, testable DAG workflows

mod definition;

pub use definition::{
    AggregateStrategy, CostEstimate, RetryPolicy, SkillDefinition, SkillNode, SkillNodeType,
};
```

**Step 4: Update three_layer/mod.rs**

```rust
pub mod orchestrator;
pub mod safety;
pub mod skill;

// Re-exports
pub use orchestrator::{GuardChecker, GuardViolation, OrchestratorState};
pub use safety::{
    Capability, CapabilityDenied, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation,
};
pub use skill::{SkillDefinition, SkillNode, SkillNodeType};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test three_layer::skill::definition::tests -v`
Expected: All 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/three_layer/skill/
git commit -m "feat(three-layer): add SkillDefinition types for DAG layer"
```

---

### Task 4.2: Create SkillRegistry

**Files:**
- Create: `core/src/three_layer/skill/registry.rs`
- Modify: `core/src/three_layer/skill/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/skill/registry.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = SkillRegistry::new();

        let skill = SkillDefinition::new(
            "test".to_string(),
            "Test Skill".to_string(),
            "A test skill".to_string(),
        );

        registry.register(skill);

        assert!(registry.get("test").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_by_capability() {
        let mut registry = SkillRegistry::new();

        let skill1 = SkillDefinition::new("s1".to_string(), "S1".to_string(), "".to_string())
            .with_capabilities(vec![Capability::FileRead]);

        let skill2 = SkillDefinition::new("s2".to_string(), "S2".to_string(), "".to_string())
            .with_capabilities(vec![Capability::FileRead, Capability::WebSearch]);

        let skill3 = SkillDefinition::new("s3".to_string(), "S3".to_string(), "".to_string())
            .with_capabilities(vec![Capability::WebSearch]);

        registry.register(skill1);
        registry.register(skill2);
        registry.register(skill3);

        let file_skills = registry.list_by_capability(&Capability::FileRead);
        assert_eq!(file_skills.len(), 2);

        let web_skills = registry.list_by_capability(&Capability::WebSearch);
        assert_eq!(web_skills.len(), 2);
    }

    #[test]
    fn test_registry_list_all() {
        let mut registry = SkillRegistry::new();

        registry.register(SkillDefinition::new("a".to_string(), "A".to_string(), "".to_string()));
        registry.register(SkillDefinition::new("b".to_string(), "B".to_string(), "".to_string()));

        assert_eq!(registry.list_all().len(), 2);
    }
}
```

**Step 2: Write minimal implementation**

```rust
// core/src/three_layer/skill/registry.rs
//! Skill Registry - manages available skills

use super::SkillDefinition;
use crate::three_layer::safety::Capability;
use std::collections::HashMap;

/// Registry of available skills
#[derive(Debug, Default)]
pub struct SkillRegistry {
    /// Builtin skills (Rust implementations)
    builtin: HashMap<String, SkillDefinition>,
    /// Custom skills (loaded from YAML)
    custom: HashMap<String, SkillDefinition>,
}

impl SkillRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a skill
    pub fn register(&mut self, skill: SkillDefinition) {
        self.builtin.insert(skill.id.clone(), skill);
    }

    /// Register a custom skill (from YAML)
    pub fn register_custom(&mut self, skill: SkillDefinition) {
        self.custom.insert(skill.id.clone(), skill);
    }

    /// Get a skill by ID
    pub fn get(&self, id: &str) -> Option<&SkillDefinition> {
        self.builtin.get(id).or_else(|| self.custom.get(id))
    }

    /// List all skills that require a specific capability
    pub fn list_by_capability(&self, capability: &Capability) -> Vec<&SkillDefinition> {
        self.builtin
            .values()
            .chain(self.custom.values())
            .filter(|s| s.required_capabilities.contains(capability))
            .collect()
    }

    /// List all registered skills
    pub fn list_all(&self) -> Vec<&SkillDefinition> {
        self.builtin.values().chain(self.custom.values()).collect()
    }

    /// Clear custom skills (for hot reload)
    pub fn clear_custom(&mut self) {
        self.custom.clear();
    }

    /// Check if a skill exists
    pub fn contains(&self, id: &str) -> bool {
        self.builtin.contains_key(id) || self.custom.contains_key(id)
    }
}
```

**Step 3: Update skill/mod.rs**

```rust
mod definition;
mod registry;

pub use definition::{
    AggregateStrategy, CostEstimate, RetryPolicy, SkillDefinition, SkillNode, SkillNodeType,
};
pub use registry::SkillRegistry;
```

**Step 4: Update three_layer/mod.rs exports**

Add `SkillRegistry` to re-exports.

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test three_layer::skill::registry::tests -v`
Expected: All 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/three_layer/skill/registry.rs core/src/three_layer/skill/mod.rs core/src/three_layer/mod.rs
git commit -m "feat(three-layer): add SkillRegistry for skill management"
```

---

## Phase 5: Resource Quota (P1)

### Task 5.1: Implement ResourceQuota

**Files:**
- Create: `core/src/three_layer/safety/quota.rs`
- Modify: `core/src/three_layer/safety/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/three_layer/safety/quota.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_check_read() {
        let quota = ResourceQuota::default();
        let tracker = QuotaTracker::new(quota);

        assert!(tracker.check_read(1024).is_ok());
        assert!(tracker.check_read(200 * 1024 * 1024).is_err()); // 200MB > 100MB limit
    }

    #[test]
    fn test_quota_tracking() {
        let quota = ResourceQuota {
            max_total_read: 1000,
            ..Default::default()
        };
        let tracker = QuotaTracker::new(quota);

        tracker.record_read(500);
        assert!(tracker.check_read(400).is_ok());
        assert!(tracker.check_read(600).is_err()); // 500 + 600 > 1000
    }

    #[test]
    fn test_quota_file_count() {
        let quota = ResourceQuota {
            max_file_count: 5,
            ..Default::default()
        };
        let tracker = QuotaTracker::new(quota);

        for _ in 0..5 {
            tracker.record_file_access();
        }

        assert!(tracker.check_file_count().is_err());
    }
}
```

**Step 2: Write minimal implementation**

```rust
// core/src/three_layer/safety/quota.rs
//! Resource quota tracking and enforcement

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Resource quota limits
#[derive(Debug, Clone)]
pub struct ResourceQuota {
    /// Maximum single file size (bytes)
    pub max_file_size: u64,
    /// Maximum total read (bytes)
    pub max_total_read: u64,
    /// Maximum total write (bytes)
    pub max_total_write: u64,
    /// Maximum file count
    pub max_file_count: u32,
    /// Operation timeout
    pub operation_timeout: Duration,
}

impl Default for ResourceQuota {
    fn default() -> Self {
        Self {
            max_file_size: 10 * 1024 * 1024,       // 10 MB
            max_total_read: 100 * 1024 * 1024,     // 100 MB
            max_total_write: 50 * 1024 * 1024,     // 50 MB
            max_file_count: 1000,
            operation_timeout: Duration::from_secs(30),
        }
    }
}

/// Error when quota is exceeded
#[derive(Debug, Clone)]
pub enum QuotaExceeded {
    FileTooLarge { size: u64, max: u64 },
    TotalReadExceeded { used: u64, requested: u64, max: u64 },
    TotalWriteExceeded { used: u64, requested: u64, max: u64 },
    FileCountExceeded { count: u32, max: u32 },
}

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaExceeded::FileTooLarge { size, max } => {
                write!(f, "File too large: {} bytes (max: {})", size, max)
            }
            QuotaExceeded::TotalReadExceeded { used, requested, max } => {
                write!(f, "Total read exceeded: {} + {} > {}", used, requested, max)
            }
            QuotaExceeded::TotalWriteExceeded { used, requested, max } => {
                write!(f, "Total write exceeded: {} + {} > {}", used, requested, max)
            }
            QuotaExceeded::FileCountExceeded { count, max } => {
                write!(f, "File count exceeded: {} >= {}", count, max)
            }
        }
    }
}

impl std::error::Error for QuotaExceeded {}

/// Tracks resource usage against quotas
#[derive(Debug)]
pub struct QuotaTracker {
    quota: ResourceQuota,
    used_read: AtomicU64,
    used_write: AtomicU64,
    file_count: AtomicU32,
}

impl QuotaTracker {
    /// Create a new tracker with the given quota
    pub fn new(quota: ResourceQuota) -> Self {
        Self {
            quota,
            used_read: AtomicU64::new(0),
            used_write: AtomicU64::new(0),
            file_count: AtomicU32::new(0),
        }
    }

    /// Check if a read operation is allowed
    pub fn check_read(&self, size: u64) -> Result<(), QuotaExceeded> {
        // Check single file size
        if size > self.quota.max_file_size {
            return Err(QuotaExceeded::FileTooLarge {
                size,
                max: self.quota.max_file_size,
            });
        }

        // Check total read
        let used = self.used_read.load(Ordering::Relaxed);
        if used + size > self.quota.max_total_read {
            return Err(QuotaExceeded::TotalReadExceeded {
                used,
                requested: size,
                max: self.quota.max_total_read,
            });
        }

        Ok(())
    }

    /// Check if a write operation is allowed
    pub fn check_write(&self, size: u64) -> Result<(), QuotaExceeded> {
        // Check single file size
        if size > self.quota.max_file_size {
            return Err(QuotaExceeded::FileTooLarge {
                size,
                max: self.quota.max_file_size,
            });
        }

        // Check total write
        let used = self.used_write.load(Ordering::Relaxed);
        if used + size > self.quota.max_total_write {
            return Err(QuotaExceeded::TotalWriteExceeded {
                used,
                requested: size,
                max: self.quota.max_total_write,
            });
        }

        Ok(())
    }

    /// Check if file count is within limit
    pub fn check_file_count(&self) -> Result<(), QuotaExceeded> {
        let count = self.file_count.load(Ordering::Relaxed);
        if count >= self.quota.max_file_count {
            return Err(QuotaExceeded::FileCountExceeded {
                count,
                max: self.quota.max_file_count,
            });
        }
        Ok(())
    }

    /// Record a read operation
    pub fn record_read(&self, size: u64) {
        self.used_read.fetch_add(size, Ordering::Relaxed);
    }

    /// Record a write operation
    pub fn record_write(&self, size: u64) {
        self.used_write.fetch_add(size, Ordering::Relaxed);
    }

    /// Record a file access
    pub fn record_file_access(&self) {
        self.file_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current usage statistics
    pub fn usage(&self) -> QuotaUsage {
        QuotaUsage {
            read_bytes: self.used_read.load(Ordering::Relaxed),
            write_bytes: self.used_write.load(Ordering::Relaxed),
            file_count: self.file_count.load(Ordering::Relaxed),
        }
    }

    /// Reset usage counters
    pub fn reset(&self) {
        self.used_read.store(0, Ordering::Relaxed);
        self.used_write.store(0, Ordering::Relaxed);
        self.file_count.store(0, Ordering::Relaxed);
    }
}

/// Current quota usage
#[derive(Debug, Clone)]
pub struct QuotaUsage {
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub file_count: u32,
}
```

**Step 3: Update safety/mod.rs**

Add quota module and exports.

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test three_layer::safety::quota::tests -v`
Expected: All 3 tests PASS

**Step 5: Commit**

```bash
git add core/src/three_layer/safety/quota.rs core/src/three_layer/safety/mod.rs
git commit -m "feat(three-layer): add ResourceQuota for P1 resource limits"
```

---

## Phase 6: Integration Tests

### Task 6.1: Create integration test for safety layer

**Files:**
- Create: `core/src/three_layer/tests.rs`
- Modify: `core/src/three_layer/mod.rs`

**Step 1: Write integration test**

```rust
// core/src/three_layer/tests.rs
//! Integration tests for Three-Layer Control

#[cfg(test)]
mod integration_tests {
    use crate::three_layer::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_full_safety_chain() {
        // Setup sandbox
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::with_defaults(vec![temp.path().to_path_buf()]);

        // Setup capability gate
        let gate = CapabilityGate::new(vec![
            Capability::FileRead,
            Capability::FileList,
        ]);

        // Create a test file
        let test_file = temp.path().join("test.txt");
        std::fs::write(&test_file, "test content").unwrap();

        // Test: Should allow reading the file
        assert!(sandbox.validate(&test_file).is_ok());
        assert!(gate.check(&Capability::FileRead).is_ok());

        // Test: Should deny writing
        assert!(gate.check(&Capability::FileWrite).is_err());

        // Test: Should deny accessing .env
        let env_file = temp.path().join(".env");
        std::fs::write(&env_file, "SECRET=xxx").unwrap();
        assert!(sandbox.validate(&env_file).is_err());
    }

    #[test]
    fn test_orchestrator_guards_integration() {
        use crate::config::types::OrchestratorGuards;
        use std::time::Instant;

        let guards = OrchestratorGuards {
            max_rounds: 5,
            max_tool_calls: 10,
            max_tokens: 1000,
            timeout_seconds: 1,
            no_progress_threshold: 2,
        };

        let checker = GuardChecker::new(guards);
        let start = Instant::now();

        // Should pass initially
        assert!(checker.check_all(0, 0, 0, start, 0).is_ok());

        // Should fail on rounds
        assert!(checker.check_rounds(5).is_err());

        // Should fail on tool calls
        assert!(checker.check_tool_calls(10).is_err());

        // Should fail on tokens
        assert!(checker.check_tokens(1000).is_err());
    }

    #[test]
    fn test_skill_registry_with_capabilities() {
        use crate::three_layer::skill::*;

        let mut registry = SkillRegistry::new();

        // Register skills with different capabilities
        registry.register(
            SkillDefinition::new("reader".to_string(), "Reader".to_string(), "".to_string())
                .with_capabilities(vec![Capability::FileRead])
        );

        registry.register(
            SkillDefinition::new("writer".to_string(), "Writer".to_string(), "".to_string())
                .with_capabilities(vec![Capability::FileWrite])
        );

        // Verify capability filtering
        let read_skills = registry.list_by_capability(&Capability::FileRead);
        assert_eq!(read_skills.len(), 1);
        assert_eq!(read_skills[0].id, "reader");
    }
}
```

**Step 2: Add tests module to mod.rs**

```rust
#[cfg(test)]
mod tests;
```

**Step 3: Run all three_layer tests**

Run: `cd core && cargo test three_layer -v`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/three_layer/tests.rs core/src/three_layer/mod.rs
git commit -m "test(three-layer): add integration tests"
```

---

## Phase 7: Documentation

### Task 7.1: Create THREE_LAYER_CONTROL.md

**Files:**
- Create: `docs/THREE_LAYER_CONTROL.md`

**Step 1: Write documentation**

```markdown
# Three-Layer Control Architecture

## Overview

The Three-Layer Control architecture provides a balanced approach to AI agent execution:

- **Top Layer (Orchestrator)**: FSM-based state machine with hard constraints
- **Middle Layer (Skill DAG)**: Stable, testable workflow pipelines
- **Bottom Layer (Tools)**: Capability-based access with sandbox

## Enabling

Add to your `config.toml`:

\`\`\`toml
[orchestrator]
use_three_layer_control = true

[orchestrator.guards]
max_rounds = 12
max_tool_calls = 30
max_tokens = 100000
timeout_seconds = 600
no_progress_threshold = 2
\`\`\`

## Architecture

See `docs/plans/2026-01-21-three-layer-control-design.md` for detailed design.

## Safety Features

### Capability System

Skills must declare required capabilities. The `CapabilityGate` enforces these restrictions.

### Path Sandbox

File operations are restricted to allowed directories. Sensitive files (`.git`, `.env`) are blocked by default.

### Resource Quota

Limits on total read/write bytes and file count prevent runaway operations.

## Orchestrator States

1. **Clarify**: Gather requirements
2. **Plan**: Select skills to execute
3. **Execute**: Run skill DAG
4. **Evaluate**: Check success criteria
5. **Reflect**: Analyze failures, adjust
6. **Stop**: Return result
```

**Step 2: Commit**

```bash
git add docs/THREE_LAYER_CONTROL.md
git commit -m "docs: add Three-Layer Control documentation"
```

---

## Summary

### Files Created/Modified

**New Files:**
- `core/src/three_layer/mod.rs`
- `core/src/three_layer/safety/mod.rs`
- `core/src/three_layer/safety/capability.rs`
- `core/src/three_layer/safety/gate.rs`
- `core/src/three_layer/safety/sandbox.rs`
- `core/src/three_layer/safety/quota.rs`
- `core/src/three_layer/orchestrator/mod.rs`
- `core/src/three_layer/orchestrator/guards.rs`
- `core/src/three_layer/orchestrator/states.rs`
- `core/src/three_layer/skill/mod.rs`
- `core/src/three_layer/skill/definition.rs`
- `core/src/three_layer/skill/registry.rs`
- `core/src/three_layer/tests.rs`
- `core/src/config/types/orchestrator.rs`
- `docs/THREE_LAYER_CONTROL.md`

**Modified Files:**
- `core/src/lib.rs` (add three_layer module)
- `core/src/config/types/mod.rs` (add orchestrator config)
- `core/src/config/mod.rs` (add orchestrator field)
- `core/src/orchestrator/mod.rs` (add deprecation)

### Test Commands

```bash
# Run all three-layer tests
cd core && cargo test three_layer -v

# Run specific module tests
cd core && cargo test three_layer::safety -v
cd core && cargo test three_layer::orchestrator -v
cd core && cargo test three_layer::skill -v

# Run integration tests
cd core && cargo test three_layer::tests -v
```

### Verification

```bash
# Full build
cd core && cargo build

# Full test suite
cd core && cargo test
```
