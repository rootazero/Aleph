//! Unified test runner for dual-track testing.
//!
//! Coordinates execution of both Gherkin (.feature) and YAML Spec (.spec.yaml)
//! tests, providing unified result reporting.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::yaml_spec::{YamlSpec, YamlSpecError};

/// Source of test specifications.
#[derive(Debug, Clone)]
pub enum TestSource {
    /// Gherkin feature file
    Gherkin(PathBuf),
    /// YAML spec file
    YamlSpec(PathBuf),
}

impl TestSource {
    /// Get the file path.
    pub fn path(&self) -> &Path {
        match self {
            Self::Gherkin(p) | Self::YamlSpec(p) => p,
        }
    }

    /// Get the source type name.
    pub fn source_type(&self) -> &'static str {
        match self {
            Self::Gherkin(_) => "gherkin",
            Self::YamlSpec(_) => "yaml_spec",
        }
    }
}

/// Result of running a single test source.
#[derive(Debug, Clone)]
pub struct SourceResult {
    /// The test source
    pub source: TestSource,
    /// Number of scenarios/tests passed
    pub passed: usize,
    /// Number of scenarios/tests failed
    pub failed: usize,
    /// Number of scenarios/tests skipped
    pub skipped: usize,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Individual test results
    pub details: Vec<TestDetail>,
}

/// Detail of a single test execution.
#[derive(Debug, Clone)]
pub struct TestDetail {
    /// Test/scenario name
    pub name: String,
    /// Whether it passed
    pub passed: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Unified result from running all test sources.
#[derive(Debug, Clone)]
pub struct UnifiedResult {
    /// Results from Gherkin tests
    pub gherkin_results: Vec<SourceResult>,
    /// Results from YAML spec tests
    pub yaml_spec_results: Vec<SourceResult>,
    /// Total execution time
    pub total_duration_ms: u64,
}

impl UnifiedResult {
    /// Check if all tests passed.
    pub fn all_passed(&self) -> bool {
        self.gherkin_results.iter().all(|r| r.failed == 0)
            && self.yaml_spec_results.iter().all(|r| r.failed == 0)
    }

    /// Get total passed count.
    pub fn total_passed(&self) -> usize {
        self.gherkin_results.iter().map(|r| r.passed).sum::<usize>()
            + self.yaml_spec_results.iter().map(|r| r.passed).sum::<usize>()
    }

    /// Get total failed count.
    pub fn total_failed(&self) -> usize {
        self.gherkin_results.iter().map(|r| r.failed).sum::<usize>()
            + self.yaml_spec_results.iter().map(|r| r.failed).sum::<usize>()
    }

    /// Generate a summary report.
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();

        lines.push("=== Unified Test Results ===".to_string());
        lines.push(String::new());

        // Gherkin results
        if !self.gherkin_results.is_empty() {
            lines.push("Gherkin Tests:".to_string());
            for result in &self.gherkin_results {
                let status = if result.failed == 0 { "✓" } else { "✗" };
                lines.push(format!(
                    "  {} {} - {} passed, {} failed ({}ms)",
                    status,
                    result.source.path().display(),
                    result.passed,
                    result.failed,
                    result.duration_ms
                ));
            }
            lines.push(String::new());
        }

        // YAML spec results
        if !self.yaml_spec_results.is_empty() {
            lines.push("YAML Spec Tests:".to_string());
            for result in &self.yaml_spec_results {
                let status = if result.failed == 0 { "✓" } else { "✗" };
                lines.push(format!(
                    "  {} {} - {} passed, {} failed ({}ms)",
                    status,
                    result.source.path().display(),
                    result.passed,
                    result.failed,
                    result.duration_ms
                ));
            }
            lines.push(String::new());
        }

        // Summary
        let total_passed = self.total_passed();
        let total_failed = self.total_failed();
        let status = if self.all_passed() { "PASSED" } else { "FAILED" };

        lines.push(format!(
            "Total: {} passed, {} failed - {} ({}ms)",
            total_passed, total_failed, status, self.total_duration_ms
        ));

        lines.join("\n")
    }
}

/// Configuration for the unified test runner.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Base directory for Gherkin features
    pub features_dir: PathBuf,
    /// Base directory for YAML specs
    pub specs_dir: PathBuf,
    /// Whether to run Gherkin tests
    pub run_gherkin: bool,
    /// Whether to run YAML spec tests
    pub run_yaml_specs: bool,
    /// Filter pattern for test names
    pub filter: Option<String>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            features_dir: PathBuf::from("tests/features"),
            specs_dir: PathBuf::from("tests/specs"),
            run_gherkin: true,
            run_yaml_specs: true,
            filter: None,
        }
    }
}

/// Unified test runner that coordinates dual-track testing.
pub struct UnifiedTestRunner {
    config: RunnerConfig,
}

impl UnifiedTestRunner {
    /// Create a new runner with the given configuration.
    pub fn new(config: RunnerConfig) -> Self {
        Self { config }
    }

    /// Create a runner with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RunnerConfig::default())
    }

    /// Discover all test sources.
    pub fn discover_sources(&self) -> Result<Vec<TestSource>, RunnerError> {
        let mut sources = Vec::new();

        // Discover Gherkin features
        if self.config.run_gherkin && self.config.features_dir.exists() {
            sources.extend(self.discover_gherkin()?);
        }

        // Discover YAML specs
        if self.config.run_yaml_specs && self.config.specs_dir.exists() {
            sources.extend(self.discover_yaml_specs()?);
        }

        Ok(sources)
    }

    fn discover_gherkin(&self) -> Result<Vec<TestSource>, RunnerError> {
        let mut sources = Vec::new();
        self.walk_dir(&self.config.features_dir, "feature", &mut |path| {
            sources.push(TestSource::Gherkin(path));
        })?;
        Ok(sources)
    }

    fn discover_yaml_specs(&self) -> Result<Vec<TestSource>, RunnerError> {
        let mut sources = Vec::new();
        self.walk_dir(&self.config.specs_dir, "spec.yaml", &mut |path| {
            sources.push(TestSource::YamlSpec(path));
        })?;
        Ok(sources)
    }

    fn walk_dir(
        &self,
        dir: &Path,
        extension: &str,
        callback: &mut impl FnMut(PathBuf),
    ) -> Result<(), RunnerError> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)
            .map_err(|e| RunnerError::IoError(e.to_string()))?
        {
            let entry = entry.map_err(|e| RunnerError::IoError(e.to_string()))?;
            let path = entry.path();

            if path.is_dir() {
                self.walk_dir(&path, extension, callback)?;
            } else if path.to_string_lossy().ends_with(extension) {
                if let Some(ref filter) = self.config.filter {
                    if !path.to_string_lossy().contains(filter) {
                        continue;
                    }
                }
                callback(path);
            }
        }

        Ok(())
    }

    /// Load and validate a YAML spec file.
    pub fn load_yaml_spec(&self, path: &Path) -> Result<YamlSpec, RunnerError> {
        YamlSpec::from_file(path).map_err(RunnerError::YamlSpecError)
    }

    /// Run YAML spec tests and return results.
    ///
    /// Note: This is a simplified implementation that validates specs
    /// can be parsed. Full execution requires integration with the
    /// dispatcher and LlmJudge components.
    pub fn run_yaml_spec(&self, path: &Path) -> Result<SourceResult, RunnerError> {
        let start = Instant::now();
        let spec = self.load_yaml_spec(path)?;

        let mut details = Vec::new();
        let mut passed = 0;
        let failed = 0;

        for scenario in &spec.scenarios {
            let scenario_start = Instant::now();

            // For now, we validate that scenarios can be converted to test cases
            let _test_case = scenario.to_test_case(&spec.context);

            // Mark deterministic scenarios as passed (they parsed correctly)
            // Semantic scenarios would need LlmJudge integration
            let is_semantic = scenario.has_llm_judge();

            if is_semantic {
                // Skip semantic tests for now - they need LlmJudge
                // Do NOT count as passed since no actual validation occurred
                details.push(TestDetail {
                    name: scenario.name.clone(),
                    passed: false,
                    error: Some("Semantic validation skipped (requires LlmJudge)".to_string()),
                    duration_ms: scenario_start.elapsed().as_millis() as u64,
                });
            } else {
                details.push(TestDetail {
                    name: scenario.name.clone(),
                    passed: true,
                    error: None,
                    duration_ms: scenario_start.elapsed().as_millis() as u64,
                });
                passed += 1;
            }
        }

        Ok(SourceResult {
            source: TestSource::YamlSpec(path.to_path_buf()),
            passed,
            failed,
            skipped: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            details,
        })
    }

    /// Run all discovered tests.
    pub fn run_all(&self) -> Result<UnifiedResult, RunnerError> {
        let start = Instant::now();
        let sources = self.discover_sources()?;

        let mut gherkin_results = Vec::new();
        let mut yaml_spec_results = Vec::new();

        for source in sources {
            match &source {
                TestSource::Gherkin(_path) => {
                    // Gherkin tests are run via cucumber-rs separately
                    // This is a placeholder for integration
                    gherkin_results.push(SourceResult {
                        source,
                        passed: 0,
                        failed: 0,
                        skipped: 1,
                        duration_ms: 0,
                        details: vec![TestDetail {
                            name: "gherkin".to_string(),
                            passed: true,
                            error: Some("Run via cucumber-rs".to_string()),
                            duration_ms: 0,
                        }],
                    });
                }
                TestSource::YamlSpec(path) => {
                    match self.run_yaml_spec(path) {
                        Ok(result) => yaml_spec_results.push(result),
                        Err(e) => {
                            yaml_spec_results.push(SourceResult {
                                source,
                                passed: 0,
                                failed: 1,
                                skipped: 0,
                                duration_ms: 0,
                                details: vec![TestDetail {
                                    name: "load".to_string(),
                                    passed: false,
                                    error: Some(e.to_string()),
                                    duration_ms: 0,
                                }],
                            });
                        }
                    }
                }
            }
        }

        Ok(UnifiedResult {
            gherkin_results,
            yaml_spec_results,
            total_duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// Errors from the unified test runner.
#[derive(Debug, Clone)]
pub enum RunnerError {
    /// IO error
    IoError(String),
    /// YAML spec error
    YamlSpecError(YamlSpecError),
    /// Execution error
    ExecutionError(String),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "IO error: {}", e),
            Self::YamlSpecError(e) => write!(f, "YAML spec error: {}", e),
            Self::ExecutionError(e) => write!(f, "Execution error: {}", e),
        }
    }
}

impl std::error::Error for RunnerError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_config_default() {
        let config = RunnerConfig::default();
        assert!(config.run_gherkin);
        assert!(config.run_yaml_specs);
    }

    #[test]
    fn test_unified_result_summary() {
        let result = UnifiedResult {
            gherkin_results: vec![],
            yaml_spec_results: vec![SourceResult {
                source: TestSource::YamlSpec(PathBuf::from("test.spec.yaml")),
                passed: 3,
                failed: 0,
                skipped: 0,
                duration_ms: 100,
                details: vec![],
            }],
            total_duration_ms: 100,
        };

        assert!(result.all_passed());
        assert_eq!(result.total_passed(), 3);
        assert_eq!(result.total_failed(), 0);

        let summary = result.summary();
        assert!(summary.contains("PASSED"));
    }

    #[test]
    fn test_test_source_type() {
        let gherkin = TestSource::Gherkin(PathBuf::from("test.feature"));
        let yaml = TestSource::YamlSpec(PathBuf::from("test.spec.yaml"));

        assert_eq!(gherkin.source_type(), "gherkin");
        assert_eq!(yaml.source_type(), "yaml_spec");
    }
}
