//! Audit command handlers for tool risk analysis and execution history
//!
//! Provides CLI commands for querying audit data:
//! - `audit tools` - List all tools with risk scores
//! - `audit tool <name>` - Show detailed tool info and execution history
//! - `audit escalations` - Show all escalation events

use alephcore::exec::approval::audit::{AuditQuery, ToolExecutionRecord};
use alephcore::exec::approval::storage::ApprovalAuditStorage;
use std::path::PathBuf;

/// ANSI color codes for risk levels
const COLOR_RESET: &str = "\x1b[0m";
const COLOR_GREEN: &str = "\x1b[32m";
const COLOR_YELLOW: &str = "\x1b[33m";
const COLOR_RED: &str = "\x1b[31m";
const COLOR_BOLD: &str = "\x1b[1m";

/// Risk level classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// Classify risk score into level
    pub fn from_score(score: u32) -> Self {
        match score {
            0..=30 => RiskLevel::Low,
            31..=70 => RiskLevel::Medium,
            _ => RiskLevel::High,
        }
    }

    /// Get color code for risk level
    pub fn color(&self) -> &'static str {
        match self {
            RiskLevel::Low => COLOR_GREEN,
            RiskLevel::Medium => COLOR_YELLOW,
            RiskLevel::High => COLOR_RED,
        }
    }

    /// Get display name for risk level
    pub fn name(&self) -> &'static str {
        match self {
            RiskLevel::Low => "LOW",
            RiskLevel::Medium => "MEDIUM",
            RiskLevel::High => "HIGH",
        }
    }
}

/// Get default database path
fn get_audit_db_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".aleph").join("approval_audit.db")
}

/// Format timestamp as human-readable string
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(timestamp, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => "<invalid timestamp>".to_string(),
    }
}

/// Handle audit tools command - list all tools with risk scores
pub async fn handle_audit_tools() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = get_audit_db_path();

    if !db_path.exists() {
        println!("No audit data found. Database does not exist at: {}", db_path.display());
        return Ok(());
    }

    let storage = ApprovalAuditStorage::new(&db_path).await?;
    let audit = AuditQuery::new(storage);

    // Get all tool names from the database
    let tool_names = get_all_tool_names(&db_path).await?;

    if tool_names.is_empty() {
        println!("No tools found in audit database");
        return Ok(());
    }

    // Print header
    println!("\n{}Tool Risk Summary{}", COLOR_BOLD, COLOR_RESET);
    println!("{}", "=".repeat(100));
    println!(
        "{:<30} {:<12} {:<15} {:<15} {:<20}",
        "TOOL NAME", "RISK LEVEL", "RISK SCORE", "EXECUTIONS", "ESCALATIONS"
    );
    println!("{}", "-".repeat(100));

    // Get and display risk summary for each tool
    for tool_name in tool_names {
        let summary = audit.get_tool_risk_summary(&tool_name).await?;
        let risk_level = RiskLevel::from_score(summary.risk_score);

        println!(
            "{:<30} {}{:<12}{} {:<15} {:<15} {:<20}",
            summary.tool_name,
            risk_level.color(),
            risk_level.name(),
            COLOR_RESET,
            summary.risk_score,
            summary.execution_count,
            summary.escalation_count
        );
    }

    println!();
    Ok(())
}

/// Handle audit tool command - show detailed tool info and execution history
pub async fn handle_audit_tool(tool_name: &str, limit: usize) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = get_audit_db_path();

    if !db_path.exists() {
        eprintln!("Error: No audit data found. Database does not exist at: {}", db_path.display());
        std::process::exit(1);
    }

    let storage = ApprovalAuditStorage::new(&db_path).await?;
    let audit = AuditQuery::new(storage);

    // Get tool risk summary
    let summary = audit.get_tool_risk_summary(tool_name).await?;
    let risk_level = RiskLevel::from_score(summary.risk_score);

    // Print tool summary
    println!("\n{}Tool: {}{}", COLOR_BOLD, tool_name, COLOR_RESET);
    println!("{}", "=".repeat(80));
    println!("Risk Level:      {}{}{}", risk_level.color(), risk_level.name(), COLOR_RESET);
    println!("Risk Score:      {}", summary.risk_score);
    println!("Executions:      {}", summary.execution_count);
    println!("Escalations:     {}", summary.escalation_count);

    if let Some(last_exec) = summary.last_executed_at {
        println!("Last Executed:   {}", format_timestamp(last_exec));
    } else {
        println!("Last Executed:   Never");
    }

    // Print capabilities
    if !summary.capabilities.is_empty() {
        println!("\n{}Capabilities:{}", COLOR_BOLD, COLOR_RESET);
        for cap in &summary.capabilities {
            println!("  - {}", cap);
        }
    }

    // Get and print execution history
    let history = audit.get_tool_execution_history(tool_name, limit).await?;

    if !history.is_empty() {
        println!("\n{}Execution History (last {}):{}", COLOR_BOLD, limit, COLOR_RESET);
        println!("{}", "-".repeat(80));

        for record in history {
            print_execution_record(&record);
        }
    } else {
        println!("\nNo execution history found");
    }

    println!();
    Ok(())
}

/// Handle audit escalations command - show all escalation events
pub async fn handle_audit_escalations(limit: usize) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = get_audit_db_path();

    if !db_path.exists() {
        println!("No audit data found. Database does not exist at: {}", db_path.display());
        return Ok(());
    }

    let storage = ApprovalAuditStorage::new(&db_path).await?;
    let audit = AuditQuery::new(storage);

    // Get all escalations
    let escalations = audit.get_all_escalations(limit).await?;

    if escalations.is_empty() {
        println!("No escalations found");
        return Ok(());
    }

    // Print header
    println!("\n{}Escalation Events (last {}){}", COLOR_BOLD, limit, COLOR_RESET);
    println!("{}", "=".repeat(100));

    for record in escalations {
        print_execution_record(&record);
    }

    println!();
    Ok(())
}

/// Print a single execution record
fn print_execution_record(record: &ToolExecutionRecord) {
    println!("\n{}Tool:{} {}", COLOR_BOLD, COLOR_RESET, record.tool_name);
    println!("  Execution ID:  {}", record.execution_id);
    println!("  Timestamp:     {}", format_timestamp(record.timestamp));

    if record.escalation_triggered {
        println!("  {}Escalation:    YES{}", COLOR_RED, COLOR_RESET);
        if let Some(ref reason) = record.escalation_reason {
            println!("  Reason:        {:?}", reason);
        }
        if let Some(ref decision) = record.user_decision {
            println!("  User Decision: {}", decision);
        }
    } else {
        println!("  Escalation:    No");
    }

    if !record.parameters.is_empty() {
        println!("  Parameters:");
        for (key, value) in &record.parameters {
            println!("    {}: {}", key, value);
        }
    }
}

/// Helper function to get all tool names from database
async fn get_all_tool_names(db_path: &PathBuf) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use rusqlite::Connection;

    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT tool_name FROM tool_executions
         UNION
         SELECT DISTINCT tool_name FROM capability_escalations
         ORDER BY tool_name"
    )?;

    let tool_names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tool_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_from_score() {
        assert_eq!(RiskLevel::from_score(0), RiskLevel::Low);
        assert_eq!(RiskLevel::from_score(15), RiskLevel::Low);
        assert_eq!(RiskLevel::from_score(30), RiskLevel::Low);
        assert_eq!(RiskLevel::from_score(31), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_score(50), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_score(70), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_score(71), RiskLevel::High);
        assert_eq!(RiskLevel::from_score(100), RiskLevel::High);
        assert_eq!(RiskLevel::from_score(200), RiskLevel::High);
    }

    #[test]
    fn test_risk_level_color() {
        assert_eq!(RiskLevel::Low.color(), COLOR_GREEN);
        assert_eq!(RiskLevel::Medium.color(), COLOR_YELLOW);
        assert_eq!(RiskLevel::High.color(), COLOR_RED);
    }

    #[test]
    fn test_risk_level_name() {
        assert_eq!(RiskLevel::Low.name(), "LOW");
        assert_eq!(RiskLevel::Medium.name(), "MEDIUM");
        assert_eq!(RiskLevel::High.name(), "HIGH");
    }

    #[test]
    fn test_format_timestamp() {
        // Test with a known timestamp: 2026-02-09 12:00:00 UTC
        let timestamp = 1770835200;
        let formatted = format_timestamp(timestamp);
        // Just check it contains expected parts (exact format depends on local timezone)
        assert!(formatted.contains("2026"));
    }

    #[test]
    fn test_get_audit_db_path() {
        let path = get_audit_db_path();
        assert!(path.to_string_lossy().contains(".aleph"));
        assert!(path.to_string_lossy().contains("approval_audit.db"));
    }

    #[tokio::test]
    async fn test_get_all_tool_names_empty() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_audit.db");

        // Create storage (this creates the tables)
        let _storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Get tool names from empty database
        let tool_names = get_all_tool_names(&db_path).await.unwrap();
        assert_eq!(tool_names.len(), 0);
    }

    #[tokio::test]
    async fn test_audit_query_with_empty_database() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_audit.db");

        // Create storage and audit query
        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();
        let audit = AuditQuery::new(storage);

        // Get summary for non-existent tool
        let summary = audit.get_tool_risk_summary("test_tool").await.unwrap();
        assert_eq!(summary.tool_name, "test_tool");
        assert_eq!(summary.execution_count, 0);
        assert_eq!(summary.escalation_count, 0);
        assert_eq!(summary.last_executed_at, None);

        // Get execution history for non-existent tool
        let history = audit.get_tool_execution_history("test_tool", 10).await.unwrap();
        assert_eq!(history.len(), 0);

        // Get all escalations from empty database
        let escalations = audit.get_all_escalations(10).await.unwrap();
        assert_eq!(escalations.len(), 0);
    }
}
