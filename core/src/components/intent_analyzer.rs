//! Intent analyzer component - detects intent and complexity for routing.
//!
//! Subscribes to: InputReceived
//! Publishes: PlanRequested or ToolCallRequested
//!
//! Uses the UnifiedIntentClassifier (v3 pipeline) which replaces both the
//! legacy IntentClassifier and the v2 ExecutionIntentDecider.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::event::{
    AlephEvent, EventContext, EventHandler, EventType, HandlerError, InputEvent, PlanRequest,
    ToolCallRequest,
};
use crate::intent::{
    IntentContext, IntentResult, StructuralContext, UnifiedIntentClassifier,
};

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
/// Uses the UnifiedIntentClassifier pipeline for intent detection.
pub struct IntentAnalyzer {
    /// Unified intent classifier (v3 pipeline)
    classifier: UnifiedIntentClassifier,
}

impl Default for IntentAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentAnalyzer {
    /// Create a new IntentAnalyzer with default UnifiedIntentClassifier
    pub fn new() -> Self {
        Self {
            classifier: UnifiedIntentClassifier::new(),
        }
    }

    /// Create IntentAnalyzer with a custom UnifiedIntentClassifier.
    pub fn with_unified_classifier(classifier: UnifiedIntentClassifier) -> Self {
        Self { classifier }
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
    pub fn analyze_complexity_for_text(&self, text: &str) -> Complexity {
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
}

/// Split string by keyword case-insensitively
fn split_by_keyword_case_insensitive(text: &str, keyword: &str) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let keyword_lower = keyword.to_lowercase();

    let mut result = Vec::new();
    let mut start = 0;

    for (idx, _) in text_lower.match_indices(&keyword_lower) {
        if idx > start {
            let segment = text_lower[start..idx].trim();
            if !segment.is_empty() {
                result.push(segment.to_string());
            }
        }
        start = idx + keyword_lower.len();
    }

    // Add remaining text
    if start < text_lower.len() {
        let segment = text_lower[start..].trim();
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
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Only handle InputReceived events
        let input = match event {
            AlephEvent::InputReceived(input) => input,
            _ => return Ok(vec![]),
        };

        // Build IntentContext from InputEvent
        let intent_ctx = Self::build_intent_context(input);

        // Classify using unified pipeline
        let result = self.classifier.classify(&input.text, &intent_ctx).await;

        tracing::debug!(
            intent_result = ?result,
            "UnifiedIntentClassifier decision"
        );

        let events = match result {
            IntentResult::Abort => {
                // Return an empty event list to signal abort (no action taken)
                vec![]
            }
            IntentResult::DirectTool {
                tool_id,
                args,
                source,
            } => {
                let tool_call = ToolCallRequest {
                    tool: tool_id,
                    parameters: serde_json::json!({
                        "args": args,
                        "input_text": input.text,
                        "source": format!("{:?}", source),
                    }),
                    plan_step_id: None,
                };
                vec![AlephEvent::ToolCallRequested(tool_call)]
            }
            IntentResult::Execute {
                confidence: _,
                metadata,
            } => {
                // Determine if planning is needed based on complexity
                if self.has_multi_step_keywords(&input.text)
                    || self.has_step_markers(&input.text)
                    || self.has_multiple_action_sentences(&input.text)
                {
                    let detected_steps = self.extract_steps(&input.text);
                    let plan_request = PlanRequest {
                        input: input.clone(),
                        intent_type: metadata.keyword_tag.clone(),
                        detected_steps,
                    };
                    vec![AlephEvent::PlanRequested(plan_request)]
                } else {
                    let tool_call = ToolCallRequest {
                        tool: metadata
                            .keyword_tag
                            .clone()
                            .unwrap_or_else(|| "general_task".to_string()),
                        parameters: serde_json::json!({
                            "action": input.text,
                            "input_text": input.text,
                            "layer": format!("{:?}", metadata.layer),
                        }),
                        plan_step_id: None,
                    };
                    vec![AlephEvent::ToolCallRequested(tool_call)]
                }
            }
            IntentResult::Converse { .. } => {
                let tool_call = ToolCallRequest {
                    tool: "general_chat".to_string(),
                    parameters: serde_json::json!({
                        "input": input.text,
                        "mode": "conversation",
                    }),
                    plan_step_id: None,
                };
                vec![AlephEvent::ToolCallRequested(tool_call)]
            }
        };

        Ok(events)
    }
}

impl IntentAnalyzer {
    /// Build an `IntentContext` from an `InputEvent`.
    fn build_intent_context(event: &InputEvent) -> IntentContext {
        let structural = event
            .context
            .as_ref()
            .map(|ctx| StructuralContext {
                selected_file: ctx.selected_text.clone(),
                clipboard_type: None,
            })
            .unwrap_or_default();

        IntentContext { structural }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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

        // Simple single-action requests
        assert_eq!(
            analyzer.analyze_complexity_for_text("打开文件"),
            Complexity::Simple
        );
        assert_eq!(
            analyzer.analyze_complexity_for_text("delete the folder"),
            Complexity::Simple
        );
        assert_eq!(
            analyzer.analyze_complexity_for_text("你好"),
            Complexity::Simple
        );
    }

    #[test]
    fn test_complexity_needs_plan() {
        let analyzer = IntentAnalyzer::new();

        // Multi-step keywords
        assert_eq!(
            analyzer.analyze_complexity_for_text("打开文件然后保存"),
            Complexity::NeedsPlan
        );
        assert_eq!(
            analyzer.analyze_complexity_for_text("open file then close"),
            Complexity::NeedsPlan
        );

        // Step markers
        assert_eq!(
            analyzer.analyze_complexity_for_text("1. Open file\n2. Edit content"),
            Complexity::NeedsPlan
        );
        assert_eq!(
            analyzer.analyze_complexity_for_text("- Create folder\n- Add files"),
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
        let event = AlephEvent::LoopStop(StopReason::Completed);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_simple_input_returns_event() {
        use crate::event::EventBus;

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let input = create_test_input("你好");
        let event = AlephEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        // The unified classifier with default config (default_to_execute=true)
        // will produce either ToolCallRequested or PlanRequested
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_handler_complex_input_returns_plan_request() {
        use crate::event::EventBus;

        let analyzer = IntentAnalyzer::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let input = create_test_input("打开文件然后复制内容接着保存到新位置");
        let event = AlephEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AlephEvent::PlanRequested(_)));

        if let AlephEvent::PlanRequested(plan_req) = &result[0] {
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
        let event = AlephEvent::InputReceived(input);
        let result = analyzer.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AlephEvent::PlanRequested(_)));
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
}
