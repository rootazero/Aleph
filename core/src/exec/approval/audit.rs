use crate::exec::approval::types::EscalationReason;
use crate::exec::approval::storage::ApprovalAuditStorage;
use crate::exec::sandbox::capabilities::{Capabilities, FileSystemCapability, NetworkCapability};
use rusqlite::Result as SqliteResult;
use std::collections::HashMap;

/// Aggregate risk information for a tool
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRiskSummary {
    pub tool_name: String,
    pub risk_score: u32,
    pub capabilities: Vec<String>,
    pub execution_count: u32,
    pub escalation_count: u32,
    pub last_executed_at: Option<i64>,
}

/// Individual execution record
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionRecord {
    pub execution_id: String,
    pub tool_name: String,
    pub timestamp: i64,
    pub parameters: HashMap<String, String>,
    pub escalation_triggered: bool,
    pub escalation_reason: Option<EscalationReason>,
    pub user_decision: Option<String>,
}

/// Query interface for audit dashboard
pub struct AuditQuery {
    storage: ApprovalAuditStorage,
}

impl AuditQuery {
    pub fn new(storage: ApprovalAuditStorage) -> Self {
        Self { storage }
    }

    /// Calculate risk score based on capabilities
    pub fn calculate_risk_score(
        capabilities: &Capabilities,
        escalation_count: u32,
    ) -> u32 {
        let mut score = 10; // Base score

        // Check filesystem capabilities
        for fs_cap in &capabilities.filesystem {
            if let FileSystemCapability::ReadWrite { .. } = fs_cap {
                score += 20;
            }
        }

        // Check network capability
        match &capabilities.network {
            NetworkCapability::AllowAll => score += 30,
            NetworkCapability::AllowDomains(_) => score += 15,
            NetworkCapability::Deny => {}
        }

        // Check process capability (exec)
        if !capabilities.process.no_fork {
            score += 40;
        }

        // Add escalation penalty
        score += escalation_count * 10;

        score
    }

    /// Get tool risk summary
    pub async fn get_tool_risk_summary(
        &self,
        tool_name: &str,
    ) -> SqliteResult<ToolRiskSummary> {
        // Get execution and escalation counts
        let execution_count = self.storage.get_execution_count(tool_name).await?;
        let escalation_count = self.storage.get_escalation_count(tool_name).await?;
        let last_executed_at = self.storage.get_last_execution_time(tool_name).await?;

        // Get capabilities from database
        let capabilities = self.storage.get_tool_capabilities(tool_name).await?;

        // Parse capabilities into Capabilities struct for risk calculation
        let parsed_capabilities = parse_capabilities_from_strings(&capabilities);

        // Calculate risk score with actual capabilities
        let risk_score = Self::calculate_risk_score(&parsed_capabilities, escalation_count);

        Ok(ToolRiskSummary {
            tool_name: tool_name.to_string(),
            risk_score,
            capabilities,
            execution_count,
            escalation_count,
            last_executed_at,
        })
    }

    /// Get tool execution history
    pub async fn get_tool_execution_history(
        &self,
        tool_name: &str,
        limit: usize,
    ) -> SqliteResult<Vec<ToolExecutionRecord>> {
        let history = self.storage.get_execution_history(tool_name, limit).await?;

        let mut records = Vec::new();
        for (execution_id, timestamp, params_json, escalation_triggered) in history {
            // Parse parameters from JSON
            let parameters: HashMap<String, String> =
                serde_json::from_str(&params_json).unwrap_or_default();

            // Get escalation details if triggered
            let (escalation_reason, user_decision) = if escalation_triggered {
                if let Some((reason_str, _path, decision)) =
                    self.storage.get_escalation_details(&execution_id).await? {
                    let reason = parse_escalation_reason(&reason_str);
                    (Some(reason), decision)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

            records.push(ToolExecutionRecord {
                execution_id,
                tool_name: tool_name.to_string(),
                timestamp,
                parameters,
                escalation_triggered,
                escalation_reason,
                user_decision,
            });
        }

        Ok(records)
    }

    /// Get all escalations
    pub async fn get_all_escalations(
        &self,
        limit: usize,
    ) -> SqliteResult<Vec<ToolExecutionRecord>> {
        let escalations = self.storage.get_all_escalations(limit).await?;

        let mut records = Vec::new();
        for (execution_id, tool_name, timestamp, reason_str, _path, user_decision) in escalations {
            let escalation_reason = parse_escalation_reason(&reason_str);

            records.push(ToolExecutionRecord {
                execution_id,
                tool_name,
                timestamp,
                parameters: HashMap::new(), // Parameters not included in escalations query
                escalation_triggered: true,
                escalation_reason: Some(escalation_reason),
                user_decision,
            });
        }

        Ok(records)
    }
}

/// Parse escalation reason from string
fn parse_escalation_reason(reason: &str) -> EscalationReason {
    match reason {
        "path_out_of_scope" => EscalationReason::PathOutOfScope,
        "sensitive_directory" => EscalationReason::SensitiveDirectory,
        "undeclared_binding" => EscalationReason::UndeclaredBinding,
        "first_execution" => EscalationReason::FirstExecution,
        _ => EscalationReason::FirstExecution, // Default fallback
    }
}

/// Parse capabilities from string representations
fn parse_capabilities_from_strings(capability_strings: &[String]) -> Capabilities {
    use std::path::PathBuf;

    let mut capabilities = Capabilities::default();

    for cap_str in capability_strings {
        if let Some(fs_type) = cap_str.strip_prefix("filesystem.") {
            // Skip "filesystem."
            match fs_type {
                "read_write" => {
                    capabilities.filesystem.push(FileSystemCapability::ReadWrite {
                        path: PathBuf::from("/tmp"), // Placeholder path
                    });
                }
                "read_only" => {
                    capabilities.filesystem.push(FileSystemCapability::ReadOnly {
                        path: PathBuf::from("/tmp"), // Placeholder path
                    });
                }
                _ => {}
            }
        } else if let Some(net_type) = cap_str.strip_prefix("network.") {
            // Skip "network."
            match net_type {
                "allow_all" => {
                    capabilities.network = NetworkCapability::AllowAll;
                }
                "allow_domains" => {
                    capabilities.network = NetworkCapability::AllowDomains(vec![]);
                }
                "deny" => {
                    capabilities.network = NetworkCapability::Deny;
                }
                _ => {}
            }
        } else if cap_str == "process.exec" {
            capabilities.process.no_fork = false;
        }
    }

    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::capabilities::{
        EnvironmentCapability, ProcessCapability,
    };
    use std::path::PathBuf;

    #[test]
    fn test_risk_score_base() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 0);
        assert_eq!(score, 10, "Base score should be 10");
    }

    #[test]
    fn test_risk_score_filesystem() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::ReadWrite {
                path: PathBuf::from("/tmp"),
            }],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 0);
        assert_eq!(score, 30, "ReadWrite filesystem adds 20 points");
    }

    #[test]
    fn test_risk_score_network_all() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::AllowAll,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 0);
        assert_eq!(score, 40, "AllowAll network adds 30 points");
    }

    #[test]
    fn test_risk_score_network_domains() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::AllowDomains(vec![
                "example.com".to_string(),
            ]),
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 0);
        assert_eq!(score, 25, "AllowDomains network adds 15 points");
    }

    #[test]
    fn test_risk_score_exec() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: false, // Allow fork/exec
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 0);
        assert_eq!(score, 50, "Exec capability adds 40 points");
    }

    #[test]
    fn test_risk_score_escalations() {
        let caps = Capabilities::default();
        let score = AuditQuery::calculate_risk_score(&caps, 3);
        assert_eq!(score, 40, "3 escalations add 30 points to base 10");
    }

    #[test]
    fn test_risk_score_combined() {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::ReadWrite {
                path: PathBuf::from("/tmp"),
            }],
            network: NetworkCapability::AllowAll,
            process: ProcessCapability {
                no_fork: false,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        let score = AuditQuery::calculate_risk_score(&caps, 2);
        // Base: 10 + FS: 20 + Network: 30 + Exec: 40 + Escalations: 20 = 120
        assert_eq!(score, 120, "Combined risk score");
    }

    #[tokio::test]
    async fn test_get_tool_risk_summary() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();
        let audit = AuditQuery::new(storage);

        // Get summary for non-existent tool
        let summary = audit.get_tool_risk_summary("test_tool").await.unwrap();
        assert_eq!(summary.tool_name, "test_tool");
        assert_eq!(summary.execution_count, 0);
        assert_eq!(summary.escalation_count, 0);
        assert_eq!(summary.last_executed_at, None);
    }

    #[tokio::test]
    async fn test_get_tool_risk_summary_with_capabilities() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Insert test data with high-risk capabilities
        let capabilities_json = r#"{
            "filesystem": [{"type": "read_write", "path": "/tmp"}],
            "network": "allow_all",
            "process": {"no_fork": false, "max_execution_time": 300, "max_memory_mb": 512},
            "environment": "restricted"
        }"#;

        // Insert data using test helpers
        storage
            .insert_test_capability_approval("risky_tool", capabilities_json, 1234567890)
            .await
            .unwrap();

        // Add some escalations
        storage
            .insert_test_escalation("risky_tool", "exec1", "path_out_of_scope", 1234567891)
            .await
            .unwrap();

        storage
            .insert_test_escalation("risky_tool", "exec2", "sensitive_directory", 1234567892)
            .await
            .unwrap();

        let audit = AuditQuery::new(storage);

        // Get summary
        let summary = audit.get_tool_risk_summary("risky_tool").await.unwrap();
        assert_eq!(summary.tool_name, "risky_tool");
        assert_eq!(summary.escalation_count, 2);

        // Risk score should be: Base(10) + ReadWrite(20) + AllowAll(30) + Exec(40) + Escalations(20) = 120
        assert_eq!(summary.risk_score, 120);

        // Check capabilities are present
        assert!(!summary.capabilities.is_empty());
        assert!(summary.capabilities.contains(&"filesystem.read_write".to_string()));
        assert!(summary.capabilities.contains(&"network.allow_all".to_string()));
        assert!(summary.capabilities.contains(&"process.exec".to_string()));
    }

    #[tokio::test]
    async fn test_get_tool_execution_history_empty() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();
        let audit = AuditQuery::new(storage);

        // Get history for non-existent tool
        let history = audit
            .get_tool_execution_history("test_tool", 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn test_get_all_escalations_empty() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();
        let audit = AuditQuery::new(storage);

        // Get all escalations when none exist
        let escalations = audit.get_all_escalations(10).await.unwrap();
        assert_eq!(escalations.len(), 0);
    }

    #[test]
    fn test_parse_escalation_reason() {
        use super::parse_escalation_reason;

        assert_eq!(
            parse_escalation_reason("path_out_of_scope"),
            EscalationReason::PathOutOfScope
        );
        assert_eq!(
            parse_escalation_reason("sensitive_directory"),
            EscalationReason::SensitiveDirectory
        );
        assert_eq!(
            parse_escalation_reason("undeclared_binding"),
            EscalationReason::UndeclaredBinding
        );
        assert_eq!(
            parse_escalation_reason("first_execution"),
            EscalationReason::FirstExecution
        );
    }
}

