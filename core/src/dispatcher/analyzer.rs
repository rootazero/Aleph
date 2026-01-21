//! Task Analyzer - Pre-analyze user input for single/multi-step tasks
//!
//! This module provides the TaskAnalyzer component that determines whether
//! a user's request should be handled as a single-step task (using Agent Loop)
//! or as a multi-step task (using DAG scheduling).

use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, info};

use crate::dispatcher::cowork_types::TaskGraph;
use crate::dispatcher::planner::{LlmTaskPlanner, TaskPlanner};
use crate::error::{AetherError, Result};
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
}

impl TaskAnalyzer {
    /// Create a new task analyzer
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            planner: LlmTaskPlanner::new(provider.clone()),
            provider,
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
        // First check for multi-step indicators (these override length check)
        // Chinese patterns
        let chinese_patterns = [
            "然后", "之后", "接着", "最后", "步骤", "分步", "依次", "首先", "其次",
        ];
        if chinese_patterns.iter().any(|p| input.contains(p)) {
            return false;
        }

        // English patterns (case insensitive)
        let english_patterns = ["first", "then", "after", "finally", "next", "step", "following"];
        let lower_input = input.to_lowercase();
        if english_patterns.iter().any(|p| lower_input.contains(p)) {
            return false;
        }

        // Symbol patterns
        let symbol_patterns = ["→", "->", "=>"];
        if symbol_patterns.iter().any(|p| input.contains(p)) {
            return false;
        }

        // If no multi-step patterns found, use length heuristic
        // Short inputs without patterns are likely single-step
        let len = input.chars().count();
        if len < 10 {
            return true;
        }

        // Medium-length inputs without patterns are likely single-step
        true
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
        let json_str = extract_json(response)?;
        let parsed: AnalysisResponse = serde_json::from_str(&json_str)?;

        match parsed {
            AnalysisResponse::Single { intent } => Ok(AnalysisResult::SingleStep { intent }),
            AnalysisResponse::Multi { tasks } => {
                // Use planner to build proper TaskGraph
                let task_graph = self.planner.plan(original_input).await?;
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
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    deps: Vec<String>,
    #[serde(default = "default_risk")]
    risk: String,
}

fn default_risk() -> String {
    "low".to_string()
}

/// Extract JSON from a response that may be wrapped in markdown code blocks
fn extract_json(response: &str) -> Result<String> {
    let trimmed = response.trim();

    // Try to find JSON in ```json code block
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(end) = trimmed[json_start..].find("```") {
            return Ok(trimmed[json_start..json_start + end].trim().to_string());
        }
    }

    // Try to find JSON in generic ``` code block
    if let Some(start) = trimmed.find("```") {
        let json_start = trimmed[start + 3..].find('\n').map(|n| start + 4 + n);
        if let Some(json_start) = json_start {
            if let Some(end) = trimmed[json_start..].find("```") {
                return Ok(trimmed[json_start..json_start + end].trim().to_string());
            }
        }
    }

    // Try direct JSON
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    // Find first { and last }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return Ok(trimmed[start..=end].to_string());
        }
    }

    Err(AetherError::Other {
        message: "Could not extract JSON from response".to_string(),
        suggestion: Some(
            "The AI did not return a valid task analysis. Try rephrasing your request."
                .to_string(),
        ),
    })
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
        let json = extract_json(response).unwrap();
        assert!(json.contains("single"));
        assert!(json.contains("test"));
    }

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the analysis:

```json
{"type": "single", "intent": "translate text"}
```

Let me know if you need more details."#;

        let json = extract_json(response).unwrap();
        assert!(json.contains("single"));
        assert!(json.contains("translate"));
    }

    #[test]
    fn test_extract_json_generic_code_block() {
        let response = r#"Analysis result:

```
{"type": "multi", "tasks": []}
```

Done."#;

        let json = extract_json(response).unwrap();
        assert!(json.contains("multi"));
    }

    #[test]
    fn test_extract_json_embedded() {
        let response = r#"Based on my analysis: {"type": "single", "intent": "answer question"} That's my conclusion."#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("single"));
    }

    #[test]
    fn test_extract_json_failure() {
        let response = "No JSON here, just text";
        let result = extract_json(response);
        assert!(result.is_err());
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
