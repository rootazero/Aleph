//! Intent analyzer component - detects intent and complexity for routing.
//!
//! Subscribes to: InputReceived
//! Publishes: PlanRequested or ToolCallRequested
//!
//! # New Architecture (v2)
//!
//! This component now supports two modes via configuration:
//! 1. Legacy mode: Uses IntentClassifier (default)
//! 2. Unified mode: Uses ExecutionIntentDecider (experimental)
//!
//! Set `use_unified_decider = true` in config to enable the new mode.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, InputEvent, PlanRequest,
    ToolCallRequest,
};
use crate::intent::classifier::{ExecutionIntent, IntentClassifier};
use crate::intent::{ContextSignals, ExecutionIntentDecider, ExecutionMode};

use super::Complexity;

// ============================================================================
// Multi-step keyword patterns for complexity detection
// ============================================================================

/// Chinese multi-step keywords
static CHINESE_MULTI_STEP_KEYWORDS: &[&str] = &[
    "然后", "接着", "之后", "并且", "同时", "再", "随后", "最后", "首先", "其次",
];

/// English multi-step keywords
static ENGLISH_MULTI_STEP_KEYWORDS: &[&str] = &[
    "then",
    "after that",
    "and then",
    "also",
    "next",
    "finally",
    "first",
    "second",
    "afterwards",
];

/// Step marker pattern: numbered lists (1. 2. 3.) or bullet points
/// Supports:
/// - Numbered: "1. ", "1、", "1) "
/// - Bullets: "- ", "* ", "• "
static STEP_MARKER_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Note: Using \u{3001} for Chinese enumeration comma (、)
    Regex::new(r"(?m)(^\s*\d+[.\u{3001})]\s*|^\s*[-*•]\s+)").unwrap()
});

/// Chinese action verbs for multi-sentence detection
static CHINESE_ACTION_VERBS: &[&str] = &[
    "创建", "删除", "移动", "复制", "修改", "编辑", "打开", "关闭", "保存", "发送", "下载", "上传",
    "安装", "卸载", "运行", "执行", "搜索", "查找", "整理", "清理", "压缩", "解压", "转换", "导出",
    "导入", "备份", "恢复", "更新", "添加", "生成", "构建", "部署",
];

/// English action verbs for multi-sentence detection
static ENGLISH_ACTION_VERBS: &[&str] = &[
    "create",
    "delete",
    "move",
    "copy",
    "modify",
    "edit",
    "open",
    "close",
    "save",
    "send",
    "download",
    "upload",
    "install",
    "uninstall",
    "run",
    "execute",
    "search",
    "find",
    "organize",
    "clean",
    "compress",
    "extract",
    "convert",
    "export",
    "import",
    "backup",
    "restore",
    "update",
    "add",
    "generate",
    "build",
    "deploy",
];

/// Sentence boundary pattern
static SENTENCE_BOUNDARY: Lazy<Regex> = Lazy::new(|| Regex::new(r"[。！？\.!?;；]").unwrap());

// ============================================================================
// IntentAnalyzer Component
// ============================================================================

/// Intent Analyzer - determines if planning is needed
///
/// This component:
/// - Subscribes to InputReceived events
/// - Analyzes input complexity using multi-step keywords, step markers, and action sentences
/// - Publishes either PlanRequested (for complex requests) or ToolCallRequested (for simple ones)
///
/// # Dual Mode Support
///
/// The analyzer supports two modes:
/// - **Legacy mode** (default): Uses `IntentClassifier` for backward compatibility
/// - **Unified mode**: Uses `ExecutionIntentDecider` for the new architecture
pub struct IntentAnalyzer {
    /// Intent classifier for determining task category and intent type (legacy)
    classifier: IntentClassifier,
    /// Unified execution intent decider (new architecture)
    decider: ExecutionIntentDecider,
    /// Use the unified decider instead of legacy classifier
    use_unified_decider: bool,
}

impl Default for IntentAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentAnalyzer {
    /// Create a new IntentAnalyzer with legacy mode
    pub fn new() -> Self {
        Self {
            classifier: IntentClassifier::new(),
            decider: ExecutionIntentDecider::new(),
            use_unified_decider: false,
        }
    }

    /// Create IntentAnalyzer with unified decider enabled
    pub fn with_unified_decider() -> Self {
        Self {
            classifier: IntentClassifier::new(),
            decider: ExecutionIntentDecider::new(),
            use_unified_decider: true,
        }
    }

    /// Create IntentAnalyzer with a custom classifier (legacy)
    pub fn with_classifier(classifier: IntentClassifier) -> Self {
        Self {
            classifier,
            decider: ExecutionIntentDecider::new(),
            use_unified_decider: false,
        }
    }

    /// Enable or disable unified decider mode
    pub fn set_unified_mode(&mut self, enabled: bool) {
        self.use_unified_decider = enabled;
    }

    /// Check if unified mode is enabled
    pub fn is_unified_mode(&self) -> bool {
        self.use_unified_decider
    }

    /// Get the execution mode using the unified decider
    ///
    /// This is the new recommended way to classify intent.
    pub fn get_execution_mode(&self, text: &str, context: Option<&ContextSignals>) -> ExecutionMode {
        self.decider.decide(text, context).mode
    }

    /// Get full decision result with metadata
    pub fn get_decision_result(
        &self,
        text: &str,
        context: Option<&ContextSignals>,
    ) -> crate::intent::DecisionResult {
        self.decider.decide(text, context)
    }

    // ========================================================================
    // Complexity Detection Methods
    // ========================================================================

    /// Check if text contains multi-step keywords (Chinese or English)
    pub fn has_multi_step_keywords(&self, text: &str) -> bool {
        let text_lower = text.to_lowercase();
        CHINESE_MULTI_STEP_KEYWORDS
            .iter()
            .any(|kw| text.contains(kw))
            || ENGLISH_MULTI_STEP_KEYWORDS
                .iter()
                .any(|kw| text_lower.contains(kw))
    }

    /// Check if text contains multiple sentences with action verbs
    pub fn has_multiple_action_sentences(&self, text: &str) -> bool {
        // Split by sentence boundaries
        let sentences: Vec<&str> = SENTENCE_BOUNDARY.split(text).collect();

        // Filter out empty segments
        let sentences: Vec<&str> = sentences
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();

        if sentences.len() < 2 {
            return false;
        }

        // Count sentences containing action verbs
        let mut action_sentence_count = 0;

        for sentence in &sentences {
            let sentence_lower = sentence.to_lowercase();

            // Check Chinese action verbs
            let has_chinese_verb = CHINESE_ACTION_VERBS.iter().any(|v| sentence.contains(v));

            // Check English action verbs
            let has_english_verb = ENGLISH_ACTION_VERBS
                .iter()
                .any(|v| sentence_lower.contains(v));

            if has_chinese_verb || has_english_verb {
                action_sentence_count += 1;
            }
        }

        // Return true if we have 2 or more action sentences
        action_sentence_count >= 2
    }

    /// Check if text contains step markers (numbered lists or bullet points)
    pub fn has_step_markers(&self, text: &str) -> bool {
        STEP_MARKER_PATTERN.is_match(text)
    }

    /// Analyze text complexity and determine if planning is needed
    ///
    /// Returns Complexity::NeedsPlan if:
    /// - Text contains multi-step keywords, OR
    /// - Text contains step markers (numbered list/bullets), OR
    /// - Text has multiple sentences with action verbs
    ///
    /// Returns Complexity::Simple otherwise
    pub fn analyze_complexity(&self, text: &str, _intent: &ExecutionIntent) -> Complexity {
        // Check for multi-step keywords
        if self.has_multi_step_keywords(text) {
            return Complexity::NeedsPlan;
        }

        // Check for step markers
        if self.has_step_markers(text) {
            return Complexity::NeedsPlan;
        }

        // Check for multiple action sentences
        if self.has_multiple_action_sentences(text) {
            return Complexity::NeedsPlan;
        }

        Complexity::Simple
    }

    /// Extract preliminary steps from text by splitting on multi-step keywords
    ///
    /// This provides a rough step breakdown before LLM-based planning.
    pub fn extract_steps(&self, text: &str) -> Vec<String> {
        let mut result = vec![text.to_string()];

        // Split by Chinese keywords
        for kw in CHINESE_MULTI_STEP_KEYWORDS {
            result = result
                .into_iter()
                .flat_map(|s| {
                    s.split(kw)
                        .filter(|p| !p.trim().is_empty())
                        .map(|p| p.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .collect();
        }

        // Split by English keywords (case-insensitive)
        for kw in ENGLISH_MULTI_STEP_KEYWORDS {
            result = result
                .into_iter()
                .flat_map(|s| split_by_keyword_case_insensitive(&s, kw))
                .collect();
        }

        // Filter empty and very short segments
        result.into_iter().filter(|s| s.len() >= 2).collect()
    }

    /// Build a direct ToolCallRequest for simple requests
    pub fn build_direct_call(
        &self,
        intent: &ExecutionIntent,
        input: &InputEvent,
    ) -> ToolCallRequest {
        match intent {
            ExecutionIntent::Executable(task) => {
                // Build tool parameters inline
                let mut params = serde_json::json!({
                    "action": task.action,
                    "input_text": input.text,
                    "confidence": task.confidence
                });
                if let Some(ref target) = task.target {
                    params["target"] = serde_json::json!(target);
                }
                if let Some(ref ctx) = input.context {
                    if let Some(ref app) = ctx.app_name {
                        params["app_context"] = serde_json::json!(app);
                    }
                    if let Some(ref selected) = ctx.selected_text {
                        params["selected_text"] = serde_json::json!(selected);
                    }
                }

                ToolCallRequest {
                    tool: task.category.as_str().to_string(),
                    parameters: params,
                    plan_step_id: None,
                }
            }
            _ => {
                // For conversational or ambiguous intents, default to general_chat
                ToolCallRequest {
                    tool: "general_chat".to_string(),
                    parameters: serde_json::json!({
                        "input": input.text
                    }),
                    plan_step_id: None,
                }
            }
        }
    }
}

/// Split string by keyword case-insensitively
fn split_by_keyword_case_insensitive(text: &str, keyword: &str) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let keyword_lower = keyword.to_lowercase();

    let mut result = Vec::new();
    let mut start = 0;

    for (idx, _) in text_lower.match_indices(&keyword_lower) {
        if idx > start {
            let segment = text[start..idx].trim();
            if !segment.is_empty() {
                result.push(segment.to_string());
            }
        }
        start = idx + keyword.len();
    }

    // Add remaining text
    if start < text.len() {
        let segment = text[start..].trim();
        if !segment.is_empty() {
            result.push(segment.to_string());
        }
    }

    if result.is_empty() {
        vec![text.to_string()]
    } else {
        result
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for IntentAnalyzer {
    fn name(&self) -> &'static str {
        "IntentAnalyzer"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::InputReceived]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Only handle InputReceived events
        let input = match event {
            AetherEvent::InputReceived(input) => input,
            _ => return Ok(vec![]),
        };

        // Use unified decider if enabled, otherwise fall back to legacy
        if self.use_unified_decider {
            return self.handle_with_unified_decider(input).await;
        }

        // Legacy path: use IntentClassifier
        let intent = self.classifier.classify(&input.text).await;

        // Analyze complexity
        let complexity = self.analyze_complexity(&input.text, &intent);

        match complexity {
            Complexity::NeedsPlan => {
                // Extract preliminary steps for the planner
                let detected_steps = self.extract_steps(&input.text);

                // Create plan request
                let plan_request = PlanRequest {
                    input: input.clone(),
                    intent_type: match &intent {
                        ExecutionIntent::Executable(task) => Some(format!("{:?}", task.category)),
                        _ => None,
                    },
                    detected_steps,
                };

                Ok(vec![AetherEvent::PlanRequested(plan_request)])
            }
            Complexity::Simple => {
                // Build direct tool call
                let tool_call = self.build_direct_call(&intent, input);

                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
        }
    }
}

impl IntentAnalyzer {
    /// Handle input using the unified ExecutionIntentDecider
    ///
    /// This is the new path that uses the cleaner decision architecture.
    async fn handle_with_unified_decider(
        &self,
        input: &InputEvent,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Build context signals from input context
        let context_signals = input.context.as_ref().map(|ctx| ContextSignals {
            selected_file: ctx.selected_text.clone(),
            active_app: ctx.app_name.clone(),
            ui_mode: None,
            clipboard_type: None,
        });

        // Get decision from unified decider
        let decision = self.decider.decide(&input.text, context_signals.as_ref());

        // Log decision metadata for debugging
        tracing::debug!(
            layer = ?decision.metadata.layer,
            confidence = decision.metadata.confidence,
            latency_us = decision.metadata.latency_us,
            "ExecutionIntentDecider decision"
        );

        match decision.mode {
            ExecutionMode::DirectTool(invocation) => {
                // Direct tool call from slash command - bypass planning entirely
                let tool_call = ToolCallRequest {
                    tool: invocation.tool_id,
                    parameters: serde_json::json!({
                        "args": invocation.args,
                        "input_text": input.text,
                    }),
                    plan_step_id: None,
                };
                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
            ExecutionMode::Skill(skill) => {
                // Skill command - run agent with skill instructions injected
                let tool_call = ToolCallRequest {
                    tool: "agent_with_skill".to_string(),
                    parameters: serde_json::json!({
                        "skill_id": skill.skill_id,
                        "skill_name": skill.display_name,
                        "instructions": skill.instructions,
                        "args": skill.args,
                        "input_text": input.text,
                    }),
                    plan_step_id: None,
                };
                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
            ExecutionMode::Mcp(mcp) => {
                // MCP command - route to MCP server
                let tool_call = ToolCallRequest {
                    tool: format!("mcp:{}", mcp.server_name),
                    parameters: serde_json::json!({
                        "server_name": mcp.server_name,
                        "tool_name": mcp.tool_name,
                        "args": mcp.args,
                        "input_text": input.text,
                    }),
                    plan_step_id: None,
                };
                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
            ExecutionMode::Custom(custom) => {
                // Custom command - run agent with custom system prompt
                let tool_call = ToolCallRequest {
                    tool: "agent_with_prompt".to_string(),
                    parameters: serde_json::json!({
                        "command_name": custom.command_name,
                        "system_prompt": custom.system_prompt,
                        "provider": custom.provider,
                        "args": custom.args,
                        "input_text": input.text,
                    }),
                    plan_step_id: None,
                };
                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
            ExecutionMode::Execute(category) => {
                // Determine if planning is needed based on complexity
                let complexity = self.analyze_complexity_for_category(&input.text, category);

                match complexity {
                    Complexity::NeedsPlan => {
                        let detected_steps = self.extract_steps(&input.text);
                        let plan_request = PlanRequest {
                            input: input.clone(),
                            intent_type: Some(category.as_str().to_string()),
                            detected_steps,
                        };
                        Ok(vec![AetherEvent::PlanRequested(plan_request)])
                    }
                    Complexity::Simple => {
                        let tool_call = ToolCallRequest {
                            tool: category.as_str().to_string(),
                            parameters: serde_json::json!({
                                "action": input.text,
                                "input_text": input.text,
                                "category": category.as_str(),
                            }),
                            plan_step_id: None,
                        };
                        Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
                    }
                }
            }
            ExecutionMode::Converse => {
                // Pure conversation - use general_chat tool
                let tool_call = ToolCallRequest {
                    tool: "general_chat".to_string(),
                    parameters: serde_json::json!({
                        "input": input.text,
                        "mode": "conversation",
                    }),
                    plan_step_id: None,
                };
                Ok(vec![AetherEvent::ToolCallRequested(tool_call)])
            }
        }
    }

    /// Analyze complexity for a given task category
    fn analyze_complexity_for_category(
        &self,
        text: &str,
        _category: crate::intent::TaskCategory,
    ) -> Complexity {
        // Use existing complexity detection logic
        // This checks for multi-step keywords, step markers, etc.
        if self.has_multi_step_keywords(text) {
            return Complexity::NeedsPlan;
        }
        if self.has_step_markers(text) {
            return Complexity::NeedsPlan;
        }
        if self.has_multiple_action_sentences(text) {
            return Complexity::NeedsPlan;
        }
        Complexity::Simple
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::InputContext;
    use crate::intent::classifier::ExecutableTask;

    fn create_test_input(text: &str) -> InputEvent {
        InputEvent {
            text: text.to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    // ========================================================================
    // Multi-step Keyword Detection Tests
    // ========================================================================

    #[test]
    fn test_multi_step_detection_chinese() {
        let analyzer = IntentAnalyzer::new();

        // Test all Chinese keywords
        assert!(analyzer.has_multi_step_keywords("打开文件然后复制内容"));
        assert!(analyzer.has_multi_step_keywords("先查找文件，接着进行编辑"));
        assert!(analyzer.has_multi_step_keywords("执行脚本之后保存结果"));
        assert!(analyzer.has_multi_step_keywords("下载文件并且解压"));
        assert!(analyzer.has_multi_step_keywords("同时处理多个任务"));
        assert!(analyzer.has_multi_step_keywords("运行程序再检查输出"));
        assert!(analyzer.has_multi_step_keywords("创建目录随后添加文件"));
        assert!(analyzer.has_multi_step_keywords("首先备份数据"));
        assert!(analyzer.has_multi_step_keywords("其次检查权限"));
        assert!(analyzer.has_multi_step_keywords("最后清理缓存"));

        // No keywords
        assert!(!analyzer.has_multi_step_keywords("打开文件"));
        assert!(!analyzer.has_multi_step_keywords("删除这个文件夹"));
    }

    #[test]
    fn test_multi_step_detection_english() {
        let analyzer = IntentAnalyzer::new();

        // Test all English keywords (case-insensitive)
        assert!(analyzer.has_multi_step_keywords("open file then save it"));
        assert!(analyzer.has_multi_step_keywords("read the content THEN process it"));
        assert!(analyzer.has_multi_step_keywords("download file and then extract"));
        assert!(analyzer.has_multi_step_keywords("delete temp files also clean cache"));
        assert!(analyzer.has_multi_step_keywords("First create directory"));
        assert!(analyzer.has_multi_step_keywords("second, add the file"));
        assert!(analyzer.has_multi_step_keywords("next, run the script"));
        assert!(analyzer.has_multi_step_keywords("finally check the result"));
        assert!(analyzer.has_multi_step_keywords("open file after that edit it"));
        assert!(analyzer.has_multi_step_keywords("save the file afterwards"));

        // No keywords
        assert!(!analyzer.has_multi_step_keywords("open file"));
        assert!(!analyzer.has_multi_step_keywords("delete folder"));
    }

    // ========================================================================
    // Step Extraction Tests
    // ========================================================================

    #[test]
    fn test_step_extraction() {
        let analyzer = IntentAnalyzer::new();

        // Chinese multi-step
        let steps = analyzer.extract_steps("打开文件然后复制内容接着保存");
        assert!(
            steps.len() >= 2,
            "Expected at least 2 steps, got {:?}",
            steps
        );

        // English multi-step
        let steps = analyzer.extract_steps("download the file then extract it and then process");
        assert!(
            steps.len() >= 2,
            "Expected at least 2 steps, got {:?}",
            steps
        );

        // Mixed
        let steps = analyzer.extract_steps("首先打开文件 then edit content 最后保存");
        assert!(
            steps.len() >= 2,
            "Expected at least 2 steps, got {:?}",
            steps
        );

        // No keywords - should return original text
        let steps = analyzer.extract_steps("打开文件");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0], "打开文件");
    }

    // ========================================================================
    // Complexity Analysis Tests
    // ========================================================================

    #[test]
    fn test_complexity_simple() {
        let analyzer = IntentAnalyzer::new();
        let intent = ExecutionIntent::Conversational;

        // Simple single-action requests
        assert_eq!(
            analyzer.analyze_complexity("打开文件", &intent),
            Complexity::Simple
        );
        assert_eq!(
            analyzer.analyze_complexity("delete the folder", &intent),
            Complexity::Simple
        );
        assert_eq!(
            analyzer.analyze_complexity("你好", &intent),
            Complexity::Simple
        );
    }

    #[test]
    fn test_complexity_needs_plan() {
        let analyzer = IntentAnalyzer::new();
        let intent = ExecutionIntent::Conversational;

        // Multi-step keywords
        assert_eq!(
            analyzer.analyze_complexity("打开文件然后保存", &intent),
            Complexity::NeedsPlan
        );
        assert_eq!(
            analyzer.analyze_complexity("open file then close", &intent),
            Complexity::NeedsPlan
        );

        // Step markers
        assert_eq!(
            analyzer.analyze_complexity("1. Open file\n2. Edit content", &intent),
            Complexity::NeedsPlan
        );
        assert_eq!(
            analyzer.analyze_complexity("- Create folder\n- Add files", &intent),
            Complexity::NeedsPlan
        );
    }

    // ========================================================================
    // Step Marker Detection Tests
    // ========================================================================

    #[test]
    fn test_step_markers() {
        let analyzer = IntentAnalyzer::new();

        // Numbered lists
        assert!(analyzer.has_step_markers("1. First step\n2. Second step"));
        assert!(analyzer.has_step_markers("1、打开文件\n2、编辑内容"));
        assert!(analyzer.has_step_markers("1) Do this\n2) Do that"));

        // Bullet points
        assert!(analyzer.has_step_markers("- Task one\n- Task two"));
        assert!(analyzer.has_step_markers("* Step A\n* Step B"));
        assert!(analyzer.has_step_markers("• Item 1\n• Item 2"));

        // No markers
        assert!(!analyzer.has_step_markers("Open the file"));
        assert!(!analyzer.has_step_markers("打开文件并编辑"));
    }

    // ========================================================================
    // Multiple Action Sentences Tests
    // ========================================================================

    #[test]
    fn test_multiple_action_sentences() {
        let analyzer = IntentAnalyzer::new();

        // Multiple Chinese action sentences
        assert!(analyzer.has_multiple_action_sentences("创建一个文件夹。删除旧文件。"));
        assert!(analyzer.has_multiple_action_sentences("运行脚本！保存结果。"));

        // Multiple English action sentences
        assert!(analyzer.has_multiple_action_sentences("Create a folder. Delete old files."));
        assert!(analyzer.has_multiple_action_sentences("Open the file! Edit the content."));

        // Single action sentence
        assert!(!analyzer.has_multiple_action_sentences("打开文件"));
        assert!(!analyzer.has_multiple_action_sentences("Create a folder"));

        // Multiple sentences but no action verbs
        assert!(!analyzer.has_multiple_action_sentences("Hello. How are you?"));
    }

    // ========================================================================
    // Build Direct Call Tests
    // ========================================================================

    #[test]
    fn test_build_direct_call_executable() {
        use crate::intent::task_category::TaskCategory;

        let analyzer = IntentAnalyzer::new();
        let input = create_test_input("整理下载文件夹");

        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理下载文件夹".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.9,
        };
        let intent = ExecutionIntent::Executable(task);

        let call = analyzer.build_direct_call(&intent, &input);

        assert_eq!(call.tool, "file_organize");
        assert!(call.parameters["target"].as_str().is_some());
        assert!(call.plan_step_id.is_none());
    }

    #[test]
    fn test_build_direct_call_conversational() {
        let analyzer = IntentAnalyzer::new();
        let input = create_test_input("你好");
        let intent = ExecutionIntent::Conversational;

        let call = analyzer.build_direct_call(&intent, &input);

        assert_eq!(call.tool, "general_chat");
        assert_eq!(call.parameters["input"], "你好");
    }

    // ========================================================================
    // EventHandler Implementation Tests
    // ========================================================================

    #[tokio::test]
    async fn test_handler_name() {
        let analyzer = IntentAnalyzer::new();
        assert_eq!(analyzer.name(), "IntentAnalyzer");
    }

    #[tokio::test]
    async fn test_handler_subscriptions() {
        let analyzer = IntentAnalyzer::new();
        let subs = analyzer.subscriptions();

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], EventType::InputReceived);
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        use crate::event::{EventBus, StopReason};

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // LoopStop event should be ignored
        let event = AetherEvent::LoopStop(StopReason::Completed);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_simple_input_returns_tool_call() {
        use crate::event::EventBus;

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let input = create_test_input("你好");
        let event = AetherEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AetherEvent::ToolCallRequested(_)));
    }

    #[tokio::test]
    async fn test_handler_complex_input_returns_plan_request() {
        use crate::event::EventBus;

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let input = create_test_input("打开文件然后复制内容接着保存到新位置");
        let event = AetherEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AetherEvent::PlanRequested(_)));

        if let AetherEvent::PlanRequested(plan_req) = &result[0] {
            assert!(!plan_req.detected_steps.is_empty());
        }
    }

    #[tokio::test]
    async fn test_handler_numbered_list_returns_plan_request() {
        use crate::event::EventBus;

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let input = create_test_input("1. 创建目录\n2. 复制文件\n3. 清理临时文件");
        let event = AetherEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AetherEvent::PlanRequested(_)));
    }

    #[test]
    fn test_split_by_keyword_case_insensitive() {
        let result = split_by_keyword_case_insensitive("open file THEN save it", "then");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "open file");
        assert_eq!(result[1], "save it");

        let result = split_by_keyword_case_insensitive("no keyword here", "then");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "no keyword here");
    }

    #[test]
    fn test_with_input_context() {
        use crate::intent::task_category::TaskCategory;

        let analyzer = IntentAnalyzer::new();

        let input = InputEvent {
            text: "整理文件".to_string(),
            topic_id: None,
            context: Some(InputContext {
                app_name: Some("Finder".to_string()),
                app_bundle_id: Some("com.apple.finder".to_string()),
                window_title: Some("Downloads".to_string()),
                selected_text: Some("test.txt".to_string()),
            }),
            timestamp: 0,
        };

        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: None,
            confidence: 0.9,
        };
        let intent = ExecutionIntent::Executable(task);

        let call = analyzer.build_direct_call(&intent, &input);

        // Should include context information
        assert!(call.parameters["app_context"].is_string());
        assert!(call.parameters["selected_text"].is_string());
    }
}
