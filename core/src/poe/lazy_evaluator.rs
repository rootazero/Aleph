//! Lazy POE Evaluator — lightweight, on-demand quality checks without full manifest contracts.
//!
//! This module provides a "lazy" alternative to the full POE cycle. Instead of requiring
//! an upfront SuccessManifest with hard/soft constraints, it runs lightweight validation
//! checks at each agent loop step and at completion time.
//!
//! # Key Concepts
//!
//! - **LightManifest**: A minimal tracking structure that records tool invocations,
//!   the original query, and a small retry budget. No upfront contract needed.
//! - **LazyPoeEvaluator**: An async-safe wrapper around `LightManifest` that provides
//!   step-level and completion-level validation.
//! - **Hallucination Detection**: Checks for fabricated artifacts (e.g., claiming PDF
//!   generation without invoking PDF tools, or referencing URLs without web search).
//! - **Query Relevance**: Ensures the LLM's final response relates to the original query
//!   via keyword overlap analysis.
//!
//! # Design Rationale
//!
//! The full POE cycle (Principle-Operation-Evaluation) is powerful but heavyweight:
//! it requires manifest generation, hard/semantic validators, and budget management.
//! For simple conversational turns or quick tool calls, this overhead is unnecessary.
//!
//! The lazy evaluator provides "good enough" quality guardrails with near-zero cost:
//! - No LLM calls (all checks are deterministic, <1ms)
//! - No manifest generation step
//! - Activates on-demand when tool calls are detected
//! - Max 2 retries to avoid infinite loops
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::poe::lazy_evaluator::LazyPoeEvaluator;
//!
//! let evaluator = LazyPoeEvaluator::new("How do I convert a file to PDF?");
//!
//! // Activate when the agent starts using tools
//! evaluator.activate().await;
//!
//! // Record tool results as they complete
//! evaluator.record_tool_result("pdf_generate", true).await;
//!
//! // Evaluate after each step
//! let directive = evaluator.evaluate_step(&action, &result).await;
//! ```

use std::collections::HashSet;

use crate::agent_loop::decision::{Action, ActionResult};
use crate::poe::interceptor::directive::StepDirective;

// ============================================================================
// ToolInvocation
// ============================================================================

/// Record of a single tool invocation during the agent loop.
///
/// Tracks whether a tool was called and whether it produced meaningful output.
/// Used by `LightManifest` to detect hallucinations (e.g., the LLM claims
/// a PDF was generated but never invoked the PDF tool).
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Name of the tool that was invoked
    pub tool_name: String,

    /// Whether the tool returned a result (success or error)
    pub had_result: bool,

    /// Whether the tool result contained non-empty output
    pub result_non_empty: bool,
}

impl ToolInvocation {
    /// Create a new tool invocation record.
    pub fn new(tool_name: impl Into<String>, had_result: bool, result_non_empty: bool) -> Self {
        Self {
            tool_name: tool_name.into(),
            had_result,
            result_non_empty,
        }
    }
}

// ============================================================================
// LightManifest
// ============================================================================

/// Default maximum number of retries for the lazy evaluator.
const DEFAULT_MAX_RETRIES: u8 = 2;

/// Lightweight manifest that tracks tool usage and retry budget without
/// requiring upfront success criteria.
///
/// Unlike `SuccessManifest`, which defines hard/soft constraints before execution,
/// `LightManifest` observes what happens during execution and validates retroactively.
///
/// # Lifecycle
///
/// 1. Created in `inactive` state when a new agent loop starts
/// 2. Activated via `activate()` when tool calls are detected
/// 3. Tool invocations recorded via `record_tool()`
/// 4. Completion validated via `LazyPoeEvaluator::validate_completion()`
/// 5. Retry budget consumed via `consume_retry()` on validation failures
#[derive(Debug, Clone)]
pub struct LightManifest {
    /// The user's original query (used for relevance checks)
    original_query: String,

    /// History of tool invocations during this session
    tools_invoked: Vec<ToolInvocation>,

    /// Maximum number of retries allowed
    max_retries: u8,

    /// Number of retries consumed so far
    retry_count: u8,

    /// Whether the lazy evaluator is actively monitoring
    active: bool,
}

impl LightManifest {
    /// Create a new inactive manifest for the given query.
    ///
    /// The manifest starts inactive and must be explicitly activated
    /// via `activate()` before it will track tool invocations.
    pub fn new(original_query: impl Into<String>) -> Self {
        Self {
            original_query: original_query.into(),
            tools_invoked: Vec::new(),
            max_retries: DEFAULT_MAX_RETRIES,
            retry_count: 0,
            active: false,
        }
    }

    /// Activate the lazy evaluator.
    ///
    /// Once active, the evaluator will track tool invocations and
    /// validate step results. Typically called when the first tool
    /// call is detected in the agent loop.
    pub fn activate(&mut self) {
        self.active = true;
    }

    /// Check whether the evaluator is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Record a tool invocation.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool that was invoked
    /// * `had_result` - Whether the tool produced any result
    /// * `result_non_empty` - Whether the result contained meaningful output
    pub fn record_tool(&mut self, tool_name: impl Into<String>, had_result: bool, result_non_empty: bool) {
        self.tools_invoked.push(ToolInvocation::new(tool_name, had_result, result_non_empty));
    }

    /// Check whether a specific tool was invoked during this session.
    pub fn tool_was_invoked(&self, tool_name: &str) -> bool {
        self.tools_invoked.iter().any(|t| t.tool_name == tool_name)
    }

    /// Get the names of all tools that were invoked.
    pub fn invoked_tool_names(&self) -> Vec<&str> {
        self.tools_invoked.iter().map(|t| t.tool_name.as_str()).collect()
    }

    /// Check whether more retries are available.
    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }

    /// Consume one retry from the budget.
    ///
    /// Returns `true` if a retry was available and consumed,
    /// `false` if the retry budget is already exhausted.
    pub fn consume_retry(&mut self) -> bool {
        if self.can_retry() {
            self.retry_count += 1;
            true
        } else {
            false
        }
    }

    /// Get the original query string.
    pub fn original_query(&self) -> &str {
        &self.original_query
    }

    /// Get the current retry count.
    pub fn retry_count(&self) -> u8 {
        self.retry_count
    }
}

// ============================================================================
// LazyPoeEvaluator
// ============================================================================

/// Async-safe lazy POE evaluator that wraps a `LightManifest` in a `tokio::sync::Mutex`.
///
/// Provides two levels of validation:
///
/// 1. **Step validation** (`evaluate_step`): Checks after each agent loop step
///    - Retryable tool errors -> hint to retry
///    - Successful but empty tool output -> hint to investigate
///
/// 2. **Completion validation** (`validate_completion`): Checks when the LLM
///    signals task completion
///    - No tools invoked despite active POE -> hint to actually use tools
///    - Hallucination detected -> hint to verify claims
///    - Low query relevance -> hint to address the original question
pub struct LazyPoeEvaluator {
    manifest: tokio::sync::Mutex<LightManifest>,
}

impl LazyPoeEvaluator {
    /// Create a new lazy evaluator for the given query.
    pub fn new(original_query: impl Into<String>) -> Self {
        Self {
            manifest: tokio::sync::Mutex::new(LightManifest::new(original_query)),
        }
    }

    /// Set the original query after construction.
    ///
    /// This is useful when the evaluator is created before the query is known
    /// (e.g., in `EventEmittingCallback` constructors where the query arrives
    /// later in `on_loop_start`).
    pub async fn set_query(&self, query: impl Into<String>) {
        self.manifest.lock().await.original_query = query.into();
    }

    /// Activate the evaluator (typically when tool calls are first detected).
    pub async fn activate(&self) {
        self.manifest.lock().await.activate();
    }

    /// Check whether the evaluator is currently active.
    pub async fn is_active(&self) -> bool {
        self.manifest.lock().await.is_active()
    }

    /// Record a tool result.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool that was invoked
    /// * `result_non_empty` - Whether the result contained meaningful output
    pub async fn record_tool_result(&self, tool_name: &str, result_non_empty: bool) {
        self.manifest.lock().await.record_tool(tool_name, true, result_non_empty);
    }

    /// Evaluate a completed step and return a directive.
    ///
    /// This is called after each agent loop step (Act + Feedback).
    /// Only performs fast, deterministic checks (<1ms).
    ///
    /// # Validation Rules
    ///
    /// - `ToolError { retryable: true }` -> `ContinueWithHint` (advisory, does NOT consume retry budget)
    /// - `ToolSuccess` with empty output -> `ContinueWithHint` (advisory)
    /// - All other cases -> `Continue`
    ///
    /// Note: Step-level hints are advisory and do NOT consume the retry budget.
    /// Only completion validation (`validate_completion`) consumes retries.
    pub async fn evaluate_step(&self, action: &Action, result: &ActionResult) -> StepDirective {
        let manifest = self.manifest.lock().await;

        if !manifest.is_active() {
            return StepDirective::Continue;
        }

        match result {
            ActionResult::ToolResults { ref results } => {
                if let Some(r) = results.first() {
                    match &r.result {
                        // Retryable tool error: suggest a retry (advisory — no budget consumed)
                        crate::agent_loop::decision::SingleToolResult::Error { retryable: true, error } => {
                            let tool_name = extract_tool_name(action);
                            StepDirective::ContinueWithHint {
                                hint: format!(
                                    "[POE-Lazy] Tool '{}' failed with retryable error: {}. \
                                     Please retry with adjusted parameters.",
                                    tool_name,
                                    truncate(error, 100),
                                ),
                            }
                        }
                        // Tool succeeded but output is empty: hint to investigate
                        crate::agent_loop::decision::SingleToolResult::Success { output, .. } => {
                            if is_empty_output(output) {
                                let tool_name = extract_tool_name(action);
                                StepDirective::ContinueWithHint {
                                    hint: format!(
                                        "[POE-Lazy] Tool '{}' returned empty output. \
                                         Consider verifying the result or trying alternative parameters.",
                                        tool_name,
                                    ),
                                }
                            } else {
                                StepDirective::Continue
                            }
                        }
                        // Non-retryable error: no intervention
                        _ => StepDirective::Continue,
                    }
                } else {
                    StepDirective::Continue
                }
            }

            // All other results: no intervention
            _ => StepDirective::Continue,
        }
    }

    /// Validate at completion time before the agent loop exits.
    ///
    /// Returns `Some(hint)` if a problem is detected, `None` if validation passes.
    /// Each failed check consumes one retry from the manifest budget.
    ///
    /// # Checks (in order)
    ///
    /// 1. **No tools invoked**: The evaluator is active but no tools were called.
    ///    This suggests the LLM skipped tool usage and may be guessing.
    /// 2. **Hallucination detected**: The completion text claims artifacts that
    ///    were never produced by tools (e.g., "PDF generated" without PDF tool).
    /// 3. **Low query relevance**: The completion text has less than 15% keyword
    ///    overlap with the original query (for queries with >= 3 words).
    pub async fn validate_completion(&self, completion_text: &str) -> Option<String> {
        let mut manifest = self.manifest.lock().await;

        if !manifest.is_active() {
            return None;
        }

        tracing::info!(
            subsystem = "poe_lazy",
            event = "validation_triggered",
            tools_invoked = manifest.tools_invoked.len(),
            retries_remaining = manifest.max_retries - manifest.retry_count,
            "POE lazy evaluator validation triggered at completion"
        );

        // Check 1: No tools invoked despite active POE
        if manifest.tools_invoked.is_empty() && manifest.consume_retry() {
            tracing::info!(
                subsystem = "poe_lazy",
                event = "validation_result",
                passed = false,
                reason = "no_tools_invoked",
                "POE lazy evaluator validation failed"
            );
            return Some(
                "[POE-Lazy] You are about to complete without having used any tools. \
                 Please verify your answer by using appropriate tools before concluding."
                    .to_string(),
            );
        }

        // Check 2: Hallucination detection
        if let Some(hint) = detect_hallucination(completion_text, &manifest) {
            if manifest.consume_retry() {
                tracing::info!(
                    subsystem = "poe_lazy",
                    event = "validation_result",
                    passed = false,
                    reason = "hallucination_detected",
                    "POE lazy evaluator validation failed"
                );
                return Some(hint);
            }
        }

        // Check 3: Query relevance
        if let Some(hint) = check_query_relevance(completion_text, &manifest) {
            if manifest.consume_retry() {
                tracing::info!(
                    subsystem = "poe_lazy",
                    event = "validation_result",
                    passed = false,
                    reason = "low_query_relevance",
                    "POE lazy evaluator validation failed"
                );
                return Some(hint);
            }
        }

        tracing::info!(
            subsystem = "poe_lazy",
            event = "validation_result",
            passed = true,
            "POE lazy evaluator validation passed"
        );

        None
    }
}

// ============================================================================
// Hallucination Detection
// ============================================================================

/// PDF-related keywords that indicate the LLM claims a PDF was generated.
const PDF_CLAIM_KEYWORDS: &[&str] = &[
    "pdf已生成",
    "pdf 已生成",
    "已生成pdf",
    "pdf ready",
    "generated pdf",
    "pdf generated",
];

/// Tool names that are considered PDF-producing tools.
const PDF_TOOL_NAMES: &[&str] = &["pdf_generate", "pdf"];

/// Tool names that are considered web-browsing tools.
const WEB_TOOL_NAMES: &[&str] = &["web_search", "browse", "search", "fetch"];

/// Detect hallucinations in the completion text.
///
/// Checks for two patterns:
/// 1. **PDF claims**: The text mentions PDF generation keywords but no PDF tool was invoked.
/// 2. **URL references**: The text contains URLs but no web-related tool was invoked.
///
/// Returns `Some(hint)` if hallucination is detected, `None` otherwise.
fn detect_hallucination(text: &str, manifest: &LightManifest) -> Option<String> {
    let text_lower = text.to_lowercase();

    // Check for PDF hallucination
    let claims_pdf = PDF_CLAIM_KEYWORDS.iter().any(|kw| text_lower.contains(kw));
    if claims_pdf {
        let used_pdf_tool = PDF_TOOL_NAMES.iter().any(|t| manifest.tool_was_invoked(t));
        if !used_pdf_tool {
            return Some(
                "[POE-Lazy] Hallucination detected: you claim a PDF was generated, \
                 but no PDF tool was invoked. Please actually generate the PDF \
                 or remove the claim."
                    .to_string(),
            );
        }
    }

    // Check for URL hallucination
    let has_url = text_lower.contains("http://")
        || text_lower.contains("https://")
        || text_lower.contains("www.");
    if has_url {
        let used_web_tool = WEB_TOOL_NAMES.iter().any(|t| manifest.tool_was_invoked(t));
        if !used_web_tool {
            return Some(
                "[POE-Lazy] Hallucination detected: you referenced URLs, \
                 but no web search or browsing tool was invoked. Please verify \
                 URLs by using a search tool or remove unverified references."
                    .to_string(),
            );
        }
    }

    None
}

// ============================================================================
// Query Relevance
// ============================================================================

/// Minimum keyword overlap ratio to consider a response relevant to the query.
const RELEVANCE_THRESHOLD: f64 = 0.15;

/// Minimum number of query words required for relevance checking.
/// Queries shorter than this are too brief for meaningful overlap analysis.
const MIN_QUERY_WORDS: usize = 3;

/// Check whether the completion text is relevant to the original query.
///
/// Uses keyword overlap: extracts words from both the query and the text,
/// then computes the fraction of query keywords present in the text.
/// If overlap is below `RELEVANCE_THRESHOLD` (15%) for queries with at
/// least `MIN_QUERY_WORDS` (3) words, a hint is returned.
///
/// Returns `Some(hint)` if relevance is too low, `None` otherwise.
fn check_query_relevance(text: &str, manifest: &LightManifest) -> Option<String> {
    let query = manifest.original_query();
    let query_words = extract_keywords(query);

    // Skip relevance check for very short queries
    if query_words.len() < MIN_QUERY_WORDS {
        return None;
    }

    let text_words = extract_keywords(text);
    if text_words.is_empty() {
        // Empty completion text is suspicious but handled by other checks
        return None;
    }

    // Compute overlap: fraction of query keywords found in the text
    let matches = query_words.iter().filter(|w| text_words.contains(*w)).count();
    let overlap = matches as f64 / query_words.len() as f64;

    if overlap < RELEVANCE_THRESHOLD {
        Some(format!(
            "[POE-Lazy] Low query relevance ({:.0}% keyword overlap). \
             Your response may not address the original question: '{}'. \
             Please re-read the query and ensure your answer is relevant.",
            overlap * 100.0,
            truncate(query, 80),
        ))
    } else {
        None
    }
}

/// Extract lowercase keywords from text, filtering out common stop words.
///
/// Returns a set of unique lowercase words with length > 1.
fn extract_keywords(text: &str) -> HashSet<String> {
    // Common English/Chinese stop words that don't carry meaning
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "shall",
        "should", "may", "might", "can", "could", "to", "of", "in", "for",
        "on", "with", "at", "by", "from", "as", "into", "about", "it", "its",
        "this", "that", "these", "those", "i", "me", "my", "we", "our", "you",
        "your", "he", "she", "they", "them", "and", "or", "but", "not", "no",
        "if", "then", "so", "how", "what", "when", "where", "which", "who",
        "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都",
        "一", "一个", "上", "也", "很", "到", "说", "要", "去", "你",
    ];

    let stop_set: HashSet<&str> = STOP_WORDS.iter().copied().collect();

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1 && !stop_set.contains(w))
        .map(|w| w.to_string())
        .collect()
}

// ============================================================================
// Helpers
// ============================================================================

/// Extract the tool name from an action, if it is a tool call.
fn extract_tool_name(action: &Action) -> &str {
    match action {
        Action::ToolCalls { calls: ref requests } => {
            requests.first().map(|r| r.tool_name.as_str()).unwrap_or("unknown")
        }
        _ => "unknown",
    }
}

/// Check whether a tool output value is considered "empty".
///
/// Empty means: null, empty/whitespace-only string, empty array, or empty object.
/// Must stay consistent with the emptiness check in `loop_callback_adapter.rs`.
fn is_empty_output(output: &serde_json::Value) -> bool {
    match output {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(arr) => arr.is_empty(),
        serde_json::Value::Object(obj) => obj.is_empty(),
        _ => false,
    }
}

/// Truncate a string to a maximum number of characters (UTF-8 safe).
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- ToolInvocation tests ---

    #[test]
    fn tool_invocation_new() {
        let inv = ToolInvocation::new("web_search", true, true);
        assert_eq!(inv.tool_name, "web_search");
        assert!(inv.had_result);
        assert!(inv.result_non_empty);
    }

    // --- LightManifest tests ---

    #[test]
    fn manifest_starts_inactive() {
        let m = LightManifest::new("test query");
        assert!(!m.is_active());
        assert_eq!(m.original_query(), "test query");
        assert_eq!(m.retry_count(), 0);
        assert!(m.can_retry());
    }

    #[test]
    fn manifest_activate() {
        let mut m = LightManifest::new("query");
        m.activate();
        assert!(m.is_active());
    }

    #[test]
    fn manifest_record_tool() {
        let mut m = LightManifest::new("query");
        m.record_tool("search", true, true);
        m.record_tool("pdf_generate", true, false);

        assert!(m.tool_was_invoked("search"));
        assert!(m.tool_was_invoked("pdf_generate"));
        assert!(!m.tool_was_invoked("browse"));

        let names = m.invoked_tool_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"search"));
        assert!(names.contains(&"pdf_generate"));
    }

    #[test]
    fn manifest_retry_budget() {
        let mut m = LightManifest::new("query");

        assert!(m.can_retry());
        assert_eq!(m.retry_count(), 0);

        assert!(m.consume_retry());
        assert_eq!(m.retry_count(), 1);
        assert!(m.can_retry());

        assert!(m.consume_retry());
        assert_eq!(m.retry_count(), 2);
        assert!(!m.can_retry());

        // Budget exhausted
        assert!(!m.consume_retry());
        assert_eq!(m.retry_count(), 2);
    }

    // --- Hallucination detection tests ---

    #[test]
    fn detect_pdf_hallucination() {
        let mut m = LightManifest::new("query");
        m.activate();

        // Claims PDF without tool
        let hint = detect_hallucination("PDF已生成，请查看", &m);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("Hallucination"));

        // Claims PDF with tool
        m.record_tool("pdf_generate", true, true);
        let hint = detect_hallucination("PDF已生成，请查看", &m);
        assert!(hint.is_none());
    }

    #[test]
    fn detect_url_hallucination() {
        let mut m = LightManifest::new("query");
        m.activate();

        // References URL without web tool
        let hint = detect_hallucination("Check out https://example.com for details", &m);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("URLs"));

        // References URL with web tool
        m.record_tool("web_search", true, true);
        let hint = detect_hallucination("Check out https://example.com for details", &m);
        assert!(hint.is_none());
    }

    #[test]
    fn no_hallucination_for_plain_text() {
        let m = LightManifest::new("query");
        let hint = detect_hallucination("Here is a simple text response without claims", &m);
        assert!(hint.is_none());
    }

    // --- Query relevance tests ---

    #[test]
    fn relevance_passes_for_related_text() {
        let m = LightManifest::new("How to convert images to PDF format in Rust");
        let hint = check_query_relevance(
            "To convert images to PDF format in Rust, you can use the printpdf crate...",
            &m,
        );
        assert!(hint.is_none());
    }

    #[test]
    fn relevance_fails_for_unrelated_text() {
        let m = LightManifest::new("How to convert images to PDF format in Rust");
        let hint = check_query_relevance(
            "The weather tomorrow will be sunny with a high of 25 degrees.",
            &m,
        );
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("relevance"));
    }

    #[test]
    fn relevance_skips_short_queries() {
        let m = LightManifest::new("hi");
        let hint = check_query_relevance("Some completely unrelated response.", &m);
        assert!(hint.is_none()); // Query too short, skip check
    }

    // --- Helper tests ---

    #[test]
    fn extract_tool_name_from_tool_call() {
        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "web_search".to_string(),
            arguments: json!({}) }]};
        assert_eq!(extract_tool_name(&action), "web_search");
    }

    #[test]
    fn extract_tool_name_from_non_tool() {
        let action = Action::Completion {
            summary: "done".to_string(),
        };
        assert_eq!(extract_tool_name(&action), "unknown");
    }

    #[test]
    fn empty_output_detection() {
        assert!(is_empty_output(&json!(null)));
        assert!(is_empty_output(&json!("")));
        assert!(is_empty_output(&json!([])));
        assert!(is_empty_output(&json!({})));
        assert!(!is_empty_output(&json!("hello")));
        assert!(!is_empty_output(&json!(42)));
        assert!(!is_empty_output(&json!({"key": "value"})));
        assert!(!is_empty_output(&json!([1, 2, 3])));
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate("hello world this is a long string", 10);
        assert!(result.ends_with("..."));
        // 10 chars + "..."
        assert!(result.len() <= 14);
    }

    #[test]
    fn truncate_multibyte_safe() {
        // Chinese characters are multi-byte in UTF-8
        let result = truncate("你好世界这是一个测试", 3);
        assert!(result.ends_with("..."));
        assert_eq!(result, "你好世...");
    }

    #[test]
    fn extract_keywords_basic() {
        let keywords = extract_keywords("How to convert images to PDF format");
        assert!(keywords.contains("convert"));
        assert!(keywords.contains("images"));
        assert!(keywords.contains("pdf"));
        assert!(keywords.contains("format"));
        // "How", "to" are stop words
        assert!(!keywords.contains("how"));
        assert!(!keywords.contains("to"));
    }

    #[test]
    fn extract_keywords_deduplicates() {
        let keywords = extract_keywords("rust rust rust code code");
        assert!(keywords.contains("rust"));
        assert!(keywords.contains("code"));
        assert_eq!(keywords.len(), 2);
    }

    // --- Async tests for LazyPoeEvaluator ---

    #[tokio::test]
    async fn evaluator_inactive_returns_continue() {
        let eval = LazyPoeEvaluator::new("test query");
        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "search".to_string(),
            arguments: json!({}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Error { error: "not found".to_string(), retryable: true },
            }]};

        let directive = eval.evaluate_step(&action, &result).await;
        assert!(matches!(directive, StepDirective::Continue));
    }

    #[tokio::test]
    async fn evaluator_retryable_error_hints() {
        let eval = LazyPoeEvaluator::new("test query");
        eval.activate().await;

        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "web_search".to_string(),
            arguments: json!({"query": "rust tutorial"}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Error { error: "timeout".to_string(), retryable: true },
            }]};

        let directive = eval.evaluate_step(&action, &result).await;
        match directive {
            StepDirective::ContinueWithHint { hint } => {
                assert!(hint.contains("web_search"));
                assert!(hint.contains("timeout"));
            }
            other => panic!("Expected ContinueWithHint, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn evaluator_empty_output_hints() {
        let eval = LazyPoeEvaluator::new("test query");
        eval.activate().await;

        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "file_read".to_string(),
            arguments: json!({"path": "/tmp/test.txt"}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Success { output: json!(""), duration_ms: 10 },
            }]};

        let directive = eval.evaluate_step(&action, &result).await;
        match directive {
            StepDirective::ContinueWithHint { hint } => {
                assert!(hint.contains("file_read"));
                assert!(hint.contains("empty"));
            }
            other => panic!("Expected ContinueWithHint, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn evaluator_normal_success_continues() {
        let eval = LazyPoeEvaluator::new("test query");
        eval.activate().await;

        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "search".to_string(),
            arguments: json!({}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Success { output: json!({"results": ["item1", "item2"]}), duration_ms: 50 },
            }]};

        let directive = eval.evaluate_step(&action, &result).await;
        assert!(matches!(directive, StepDirective::Continue));
    }

    #[tokio::test]
    async fn evaluator_step_hints_do_not_consume_retry_budget() {
        let eval = LazyPoeEvaluator::new("test query");
        eval.activate().await;

        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "search".to_string(),
            arguments: json!({}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Error { error: "fail".to_string(), retryable: true },
            }]};

        // Step hints are advisory — they never exhaust the retry budget
        let d1 = eval.evaluate_step(&action, &result).await;
        assert!(matches!(d1, StepDirective::ContinueWithHint { .. }));

        let d2 = eval.evaluate_step(&action, &result).await;
        assert!(matches!(d2, StepDirective::ContinueWithHint { .. }));

        // Third call still hints (budget not consumed by step evaluation)
        let d3 = eval.evaluate_step(&action, &result).await;
        assert!(matches!(d3, StepDirective::ContinueWithHint { .. }));

        // Completion validation should still have full retry budget
        eval.record_tool_result("search", false).await;
        let hint = eval.validate_completion("PDF已生成").await;
        assert!(hint.is_some()); // First completion retry consumed
    }

    #[tokio::test]
    async fn validate_completion_no_tools() {
        let eval = LazyPoeEvaluator::new("test query");
        eval.activate().await;

        let hint = eval.validate_completion("Here is my answer.").await;
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("without having used any tools"));
    }

    #[tokio::test]
    async fn validate_completion_hallucination() {
        let eval = LazyPoeEvaluator::new("generate a report");
        eval.activate().await;
        eval.record_tool_result("text_editor", true).await;

        let hint = eval
            .validate_completion("PDF已生成，请查看输出目录")
            .await;
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("Hallucination"));
    }

    #[tokio::test]
    async fn validate_completion_passes_when_clean() {
        let eval = LazyPoeEvaluator::new("search for Rust tutorials");
        eval.activate().await;
        eval.record_tool_result("web_search", true).await;

        let hint = eval
            .validate_completion("Here are some Rust tutorials I found via search...")
            .await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn validate_completion_inactive_skips() {
        let eval = LazyPoeEvaluator::new("test query");
        // Not activated

        let hint = eval.validate_completion("any text").await;
        assert!(hint.is_none());
    }

    // --- Integration tests: full lazy POE flow ---

    #[tokio::test]
    async fn integration_full_lazy_poe_flow() {
        // Simulate: user asks for research, agent uses tools, completes
        let eval = LazyPoeEvaluator::new("帮我查一下比特币最新价格");

        // 1. Evaluator starts inactive
        assert!(!eval.is_active().await);

        // 2. Agent decides to use a tool → activate
        eval.activate().await;
        assert!(eval.is_active().await);

        // 3. Tool executes with good result
        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "web_search".to_string(),
            arguments: json!({"query": "bitcoin price"}) }]};
        let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
            call_id: String::new(), tool_name: String::new(),
            result: crate::agent_loop::decision::SingleToolResult::Success { output: json!({"price": "67000", "source": "coinbase"}), duration_ms: 500 },
            }]};
        eval.record_tool_result("web_search", true).await;

        // 4. Step evaluation — should continue (good result)
        let directive = eval.evaluate_step(&action, &result).await;
        assert!(matches!(directive, StepDirective::Continue));

        // 5. Completion with valid summary — should pass
        let hint = eval
            .validate_completion("根据搜索结果，比特币当前价格为 $67,000")
            .await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn integration_hallucination_retry_flow() {
        let eval = LazyPoeEvaluator::new("生成一份PDF报告");
        eval.activate().await;

        // Agent completes without calling pdf_generate → hallucination
        let hint1 = eval.validate_completion("PDF已生成，请查收").await;
        assert!(hint1.is_some()); // First retry consumed

        // Second attempt, still no tool called
        let hint2 = eval.validate_completion("PDF已生成").await;
        assert!(hint2.is_some()); // Second retry consumed

        // Third attempt — budget exhausted, accepted (best effort)
        let hint3 = eval.validate_completion("PDF已生成").await;
        assert!(hint3.is_none()); // Accepted despite hallucination
    }

    #[tokio::test]
    async fn integration_step_hints_preserve_completion_budget() {
        // Step-level hints should NOT consume retry budget
        let eval = LazyPoeEvaluator::new("search for data");
        eval.activate().await;

        let action = Action::ToolCalls { calls: vec![crate::agent_loop::decision::ToolCallRequest { call_id: String::new(),
            tool_name: "search".to_string(),
            arguments: json!({}) }]};

        // Multiple tool errors at step level — should always hint, never exhaust budget
        for _ in 0..5 {
            let result = ActionResult::ToolResults { results: vec![crate::agent_loop::decision::ToolCallResult {
                call_id: String::new(), tool_name: String::new(),
                result: crate::agent_loop::decision::SingleToolResult::Error { error: "timeout".to_string(), retryable: true },
                }]};
            let directive = eval.evaluate_step(&action, &result).await;
            assert!(matches!(directive, StepDirective::ContinueWithHint { .. }));
        }

        // Completion validation should still have full retry budget (2 retries)
        let hint1 = eval.validate_completion("Here is the data from my search").await;
        assert!(hint1.is_some()); // No tools recorded → first retry

        eval.record_tool_result("search", true).await;
        let hint2 = eval.validate_completion("Here is the data from my search").await;
        assert!(hint2.is_none()); // Now clean — tool recorded, relevant response
    }

    #[tokio::test]
    async fn integration_set_query_after_construction() {
        // Simulates the EventEmittingCallback pattern: new("") then set_query()
        let eval = LazyPoeEvaluator::new("");
        eval.set_query("帮我生成PDF报告").await;
        eval.activate().await;

        // First completion fails: no tools invoked
        let hint1 = eval.validate_completion("PDF已生成").await;
        assert!(hint1.is_some());
        assert!(hint1.unwrap().contains("tools"));

        // Record a non-PDF tool — should still catch hallucination
        eval.record_tool_result("text_editor", true).await;
        let hint2 = eval.validate_completion("PDF已生成").await;
        assert!(hint2.is_some());
        assert!(hint2.unwrap().contains("Hallucination"));
    }
}
