//! Integration tests for sandbox execution
//!
//! Tests end-to-end sandbox execution flow including profile generation,
//! command execution, audit logging, and cleanup.

#[cfg(test)]
mod integration_tests {
    use crate::exec::sandbox::adapter::{SandboxAdapter, SandboxCommand};
    use crate::exec::sandbox::audit::ExecutionStatus;
    use crate::exec::sandbox::capabilities::{
        Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability,
        ProcessCapability,
    };
    use crate::exec::sandbox::executor::{FallbackPolicy, SandboxManager};
    use crate::exec::sandbox::platforms::macos::MacOSSandbox;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_end_to_end_sandbox_execution() {
        // Create sandbox manager
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter);

        // Skip if sandbox not available
        if !manager.is_available() {
            println!("Skipping test: sandbox not available on this platform");
            return;
        }

        // Create simple echo command
        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["Hello from sandbox".to_string()],
            working_dir: None,
        };

        // Execute with default capabilities
        let caps = Capabilities::default();
        let result = manager
            .execute_sandboxed("test-skill-001", command, caps)
            .await;

        // Verify execution succeeded
        assert!(result.is_ok());
        let (exec_result, audit_log) = result.unwrap();

        // Verify execution result
        assert_eq!(exec_result.exit_code, Some(0));
        assert!(exec_result.stdout.contains("Hello from sandbox"));
        assert!(exec_result.sandboxed);

        // Verify audit log
        assert_eq!(audit_log.skill_id, "test-skill-001");
        assert_eq!(audit_log.sandbox_platform, "macos");
        assert!(audit_log.is_success());
        assert!(matches!(
            audit_log.execution_result,
            ExecutionStatus::Success { .. }
        ));
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_sandbox_with_file_system_capabilities() {
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter);

        if !manager.is_available() {
            println!("Skipping test: sandbox not available");
            return;
        }

        // Create capabilities with file system access
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

        // Command that writes to a file
        let command = SandboxCommand {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo 'test data' > test.txt && cat test.txt".to_string(),
            ],
            working_dir: None,
        };

        let result = manager
            .execute_sandboxed("test-skill-002", command, caps)
            .await;

        assert!(result.is_ok());
        let (exec_result, _) = result.unwrap();
        assert_eq!(exec_result.exit_code, Some(0));
        assert!(exec_result.stdout.contains("test data"));
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_audit_log_generation() {
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter);

        if !manager.is_available() {
            println!("Skipping test: sandbox not available");
            return;
        }

        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["audit test".to_string()],
            working_dir: None,
        };

        let caps = Capabilities::default();
        let result = manager
            .execute_sandboxed("audit-test-skill", command, caps.clone())
            .await;

        assert!(result.is_ok());
        let (_, audit_log) = result.unwrap();

        // Verify audit log structure
        assert_eq!(audit_log.skill_id, "audit-test-skill");
        assert_eq!(audit_log.capabilities, caps);
        assert!(audit_log.timestamp > 0);
        assert!(audit_log.violations.is_empty());

        // Verify serialization
        let json = serde_json::to_string(&audit_log).unwrap();
        assert!(json.contains("audit-test-skill"));
        assert!(json.contains("macos"));
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_sandbox_unavailable_deny_policy() {
        // Create a mock adapter that reports as unsupported
        struct UnsupportedAdapter;

        #[async_trait::async_trait]
        impl SandboxAdapter for UnsupportedAdapter {
            fn is_supported(&self) -> bool {
                false
            }

            fn platform_name(&self) -> &str {
                "unsupported"
            }

            fn generate_profile(
                &self,
                _caps: &Capabilities,
            ) -> crate::error::Result<crate::exec::sandbox::adapter::SandboxProfile> {
                unreachable!()
            }

            async fn execute_sandboxed(
                &self,
                _command: &SandboxCommand,
                _profile: &crate::exec::sandbox::adapter::SandboxProfile,
            ) -> crate::error::Result<crate::exec::sandbox::adapter::ExecutionResult> {
                unreachable!()
            }

            fn cleanup(
                &self,
                _profile: &crate::exec::sandbox::adapter::SandboxProfile,
            ) -> crate::error::Result<()> {
                unreachable!()
            }
        }

        let adapter: Arc<dyn SandboxAdapter> = Arc::new(UnsupportedAdapter);
        let manager = SandboxManager::new(adapter).with_fallback_policy(FallbackPolicy::Deny);

        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["test".to_string()],
            working_dir: None,
        };

        let result = manager
            .execute_sandboxed("test-skill", command, Capabilities::default())
            .await;

        // Should fail with SandboxUnavailable error
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            crate::error::AlephError::SandboxUnavailable { .. }
        ));
    }

    #[tokio::test]
    #[cfg(not(target_os = "macos"))]
    async fn test_sandbox_unavailable_on_unsupported_platform() {
        // On non-macOS platforms, sandbox should report as unavailable
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter);

        assert!(!manager.is_available());

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
