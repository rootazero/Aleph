//! Integration tests for Three-Layer Control

#[cfg(test)]
mod integration_tests {
    use crate::three_layer::*;
    use tempfile::TempDir;

    #[test]
    fn test_full_safety_chain() {
        // Setup sandbox
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::with_defaults(vec![temp.path().to_path_buf()]);

        // Setup capability gate
        let gate = CapabilityGate::new(vec![Capability::FileRead, Capability::FileList]);

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
                .with_capabilities(vec![Capability::FileRead]),
        );

        registry.register(
            SkillDefinition::new("writer".to_string(), "Writer".to_string(), "".to_string())
                .with_capabilities(vec![Capability::FileWrite]),
        );

        // Verify capability filtering
        let read_skills = registry.list_by_capability(&Capability::FileRead);
        assert_eq!(read_skills.len(), 1);
        assert_eq!(read_skills[0].id, "reader");
    }
}
