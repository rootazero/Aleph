//! Task Analyzer - Pre-analyze user input for single/multi-step tasks
//!
//! This module provides the TaskAnalyzer component that determines whether
//! a user's request should be handled as a single-step task (using Agent Loop)
//! or as a multi-step task (using DAG scheduling).

use serde::Deserialize;
use crate::sync_primitives::Arc;
use tracing::{debug, info, warn};

use crate::config::GenerationConfig;
use crate::dispatcher::planner::GenerationProviders;
use crate::generation::GenerationType;
use crate::utils::json_extract::extract_json_robust;

/// Multi-step indicator patterns
const MULTI_STEP_PATTERNS: &[&str] = &[
    // Chinese patterns
    "然后",
    "之后",
    "接着",
    "最后",
    "步骤",
    "分步",
    "依次",
    "首先",
    "其次",
    // English patterns (case will be checked insensitively)
    "first",
    "then",
    "after",
    "finally",
    "next",
    "step",
    "following",
    // Symbol patterns
    "→",
    "->",
    "=>",
];

/// Minimum length threshold for single-step heuristic
const SINGLE_STEP_LENGTH_THRESHOLD: usize = 10;

use crate::dispatcher::agent_types::TaskGraph;
use crate::dispatcher::planner::{LlmTaskPlanner, TaskPlanner};
use crate::error::Result;
use crate::providers::AiProvider;

/// Result of task analysis
#[derive(Debug, Clone)]
pub enum AnalysisResult {
    /// Single-step task, use Agent Loop directly
    SingleStep {
        /// Extracted intent from the input
        intent: String,
    },
    /// Multi-step task, needs DAG scheduling
    MultiStep {
        /// Generated task graph for execution
        task_graph: TaskGraph,
        /// Whether user confirmation is required (due to high-risk tasks)
        requires_confirmation: bool,
    },
}

/// Task analyzer that determines if input requires multi-step execution
pub struct TaskAnalyzer {
    provider: Arc<dyn AiProvider>,
    planner: LlmTaskPlanner,
    generation_config: Option<GenerationConfig>,
}

impl TaskAnalyzer {
    /// Create a new task analyzer
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            planner: LlmTaskPlanner::new(provider.clone()),
            provider,
            generation_config: None,
        }
    }

    /// Create a new task analyzer with generation config for provider-aware planning
    pub fn with_generation_config(
        provider: Arc<dyn AiProvider>,
        generation_config: GenerationConfig,
    ) -> Self {
        Self {
            planner: LlmTaskPlanner::new(provider.clone()),
            provider,
            generation_config: Some(generation_config),
        }
    }

    /// Analyze user input to determine execution strategy
    pub async fn analyze(&self, input: &str) -> Result<AnalysisResult> {
        info!("Analyzing input for task complexity: {}", input);

        // Quick heuristic check first
        if self.is_likely_single_step(input) {
            debug!("Heuristic: likely single-step task");
            return Ok(AnalysisResult::SingleStep {
                intent: input.to_string(),
            });
        }

        // Use LLM to determine if multi-step is needed
        let analysis_prompt = self.build_analysis_prompt(input);
        let response = self
            .provider
            .process(&analysis_prompt, Some(ANALYSIS_SYSTEM_PROMPT))
            .await?;

        self.parse_analysis_response(&response, input).await
    }

    /// Quick heuristic to skip LLM call for obvious single-step tasks
    pub fn is_likely_single_step(&self, input: &str) -> bool {
        let input_lower = input.to_lowercase();

        // Check for multi-step patterns first
        for pattern in MULTI_STEP_PATTERNS {
            if input_lower.contains(&pattern.to_lowercase()) {
                return false;
            }
        }

        // If no multi-step patterns found, use length heuristic
        // Short inputs without patterns are definitely single-step
        let len = input.chars().count();
        if len < SINGLE_STEP_LENGTH_THRESHOLD {
            return true;
        }

        // Medium/long inputs without patterns are also likely single-step
        true
    }

    /// Build GenerationProviders from config for the planner
    fn build_generation_providers(&self) -> GenerationProviders {
        let Some(config) = &self.generation_config else {
            debug!("build_generation_providers: generation_config is None");
            return GenerationProviders::default();
        };

        debug!(
            "build_generation_providers: generation_config has {} providers",
            config.providers.len()
        );

        let mut providers = GenerationProviders::default();

        // Image providers
        let image_providers_from_config = config.get_providers_for_type(GenerationType::Image);
        debug!(
            "build_generation_providers: found {} image providers from config",
            image_providers_from_config.len()
        );

        for (name, provider_config) in image_providers_from_config {
            debug!(
                "build_generation_providers: processing image provider '{}', model={:?}, models_keys={:?}",
                name, provider_config.model, provider_config.models.keys().collect::<Vec<_>>()
            );
            let mut models = Vec::new();
            // Add default model if set
            if let Some(ref model) = provider_config.model {
                models.push(model.clone());
            }
            // Add all model aliases
            models.extend(provider_config.models.keys().cloned());
            if !models.is_empty() {
                debug!(
                    "build_generation_providers: adding image provider '{}' with models {:?}",
                    name, models
                );
                providers.image.push((name.to_string(), models));
            }
        }

        // Video providers
        for (name, provider_config) in config.get_providers_for_type(GenerationType::Video) {
            let mut models = Vec::new();
            if let Some(ref model) = provider_config.model {
                models.push(model.clone());
            }
            models.extend(provider_config.models.keys().cloned());
            if !models.is_empty() {
                providers.video.push((name.to_string(), models));
            }
        }

        // Audio providers
        for (name, provider_config) in config.get_providers_for_type(GenerationType::Audio) {
            let mut models = Vec::new();
            if let Some(ref model) = provider_config.model {
                models.push(model.clone());
            }
            models.extend(provider_config.models.keys().cloned());
            if !models.is_empty() {
                providers.audio.push((name.to_string(), models));
            }
        }

        debug!(
            "build_generation_providers: final result - image={}, video={}, audio={}",
            providers.image.len(),
            providers.video.len(),
            providers.audio.len()
        );

        providers
    }

    fn build_analysis_prompt(&self, input: &str) -> String {
        format!(
            r#"分析以下用户请求，判断是否需要多步骤执行：

用户请求: "{}"

如果任务可以一步完成（如：回答问题、简单翻译、单个工具调用），返回：
{{"type": "single", "intent": "简短描述意图"}}

如果任务需要多个步骤（如：分析A然后用结果做B），返回：
{{"type": "multi", "tasks": [
  {{"id": "t1", "name": "步骤名称", "description": "详细描述", "deps": [], "risk": "low"}},
  {{"id": "t2", "name": "步骤名称", "description": "详细描述", "deps": ["t1"], "risk": "low"}}
]}}

risk 值: "low"（分析、生成文本） 或 "high"（调用API、执行代码、修改文件）

只返回 JSON，不要其他文字。"#,
            input
        )
    }

    async fn parse_analysis_response(
        &self,
        response: &str,
        original_input: &str,
    ) -> Result<AnalysisResult> {
        let json_value = match extract_json_robust(response) {
            Some(v) => v,
            None => {
                warn!("No JSON found in analysis response, falling back to SingleStep");
                return Ok(AnalysisResult::SingleStep {
                    intent: original_input.to_string(),
                });
            }
        };
        let parsed: AnalysisResponse = match serde_json::from_value(json_value) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse analysis JSON: {}, falling back to SingleStep", e);
                return Ok(AnalysisResult::SingleStep {
                    intent: original_input.to_string(),
                });
            }
        };

        match parsed {
            AnalysisResponse::Single { intent } => Ok(AnalysisResult::SingleStep { intent }),
            AnalysisResponse::Multi { tasks } => {
                // Use planner with providers if available to ensure correct provider names
                let providers = self.build_generation_providers();
                let has_providers = !providers.image.is_empty()
                    || !providers.video.is_empty()
                    || !providers.audio.is_empty();

                info!(
                    "parse_analysis_response: has_providers={}, image={}, video={}, audio={}",
                    has_providers,
                    providers.image.len(),
                    providers.video.len(),
                    providers.audio.len()
                );

                let task_graph = if !has_providers {
                    info!("parse_analysis_response: using plan() without providers");
                    self.planner.plan(original_input).await?
                } else {
                    info!(
                        "parse_analysis_response: using plan_with_providers() with {:?}",
                        providers.image
                    );
                    self.planner
                        .plan_with_providers(original_input, &providers)
                        .await?
                };

                let requires_confirmation = tasks.iter().any(|t| t.risk == "high");

                Ok(AnalysisResult::MultiStep {
                    task_graph,
                    requires_confirmation,
                })
            }
        }
    }

    /// Create a mock analyzer for testing
    #[cfg(test)]
    pub fn new_mock() -> Self {
        use crate::providers::MockProvider;
        // Mock response for single-step by default
        let provider = Arc::new(MockProvider::new(
            r#"{"type": "single", "intent": "test intent"}"#,
        ));
        Self::new(provider)
    }

    /// Create a mock analyzer with multi-step response for testing
    #[cfg(test)]
    pub fn new_mock_multi() -> Self {
        use crate::providers::MockProvider;
        // Mock response that returns multi-step
        let provider = Arc::new(MockProvider::new(
            r#"{"type": "multi", "tasks": [
                {"id": "t1", "name": "Task 1", "deps": [], "risk": "low"},
                {"id": "t2", "name": "Task 2", "deps": ["t1"], "risk": "high"}
            ]}"#,
        ));
        Self::new(provider)
    }
}

const ANALYSIS_SYSTEM_PROMPT: &str = r#"你是一个任务分析器。分析用户请求，判断是单步任务还是多步任务。
只返回 JSON 格式响应，不要其他文字。"#;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnalysisResponse {
    Single { intent: String },
    Multi { tasks: Vec<TaskDef> },
}

#[derive(Debug, Deserialize)]
struct TaskDef {
    #[allow(dead_code)] // Deserialized from LLM JSON response
    id: String,
    #[allow(dead_code)] // Deserialized from LLM JSON response
    name: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from LLM JSON response
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from LLM JSON response
    deps: Vec<String>,
    #[serde(default = "default_risk")]
    risk: String,
}

fn default_risk() -> String {
    "low".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_single_step_short_input() {
        let analyzer = TaskAnalyzer::new_mock();

        // Short inputs
        assert!(analyzer.is_likely_single_step("你好"));
        assert!(analyzer.is_likely_single_step("What time is it?"));
        assert!(analyzer.is_likely_single_step("Hello"));
    }

    #[test]
    fn test_is_likely_single_step_no_patterns() {
        let analyzer = TaskAnalyzer::new_mock();

        // Longer inputs without multi-step patterns
        assert!(analyzer.is_likely_single_step("请帮我翻译这段英文文本成中文"));
        assert!(analyzer.is_likely_single_step("Explain the concept of machine learning"));
    }

    #[test]
    fn test_is_likely_single_step_with_chinese_patterns() {
        let analyzer = TaskAnalyzer::new_mock();

        // Multi-step indicators in Chinese
        assert!(!analyzer.is_likely_single_step("分析这个文档，然后生成摘要"));
        assert!(!analyzer.is_likely_single_step("首先读取文件，之后分析内容"));
        assert!(!analyzer.is_likely_single_step("请依次完成这几个任务"));
        assert!(!analyzer.is_likely_single_step("接着处理第二个步骤"));
    }

    #[test]
    fn test_is_likely_single_step_with_english_patterns() {
        let analyzer = TaskAnalyzer::new_mock();

        // Multi-step indicators in English
        assert!(!analyzer.is_likely_single_step("First read the file, then analyze it"));
        assert!(!analyzer.is_likely_single_step("After downloading, process the data"));
        assert!(!analyzer.is_likely_single_step("Step 1: read, Step 2: analyze"));
    }

    #[test]
    fn test_is_likely_single_step_with_symbol_patterns() {
        let analyzer = TaskAnalyzer::new_mock();

        // Multi-step indicators with symbols
        assert!(!analyzer.is_likely_single_step("分析文档 → 生成摘要 → 保存结果"));
        assert!(!analyzer.is_likely_single_step("read file -> analyze -> output"));
        assert!(!analyzer.is_likely_single_step("parse => transform => render"));
    }

    #[test]
    fn test_extract_json_direct() {
        let response = r#"{"type": "single", "intent": "test"}"#;
        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["type"], "single");
    }

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the analysis:

```json
{"type": "single", "intent": "translate text"}
```

Let me know if you need more details."#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["intent"], "translate text");
    }

    #[test]
    fn test_extract_json_generic_code_block() {
        let response = r#"Analysis result:

```
{"type": "multi", "tasks": []}
```

Done."#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["type"], "multi");
    }

    #[test]
    fn test_extract_json_embedded() {
        let response = r#"Based on my analysis: {"type": "single", "intent": "answer question"} That's my conclusion."#;
        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["type"], "single");
    }

    #[test]
    fn test_extract_json_plain_text_fallback() {
        // Plain text (no JSON) should return None from extract_json_robust
        let response = "No JSON here, just text";
        let result = extract_json_robust(response);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_analyze_returns_single_step() {
        let analyzer = TaskAnalyzer::new_mock();
        let result = analyzer.analyze("What is the weather today?").await;

        // Should return SingleStep from mock (heuristic triggers for short input)
        match result {
            Ok(AnalysisResult::SingleStep { intent }) => {
                // Heuristic returns input as intent
                assert!(intent.contains("weather"));
            }
            _ => panic!("Expected SingleStep result"),
        }
    }

    #[test]
    fn test_analysis_response_parsing_single() {
        let json = r#"{"type": "single", "intent": "translate this text"}"#;
        let parsed: AnalysisResponse = serde_json::from_str(json).unwrap();

        match parsed {
            AnalysisResponse::Single { intent } => {
                assert_eq!(intent, "translate this text");
            }
            _ => panic!("Expected Single variant"),
        }
    }

    #[test]
    fn test_analysis_response_parsing_multi() {
        let json = r#"{"type": "multi", "tasks": [
            {"id": "t1", "name": "Analyze document", "deps": [], "risk": "low"},
            {"id": "t2", "name": "Generate summary", "deps": ["t1"], "risk": "high"}
        ]}"#;
        let parsed: AnalysisResponse = serde_json::from_str(json).unwrap();

        match parsed {
            AnalysisResponse::Multi { tasks } => {
                assert_eq!(tasks.len(), 2);
                assert_eq!(tasks[0].id, "t1");
                assert_eq!(tasks[1].risk, "high");
            }
            _ => panic!("Expected Multi variant"),
        }
    }

    #[test]
    fn test_analysis_response_default_risk() {
        let json = r#"{"type": "multi", "tasks": [
            {"id": "t1", "name": "Task without risk field", "deps": []}
        ]}"#;
        let parsed: AnalysisResponse = serde_json::from_str(json).unwrap();

        match parsed {
            AnalysisResponse::Multi { tasks } => {
                assert_eq!(tasks[0].risk, "low"); // default value
            }
            _ => panic!("Expected Multi variant"),
        }
    }
}
