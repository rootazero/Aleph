//! Integration tests for the complete approval workflow
//!
//! Tests end-to-end flows for capability approval, trust stage transitions,
//! runtime escalation, and multi-tool approval scenarios.

use alephcore::exec::approval::audit::AuditQuery;
use alephcore::exec::approval::binding::check_binding_compliance;
use alephcore::exec::approval::escalation::check_path_escalation;
use alephcore::exec::approval::storage::ApprovalAuditStorage;
use alephcore::exec::approval::types::{
    CapabilityApprovalRequest, EscalationReason, TrustStage,
};
use alephcore::exec::sandbox::capabilities::{
    Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability,
    ProcessCapability,
};
use alephcore::exec::sandbox::parameter_binding::{
    CapabilityOverrides, RequiredCapabilities,
};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper: Create test capabilities with filesystem access
fn create_test_capabilities(path: &str) -> Capabilities {
    Capabilities {
        filesystem: vec![FileSystemCapability::ReadWrite {
            path: PathBuf::from(path),
        }],
        network: NetworkCapability::Deny,
        process: ProcessCapability {
            no_fork: true,
            max_execution_time: 300,
            max_memory_mb: Some(512),
        },
        environment: EnvironmentCapability::Restricted,
    }
}

/// Helper: Create test required capabilities
fn create_required_capabilities(preset: &str, description: &str) -> RequiredCapabilities {
    RequiredCapabilities {
        base_preset: preset.to_string(),
        description: description.to_string(),
        overrides: CapabilityOverrides::default(),
        parameter_bindings: HashMap::new(),
    }
}

/// Helper: Create approval request
fn create_approval_request(
    tool_name: &str,
    capabilities: Capabilities,
    stage: TrustStage,
) -> CapabilityApprovalRequest {
    CapabilityApprovalRequest {
        tool_name: tool_name.to_string(),
        tool_description: format!("Test tool: {}", tool_name),
        required_capabilities: create_required_capabilities("file_processor", "Process files"),
        resolved_capabilities: capabilities,
        trust_stage: stage,
    }
}

/// Scenario 1: Draft → Trial → Verified Flow
///
/// This test simulates the complete lifecycle of a tool from initial generation
/// to verified status through user approval and execution.
#[tokio::test]
async fn test_draft_to_verified_flow() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Setup: Create storage and audit query
    let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

    // Step 1: Tool generated in Draft stage
    let tool_name = "file_processor";
    let capabilities = create_test_capabilities("/tmp");
    let request = create_approval_request(tool_name, capabilities.clone(), TrustStage::Draft);

    assert_eq!(request.trust_stage, TrustStage::Draft);
    assert_eq!(request.tool_name, tool_name);

    // Step 2: User approves with Session scope (Draft → Trial)
    // In real implementation, this would be handled by approval manager
    // Here we simulate the state transition
    let trial_request = create_approval_request(tool_name, capabilities.clone(), TrustStage::Trial);
    assert_eq!(trial_request.trust_stage, TrustStage::Trial);

    // Step 3: First execution shows preview (Trial stage)
    // Simulate execution by recording it in the database
    let execution_id = "exec_001";
    let params_json = r#"{"file_path": "/tmp/test.txt"}"#;
    let timestamp = chrono::Utc::now().timestamp();

    storage
        .insert_test_execution(tool_name, execution_id, params_json, timestamp)
        .await
        .unwrap();

    // Verify execution was recorded
    let execution_count = storage.get_execution_count(tool_name).await.unwrap();
    assert_eq!(execution_count, 1, "First execution should be recorded");

    // Step 4: User confirms, tool moves to Verified
    let verified_request =
        create_approval_request(tool_name, capabilities.clone(), TrustStage::Verified);
    assert_eq!(verified_request.trust_stage, TrustStage::Verified);

    // Step 5: Subsequent executions are silent (no preview)
    let execution_id_2 = "exec_002";
    storage
        .insert_test_execution(tool_name, execution_id_2, params_json, timestamp + 10)
        .await
        .unwrap();

    let execution_count = storage.get_execution_count(tool_name).await.unwrap();
    assert_eq!(
        execution_count, 2,
        "Subsequent executions should be recorded"
    );

    // Verify: All state transitions correct
    let audit = AuditQuery::new(storage);
    let summary = audit.get_tool_risk_summary(tool_name).await.unwrap();
    assert_eq!(summary.tool_name, tool_name);
    assert_eq!(summary.execution_count, 2);
    assert_eq!(summary.escalation_count, 0);
}

/// Scenario 2: Runtime Escalation Flow
///
/// This test simulates a tool in Verified stage that encounters a runtime
/// parameter exceeding its approved scope, triggering escalation.
#[tokio::test]
async fn test_runtime_escalation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Setup: Create storage
    let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

    // Step 1: Tool in Verified stage with approved paths
    let tool_name = "file_reader";
    let approved_paths = vec!["/tmp/*".to_string()];

    // Record initial approval
    let capabilities_json = r#"{
        "filesystem": [{"type": "read_only", "path": "/tmp"}],
        "network": "deny",
        "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool_name, capabilities_json, chrono::Utc::now().timestamp())
        .await
        .unwrap();

    // Step 2: Execute with out-of-scope parameter
    let mut runtime_params = HashMap::new();
    runtime_params.insert("file_path".to_string(), "/etc/passwd".to_string());

    // Step 3: Verify escalation triggered
    let escalation = check_path_escalation(&runtime_params, &approved_paths);
    assert!(
        escalation.is_some(),
        "Escalation should be triggered for out-of-scope path"
    );

    let trigger = escalation.unwrap();
    assert_eq!(trigger.reason, EscalationReason::PathOutOfScope);
    assert_eq!(
        trigger.requested_path,
        Some(PathBuf::from("/etc/passwd"))
    );

    // Step 4: Record user decision in audit log
    let execution_id = "exec_escalation_001";
    storage
        .insert_test_escalation(
            tool_name,
            execution_id,
            "path_out_of_scope",
            chrono::Utc::now().timestamp(),
        )
        .await
        .unwrap();

    // Step 5: Verify audit log correct
    let escalation_count = storage.get_escalation_count(tool_name).await.unwrap();
    assert_eq!(escalation_count, 1, "Escalation should be recorded");

    let escalation_details = storage
        .get_escalation_details(execution_id)
        .await
        .unwrap();
    assert!(escalation_details.is_some());

    let (reason, _path, _decision) = escalation_details.unwrap();
    assert_eq!(reason, "path_out_of_scope");
}

/// Scenario 3: Multi-Tool Approval
///
/// This test simulates approving multiple tools with different scopes
/// and verifying that approval states persist correctly.
#[tokio::test]
async fn test_multi_tool_approval() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Setup: Create storage
    let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

    let timestamp = chrono::Utc::now().timestamp();

    // Step 1: Create 3 tools with different capabilities
    let tool1 = "file_reader";
    let tool2 = "network_fetcher";
    let tool3 = "process_executor";

    // Tool 1: File reader (Once scope - single use)
    let caps1_json = r#"{
        "filesystem": [{"type": "read_only", "path": "/tmp"}],
        "network": "deny",
        "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool1, caps1_json, timestamp)
        .await
        .unwrap();

    // Tool 2: Network fetcher (Session scope - multiple uses in session)
    let caps2_json = r#"{
        "filesystem": [{"type": "temp_workspace"}],
        "network": "allow_all",
        "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool2, caps2_json, timestamp + 1)
        .await
        .unwrap();

    // Tool 3: Process executor (Permanent scope - always allowed)
    let caps3_json = r#"{
        "filesystem": [{"type": "read_write", "path": "/tmp"}],
        "network": "deny",
        "process": {"no_fork": false, "max_execution_time": 600, "max_memory_mb": 1024},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool3, caps3_json, timestamp + 2)
        .await
        .unwrap();

    // Step 2: Execute all tools
    storage
        .insert_test_execution(tool1, "exec_t1_001", r#"{"file": "/tmp/data.txt"}"#, timestamp + 10)
        .await
        .unwrap();

    storage
        .insert_test_execution(
            tool2,
            "exec_t2_001",
            r#"{"url": "https://example.com"}"#,
            timestamp + 11,
        )
        .await
        .unwrap();

    storage
        .insert_test_execution(
            tool2,
            "exec_t2_002",
            r#"{"url": "https://example.org"}"#,
            timestamp + 12,
        )
        .await
        .unwrap();

    storage
        .insert_test_execution(
            tool3,
            "exec_t3_001",
            r#"{"command": "ls -la"}"#,
            timestamp + 13,
        )
        .await
        .unwrap();

    // Step 3: Verify approval states persist correctly
    let audit = AuditQuery::new(storage);

    let summary1 = audit.get_tool_risk_summary(tool1).await.unwrap();
    assert_eq!(summary1.tool_name, tool1);
    assert_eq!(summary1.execution_count, 1);
    assert!(summary1.capabilities.contains(&"filesystem.read_only".to_string()));

    let summary2 = audit.get_tool_risk_summary(tool2).await.unwrap();
    assert_eq!(summary2.tool_name, tool2);
    assert_eq!(summary2.execution_count, 2, "Session scope allows multiple executions");
    assert!(summary2.capabilities.contains(&"network.allow_all".to_string()));

    let summary3 = audit.get_tool_risk_summary(tool3).await.unwrap();
    assert_eq!(summary3.tool_name, tool3);
    assert_eq!(summary3.execution_count, 1);
    assert!(summary3.capabilities.contains(&"filesystem.read_write".to_string()));
    assert!(summary3.capabilities.contains(&"process.exec".to_string()));

    // Verify risk scores are calculated correctly
    assert!(summary1.risk_score < summary2.risk_score, "File reader should be lower risk than network fetcher");
    assert!(summary2.risk_score < summary3.risk_score, "Network fetcher should be lower risk than process executor");
}

/// Test: Binding compliance check during execution
#[tokio::test]
async fn test_binding_compliance_during_execution() {
    // Setup: Define parameter bindings
    let mut declared_bindings = HashMap::new();
    declared_bindings.insert("file_path".to_string(), "/tmp/*.txt".to_string());
    declared_bindings.insert("output_dir".to_string(), "/tmp/output".to_string());

    // Test 1: Compliant parameters
    let mut runtime_params = HashMap::new();
    runtime_params.insert("file_path".to_string(), "/tmp/data.txt".to_string());
    runtime_params.insert("output_dir".to_string(), "/tmp/output".to_string());

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_ok(), "Compliant parameters should pass");

    // Test 2: Non-compliant file path (wrong extension)
    let mut runtime_params = HashMap::new();
    runtime_params.insert("file_path".to_string(), "/tmp/data.json".to_string());
    runtime_params.insert("output_dir".to_string(), "/tmp/output".to_string());

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_err(), "Non-compliant file extension should fail");
    assert_eq!(result.unwrap_err().reason, EscalationReason::UndeclaredBinding);

    // Test 3: Extra undeclared parameter
    let mut runtime_params = HashMap::new();
    runtime_params.insert("file_path".to_string(), "/tmp/data.txt".to_string());
    runtime_params.insert("output_dir".to_string(), "/tmp/output".to_string());
    runtime_params.insert("extra_param".to_string(), "value".to_string());

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_err(), "Extra undeclared parameter should fail");
}

/// Test: First execution escalation (Trial stage)
#[tokio::test]
async fn test_first_execution_escalation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

    let tool_name = "new_tool";
    let timestamp = chrono::Utc::now().timestamp();

    // Tool in Trial stage - first execution should trigger escalation
    let capabilities_json = r#"{
        "filesystem": [{"type": "read_write", "path": "/tmp"}],
        "network": "deny",
        "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool_name, capabilities_json, timestamp)
        .await
        .unwrap();

    // Check execution count before first execution
    let count_before = storage.get_execution_count(tool_name).await.unwrap();
    assert_eq!(count_before, 0);

    // Record first execution with escalation
    let execution_id = "exec_first";
    storage
        .insert_test_execution(
            tool_name,
            execution_id,
            r#"{"file": "/tmp/test.txt"}"#,
            timestamp + 1,
        )
        .await
        .unwrap();

    storage
        .insert_test_escalation(tool_name, execution_id, "first_execution", timestamp + 1)
        .await
        .unwrap();

    // Verify escalation was recorded
    let escalation_count = storage.get_escalation_count(tool_name).await.unwrap();
    assert_eq!(escalation_count, 1);

    let details = storage
        .get_escalation_details(execution_id)
        .await
        .unwrap();
    assert!(details.is_some());

    let (reason, _, _) = details.unwrap();
    assert_eq!(reason, "first_execution");
}

/// Test: Concurrent tool executions
#[tokio::test]
async fn test_concurrent_tool_executions() {
    use std::sync::Arc;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let storage = Arc::new(ApprovalAuditStorage::new(&db_path).await.unwrap());

    let tool_name = "concurrent_tool";
    let timestamp = chrono::Utc::now().timestamp();

    // Setup tool approval
    let capabilities_json = r#"{
        "filesystem": [{"type": "read_write", "path": "/tmp"}],
        "network": "deny",
        "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
        "environment": "restricted"
    }"#;

    storage
        .insert_test_capability_approval(tool_name, capabilities_json, timestamp)
        .await
        .unwrap();

    // Simulate concurrent executions
    let mut handles = vec![];

    for i in 0..5 {
        let storage_clone = Arc::clone(&storage);
        let exec_id = format!("exec_concurrent_{}", i);
        let params = format!(r#"{{"file": "/tmp/file_{}.txt"}}"#, i);

        let handle = tokio::spawn(async move {
            storage_clone
                .insert_test_execution(tool_name, &exec_id, &params, timestamp + i as i64)
                .await
        });

        handles.push(handle);
    }

    // Wait for all executions to complete
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Verify all executions were recorded
    let execution_count = storage.get_execution_count(tool_name).await.unwrap();
    assert_eq!(execution_count, 5, "All concurrent executions should be recorded");

    // Verify execution history
    // Try to unwrap Arc to get the storage back
    match Arc::try_unwrap(storage) {
        Ok(storage) => {
            let audit = AuditQuery::new(storage);
            let history = audit
                .get_tool_execution_history(tool_name, 10)
                .await
                .unwrap();
            assert_eq!(history.len(), 5);
        }
        Err(_) => {
            panic!("Failed to unwrap Arc - there are still references to storage");
        }
    }
}

/// Test: Audit query for all escalations
#[tokio::test]
async fn test_audit_all_escalations() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

    let timestamp = chrono::Utc::now().timestamp();

    // Create multiple tools with escalations
    let tools = ["tool_a", "tool_b", "tool_c"];

    for (idx, tool_name) in tools.iter().enumerate() {
        // Insert escalations
        storage
            .insert_test_escalation(
                tool_name,
                &format!("exec_{}", idx),
                "path_out_of_scope",
                timestamp + idx as i64,
            )
            .await
            .unwrap();
    }

    // Query all escalations
    let audit = AuditQuery::new(storage);
    let all_escalations = audit.get_all_escalations(10).await.unwrap();
    assert_eq!(all_escalations.len(), 3, "Should retrieve all escalations");

    // Verify escalations are sorted by timestamp (most recent first)
    for (i, record) in all_escalations.iter().enumerate() {
        assert!(record.escalation_triggered);
        assert_eq!(
            record.escalation_reason,
            Some(EscalationReason::PathOutOfScope)
        );

        // Check descending order
        if i > 0 {
            assert!(
                record.timestamp <= all_escalations[i - 1].timestamp,
                "Escalations should be sorted by timestamp descending"
            );
        }
    }
}
