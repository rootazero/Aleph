//! System 1: Deterministic safety scanner for POE manifests.
//!
//! Scans ValidationRules for known dangerous patterns with zero LLM cost.
//! - Tier 0 (HardReject): Destructive to host OS, irrecoverable (rm -rf /, fork bomb, dd to block device)
//! - Tier 1 (MandatorySignature): Significant data loss, credential exposure, privilege escalation

use crate::poe::types::{SuccessManifest, ValidationRule};
use std::path::Path;

// ============================================================================
// Scan Result
// ============================================================================

/// Result of a static safety scan on a POE manifest.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanResult {
    /// Tier 0: Abort immediately, no override mechanism.
    HardReject { reason: String },
    /// Tier 1: Force user confirmation before proceeding.
    MandatorySignature { reason: String },
    /// Clearly safe operation.
    Safe,
    /// Documentation-only changes.
    Negligible,
    /// Gray zone — needs System 2 (LLM) analysis.
    Indeterminate,
}

impl ScanResult {
    /// Returns the reason string if present, or a default description.
    pub fn reason(&self) -> &str {
        match self {
            ScanResult::HardReject { reason } => reason,
            ScanResult::MandatorySignature { reason } => reason,
            ScanResult::Safe => "Operation is clearly safe",
            ScanResult::Negligible => "Documentation-only changes",
            ScanResult::Indeterminate => "Needs further analysis (System 2)",
        }
    }

    /// Severity level for comparison (higher = more severe).
    fn severity(&self) -> u8 {
        match self {
            ScanResult::Negligible => 0,
            ScanResult::Safe => 1,
            ScanResult::Indeterminate => 2,
            ScanResult::MandatorySignature { .. } => 3,
            ScanResult::HardReject { .. } => 4,
        }
    }
}

// ============================================================================
// Static Safety Scanner
// ============================================================================

/// Deterministic safety scanner that checks POE manifests for dangerous patterns.
///
/// Operates at zero LLM cost using pure pattern matching against known
/// dangerous command patterns and sensitive file paths.
pub struct StaticSafetyScanner;

impl StaticSafetyScanner {
    /// Create a new scanner instance.
    pub fn new() -> Self {
        Self
    }

    /// Scan a manifest and return the highest-severity result.
    ///
    /// Iterates all hard_constraints and soft_metrics rules, checking each
    /// against Tier 0 and Tier 1 patterns. Short-circuits on HardReject.
    pub fn scan(&self, manifest: &SuccessManifest) -> ScanResult {
        let mut highest = ScanResult::Negligible;
        let mut non_doc_count = 0u32;

        // Collect all rules: hard_constraints directly + soft_metrics' inner rules
        let hard_rules = manifest.hard_constraints.iter();
        let soft_rules = manifest.soft_metrics.iter().map(|m| &m.rule);

        for rule in hard_rules.chain(soft_rules) {
            let result = self.scan_rule(rule);

            // Short-circuit on HardReject
            if result.severity() == 4 {
                return result;
            }

            // Track non-doc rules
            if !self.is_doc_only_rule(rule) {
                non_doc_count += 1;
            }

            // Escalate to highest severity
            if result.severity() > highest.severity() {
                highest = result;
            }
        }

        // If we found a MandatorySignature, return it
        if highest.severity() >= 3 {
            return highest;
        }

        // If all rules are doc-only, return Negligible
        if non_doc_count == 0 {
            return ScanResult::Negligible;
        }

        // Multiple non-doc constraints → gray zone, needs System 2
        if non_doc_count > 1 {
            return ScanResult::Indeterminate;
        }

        // Single simple non-doc operation
        ScanResult::Safe
    }

    /// Scan a single rule against Tier 0 and Tier 1 patterns.
    fn scan_rule(&self, rule: &ValidationRule) -> ScanResult {
        match rule {
            ValidationRule::CommandPasses { cmd, args, .. }
            | ValidationRule::CommandOutputContains { cmd, args, .. } => {
                // Build the full command string for pattern matching
                let full_cmd = if args.is_empty() {
                    cmd.clone()
                } else {
                    format!("{} {}", cmd, args.join(" "))
                };
                self.scan_command(&full_cmd)
            }

            ValidationRule::FileExists { path }
            | ValidationRule::FileNotExists { path } => {
                self.scan_file_path(path)
            }

            ValidationRule::FileContains { path, pattern }
            | ValidationRule::FileNotContains { path, pattern } => {
                let path_result = self.scan_file_path(path);
                let pattern_result = self.scan_command(pattern);
                if path_result.severity() >= pattern_result.severity() {
                    path_result
                } else {
                    pattern_result
                }
            }

            ValidationRule::DirStructureMatch { root, expected } => {
                let path_result = self.scan_file_path(root);
                let expected_result = self.scan_command(expected);
                if path_result.severity() >= expected_result.severity() {
                    path_result
                } else {
                    expected_result
                }
            }

            ValidationRule::JsonSchemaValid { path, .. } => {
                self.scan_file_path(path)
            }

            ValidationRule::SemanticCheck { .. } => {
                // Semantic checks are evaluated by LLM, can't statically assess
                ScanResult::Safe
            }
        }
    }

    /// Check a command string against Tier 0 and Tier 1 patterns.
    fn scan_command(&self, cmd: &str) -> ScanResult {
        let lower = cmd.to_lowercase();

        // === Tier 0: HardReject ===

        // rm -rf / or rm -rf /*
        if self.matches_rm_rf_root(&lower) {
            return ScanResult::HardReject {
                reason: "Destructive: rm -rf targeting root filesystem".into(),
            };
        }

        // Fork bomb
        if self.matches_fork_bomb(cmd) {
            return ScanResult::HardReject {
                reason: "Destructive: fork bomb detected".into(),
            };
        }

        // dd to block device
        if self.matches_dd_block_device(&lower) {
            return ScanResult::HardReject {
                reason: "Destructive: dd writing to block device".into(),
            };
        }

        // mkfs
        if self.matches_mkfs(&lower) {
            return ScanResult::HardReject {
                reason: "Destructive: filesystem format command (mkfs)".into(),
            };
        }

        // === Tier 1: MandatorySignature ===

        // sudo
        if self.matches_sudo(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Privilege escalation: sudo command".into(),
            };
        }

        // SQL destructive operations
        if self.matches_sql_destructive(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Data destruction: SQL DROP/TRUNCATE operation".into(),
            };
        }

        // git force push
        if self.matches_git_force_push(&lower) {
            return ScanResult::MandatorySignature {
                reason: "History rewrite: git push --force".into(),
            };
        }

        // chmod +s (setuid)
        if self.matches_chmod_setuid(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Privilege escalation: chmod +s (setuid)".into(),
            };
        }

        // chown
        if self.matches_chown(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Ownership change: chown command".into(),
            };
        }

        // curl/wget with mutating methods
        if self.matches_http_mutation(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Network mutation: HTTP POST/PUT/DELETE/PATCH request".into(),
            };
        }

        // Package publishing
        if self.matches_package_publish(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Public release: package/image publish".into(),
            };
        }

        // ssh/scp
        if self.matches_ssh_scp(&lower) {
            return ScanResult::MandatorySignature {
                reason: "Remote access: ssh/scp command".into(),
            };
        }

        // rm (any file deletion)
        if self.matches_rm(&lower) {
            return ScanResult::MandatorySignature {
                reason: "File deletion: rm command".into(),
            };
        }

        ScanResult::Safe
    }

    /// Check a file path against sensitive path patterns.
    fn scan_file_path(&self, path: &Path) -> ScanResult {
        let path_str = path.to_string_lossy().to_lowercase();

        // Sensitive credential/secret paths
        let sensitive_patterns = [
            ".ssh", "id_rsa", "id_ed25519", ".env", "secret", "credential", ".key", ".pem",
        ];
        for pattern in &sensitive_patterns {
            if path_str.contains(pattern) {
                return ScanResult::MandatorySignature {
                    reason: format!(
                        "Sensitive file access: path contains '{}'",
                        pattern
                    ),
                };
            }
        }

        // System directories
        let system_dirs = ["/etc/", "/usr/", "/var/", "/sys/", "/proc/"];
        for dir in &system_dirs {
            if path_str.contains(dir) {
                return ScanResult::MandatorySignature {
                    reason: format!(
                        "System directory access: path contains '{}'",
                        dir
                    ),
                };
            }
        }

        ScanResult::Safe
    }

    /// Check if a rule only touches documentation files.
    fn is_doc_only_rule(&self, rule: &ValidationRule) -> bool {
        let doc_extensions = [".md", ".txt", ".rst", ".adoc"];

        match rule {
            ValidationRule::FileExists { path }
            | ValidationRule::FileNotExists { path }
            | ValidationRule::FileContains { path, .. }
            | ValidationRule::FileNotContains { path, .. }
            | ValidationRule::JsonSchemaValid { path, .. } => {
                let path_str = path.to_string_lossy().to_lowercase();
                doc_extensions.iter().any(|ext| path_str.ends_with(ext))
            }

            ValidationRule::DirStructureMatch { expected, .. } => {
                // If all listed files are documentation
                let lower = expected.to_lowercase();
                let parts: Vec<&str> = lower.split(',').map(|s| s.trim()).collect();
                parts.iter().all(|part| {
                    doc_extensions.iter().any(|ext| part.ends_with(ext))
                })
            }

            // Commands and semantic checks are never doc-only
            ValidationRule::CommandPasses { .. }
            | ValidationRule::CommandOutputContains { .. }
            | ValidationRule::SemanticCheck { .. } => false,
        }
    }

    // ========================================================================
    // Pattern Matchers
    // ========================================================================

    /// Match `rm -rf /` or `rm -rf /*` (root filesystem deletion).
    fn matches_rm_rf_root(&self, lower: &str) -> bool {
        // Match patterns like: rm -rf /, rm -rf /*, rm -rf --no-preserve-root /
        let trimmed = lower.trim();
        if !trimmed.contains("rm") {
            return false;
        }
        // Check for rm with -rf or -r -f flags followed by / or /*
        let has_rf = (trimmed.contains("-rf") || trimmed.contains("-fr"))
            || (trimmed.contains("-r") && trimmed.contains("-f"));

        if !has_rf {
            return false;
        }

        // Check if target is root
        // Split by whitespace and look for / or /* as a standalone argument
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        parts.iter().any(|&p| p == "/" || p == "/*")
    }

    /// Match fork bomb patterns.
    fn matches_fork_bomb(&self, cmd: &str) -> bool {
        // Common fork bomb: :(){ :|:& };: or :(){:|:&};:
        let normalized: String = cmd.chars().filter(|c| !c.is_whitespace()).collect();
        normalized.contains(":()")
            && normalized.contains(":|:")
            && normalized.contains("&")
    }

    /// Match dd writing to block devices.
    fn matches_dd_block_device(&self, lower: &str) -> bool {
        if !lower.contains("dd ") && !lower.starts_with("dd ") {
            // Also handle bare "dd" at start
            if !lower.starts_with("dd ") && lower != "dd" {
                return false;
            }
        }

        let block_device_prefixes = [
            "of=/dev/sd",
            "of=/dev/nvme",
            "of=/dev/hd",
            "of=/dev/vd",
        ];
        block_device_prefixes.iter().any(|prefix| lower.contains(prefix))
    }

    /// Match mkfs commands.
    fn matches_mkfs(&self, lower: &str) -> bool {
        let trimmed = lower.trim();
        trimmed.starts_with("mkfs") || trimmed.contains(" mkfs")
    }

    /// Match sudo as a command.
    fn matches_sudo(&self, lower: &str) -> bool {
        let trimmed = lower.trim();
        trimmed.starts_with("sudo ") || trimmed == "sudo"
    }

    /// Match SQL destructive operations.
    fn matches_sql_destructive(&self, lower: &str) -> bool {
        lower.contains("drop table")
            || lower.contains("drop database")
            || lower.contains("truncate")
    }

    /// Match git push --force or git push -f.
    fn matches_git_force_push(&self, lower: &str) -> bool {
        if !lower.contains("git") || !lower.contains("push") {
            return false;
        }
        lower.contains("--force") || lower.contains(" -f")
    }

    /// Match chmod +s (setuid).
    fn matches_chmod_setuid(&self, lower: &str) -> bool {
        lower.contains("chmod") && lower.contains("+s")
    }

    /// Match chown command.
    fn matches_chown(&self, lower: &str) -> bool {
        let trimmed = lower.trim();
        trimmed.starts_with("chown ") || trimmed.contains(" chown ")
    }

    /// Match curl/wget with mutating HTTP methods.
    fn matches_http_mutation(&self, lower: &str) -> bool {
        let has_tool = lower.contains("curl") || lower.contains("wget");
        if !has_tool {
            return false;
        }
        let methods = ["-x post", "-x put", "-x delete", "-x patch",
                       "--request post", "--request put", "--request delete", "--request patch",
                       "-d ", "--data", "--data-raw", "--data-binary"];
        methods.iter().any(|m| lower.contains(m))
    }

    /// Match package/image publish commands.
    fn matches_package_publish(&self, lower: &str) -> bool {
        lower.contains("npm publish")
            || lower.contains("cargo publish")
            || lower.contains("docker push")
    }

    /// Match ssh/scp commands.
    fn matches_ssh_scp(&self, lower: &str) -> bool {
        let trimmed = lower.trim();
        trimmed.starts_with("ssh ") || trimmed.starts_with("scp ")
            || trimmed.contains(" ssh ") || trimmed.contains(" scp ")
    }

    /// Match any rm command (not just rm -rf /).
    fn matches_rm(&self, lower: &str) -> bool {
        let trimmed = lower.trim();
        trimmed.starts_with("rm ") || trimmed.contains(" rm ")
    }
}

impl Default for StaticSafetyScanner {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SoftMetric, SuccessManifest, ValidationRule};
    use std::path::PathBuf;

    /// Helper: create a manifest with a single CommandPasses hard constraint.
    fn manifest_with_command(cmd: &str, args: &[&str]) -> SuccessManifest {
        SuccessManifest::new("test", "test objective")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: cmd.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                timeout_ms: 30_000,
            })
    }

    /// Helper: create a manifest with a single FileContains hard constraint.
    fn manifest_with_file(path: &str, pattern: &str) -> SuccessManifest {
        SuccessManifest::new("test", "test objective")
            .with_hard_constraint(ValidationRule::FileContains {
                path: PathBuf::from(path),
                pattern: pattern.to_string(),
            })
    }

    #[test]
    fn test_tier0_rm_rf_root() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("rm", &["-rf", "/"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::HardReject { .. }),
            "rm -rf / should be HardReject, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier0_fork_bomb() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("bash", &["-c", ":(){ :|:& };:"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::HardReject { .. }),
            "Fork bomb should be HardReject, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier0_dd_block_device() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("dd", &["if=/dev/zero", "of=/dev/sda", "bs=1M"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::HardReject { .. }),
            "dd to block device should be HardReject, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier0_mkfs() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("mkfs.ext4", &["/dev/sdb1"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::HardReject { .. }),
            "mkfs should be HardReject, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier1_sudo() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("sudo", &["apt", "install", "nginx"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "sudo should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier1_drop_table() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("psql", &["-c", "DROP TABLE users;"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "DROP TABLE should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier1_ssh_key_access() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_file("~/.ssh/id_rsa", ".*");
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "SSH key access should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tier1_git_force_push() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("git", &["push", "--force", "origin", "main"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "git push --force should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_safe_cargo_test() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_command("cargo", &["test"]);
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::Safe),
            "cargo test should be Safe, got: {:?}",
            result
        );
    }

    #[test]
    fn test_negligible_readme() {
        let scanner = StaticSafetyScanner::new();
        let manifest = SuccessManifest::new("test", "Update docs")
            .with_hard_constraint(ValidationRule::FileContains {
                path: PathBuf::from("README.md"),
                pattern: "# Project".to_string(),
            });
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::Negligible),
            "README.md only should be Negligible, got: {:?}",
            result
        );
    }

    #[test]
    fn test_indeterminate_multi_constraint() {
        let scanner = StaticSafetyScanner::new();
        let manifest = SuccessManifest::new("test", "Complex task")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".to_string(),
                args: vec!["build".to_string()],
                timeout_ms: 60_000,
            })
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("src/main.rs"),
            });
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::Indeterminate),
            "Multiple non-doc constraints should be Indeterminate, got: {:?}",
            result
        );
    }

    #[test]
    fn test_soft_metrics_are_scanned() {
        let scanner = StaticSafetyScanner::new();
        // Hard constraint is safe, but soft metric contains dangerous command
        let manifest = SuccessManifest::new("test", "Test soft metric scanning")
            .with_hard_constraint(ValidationRule::FileContains {
                path: PathBuf::from("README.md"),
                pattern: "hello".to_string(),
            })
            .with_soft_metric(SoftMetric::new(ValidationRule::CommandPasses {
                cmd: "sudo".to_string(),
                args: vec!["rm".to_string(), "-rf".to_string(), "/tmp/test".to_string()],
                timeout_ms: 30_000,
            }));
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "Soft metric with sudo should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_reason_method() {
        let reject = ScanResult::HardReject {
            reason: "test reason".into(),
        };
        assert_eq!(reject.reason(), "test reason");
        assert_eq!(ScanResult::Safe.reason(), "Operation is clearly safe");
        assert_eq!(ScanResult::Negligible.reason(), "Documentation-only changes");
        assert_eq!(
            ScanResult::Indeterminate.reason(),
            "Needs further analysis (System 2)"
        );
    }

    #[test]
    fn test_system_directory_access() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_file("/etc/passwd", "root");
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            "/etc/ access should be MandatorySignature, got: {:?}",
            result
        );
    }

    #[test]
    fn test_env_file_access() {
        let scanner = StaticSafetyScanner::new();
        let manifest = manifest_with_file("/app/.env", "API_KEY");
        let result = scanner.scan(&manifest);
        assert!(
            matches!(result, ScanResult::MandatorySignature { .. }),
            ".env access should be MandatorySignature, got: {:?}",
            result
        );
    }
}
