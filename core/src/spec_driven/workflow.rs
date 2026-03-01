//! SpecDrivenWorkflow - orchestrates the entire spec-driven development cycle.
//!
//! Workflow phases:
//! 1. Generate specification from requirement
//! 2. Generate test cases from specification
//! 3. Execute implementation via supervisor
//! 4. Run tests and evaluate
//! 5. Iterate or finalize

use crate::sync_primitives::Arc;

use tracing::{error, info};

use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use crate::supervisor::{ClaudeSupervisor, SupervisorConfig, SupervisorEvent};

use super::judge::LlmJudge;
use super::spec_writer::SpecWriter;
use super::test_writer::TestWriter;
use super::types::{EvaluationResult, Spec, TestCase, TestResult, WorkflowConfig, WorkflowResult};

/// Callback trait for workflow events.
#[allow(unused_variables)]
pub trait WorkflowCallback: Send + Sync {
    /// Called when workflow starts.
    fn on_start(&self, requirement: &str) {}

    /// Called when spec is generated.
    fn on_spec_ready(&self, spec: &Spec) {}

    /// Called when tests are generated.
    fn on_tests_ready(&self, tests: &[TestCase]) {}

    /// Called when implementation phase starts.
    fn on_implementation_start(&self) {}

    /// Called with supervisor output.
    fn on_supervisor_output(&self, output: &str) {}

    /// Called when evaluation is complete.
    fn on_evaluation(&self, result: &EvaluationResult) {}

    /// Called when iteration starts.
    fn on_iteration(&self, iteration: u32, feedback: &str) {}

    /// Called when workflow completes.
    fn on_complete(&self, result: &WorkflowResult) {}
}

/// No-op callback for testing.
pub struct NoOpWorkflowCallback;

impl WorkflowCallback for NoOpWorkflowCallback {}

/// The spec-driven development workflow orchestrator.
pub struct SpecDrivenWorkflow {
    spec_writer: SpecWriter,
    test_writer: TestWriter,
    judge: LlmJudge,
    config: WorkflowConfig,
    callback: Arc<dyn WorkflowCallback>,
}

impl SpecDrivenWorkflow {
    /// Create a new workflow with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            spec_writer: SpecWriter::new(provider.clone()),
            test_writer: TestWriter::new(provider.clone()),
            judge: LlmJudge::new(provider),
            config: WorkflowConfig::default(),
            callback: Arc::new(NoOpWorkflowCallback),
        }
    }

    /// Set workflow configuration.
    pub fn with_config(mut self, config: WorkflowConfig) -> Self {
        self.config = config;
        self
    }

    /// Set callback handler.
    pub fn with_callback(mut self, callback: Arc<dyn WorkflowCallback>) -> Self {
        self.callback = callback;
        self
    }

    /// Run the complete workflow for a requirement.
    pub async fn run(&self, requirement: &str, workspace: &str) -> Result<WorkflowResult> {
        info!(requirement = %requirement, workspace = %workspace, "Starting workflow");
        self.callback.on_start(requirement);

        // Phase 1: Generate Spec
        let spec = self.spec_writer.generate(requirement).await?;
        self.callback.on_spec_ready(&spec);

        // Phase 2: Generate Tests
        let tests = self.test_writer.generate(&spec).await?;
        self.callback.on_tests_ready(&tests);

        // Phase 3-5: Implementation cycle
        let mut iteration = 0;
        let mut last_evaluation: Option<EvaluationResult> = None;
        let mut feedback = String::new();

        while iteration < self.config.max_iterations {
            iteration += 1;
            info!(iteration = iteration, "Starting implementation iteration");

            if iteration > 1 {
                self.callback.on_iteration(iteration, &feedback);
            }

            // Phase 3: Implement
            self.callback.on_implementation_start();
            let impl_result = self
                .implement(&spec, &tests, workspace, &feedback)
                .await?;

            // Phase 4: Test & Evaluate
            let test_results = self.run_tests(&tests, workspace).await?;
            let evaluation = self
                .judge
                .evaluate(&spec, &tests, &test_results, &impl_result)
                .await?;

            self.callback.on_evaluation(&evaluation);
            last_evaluation = Some(evaluation.clone());

            // Phase 5: Check if acceptable
            if evaluation.score >= self.config.min_score && evaluation.is_acceptable {
                let result = WorkflowResult::Success {
                    spec: Box::new(spec.clone()),
                    tests,
                    evaluation: Box::new(evaluation),
                };
                self.callback.on_complete(&result);
                return Ok(result);
            }

            // Prepare feedback for next iteration
            feedback = format!(
                "Previous attempt scored {:.0}%. Issues:\n{}\n\nSuggestions:\n{}",
                evaluation.score * 100.0,
                evaluation.feedback,
                evaluation.suggestions.join("\n")
            );
        }

        // Failed after max iterations
        let result = WorkflowResult::Failed {
            reason: format!(
                "Failed to meet acceptance criteria after {} iterations",
                self.config.max_iterations
            ),
            last_evaluation,
        };
        self.callback.on_complete(&result);
        Ok(result)
    }

    /// Implement the spec via supervisor.
    async fn implement(
        &self,
        spec: &Spec,
        tests: &[TestCase],
        workspace: &str,
        feedback: &str,
    ) -> Result<String> {
        // Build implementation prompt
        let prompt = self.build_implementation_prompt(spec, tests, feedback);

        // Create supervisor
        let config = SupervisorConfig::new(workspace)
            .with_command("claude")
            .with_args(vec!["--print".into()]);

        let mut supervisor = ClaudeSupervisor::new(config);
        let rx = supervisor.spawn().map_err(|e| AlephError::Other {
            message: format!("Supervisor spawn failed: {}", e),
            suggestion: Some("Ensure Claude CLI is installed".to_string()),
        })?;

        // Send prompt
        supervisor.writeln(&prompt).map_err(|e| AlephError::Other {
            message: format!("Supervisor write failed: {}", e),
            suggestion: None,
        })?;

        // Collect output
        let mut output = String::new();
        let mut rx = rx;

        while let Some(event) = rx.recv().await {
            match event {
                SupervisorEvent::Output(line) => {
                    self.callback.on_supervisor_output(&line);
                    output.push_str(&line);
                    output.push('\n');
                }
                SupervisorEvent::Exited(_) => break,
                SupervisorEvent::Error(e) => {
                    error!(error = %e, "Supervisor error");
                }
                _ => {}
            }
        }

        Ok(output)
    }

    /// Build implementation prompt.
    fn build_implementation_prompt(
        &self,
        spec: &Spec,
        tests: &[TestCase],
        feedback: &str,
    ) -> String {
        let criteria = spec
            .acceptance_criteria
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n");

        let test_names = tests
            .iter()
            .map(|t| format!("- {}", t.name))
            .collect::<Vec<_>>()
            .join("\n");

        let feedback_section = if feedback.is_empty() {
            String::new()
        } else {
            format!("\n## Previous Feedback\n{}\n", feedback)
        };

        format!(
            r#"Implement the following specification:

## Specification
Title: {}
Description: {}

## Acceptance Criteria
{}

## Tests to Pass
{}

## Target
Language: {}
Output: {}
{}
Please implement this and ensure all tests pass."#,
            spec.title,
            spec.description,
            criteria,
            test_names,
            spec.target.language,
            spec.target
                .output_path
                .as_deref()
                .unwrap_or("appropriate location"),
            feedback_section
        )
    }

    /// Run tests (placeholder - actual implementation depends on language).
    async fn run_tests(&self, tests: &[TestCase], _workspace: &str) -> Result<Vec<TestResult>> {
        // For now, return placeholder results
        // Real implementation would run actual tests based on target language
        Ok(tests
            .iter()
            .map(|t| TestResult {
                test_name: t.name.clone(),
                passed: true, // Placeholder
                actual_output: None,
                error: None,
                duration_ms: 0,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_config_default() {
        let config = WorkflowConfig::default();
        assert_eq!(config.max_iterations, 3);
        assert_eq!(config.min_score, 0.8);
    }

    #[test]
    fn test_build_implementation_prompt() {
        let spec = Spec::new("id", "Test", "Test spec")
            .with_criterion("Must work")
            .with_language("rust");

        let tests = vec![TestCase::new("test_it", "Test it works")];

        let workflow = SpecDrivenWorkflow::new(Arc::new(MockProvider));
        let prompt = workflow.build_implementation_prompt(&spec, &tests, "");

        assert!(prompt.contains("Test"));
        assert!(prompt.contains("Must work"));
        assert!(prompt.contains("test_it"));
        assert!(prompt.contains("rust"));
    }

    struct MockProvider;

    impl crate::providers::AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            Box::pin(async { Ok("{}".to_string()) })
        }

        fn process_with_thinking(
            &self,
            input: &str,
            system_prompt: Option<&str>,
            _level: crate::agents::thinking::ThinkLevel,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn process_with_image(
            &self,
            input: &str,
            _image: Option<&crate::ImageData>,
            system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn process_with_attachments(
            &self,
            input: &str,
            _attachments: Option<&[crate::core::MediaAttachment]>,
            system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "gray"
        }
    }
}
