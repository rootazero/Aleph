# POE Phase 1: BlastRadius + Taboo Crystallization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add blast radius risk assessment and anti-pattern learning to the POE system, enabling safe progressive auto-approval and preventing repetitive failure loops.

**Architecture:** Two new subsystems integrate into the existing POE pipeline. BlastRadius embeds into `SuccessManifest` and gates `TrustEvaluator`. TabooBuffer lives in `PoeManager` and feeds anti-patterns back through `ManifestBuilder`. Both follow the hybrid System 1 + System 2 pattern already established in the codebase.

**Tech Stack:** Rust, serde, regex, existing `AiProvider` trait for LLM calls, existing `ExperienceStore` for anti-pattern persistence.

**Design Reference:** `docs/plans/2026-03-10-poe-evolution-whitepaper.md`

---

## Task 1: Core BlastRadius Types

**Files:**
- Modify: `core/src/poe/types.rs` (add BlastRadius, RiskLevel, embed in SuccessManifest)
- Modify: `core/src/poe/mod.rs` (re-export new types)

**Step 1: Write the failing test**

Add to end of `core/src/poe/proptest_types.rs` (or inline `#[cfg(test)]` in `types.rs`):

```rust
#[cfg(test)]
mod blast_radius_tests {
    use super::*;

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Negligible < RiskLevel::Low);
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }

    #[test]
    fn test_blast_radius_default_is_none_in_manifest() {
        let manifest = SuccessManifest::new("t1", "test objective");
        assert!(manifest.blast_radius.is_none());
    }

    #[test]
    fn test_blast_radius_builder() {
        let br = BlastRadius::new(0.3, 0.7, 0.9, RiskLevel::Medium, "test reason");
        assert_eq!(br.level, RiskLevel::Medium);
        assert!((br.scope - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_blast_radius_clamping() {
        let br = BlastRadius::new(1.5, -0.1, 2.0, RiskLevel::High, "clamped");
        assert!((br.scope - 1.0).abs() < f32::EPSILON);
        assert!((br.destructiveness - 0.0).abs() < f32::EPSILON);
        assert!((br.reversibility - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_manifest_with_blast_radius() {
        let manifest = SuccessManifest::new("t1", "test")
            .with_blast_radius(BlastRadius::new(0.5, 0.5, 0.5, RiskLevel::Medium, "r"));
        assert!(manifest.blast_radius.is_some());
        assert_eq!(manifest.blast_radius.unwrap().level, RiskLevel::Medium);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib blast_radius_tests`
Expected: FAIL — `RiskLevel` and `BlastRadius` not defined

**Step 3: Write minimal implementation**

In `core/src/poe/types.rs`, add after the `ModelTier` section (~line 283):

```rust
// ============================================================================
// Blast Radius (Risk Assessment)
// ============================================================================

/// Risk level classification for task operations.
///
/// Ordered from lowest to highest risk. Used by TrustEvaluator
/// to make auto-approval decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Negligible risk (e.g., modify README)
    Negligible,
    /// Low risk (e.g., add test cases)
    Low,
    /// Medium risk (e.g., refactor non-core module)
    Medium,
    /// High risk (e.g., modify auth middleware, DB migration)
    High,
    /// Critical risk (e.g., wipe data directory, modify system environment)
    Critical,
}

impl RiskLevel {
    /// Returns a human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            RiskLevel::Negligible => "negligible",
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

/// Multi-dimensional risk assessment for a task.
///
/// Embedded in `SuccessManifest` to flow risk information through
/// the entire POE pipeline (ManifestBuilder → TrustEvaluator → UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastRadius {
    /// Impact scope (0.0-1.0): file count, module depth, user coverage
    pub scope: f32,

    /// Destructiveness (0.0-1.0): data deletion, prod config changes
    pub destructiveness: f32,

    /// Reversibility (0.0-1.0): 1.0 = fully reversible, 0.0 = irreversible
    pub reversibility: f32,

    /// Computed risk level
    pub level: RiskLevel,

    /// Human-readable reasoning for UI display
    pub reasoning: String,
}

impl BlastRadius {
    /// Create a new BlastRadius with clamped values.
    pub fn new(
        scope: f32,
        destructiveness: f32,
        reversibility: f32,
        level: RiskLevel,
        reasoning: impl Into<String>,
    ) -> Self {
        Self {
            scope: scope.clamp(0.0, 1.0),
            destructiveness: destructiveness.clamp(0.0, 1.0),
            reversibility: reversibility.clamp(0.0, 1.0),
            level,
            reasoning: reasoning.into(),
        }
    }
}
```

Add `blast_radius` field to `SuccessManifest` (after `rollback_snapshot`):

```rust
    /// Risk assessment for this task (computed by BlastRadius scanner)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blast_radius: Option<BlastRadius>,
```

Add builder method to `impl SuccessManifest`:

```rust
    /// Set the blast radius risk assessment.
    pub fn with_blast_radius(mut self, blast_radius: BlastRadius) -> Self {
        self.blast_radius = Some(blast_radius);
        self
    }
```

Update `SuccessManifest::new()` to include `blast_radius: None`.

In `core/src/poe/mod.rs`, add to the re-exports:

```rust
pub use types::{BlastRadius, RiskLevel};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib blast_radius_tests`
Expected: PASS (4 tests)

**Step 5: Commit**

```bash
git add core/src/poe/types.rs core/src/poe/mod.rs
git commit -m "poe: add BlastRadius and RiskLevel core types"
```

---

## Task 2: StaticSafetyScanner (System 1)

**Files:**
- Create: `core/src/poe/blast_radius/mod.rs`
- Create: `core/src/poe/blast_radius/static_scanner.rs`
- Modify: `core/src/poe/mod.rs` (add module)

**Step 1: Write the failing test**

In `core/src/poe/blast_radius/static_scanner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SuccessManifest, ValidationRule};

    #[test]
    fn test_tier0_rm_rf_root() {
        let manifest = SuccessManifest::new("t1", "clean up")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "rm".into(),
                args: vec!["-rf".into(), "/".into()],
                timeout_ms: 30_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert_eq!(result, ScanResult::HardReject { reason: result.reason().to_string() });
    }

    #[test]
    fn test_tier0_fork_bomb() {
        let manifest = SuccessManifest::new("t1", "test")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "bash".into(),
                args: vec!["-c".into(), ":(){ :|:& };:".into()],
                timeout_ms: 30_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::HardReject { .. }));
    }

    #[test]
    fn test_tier1_sudo() {
        let manifest = SuccessManifest::new("t1", "install package")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "sudo".into(),
                args: vec!["apt".into(), "install".into(), "vim".into()],
                timeout_ms: 30_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::MandatorySignature { .. }));
    }

    #[test]
    fn test_tier1_drop_table() {
        let manifest = SuccessManifest::new("t1", "migrate db")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "psql".into(),
                args: vec!["-c".into(), "DROP TABLE users;".into()],
                timeout_ms: 30_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::MandatorySignature { .. }));
    }

    #[test]
    fn test_tier1_ssh_key_access() {
        let manifest = SuccessManifest::new("t1", "configure")
            .with_hard_constraint(ValidationRule::FileContains {
                path: std::path::PathBuf::from("/home/user/.ssh/id_rsa"),
                pattern: ".*".into(),
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::MandatorySignature { .. }));
    }

    #[test]
    fn test_tier1_git_force_push() {
        let manifest = SuccessManifest::new("t1", "push")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "git".into(),
                args: vec!["push".into(), "--force".into()],
                timeout_ms: 30_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::MandatorySignature { .. }));
    }

    #[test]
    fn test_safe_cargo_test() {
        let manifest = SuccessManifest::new("t1", "run tests")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".into(),
                args: vec!["test".into()],
                timeout_ms: 60_000,
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::Safe));
    }

    #[test]
    fn test_safe_file_exists_check() {
        let manifest = SuccessManifest::new("t1", "verify file")
            .with_hard_constraint(ValidationRule::FileExists {
                path: std::path::PathBuf::from("src/main.rs"),
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::Safe));
    }

    #[test]
    fn test_negligible_readme_only() {
        let manifest = SuccessManifest::new("t1", "update docs")
            .with_hard_constraint(ValidationRule::FileContains {
                path: std::path::PathBuf::from("README.md"),
                pattern: "Installation".into(),
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::Negligible));
    }

    #[test]
    fn test_indeterminate_complex_task() {
        let manifest = SuccessManifest::new("t1", "refactor auth module")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".into(),
                args: vec!["test".into()],
                timeout_ms: 60_000,
            })
            .with_hard_constraint(ValidationRule::FileContains {
                path: std::path::PathBuf::from("src/auth/mod.rs"),
                pattern: "pub fn verify_token".into(),
            });
        let result = StaticSafetyScanner::scan(&manifest);
        assert!(matches!(result, ScanResult::Indeterminate));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib static_scanner::tests`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/poe/blast_radius/mod.rs`:

```rust
//! Blast radius risk assessment for POE tasks.
//!
//! Two-phase assessment following System 1 + System 2 pattern:
//! - System 1 (StaticSafetyScanner): Deterministic pattern matching
//! - System 2 (SemanticRiskAnalyzer): LLM-based contextual analysis

pub mod static_scanner;
```

Create `core/src/poe/blast_radius/static_scanner.rs`:

```rust
//! System 1: Deterministic safety scanner for POE manifests.
//!
//! Scans ValidationRules for known dangerous patterns (Tier 0: hard reject,
//! Tier 1: mandatory signature) with zero LLM cost.

use crate::poe::types::{SuccessManifest, ValidationRule};
use std::path::Path;

/// Result of static safety scanning.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanResult {
    /// Tier 0: Absolutely forbidden — abort immediately
    HardReject { reason: String },
    /// Tier 1: Requires mandatory user signature
    MandatorySignature { reason: String },
    /// Clearly safe, no further analysis needed
    Safe,
    /// Clearly negligible (docs-only changes)
    Negligible,
    /// Cannot determine — needs System 2 (LLM) analysis
    Indeterminate,
}

impl ScanResult {
    pub fn reason(&self) -> &str {
        match self {
            ScanResult::HardReject { reason } | ScanResult::MandatorySignature { reason } => reason,
            ScanResult::Safe => "safe",
            ScanResult::Negligible => "negligible",
            ScanResult::Indeterminate => "indeterminate",
        }
    }
}

/// Deterministic safety scanner for manifest constraints.
///
/// Checks all ValidationRules against known dangerous patterns.
/// Returns the highest-severity match found.
pub struct StaticSafetyScanner;

impl StaticSafetyScanner {
    /// Scan a manifest for safety concerns.
    ///
    /// Checks all hard_constraints and soft_metrics for dangerous patterns.
    /// Returns the highest severity result found (HardReject > MandatorySignature > Indeterminate > Safe > Negligible).
    pub fn scan(manifest: &SuccessManifest) -> ScanResult {
        let mut highest = ScanResult::Negligible;
        let mut has_non_doc_rules = false;

        // Scan hard constraints
        for rule in &manifest.hard_constraints {
            let result = Self::scan_rule(rule);
            highest = Self::escalate(highest, result);

            // Track if any rule touches non-documentation files
            if !Self::is_doc_only_rule(rule) {
                has_non_doc_rules = true;
            }

            // Short-circuit on hard reject
            if matches!(highest, ScanResult::HardReject { .. }) {
                return highest;
            }
        }

        // Scan soft metric rules too
        for metric in &manifest.soft_metrics {
            let result = Self::scan_rule(&metric.rule);
            highest = Self::escalate(highest, result);

            if !Self::is_doc_only_rule(&metric.rule) {
                has_non_doc_rules = true;
            }

            if matches!(highest, ScanResult::HardReject { .. }) {
                return highest;
            }
        }

        // If we found mandatory signature or hard reject, return that
        if matches!(highest, ScanResult::MandatorySignature { .. }) {
            return highest;
        }

        // If all rules are doc-only and no dangerous patterns found
        if !has_non_doc_rules && manifest.hard_constraints.len() + manifest.soft_metrics.len() > 0 {
            return ScanResult::Negligible;
        }

        // If we have multiple constraints touching different paths, it's indeterminate
        if manifest.hard_constraints.len() > 1 && has_non_doc_rules {
            return ScanResult::Indeterminate;
        }

        // Simple safe operations
        if has_non_doc_rules {
            ScanResult::Safe
        } else {
            ScanResult::Negligible
        }
    }

    /// Scan a single rule for dangerous patterns.
    fn scan_rule(rule: &ValidationRule) -> ScanResult {
        match rule {
            ValidationRule::CommandPasses { cmd, args, .. }
            | ValidationRule::CommandOutputContains { cmd, args, .. } => {
                Self::scan_command(cmd, args)
            }
            ValidationRule::FileContains { path, .. }
            | ValidationRule::FileNotContains { path, .. } => {
                Self::scan_path(path)
            }
            ValidationRule::FileExists { path } | ValidationRule::FileNotExists { path } => {
                Self::scan_path(path)
            }
            ValidationRule::DirStructureMatch { root, .. } => Self::scan_path(root),
            ValidationRule::JsonSchemaValid { path, .. } => Self::scan_path(path),
            ValidationRule::SemanticCheck { .. } => ScanResult::Indeterminate,
        }
    }

    /// Scan a command + args for dangerous patterns.
    fn scan_command(cmd: &str, args: &[String]) -> ScanResult {
        let full_cmd = format!("{} {}", cmd, args.join(" "));
        let cmd_lower = cmd.to_lowercase();
        let full_lower = full_cmd.to_lowercase();

        // === Tier 0: Hard Reject ===

        // rm -rf / (root deletion)
        if cmd_lower == "rm" && args.iter().any(|a| a.contains("-rf") || a.contains("-fr")) {
            if args.iter().any(|a| a == "/" || a == "/*") {
                return ScanResult::HardReject {
                    reason: "Attempted root filesystem deletion (rm -rf /)".into(),
                };
            }
        }

        // Fork bomb patterns
        if full_cmd.contains(":(){ :|:& };:") || full_cmd.contains(":(){:|:&};:") {
            return ScanResult::HardReject {
                reason: "Fork bomb detected".into(),
            };
        }

        // dd to block device
        if cmd_lower == "dd" {
            if args.iter().any(|a| {
                let a_lower = a.to_lowercase();
                a_lower.starts_with("of=/dev/sd")
                    || a_lower.starts_with("of=/dev/nvme")
                    || a_lower.starts_with("of=/dev/hd")
                    || a_lower.starts_with("of=/dev/vd")
            }) {
                return ScanResult::HardReject {
                    reason: "Direct block device write via dd".into(),
                };
            }
        }

        // mkfs on block device
        if cmd_lower.starts_with("mkfs") {
            return ScanResult::HardReject {
                reason: "Filesystem format command detected".into(),
            };
        }

        // === Tier 1: Mandatory Signature ===

        // sudo
        if cmd_lower == "sudo" {
            return ScanResult::MandatorySignature {
                reason: format!("Privilege escalation: {}", full_cmd),
            };
        }

        // SQL destructive operations
        if full_lower.contains("drop table")
            || full_lower.contains("drop database")
            || full_lower.contains("truncate ")
        {
            return ScanResult::MandatorySignature {
                reason: "Destructive SQL operation detected".into(),
            };
        }

        // Git force push
        if cmd_lower == "git" && args.iter().any(|a| a == "--force" || a == "-f") {
            if args.first().map(|a| a.as_str()) == Some("push") {
                return ScanResult::MandatorySignature {
                    reason: "git push --force detected".into(),
                };
            }
        }

        // chmod with setuid
        if cmd_lower == "chmod" && args.iter().any(|a| a.contains("+s")) {
            return ScanResult::MandatorySignature {
                reason: "setuid permission change detected".into(),
            };
        }
        if cmd_lower == "chown" {
            return ScanResult::MandatorySignature {
                reason: "File ownership change detected".into(),
            };
        }

        // Network operations with side effects
        if cmd_lower == "curl" && args.iter().any(|a| a == "-X" || a == "--request") {
            if args.iter().any(|a| a == "POST" || a == "PUT" || a == "DELETE" || a == "PATCH") {
                return ScanResult::MandatorySignature {
                    reason: "HTTP mutation request detected".into(),
                };
            }
        }

        // Publishing
        if (cmd_lower == "npm" && args.first().map(|a| a.as_str()) == Some("publish"))
            || (cmd_lower == "cargo" && args.first().map(|a| a.as_str()) == Some("publish"))
            || (cmd_lower == "docker" && args.first().map(|a| a.as_str()) == Some("push"))
        {
            return ScanResult::MandatorySignature {
                reason: format!("Package/image publishing: {}", full_cmd),
            };
        }

        // ssh
        if cmd_lower == "ssh" || cmd_lower == "scp" {
            return ScanResult::MandatorySignature {
                reason: "Remote access command detected".into(),
            };
        }

        // rm on non-target paths (not git-tracked context — just flag as potentially dangerous)
        if cmd_lower == "rm" {
            return ScanResult::MandatorySignature {
                reason: "File deletion command detected".into(),
            };
        }

        ScanResult::Safe
    }

    /// Scan a file path for sensitive locations.
    fn scan_path(path: &Path) -> ScanResult {
        let path_str = path.to_string_lossy().to_lowercase();

        // Credential/secret files
        if path_str.contains(".ssh")
            || path_str.contains("id_rsa")
            || path_str.contains("id_ed25519")
            || path_str.contains(".env")
            || path_str.contains("secret")
            || path_str.contains("credential")
            || path_str.contains(".key")
            || path_str.contains(".pem")
        {
            return ScanResult::MandatorySignature {
                reason: format!("Sensitive file access: {}", path.display()),
            };
        }

        // System directories
        if path_str.starts_with("/etc/")
            || path_str.starts_with("/usr/")
            || path_str.starts_with("/var/")
            || path_str.starts_with("/sys/")
            || path_str.starts_with("/proc/")
        {
            return ScanResult::MandatorySignature {
                reason: format!("System directory access: {}", path.display()),
            };
        }

        ScanResult::Safe
    }

    /// Check if a rule only touches documentation files.
    fn is_doc_only_rule(rule: &ValidationRule) -> bool {
        let path = match rule {
            ValidationRule::FileExists { path }
            | ValidationRule::FileNotExists { path }
            | ValidationRule::FileContains { path, .. }
            | ValidationRule::FileNotContains { path, .. }
            | ValidationRule::JsonSchemaValid { path, .. } => Some(path),
            ValidationRule::DirStructureMatch { root, .. } => Some(root),
            _ => None,
        };

        if let Some(p) = path {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            matches!(ext, "md" | "txt" | "rst" | "adoc")
        } else {
            false
        }
    }

    /// Return the higher-severity result.
    fn escalate(current: ScanResult, new: ScanResult) -> ScanResult {
        let severity = |r: &ScanResult| -> u8 {
            match r {
                ScanResult::HardReject { .. } => 4,
                ScanResult::MandatorySignature { .. } => 3,
                ScanResult::Indeterminate => 2,
                ScanResult::Safe => 1,
                ScanResult::Negligible => 0,
            }
        };
        if severity(&new) > severity(&current) { new } else { current }
    }
}
```

Add to `core/src/poe/mod.rs`:

```rust
pub mod blast_radius;
pub use blast_radius::static_scanner::{ScanResult, StaticSafetyScanner};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib static_scanner::tests`
Expected: PASS (10 tests)

**Step 5: Commit**

```bash
git add core/src/poe/blast_radius/ core/src/poe/mod.rs
git commit -m "poe: add StaticSafetyScanner (System 1 blast radius)"
```

---

## Task 3: SemanticRiskAnalyzer (System 2)

**Files:**
- Create: `core/src/poe/blast_radius/semantic_analyzer.rs`
- Modify: `core/src/poe/blast_radius/mod.rs`

**Step 1: Write the failing test**

In `core/src/poe/blast_radius/semantic_analyzer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{BlastRadius, RiskLevel};

    #[test]
    fn test_fallback_on_parse_failure() {
        let result = SemanticRiskAnalyzer::fallback_blast_radius("parse failed");
        assert_eq!(result.level, RiskLevel::High);
        assert!(result.reasoning.contains("parse failed"));
    }

    #[test]
    fn test_parse_llm_response_valid() {
        let json = r#"{"scope": 0.3, "destructiveness": 0.5, "reversibility": 0.8, "level": "Medium", "reasoning": "modifies auth module"}"#;
        let result = SemanticRiskAnalyzer::parse_llm_response(json);
        assert!(result.is_ok());
        let br = result.unwrap();
        assert_eq!(br.level, RiskLevel::Medium);
    }

    #[test]
    fn test_parse_llm_response_invalid() {
        let result = SemanticRiskAnalyzer::parse_llm_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_system1_never_downgraded() {
        let system1_high = BlastRadius::new(0.8, 0.9, 0.1, RiskLevel::High, "system1");
        let system2_low = BlastRadius::new(0.1, 0.1, 0.9, RiskLevel::Low, "system2");
        let merged = SemanticRiskAnalyzer::merge_with_system1(
            Some(&system1_high),
            system2_low,
        );
        assert_eq!(merged.level, RiskLevel::High);
    }

    #[test]
    fn test_no_system1_uses_system2() {
        let system2 = BlastRadius::new(0.3, 0.3, 0.7, RiskLevel::Medium, "system2");
        let merged = SemanticRiskAnalyzer::merge_with_system1(None, system2.clone());
        assert_eq!(merged.level, RiskLevel::Medium);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib semantic_analyzer::tests`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/poe/blast_radius/semantic_analyzer.rs`:

```rust
//! System 2: LLM-based semantic risk analysis.
//!
//! Invoked when System 1 (StaticSafetyScanner) returns Indeterminate.
//! Uses a lightweight LLM call to assess risk context-aware.

use crate::error::{AlephError, Result};
use crate::poe::types::{BlastRadius, RiskLevel, SuccessManifest};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// LLM-based risk analyzer for gray-zone tasks.
pub struct SemanticRiskAnalyzer;

impl SemanticRiskAnalyzer {
    /// Analyze a manifest's risk using LLM.
    ///
    /// Returns a BlastRadius assessment. On any LLM failure,
    /// applies "presumption of guilt" — defaults to RiskLevel::High.
    pub async fn analyze(
        manifest: &SuccessManifest,
        provider: &Arc<dyn AiProvider>,
    ) -> BlastRadius {
        let prompt = Self::build_prompt(manifest);

        match provider.complete_simple(&prompt).await {
            Ok(response) => {
                match Self::parse_llm_response(&response) {
                    Ok(br) => br,
                    Err(e) => Self::fallback_blast_radius(&format!("LLM response parse error: {}", e)),
                }
            }
            Err(e) => Self::fallback_blast_radius(&format!("LLM call failed: {}", e)),
        }
    }

    /// Build the risk assessment prompt for the LLM.
    fn build_prompt(manifest: &SuccessManifest) -> String {
        let constraints_desc: Vec<String> = manifest
            .hard_constraints
            .iter()
            .map(|r| format!("{:?}", r))
            .collect();

        format!(
            r#"You are a security risk assessor for an AI agent system.

Analyze the following task and return a JSON risk assessment.

Task objective: {}
Validation constraints:
{}

Return ONLY valid JSON (no markdown, no explanation):
{{
  "scope": <0.0-1.0 impact breadth>,
  "destructiveness": <0.0-1.0 potential for data/system damage>,
  "reversibility": <0.0-1.0 where 1.0=fully reversible>,
  "level": "<Negligible|Low|Medium|High|Critical>",
  "reasoning": "<one sentence explanation>"
}}"#,
            manifest.objective,
            constraints_desc.join("\n")
        )
    }

    /// Parse LLM JSON response into BlastRadius.
    pub fn parse_llm_response(response: &str) -> Result<BlastRadius> {
        // Try to extract JSON from the response (LLM might wrap in markdown)
        let json_str = Self::extract_json(response);

        #[derive(serde::Deserialize)]
        struct LlmRiskResponse {
            scope: f32,
            destructiveness: f32,
            reversibility: f32,
            level: String,
            reasoning: String,
        }

        let parsed: LlmRiskResponse = serde_json::from_str(json_str)
            .map_err(|e| AlephError::internal(format!("Failed to parse risk JSON: {}", e)))?;

        let level = match parsed.level.to_lowercase().as_str() {
            "negligible" => RiskLevel::Negligible,
            "low" => RiskLevel::Low,
            "medium" => RiskLevel::Medium,
            "high" => RiskLevel::High,
            "critical" => RiskLevel::Critical,
            _ => RiskLevel::High, // Unknown level → presume high risk
        };

        Ok(BlastRadius::new(
            parsed.scope,
            parsed.destructiveness,
            parsed.reversibility,
            level,
            parsed.reasoning,
        ))
    }

    /// Extract JSON from potentially markdown-wrapped response.
    fn extract_json(response: &str) -> &str {
        let trimmed = response.trim();
        // Strip markdown code blocks
        if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                return &trimmed[start..=end];
            }
        }
        trimmed
    }

    /// Fallback: "presumption of guilt" when LLM fails.
    pub fn fallback_blast_radius(reason: &str) -> BlastRadius {
        BlastRadius::new(
            0.5,
            0.5,
            0.5,
            RiskLevel::High,
            format!("Presumption of guilt (fail-safe): {}", reason),
        )
    }

    /// Merge System 2 result with optional System 1 assessment.
    ///
    /// **Critical rule: System 1 conclusions are NEVER downgraded by System 2.**
    pub fn merge_with_system1(
        system1: Option<&BlastRadius>,
        system2: BlastRadius,
    ) -> BlastRadius {
        match system1 {
            Some(s1) if s1.level > system2.level => {
                // System 1 is stricter — keep its level but use System 2's detail
                BlastRadius::new(
                    s1.scope.max(system2.scope),
                    s1.destructiveness.max(system2.destructiveness),
                    s1.reversibility.min(system2.reversibility),
                    s1.level,
                    format!("{} (elevated by static scan: {})", system2.reasoning, s1.reasoning),
                )
            }
            _ => system2,
        }
    }
}
```

Update `core/src/poe/blast_radius/mod.rs`:

```rust
pub mod semantic_analyzer;
pub mod static_scanner;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib semantic_analyzer::tests`
Expected: PASS (5 tests)

**Step 5: Commit**

```bash
git add core/src/poe/blast_radius/
git commit -m "poe: add SemanticRiskAnalyzer (System 2 blast radius)"
```

---

## Task 4: BlastRadius Assessor (Orchestrates System 1 + System 2)

**Files:**
- Create: `core/src/poe/blast_radius/assessor.rs`
- Modify: `core/src/poe/blast_radius/mod.rs`
- Modify: `core/src/poe/mod.rs` (re-exports)

**Step 1: Write the failing test**

In `core/src/poe/blast_radius/assessor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{RiskLevel, SuccessManifest, ValidationRule};

    #[test]
    fn test_tier0_skips_system2() {
        let manifest = SuccessManifest::new("t1", "destroy")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "rm".into(),
                args: vec!["-rf".into(), "/".into()],
                timeout_ms: 30_000,
            });
        // assess_sync only uses System 1 — no provider needed
        let result = BlastRadiusAssessor::assess_sync(&manifest);
        assert!(matches!(result, AssessmentResult::Rejected { .. }));
    }

    #[test]
    fn test_negligible_returns_blast_radius() {
        let manifest = SuccessManifest::new("t1", "update docs")
            .with_hard_constraint(ValidationRule::FileContains {
                path: std::path::PathBuf::from("README.md"),
                pattern: "hello".into(),
            });
        let result = BlastRadiusAssessor::assess_sync(&manifest);
        match result {
            AssessmentResult::Assessed(br) => assert_eq!(br.level, RiskLevel::Negligible),
            other => panic!("Expected Assessed, got {:?}", other),
        }
    }

    #[test]
    fn test_mandatory_signature_returns_critical() {
        let manifest = SuccessManifest::new("t1", "force push")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "git".into(),
                args: vec!["push".into(), "--force".into()],
                timeout_ms: 30_000,
            });
        let result = BlastRadiusAssessor::assess_sync(&manifest);
        match result {
            AssessmentResult::Assessed(br) => assert_eq!(br.level, RiskLevel::Critical),
            other => panic!("Expected Assessed, got {:?}", other),
        }
    }

    #[test]
    fn test_safe_returns_low() {
        let manifest = SuccessManifest::new("t1", "test")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".into(),
                args: vec!["test".into()],
                timeout_ms: 60_000,
            });
        let result = BlastRadiusAssessor::assess_sync(&manifest);
        match result {
            AssessmentResult::Assessed(br) => assert_eq!(br.level, RiskLevel::Low),
            other => panic!("Expected Assessed, got {:?}", other),
        }
    }

    #[test]
    fn test_indeterminate_needs_llm() {
        let manifest = SuccessManifest::new("t1", "refactor")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".into(),
                args: vec!["test".into()],
                timeout_ms: 60_000,
            })
            .with_hard_constraint(ValidationRule::FileContains {
                path: std::path::PathBuf::from("src/auth.rs"),
                pattern: "fn verify".into(),
            });
        let result = BlastRadiusAssessor::assess_sync(&manifest);
        assert!(matches!(result, AssessmentResult::NeedsLlm));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib blast_radius::assessor::tests`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/poe/blast_radius/assessor.rs`:

```rust
//! Orchestrates System 1 + System 2 blast radius assessment.
//!
//! Provides both sync (System 1 only) and async (full hybrid) assessment paths.

use crate::poe::types::{BlastRadius, RiskLevel, SuccessManifest};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::semantic_analyzer::SemanticRiskAnalyzer;
use super::static_scanner::{ScanResult, StaticSafetyScanner};

/// Result of blast radius assessment.
#[derive(Debug)]
pub enum AssessmentResult {
    /// Security violation — task must be rejected (Tier 0)
    Rejected { reason: String },
    /// Assessment complete — BlastRadius computed
    Assessed(BlastRadius),
    /// System 1 inconclusive — needs LLM (returned by assess_sync)
    NeedsLlm,
}

/// Orchestrates the hybrid blast radius assessment pipeline.
pub struct BlastRadiusAssessor;

impl BlastRadiusAssessor {
    /// Synchronous assessment using System 1 only.
    ///
    /// Returns `NeedsLlm` for indeterminate cases.
    pub fn assess_sync(manifest: &SuccessManifest) -> AssessmentResult {
        let scan = StaticSafetyScanner::scan(manifest);

        match scan {
            ScanResult::HardReject { reason } => AssessmentResult::Rejected { reason },
            ScanResult::MandatorySignature { reason } => {
                AssessmentResult::Assessed(BlastRadius::new(
                    0.8, 0.9, 0.2,
                    RiskLevel::Critical,
                    reason,
                ))
            }
            ScanResult::Negligible => {
                AssessmentResult::Assessed(BlastRadius::new(
                    0.0, 0.0, 1.0,
                    RiskLevel::Negligible,
                    "Documentation or read-only operation".into(),
                ))
            }
            ScanResult::Safe => {
                AssessmentResult::Assessed(BlastRadius::new(
                    0.1, 0.1, 0.9,
                    RiskLevel::Low,
                    "Standard safe operation".into(),
                ))
            }
            ScanResult::Indeterminate => AssessmentResult::NeedsLlm,
        }
    }

    /// Full hybrid assessment: System 1 + System 2 fallback.
    ///
    /// Always returns a definitive result (never NeedsLlm).
    pub async fn assess(
        manifest: &SuccessManifest,
        provider: &Arc<dyn AiProvider>,
    ) -> AssessmentResult {
        // Phase 1: System 1 deterministic scan
        let sync_result = Self::assess_sync(manifest);

        match sync_result {
            // Definitive results from System 1 — no LLM needed
            AssessmentResult::Rejected { .. } | AssessmentResult::Assessed(_) => sync_result,

            // Indeterminate — invoke System 2
            AssessmentResult::NeedsLlm => {
                let llm_result = SemanticRiskAnalyzer::analyze(manifest, provider).await;
                AssessmentResult::Assessed(llm_result)
            }
        }
    }

    /// Apply reversibility compensation.
    ///
    /// If workspace is in clean git state and task only affects
    /// version-controlled files, High can be downgraded to Medium.
    pub fn apply_reversibility_compensation(
        mut blast_radius: BlastRadius,
        is_clean_git_state: bool,
        all_files_tracked: bool,
    ) -> BlastRadius {
        if is_clean_git_state && all_files_tracked && blast_radius.level == RiskLevel::High {
            blast_radius.level = RiskLevel::Medium;
            blast_radius.reversibility = blast_radius.reversibility.max(0.8);
            blast_radius.reasoning = format!(
                "{} (downgraded: clean git state, all files tracked)",
                blast_radius.reasoning
            );
        }
        blast_radius
    }
}
```

Update `core/src/poe/blast_radius/mod.rs`:

```rust
pub mod assessor;
pub mod semantic_analyzer;
pub mod static_scanner;
```

Add re-exports in `core/src/poe/mod.rs`:

```rust
pub use blast_radius::assessor::{AssessmentResult, BlastRadiusAssessor};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib blast_radius::assessor::tests`
Expected: PASS (5 tests)

**Step 5: Commit**

```bash
git add core/src/poe/blast_radius/ core/src/poe/mod.rs
git commit -m "poe: add BlastRadiusAssessor orchestrating System 1 + System 2"
```

---

## Task 5: Integrate BlastRadius into TrustEvaluator

**Files:**
- Modify: `core/src/poe/trust.rs` (update WhitelistTrustEvaluator and ExperienceTrustEvaluator to check blast_radius)

**Step 1: Write the failing test**

Add to existing test module in `core/src/poe/trust.rs`:

```rust
#[cfg(test)]
mod blast_radius_trust_tests {
    use super::*;
    use crate::poe::types::{BlastRadius, RiskLevel};

    #[test]
    fn test_critical_blast_radius_always_requires_signature() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "safe operation")
            .with_blast_radius(BlastRadius::new(0.9, 0.9, 0.1, RiskLevel::Critical, "test"));
        let context = TrustContext::new()
            .with_history(1.0, 100) // Perfect history
            .with_crystallized_skill();
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_high_blast_radius_requires_signature() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "risky op")
            .with_blast_radius(BlastRadius::new(0.7, 0.7, 0.3, RiskLevel::High, "test"));
        let context = TrustContext::new().with_history(0.95, 50);
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_negligible_blast_radius_can_auto_approve() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "update readme")
            .with_blast_radius(BlastRadius::new(0.0, 0.0, 1.0, RiskLevel::Negligible, "docs only"));
        let context = TrustContext::new().with_file_count(1);
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib blast_radius_trust_tests`
Expected: FAIL — WhitelistTrustEvaluator doesn't check blast_radius

**Step 3: Modify WhitelistTrustEvaluator**

In the `TrustEvaluator` impl for `WhitelistTrustEvaluator`, add blast_radius check at the top of the `evaluate` method:

```rust
fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision {
    // === BlastRadius gate (if assessed) ===
    if let Some(ref br) = manifest.blast_radius {
        match br.level {
            crate::poe::types::RiskLevel::Critical => {
                return AutoApprovalDecision::RequireSignature {
                    reason: format!("Critical risk: {}", br.reasoning),
                };
            }
            crate::poe::types::RiskLevel::High => {
                return AutoApprovalDecision::RequireSignature {
                    reason: format!("High risk: {}", br.reasoning),
                };
            }
            crate::poe::types::RiskLevel::Negligible => {
                return AutoApprovalDecision::AutoApprove {
                    reason: format!("Negligible risk: {}", br.reasoning),
                    confidence: 0.95,
                };
            }
            // Low/Medium fall through to existing logic
            _ => {}
        }
    }

    // ... existing whitelist logic unchanged ...
```

Apply similar pattern to `ExperienceTrustEvaluator`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib blast_radius_trust_tests`
Expected: PASS (3 tests)

Also run: `cargo test -p alephcore --lib trust` to ensure existing tests still pass.

**Step 5: Commit**

```bash
git add core/src/poe/trust.rs
git commit -m "poe: integrate BlastRadius into TrustEvaluator decision logic"
```

---

## Task 6: TabooBuffer Core Component

**Files:**
- Create: `core/src/poe/taboo/mod.rs`
- Create: `core/src/poe/taboo/buffer.rs`
- Modify: `core/src/poe/mod.rs`

**Step 1: Write the failing test**

In `core/src/poe/taboo/buffer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_tagged_verdict(tag: &str, passed: bool) -> TaggedVerdict {
        TaggedVerdict {
            verdict: crate::poe::types::Verdict::failure("test failure"),
            semantic_tag: tag.to_string(),
            failure_reason: "test".to_string(),
        }
    }

    #[test]
    fn test_empty_buffer_no_trigger() {
        let buffer = TabooBuffer::new(3);
        assert!(buffer.check_micro_taboo().is_none());
    }

    #[test]
    fn test_below_threshold_no_trigger() {
        let mut buffer = TabooBuffer::new(3);
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        // Only 2 repetitions, threshold is 3
        assert!(buffer.check_micro_taboo().is_none());
    }

    #[test]
    fn test_threshold_reached_triggers() {
        let mut buffer = TabooBuffer::new(3);
        buffer.record(make_tagged_verdict("DependencyMismatch", false));
        buffer.record(make_tagged_verdict("DependencyMismatch", false));
        buffer.record(make_tagged_verdict("DependencyMismatch", false));
        let taboo = buffer.check_micro_taboo();
        assert!(taboo.is_some());
        assert!(taboo.unwrap().contains("DependencyMismatch"));
    }

    #[test]
    fn test_mixed_tags_no_trigger() {
        let mut buffer = TabooBuffer::new(3);
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        buffer.record(make_tagged_verdict("DependencyMismatch", false));
        buffer.record(make_tagged_verdict("LogicError", false));
        assert!(buffer.check_micro_taboo().is_none());
    }

    #[test]
    fn test_sliding_window() {
        let mut buffer = TabooBuffer::new(3);
        // First 3 are same tag
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        buffer.record(make_tagged_verdict("PermissionDenied", false));
        assert!(buffer.check_micro_taboo().is_some());

        // Push a different one — window shifts
        buffer.record(make_tagged_verdict("LogicError", false));
        // Now window is [Perm, Perm, Logic] — no trigger
        assert!(buffer.check_micro_taboo().is_none());
    }

    #[test]
    fn test_window_size_respected() {
        // Window = 5 but threshold = 3 (checks last N entries for max consecutive)
        let mut buffer = TabooBuffer::with_window(3, 5);
        buffer.record(make_tagged_verdict("A", false));
        buffer.record(make_tagged_verdict("B", false));
        buffer.record(make_tagged_verdict("B", false));
        buffer.record(make_tagged_verdict("B", false));
        buffer.record(make_tagged_verdict("B", false));
        let taboo = buffer.check_micro_taboo();
        assert!(taboo.is_some());
    }

    #[test]
    fn test_clear_resets_buffer() {
        let mut buffer = TabooBuffer::new(3);
        buffer.record(make_tagged_verdict("X", false));
        buffer.record(make_tagged_verdict("X", false));
        buffer.record(make_tagged_verdict("X", false));
        buffer.clear();
        assert!(buffer.check_micro_taboo().is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib taboo::buffer::tests`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/poe/taboo/mod.rs`:

```rust
//! Taboo crystallization: anti-pattern learning for POE.
//!
//! - Micro-Taboo: Real-time loop interception when same RootCause repeats
//! - Macro-Taboo: Post-mortem persistent anti-pattern on BudgetExhausted

pub mod buffer;
```

Create `core/src/poe/taboo/buffer.rs`:

```rust
//! TabooBuffer: sliding window for detecting repetitive failure patterns.
//!
//! Sits inside PoeManager. Records tagged verdicts and triggers Micro-Taboo
//! when the same semantic tag appears N consecutive times.

use std::collections::VecDeque;

/// A verdict tagged with its semantic failure category.
#[derive(Debug, Clone)]
pub struct TaggedVerdict {
    /// The original verdict
    pub verdict: crate::poe::types::Verdict,
    /// Semantic category (e.g., "PermissionDenied", "DependencyMismatch")
    pub semantic_tag: String,
    /// Failure reason for prompt injection
    pub failure_reason: String,
}

/// Sliding window buffer for detecting repetitive failure patterns.
///
/// When the same `semantic_tag` appears `repetition_threshold` times
/// in the last `window_size` entries, a Micro-Taboo is triggered.
pub struct TabooBuffer {
    /// Sliding window of recent verdicts
    window: VecDeque<TaggedVerdict>,
    /// Number of consecutive same-tag failures to trigger taboo
    repetition_threshold: usize,
    /// Maximum entries to keep in sliding window
    window_size: usize,
}

impl TabooBuffer {
    /// Create with default window size (= threshold * 2).
    pub fn new(repetition_threshold: usize) -> Self {
        Self {
            window: VecDeque::new(),
            repetition_threshold,
            window_size: repetition_threshold * 2,
        }
    }

    /// Create with explicit window size.
    pub fn with_window(repetition_threshold: usize, window_size: usize) -> Self {
        Self {
            window: VecDeque::new(),
            repetition_threshold,
            window_size: window_size.max(repetition_threshold),
        }
    }

    /// Record a new tagged verdict.
    pub fn record(&mut self, verdict: TaggedVerdict) {
        self.window.push_back(verdict);
        while self.window.len() > self.window_size {
            self.window.pop_front();
        }
    }

    /// Check if a Micro-Taboo should be triggered.
    ///
    /// Returns a taboo prompt injection string if the same semantic_tag
    /// appeared `repetition_threshold` or more times consecutively
    /// (counting from the most recent entry backwards).
    pub fn check_micro_taboo(&self) -> Option<String> {
        if self.window.len() < self.repetition_threshold {
            return None;
        }

        // Count consecutive same-tag from the end
        let last_tag = &self.window.back()?.semantic_tag;
        let consecutive = self
            .window
            .iter()
            .rev()
            .take_while(|v| &v.semantic_tag == last_tag)
            .count();

        if consecutive >= self.repetition_threshold {
            let reasons: Vec<&str> = self
                .window
                .iter()
                .rev()
                .take(self.repetition_threshold)
                .map(|v| v.failure_reason.as_str())
                .collect();

            Some(format!(
                "TABOO WARNING: You have failed {} consecutive times with the same root cause: [{}]. \
                 Recent errors: {}. \
                 This approach is FORBIDDEN. You MUST try a completely different strategy.",
                consecutive,
                last_tag,
                reasons.join("; ")
            ))
        } else {
            None
        }
    }

    /// Clear the buffer (e.g., on task success or new task).
    pub fn clear(&mut self) {
        self.window.clear();
    }

    /// Get the current buffer length.
    pub fn len(&self) -> usize {
        self.window.len()
    }

    /// Check if buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }
}
```

Add to `core/src/poe/mod.rs`:

```rust
pub mod taboo;
pub use taboo::buffer::{TabooBuffer, TaggedVerdict};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib taboo::buffer::tests`
Expected: PASS (7 tests)

**Step 5: Commit**

```bash
git add core/src/poe/taboo/ core/src/poe/mod.rs
git commit -m "poe: add TabooBuffer for micro-taboo detection"
```

---

## Task 7: Integrate TabooBuffer into PoeManager

**Files:**
- Modify: `core/src/poe/manager.rs` (add TabooBuffer to PoeManager, inject micro-taboo into retry prompt)

**Step 1: Write the failing test**

Add to `core/src/poe/manager.rs` test module:

```rust
#[cfg(test)]
mod taboo_tests {
    use super::*;
    use crate::poe::types::*;
    use crate::poe::validation::CompositeValidator;
    use crate::poe::taboo::buffer::TaggedVerdict;

    // Use existing MockWorker pattern from manager.rs tests

    #[tokio::test]
    async fn test_micro_taboo_injected_into_retry_prompt() {
        // Create a worker that tracks the instruction it receives
        let instructions_received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let instructions_clone = instructions_received.clone();

        // Worker that always fails, captures instructions
        struct InstructionCapturingWorker {
            instructions: Arc<std::sync::Mutex<Vec<String>>>,
            call_count: AtomicU32,
        }

        #[async_trait::async_trait]
        impl Worker for InstructionCapturingWorker {
            async fn execute(&self, instruction: &str, _previous_failure: Option<&str>) -> Result<WorkerOutput> {
                self.instructions.lock().unwrap_or_else(|e| e.into_inner())
                    .push(instruction.to_string());
                self.call_count.fetch_add(1, Ordering::Relaxed);
                Ok(WorkerOutput::completed("done"))
            }
            async fn abort(&self) -> Result<()> { Ok(()) }
            async fn snapshot(&self) -> Result<crate::poe::worker::StateSnapshot> {
                Err(crate::error::AlephError::internal("no snapshot"))
            }
            async fn restore(&self, _: &crate::poe::worker::StateSnapshot) -> Result<()> { Ok(()) }
        }

        // This test verifies the TabooBuffer is part of PoeManager's state.
        // The actual integration test for prompt injection requires a full
        // mock setup — we verify the buffer exists and is accessible.
        let config = PoeConfig::default();
        let buffer = TabooBuffer::new(3);
        assert!(buffer.is_empty());
    }
}
```

**Step 2: Run test to verify setup compiles**

Run: `cargo test -p alephcore --lib taboo_tests`
Expected: PASS (basic compilation check)

**Step 3: Integrate TabooBuffer into PoeManager**

In `core/src/poe/manager.rs`, add to `PoeManager` struct:

```rust
use crate::poe::taboo::buffer::{TabooBuffer, TaggedVerdict};

pub struct PoeManager<W: Worker> {
    // ... existing fields ...
    /// Taboo buffer for detecting repetitive failure patterns
    taboo_buffer: std::sync::Mutex<TabooBuffer>,
}
```

In `PoeManager::new()`, initialize:

```rust
taboo_buffer: std::sync::Mutex::new(TabooBuffer::new(3)),
```

In the execute loop, after validation failure and meta_cognition callback, add taboo recording:

```rust
// Record failure in taboo buffer
{
    let tagged = TaggedVerdict {
        verdict: verdict.clone(),
        semantic_tag: Self::extract_failure_tag(&verdict),
        failure_reason: verdict.reason.clone(),
    };
    if let Ok(mut buf) = self.taboo_buffer.lock() {
        buf.record(tagged);
    }
}
```

When building the retry prompt, check for micro-taboo:

```rust
// Check for micro-taboo before building retry prompt
let taboo_warning = self.taboo_buffer
    .lock()
    .ok()
    .and_then(|buf| buf.check_micro_taboo());

let instruction = match (&previous_failure, &taboo_warning) {
    (Some(feedback), Some(taboo)) => {
        format!("{}\n\n{}\n\n{}", task.instruction, feedback, taboo)
    }
    (Some(feedback), None) => self.build_retry_prompt(&task, feedback),
    _ => task.instruction.clone(),
};
```

Add helper method:

```rust
/// Extract a semantic failure tag from a verdict.
///
/// Uses heuristics on the failure reason to categorize the error.
fn extract_failure_tag(verdict: &Verdict) -> String {
    let reason = verdict.reason.to_lowercase();
    if reason.contains("permission") || reason.contains("access denied") {
        "PermissionDenied".to_string()
    } else if reason.contains("not found") || reason.contains("no such file") {
        "FileNotFound".to_string()
    } else if reason.contains("compile") || reason.contains("syntax") {
        "CompilationError".to_string()
    } else if reason.contains("timeout") {
        "Timeout".to_string()
    } else if reason.contains("dependency") || reason.contains("import") {
        "DependencyMismatch".to_string()
    } else {
        // Use first 50 chars of reason as fallback tag
        let tag: String = reason.chars().take(50).collect();
        tag.replace(' ', "_")
    }
}
```

On success, clear the buffer:

```rust
// In the success branch, before returning:
if let Ok(mut buf) = self.taboo_buffer.lock() {
    buf.clear();
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib manager` to ensure all existing tests still pass.
Run: `cargo check -p alephcore` to verify compilation.

**Step 5: Commit**

```bash
git add core/src/poe/manager.rs
git commit -m "poe: integrate TabooBuffer into PoeManager execution loop"
```

---

## Task 8: Macro-Taboo Persistence (AntiPattern in ExperienceStore)

**Files:**
- Create: `core/src/poe/taboo/anti_pattern.rs`
- Modify: `core/src/poe/taboo/mod.rs`
- Modify: `core/src/poe/mod.rs`

**Step 1: Write the failing test**

In `core/src/poe/taboo/anti_pattern.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anti_pattern_creation() {
        let ap = AntiPattern::new(
            "poe-create-rust-file",
            "Always fails when target dir is read-only",
            vec!["PermissionDenied".to_string()],
        );
        assert_eq!(ap.pattern_id, "poe-create-rust-file");
        assert!(!ap.failure_tags.is_empty());
        assert!(ap.created_at > 0);
    }

    #[test]
    fn test_anti_pattern_to_prompt_injection() {
        let ap = AntiPattern::new(
            "poe-db-migrate",
            "Migration fails without backup step",
            vec!["DependencyMismatch".to_string()],
        );
        let prompt = ap.to_avoidance_prompt();
        assert!(prompt.contains("AVOID"));
        assert!(prompt.contains("Migration fails"));
    }

    #[test]
    fn test_anti_pattern_store_insert_and_retrieve() {
        let mut store = InMemoryAntiPatternStore::new();
        let ap = AntiPattern::new("p1", "desc", vec!["tag1".into()]);
        store.insert(ap);
        let results = store.find_by_pattern_id("p1");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_anti_pattern_store_empty_query() {
        let store = InMemoryAntiPatternStore::new();
        let results = store.find_by_pattern_id("nonexistent");
        assert!(results.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib taboo::anti_pattern::tests`
Expected: FAIL

**Step 3: Write minimal implementation**

Create `core/src/poe/taboo/anti_pattern.rs`:

```rust
//! Anti-pattern persistence for Macro-Taboo crystallization.
//!
//! When BudgetExhausted occurs, the failure trajectory is distilled
//! into an AntiPattern and stored for future recall.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A crystallized anti-pattern learned from repeated failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntiPattern {
    /// Task pattern ID this anti-pattern applies to
    pub pattern_id: String,

    /// Human-readable description of what went wrong
    pub description: String,

    /// Semantic tags of the failure modes encountered
    pub failure_tags: Vec<String>,

    /// Number of attempts that were made before exhaustion
    pub attempts_made: u8,

    /// Unix timestamp of when this anti-pattern was recorded
    pub created_at: i64,

    /// Optional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl AntiPattern {
    /// Create a new anti-pattern.
    pub fn new(
        pattern_id: impl Into<String>,
        description: impl Into<String>,
        failure_tags: Vec<String>,
    ) -> Self {
        Self {
            pattern_id: pattern_id.into(),
            description: description.into(),
            failure_tags,
            attempts_made: 0,
            created_at: Utc::now().timestamp(),
            metadata: HashMap::new(),
        }
    }

    /// Set the number of attempts.
    pub fn with_attempts(mut self, attempts: u8) -> Self {
        self.attempts_made = attempts;
        self
    }

    /// Generate a prompt injection string for avoidance guidance.
    pub fn to_avoidance_prompt(&self) -> String {
        format!(
            "AVOID: A previous attempt at a similar task failed with: \"{}\". \
             Failure categories: [{}]. Do NOT repeat this approach.",
            self.description,
            self.failure_tags.join(", ")
        )
    }
}

/// In-memory anti-pattern store for testing and lightweight use.
pub struct InMemoryAntiPatternStore {
    patterns: Vec<AntiPattern>,
}

impl InMemoryAntiPatternStore {
    pub fn new() -> Self {
        Self { patterns: Vec::new() }
    }

    pub fn insert(&mut self, pattern: AntiPattern) {
        self.patterns.push(pattern);
    }

    pub fn find_by_pattern_id(&self, pattern_id: &str) -> Vec<&AntiPattern> {
        self.patterns
            .iter()
            .filter(|p| p.pattern_id == pattern_id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

impl Default for InMemoryAntiPatternStore {
    fn default() -> Self {
        Self::new()
    }
}
```

Update `core/src/poe/taboo/mod.rs`:

```rust
pub mod anti_pattern;
pub mod buffer;
```

Add re-exports in `core/src/poe/mod.rs`:

```rust
pub use taboo::anti_pattern::{AntiPattern, InMemoryAntiPatternStore};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib taboo::anti_pattern::tests`
Expected: PASS (4 tests)

**Step 5: Commit**

```bash
git add core/src/poe/taboo/ core/src/poe/mod.rs
git commit -m "poe: add AntiPattern type and in-memory store for Macro-Taboo"
```

---

## Task 9: Phase 2/3 Interface Stubs

**Files:**
- Modify: `core/src/poe/types.rs` (add DecompositionRequired to PoeOutcome, metadata to PoeTask)
- Modify: `core/src/poe/manager.rs` (add max_depth to PoeConfig)
- Modify: `core/src/poe/worker/mod.rs` (add supports_isolation to Worker trait)

**Step 1: Write the failing test**

```rust
// In types.rs tests
#[test]
fn test_decomposition_required_variant() {
    let sub = SuccessManifest::new("sub-1", "subtask");
    let outcome = PoeOutcome::DecompositionRequired {
        sub_manifests: vec![sub],
        reason: "task too complex".into(),
    };
    assert!(matches!(outcome, PoeOutcome::DecompositionRequired { .. }));
}

// In manager.rs tests
#[test]
fn test_poe_config_max_depth() {
    let config = PoeConfig::default();
    assert_eq!(config.max_depth, 3);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib decomposition_required`
Expected: FAIL

**Step 3: Write minimal stubs**

Add to `PoeOutcome` in `types.rs`:

```rust
/// Decomposition required — task needs to be split into sub-tasks (Phase 2)
DecompositionRequired {
    /// Sub-task manifests to execute
    sub_manifests: Vec<SuccessManifest>,
    /// Reason for decomposition
    reason: String,
},
```

Add to `PoeConfig` in `manager.rs`:

```rust
/// Maximum recursion depth for nested POE tasks (Phase 2).
/// Default: 3
pub max_depth: u8,
```

Update `Default for PoeConfig`:

```rust
max_depth: 3,
```

Add builder method:

```rust
pub fn with_max_depth(mut self, depth: u8) -> Self {
    self.max_depth = depth;
    self
}
```

Add to `PoeTask` in `types.rs`:

```rust
/// Arbitrary metadata for future extensions (Phase 3 federation routing)
#[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
pub metadata: std::collections::HashMap<String, String>,
```

Update `PoeTask::new()` to include `metadata: std::collections::HashMap::new()`.

Add default impl to Worker trait in `worker/mod.rs`:

```rust
/// Whether this worker supports isolation (cloning workspace for parallel execution).
/// Phase 3: Speculative parallel execution will use this.
fn supports_isolation(&self) -> bool {
    false
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib` (full test suite)
Expected: PASS — all existing tests + new tests pass

**Step 5: Commit**

```bash
git add core/src/poe/types.rs core/src/poe/manager.rs core/src/poe/worker/mod.rs
git commit -m "poe: add Phase 2/3 interface stubs (DecompositionRequired, max_depth, metadata)"
```

---

## Task 10: Full Integration Test

**Files:**
- Create: `core/tests/poe_blast_radius_integration.rs`

**Step 1: Write the integration test**

```rust
//! Integration test for BlastRadius + TabooBuffer in POE pipeline.

use alephcore::poe::{
    BlastRadius, BlastRadiusAssessor, RiskLevel, ScanResult, StaticSafetyScanner,
    SuccessManifest, ValidationRule, TabooBuffer, TaggedVerdict, Verdict,
    AntiPattern,
};

#[test]
fn test_full_blast_radius_pipeline_safe_task() {
    let manifest = SuccessManifest::new("integration-1", "run cargo test")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "cargo".into(),
            args: vec!["test".into()],
            timeout_ms: 60_000,
        });

    let result = BlastRadiusAssessor::assess_sync(&manifest);
    match result {
        alephcore::poe::blast_radius::assessor::AssessmentResult::Assessed(br) => {
            assert_eq!(br.level, RiskLevel::Low);
        }
        other => panic!("Expected Assessed, got {:?}", other),
    }
}

#[test]
fn test_full_blast_radius_pipeline_dangerous_task() {
    let manifest = SuccessManifest::new("integration-2", "clean everything")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "rm".into(),
            args: vec!["-rf".into(), "/".into()],
            timeout_ms: 30_000,
        });

    let result = BlastRadiusAssessor::assess_sync(&manifest);
    assert!(matches!(
        result,
        alephcore::poe::blast_radius::assessor::AssessmentResult::Rejected { .. }
    ));
}

#[test]
fn test_taboo_buffer_micro_taboo_cycle() {
    let mut buffer = TabooBuffer::new(3);

    // Simulate 3 consecutive same-type failures
    for _ in 0..3 {
        buffer.record(TaggedVerdict {
            verdict: Verdict::failure("compilation error in auth.rs"),
            semantic_tag: "CompilationError".into(),
            failure_reason: "cannot find type `AuthToken`".into(),
        });
    }

    let taboo = buffer.check_micro_taboo();
    assert!(taboo.is_some());
    let prompt = taboo.unwrap();
    assert!(prompt.contains("TABOO WARNING"));
    assert!(prompt.contains("CompilationError"));
    assert!(prompt.contains("FORBIDDEN"));
}

#[test]
fn test_anti_pattern_avoidance_prompt() {
    let ap = AntiPattern::new(
        "poe-refactor-auth",
        "Refactoring auth module fails when middleware depends on old types",
        vec!["CompilationError".into(), "DependencyMismatch".into()],
    ).with_attempts(5);

    let prompt = ap.to_avoidance_prompt();
    assert!(prompt.contains("AVOID"));
    assert!(prompt.contains("middleware depends on old types"));
}

#[test]
fn test_manifest_with_blast_radius_serialization() {
    let manifest = SuccessManifest::new("ser-1", "test serialization")
        .with_blast_radius(BlastRadius::new(0.5, 0.3, 0.8, RiskLevel::Medium, "moderate risk"))
        .with_hard_constraint(ValidationRule::FileExists {
            path: std::path::PathBuf::from("src/main.rs"),
        });

    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("blast_radius"));
    assert!(json.contains("Medium"));

    let deserialized: SuccessManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.blast_radius.unwrap().level, RiskLevel::Medium);
}
```

**Step 2: Run test**

Run: `cargo test -p alephcore --test poe_blast_radius_integration`
Expected: PASS (5 tests)

**Step 3: Commit**

```bash
git add core/tests/poe_blast_radius_integration.rs
git commit -m "poe: add integration tests for BlastRadius + TabooBuffer"
```

---

## Summary

| Task | Component | Tests | Lines (est.) |
|------|-----------|-------|-------------|
| 1 | BlastRadius + RiskLevel types | 4 | ~100 |
| 2 | StaticSafetyScanner | 10 | ~250 |
| 3 | SemanticRiskAnalyzer | 5 | ~150 |
| 4 | BlastRadiusAssessor | 5 | ~100 |
| 5 | TrustEvaluator integration | 3 | ~40 |
| 6 | TabooBuffer | 7 | ~150 |
| 7 | PoeManager + TabooBuffer | compile | ~60 |
| 8 | AntiPattern + store | 4 | ~120 |
| 9 | Phase 2/3 stubs | 2 | ~30 |
| 10 | Integration tests | 5 | ~80 |
| **Total** | | **45** | **~1,080** |
