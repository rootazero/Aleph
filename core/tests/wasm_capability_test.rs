//! Integration test for WASM capability kernel end-to-end.
//!
//! Verifies that the capability model, kernel, and leak detector
//! work together correctly as a security enforcement stack.

#[cfg(feature = "plugin-wasm")]
mod tests {
    // Use public re-exports from the wasm module
    use alephcore::extension::runtime::wasm::{
        WasmCapabilities, WasmCapabilityKernel, WasmResourceLimits,
        WorkspaceCapability, HttpCapability, EndpointPattern,
    };
    use alephcore::exec::leak_detector::LeakDetector;

    #[test]
    fn test_full_capability_lifecycle() {
        // 1. Plugin with http + workspace capabilities
        let caps = WasmCapabilities {
            workspace: Some(WorkspaceCapability {
                allowed_prefixes: vec!["data/".to_string()],
            }),
            http: Some(HttpCapability {
                allowlist: vec![EndpointPattern {
                    host: "api.example.com".to_string(),
                    path_prefix: "/v1/".to_string(),
                    methods: vec!["GET".to_string()],
                }],
                credentials: vec![],
                rate_limit: None,
                timeout_secs: 30,
                max_request_bytes: 1_048_576,
                max_response_bytes: 10_485_760,
            }),
            tool_invoke: None,
            secrets: None,
        };

        let kernel = WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        );

        // 2. Workspace read — allowed
        assert!(kernel.check_workspace_read("data/input.json").is_ok());

        // 3. Workspace read — denied (wrong prefix)
        assert!(kernel.check_workspace_read("secrets/key.pem").is_err());

        // 4. Workspace read — denied (traversal)
        assert!(kernel.check_workspace_read("data/../secrets/key").is_err());

        // 5. Tool invoke — denied (not declared)
        assert!(kernel.resolve_tool_alias("anything").is_err());

        // 6. Log — works (always allowed)
        assert!(kernel.log("info", "test message").is_ok());

        // 7. Clock — works (always allowed)
        assert!(kernel.now_millis() > 0);
    }

    #[test]
    fn test_resource_limits_enforcement() {
        let limits = WasmResourceLimits {
            max_http_calls: 3,
            max_tool_invokes: 2,
            max_log_entries: 5,
            ..Default::default()
        };

        let kernel = WasmCapabilityKernel::new(
            "limited-plugin".to_string(),
            WasmCapabilities::default(),
            limits,
        );

        // HTTP limits
        assert!(kernel.check_http_limit().is_ok());
        assert!(kernel.check_http_limit().is_ok());
        assert!(kernel.check_http_limit().is_ok());
        assert!(kernel.check_http_limit().is_err()); // 4th call exceeds limit

        // Tool invoke limits
        assert!(kernel.check_tool_invoke_limit().is_ok());
        assert!(kernel.check_tool_invoke_limit().is_ok());
        assert!(kernel.check_tool_invoke_limit().is_err()); // 3rd call exceeds limit

        // Log limits
        for i in 0..5 {
            assert!(kernel.log("info", &format!("msg {}", i)).is_ok());
        }
        assert!(kernel.log("info", "one too many").is_err());
    }

    #[test]
    fn test_leak_detector_integration() {
        let detector = LeakDetector::default_patterns();

        // Outbound with API key → blocked
        let result = detector.scan_outbound("key=sk-abcdefghijklmnopqrstuvwxyz12345");
        assert!(result.has_blocks());

        // Clean outbound → passes
        let result = detector.scan_outbound("Hello, this is clean content");
        assert!(result.is_clean());

        // Inbound with leaked key → blocked
        let result = detector.scan_inbound("response: sk-ant-api03-secret1234567890abcdef");
        assert!(result.has_blocks());
    }

    #[test]
    fn test_default_deny_no_capabilities() {
        let kernel = WasmCapabilityKernel::new(
            "locked-down".to_string(),
            WasmCapabilities::default(),
            WasmResourceLimits::default(),
        );

        // Everything should be denied
        assert!(kernel.check_workspace_read("anything").is_err());
        assert!(!kernel.check_secret_pattern("anything"));
        assert!(kernel.resolve_tool_alias("anything").is_err());

        // But log and clock always work (no capability needed)
        assert!(kernel.log("info", "still works").is_ok());
        assert!(kernel.now_millis() > 0);
    }
}
