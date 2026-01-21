//! Unified Execution Intent Decider
//!
//! This module provides a single decision point for determining whether user input
//! should trigger task execution or conversation mode. The key principle is that
//! this decision is made BEFORE entering the LLM, so prompts only need to describe
//! "how to do", never "whether to do".
//!
//! # Architecture
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ ExecutionIntentDecider (single decision point)              │
//! │                                                             │
//! │  L0: Slash Commands (/screenshot, /ocr) → DirectTool        │
//! │      ↓ (no match)                                           │
//! │  L1: Regex Patterns ("打开", "运行") → Execute(category)     │
//! │      ↓ (no match)                                           │
//! │  L2: Context Signals (selected file) → Execute(category)    │
//! │      ↓ (no match)                                           │
//! │  L3: Semantic Analysis (lightweight LLM) → Execute|Converse │
//! │      ↓ (ambiguous)                                          │
//! │  L4: Default → Execute (bias toward action)                 │
//! │                                                             │
//! └─────────────────────────────────────────────────────────────┘
//!         ↓
//!     ExecutionMode
//!         ↓
//!     Select appropriate prompt (executor or conversational)
//! ```
//!
//! # Design Principles
//!
//! 1. **Single decision point**: All "execute vs converse" logic lives here
//! 2. **Fast path priority**: 90%+ of requests resolved at L0/L1 (<5ms)
//! 3. **Bias toward execution**: When uncertain, assume user wants action
//! 4. **No prompt contamination**: Prompts never contain decision logic

use super::TaskCategory;
use crate::command::{CommandContext, CommandParser, ParsedCommand};
use crate::dispatcher::ToolSourceType;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// =============================================================================
// Core Types
// =============================================================================

/// Execution mode determined by the decider
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Direct tool invocation from slash command (builtin tools)
    /// Example: /screenshot, /ocr, /search
    DirectTool(ToolInvocation),

    /// Skill-based execution with injected instructions
    /// Example: /knowledge-graph, /translate
    Skill(SkillInvocation),

    /// MCP server tool execution
    /// Example: /git, /docker
    Mcp(McpInvocation),

    /// Custom command with system prompt
    /// Example: /translate (from routing rules)
    Custom(CustomInvocation),

    /// Execute a task using AI with tools
    /// The AI receives an executor prompt and relevant tools
    Execute(TaskCategory),

    /// Pure conversation mode
    /// The AI receives a conversational prompt, no tools
    Converse,
}

/// Direct tool invocation details (for builtin commands)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    /// Tool identifier (e.g., "screenshot", "ocr")
    pub tool_id: String,
    /// Raw arguments from the command
    pub args: String,
}

/// Skill invocation details
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    /// Skill identifier
    pub skill_id: String,
    /// Display name for the skill
    pub display_name: String,
    /// Skill instructions to inject as context
    pub instructions: String,
    /// Raw arguments from the command
    pub args: String,
}

/// MCP server invocation details
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInvocation {
    /// MCP server name
    pub server_name: String,
    /// Specific tool name within the server (if any)
    pub tool_name: Option<String>,
    /// Raw arguments from the command
    pub args: String,
}

/// Custom command invocation details
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomInvocation {
    /// Command name
    pub command_name: String,
    /// System prompt to inject
    pub system_prompt: Option<String>,
    /// Provider override
    pub provider: Option<String>,
    /// Raw arguments from the command
    pub args: String,
}

/// Decision metadata for debugging and metrics
#[derive(Debug, Clone)]
pub struct DecisionMetadata {
    /// Which layer made the decision
    pub layer: IntentLayer,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
    /// Time taken to decide (microseconds)
    pub latency_us: u64,
    /// Matched pattern or keyword (if any)
    pub matched_pattern: Option<String>,
}

/// Intent decision layer indicator
///
/// These layers are distinct from Dispatcher's RoutingLayer:
/// - IntentLayer (here): Decides "execute vs converse" (Phase 1)
/// - RoutingLayer (Dispatcher): Decides "which tool/model" (Phase 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentLayer {
    /// Slash command direct mapping (<1ms)
    /// Example: /screenshot, /search
    SlashCommand,
    /// Regex pattern match (<5ms)
    /// Example: "整理.*文件夹", "what is"
    PatternMatch,
    /// Context signal match (<20ms)
    /// Example: file selected, clipboard type
    ContextSignal,
    /// Semantic/LLM analysis (<500ms)
    /// Uses lightweight LLM for ambiguous cases
    SemanticAnalysis,
    /// Default fallback (<1ms)
    /// Bias toward execution when uncertain
    DefaultFallback,
}

impl IntentLayer {
    /// Get typical latency for this layer
    pub fn typical_latency(&self) -> &'static str {
        match self {
            Self::SlashCommand => "<1ms",
            Self::PatternMatch => "<5ms",
            Self::ContextSignal => "<20ms",
            Self::SemanticAnalysis => "<500ms",
            Self::DefaultFallback => "<1ms",
        }
    }

    /// Get layer description for logging
    pub fn description(&self) -> &'static str {
        match self {
            Self::SlashCommand => "Slash command direct mapping",
            Self::PatternMatch => "Regex pattern match",
            Self::ContextSignal => "Context signal (file/app/clipboard)",
            Self::SemanticAnalysis => "Semantic LLM analysis",
            Self::DefaultFallback => "Default execution bias",
        }
    }
}

// Backward compatibility alias
#[deprecated(since = "0.2.0", note = "Use IntentLayer instead")]
pub type DecisionLayer = IntentLayer;

/// Decision result with mode and metadata
#[derive(Debug, Clone)]
pub struct DecisionResult {
    /// The determined execution mode
    pub mode: ExecutionMode,
    /// Decision metadata for debugging
    pub metadata: DecisionMetadata,
}

// =============================================================================
// Slash Command Definitions (L0)
// =============================================================================

/// Slash command definition
#[derive(Debug, Clone)]
pub struct SlashCommand {
    /// Command name (without /)
    pub name: &'static str,
    /// Tool ID to invoke
    pub tool_id: &'static str,
    /// Short description
    pub description: &'static str,
}

/// Built-in slash commands that directly invoke tools
static SLASH_COMMANDS: LazyLock<HashMap<&'static str, SlashCommand>> = LazyLock::new(|| {
    let commands = vec![
        SlashCommand {
            name: "screenshot",
            tool_id: "screenshot",
            description: "Capture screen content",
        },
        SlashCommand {
            name: "ocr",
            tool_id: "vision_ocr",
            description: "Extract text from image",
        },
        SlashCommand {
            name: "search",
            tool_id: "search",
            description: "Search the web",
        },
        SlashCommand {
            name: "youtube",
            tool_id: "youtube_transcript",
            description: "Get YouTube video transcript",
        },
        SlashCommand {
            name: "webfetch",
            tool_id: "web_fetch",
            description: "Fetch web page content",
        },
        SlashCommand {
            name: "gen",
            tool_id: "generate_image",
            description: "Generate image from prompt",
        },
    ];

    commands.into_iter().map(|cmd| (cmd.name, cmd)).collect()
});

// =============================================================================
// Regex Patterns (L1)
// =============================================================================

/// Execution intent patterns (match → Execute mode)
static EXECUTE_PATTERNS: LazyLock<Vec<(Regex, TaskCategory)>> = LazyLock::new(|| {
    vec![
        // File operations - Chinese
        (
            Regex::new(r"(?i)(整理|归类|分类|排序).*(文件|文件夹|目录|下载)").unwrap(),
            TaskCategory::FileOrganize,
        ),
        (
            Regex::new(r"(?i)(移动|复制|拷贝|剪切).*(文件|图片|文档)").unwrap(),
            TaskCategory::FileTransfer,
        ),
        (
            Regex::new(r"(?i)(删除|清理|清空|移除).*(文件|文件夹|缓存)").unwrap(),
            TaskCategory::FileCleanup,
        ),
        (
            Regex::new(r"(?i)(读取|打开|查看|显示).*(文件|内容|文档)").unwrap(),
            TaskCategory::FileOperation,
        ),
        // File operations - English
        (
            Regex::new(r"(?i)(organize|sort|classify|arrange).*(files?|folders?|downloads?)").unwrap(),
            TaskCategory::FileOrganize,
        ),
        (
            Regex::new(r"(?i)(move|copy|transfer).*(files?|images?|documents?)").unwrap(),
            TaskCategory::FileTransfer,
        ),
        (
            Regex::new(r"(?i)(delete|remove|clean|clear).*(files?|folders?|cache)").unwrap(),
            TaskCategory::FileCleanup,
        ),
        // Code execution
        (
            Regex::new(r"(?i)(运行|执行|跑).*(脚本|代码|程序|命令)").unwrap(),
            TaskCategory::CodeExecution,
        ),
        (
            Regex::new(r"(?i)(run|execute).*(script|code|program|command)").unwrap(),
            TaskCategory::CodeExecution,
        ),
        // App automation
        (
            Regex::new(r"(?i)(打开|启动|开启).*(应用|软件|程序|app)").unwrap(),
            TaskCategory::AppLaunch,
        ),
        (
            Regex::new(r"(?i)(open|launch|start).*(app|application|program)").unwrap(),
            TaskCategory::AppLaunch,
        ),
        // Content generation
        (
            Regex::new(r"(?i)(生成|创建|绘制|制作).*(图片|图像|图|画|海报)").unwrap(),
            TaskCategory::ImageGeneration,
        ),
        // "画" as verb (draw something)
        (
            Regex::new(r"(?i)^画.+").unwrap(),
            TaskCategory::ImageGeneration,
        ),
        (
            Regex::new(r"(?i)(generate|create|make).*(image|picture|illustration|poster)")
                .unwrap(),
            TaskCategory::ImageGeneration,
        ),
        // "draw" as verb (draw something)
        (
            Regex::new(r"(?i)^draw\s+.+").unwrap(),
            TaskCategory::ImageGeneration,
        ),
        (
            Regex::new(r"(?i)(生成|创建|写|制作).*(文档|报告|PPT|PDF|Excel)").unwrap(),
            TaskCategory::DocumentGeneration,
        ),
        (
            Regex::new(r"(?i)(generate|create|write|make).*(document|report|ppt|pdf|excel)")
                .unwrap(),
            TaskCategory::DocumentGeneration,
        ),
        // Web operations
        (
            Regex::new(r"(?i)(搜索|查找|搜一下|查一查|查询)").unwrap(),
            TaskCategory::WebSearch,
        ),
        (
            Regex::new(r"(?i)(search|look up|find|google)").unwrap(),
            TaskCategory::WebSearch,
        ),
        (
            Regex::new(r"(?i)(下载|获取).*(视频|音频|youtube|bilibili)").unwrap(),
            TaskCategory::MediaDownload,
        ),
    ]
});

/// Conversation intent patterns (match → Converse mode)
static CONVERSE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Questions - Chinese
        Regex::new(r"(?i)^(什么是|为什么|如何|怎么|怎样|能否解释)").unwrap(),
        Regex::new(r"(?i)(是什么意思|是什么$|怎么理解|什么区别)").unwrap(),
        Regex::new(r"(?i)(分析|解释|总结|概括|理解|说明).{0,10}(一下|给我|帮我)?$").unwrap(),
        // Questions - English
        Regex::new(r"(?i)^(what is|why|how|can you explain|could you)").unwrap(),
        Regex::new(r"(?i)(what does|what's|explain|describe|summarize)").unwrap(),
        // Analysis requests (read-only)
        Regex::new(r"(?i)(analyze|analyse|review|compare|evaluate)").unwrap(),
    ]
});

// =============================================================================
// ExecutionIntentDecider
// =============================================================================

/// Configuration for the decider
#[derive(Debug, Clone)]
pub struct DeciderConfig {
    /// Enable L3 semantic analysis (requires AI provider)
    pub enable_l3_semantic: bool,
    /// Default to execute when ambiguous
    pub default_to_execute: bool,
    /// Custom slash commands (extend built-in)
    pub custom_slash_commands: HashMap<String, String>,
}

impl Default for DeciderConfig {
    fn default() -> Self {
        Self {
            enable_l3_semantic: false,
            default_to_execute: true, // Bias toward action
            custom_slash_commands: HashMap::new(),
        }
    }
}

/// Context signals for L2 decision
#[derive(Debug, Clone, Default)]
pub struct ContextSignals {
    /// Selected file path (if any)
    pub selected_file: Option<String>,
    /// Active application name
    pub active_app: Option<String>,
    /// Current panel/mode in UI
    pub ui_mode: Option<String>,
    /// Clipboard content type
    pub clipboard_type: Option<String>,
}

/// Unified execution intent decider
///
/// Single decision point for "execute vs converse" determination.
pub struct ExecutionIntentDecider {
    config: DeciderConfig,
    /// Optional command parser for dynamic command resolution
    /// (supports skills, MCP, custom commands)
    command_parser: Option<Arc<CommandParser>>,
}

impl ExecutionIntentDecider {
    /// Create a new decider with default config
    pub fn new() -> Self {
        Self {
            config: DeciderConfig::default(),
            command_parser: None,
        }
    }

    /// Create a new decider with custom config
    pub fn with_config(config: DeciderConfig) -> Self {
        Self {
            config,
            command_parser: None,
        }
    }

    /// Set the command parser for dynamic command resolution
    ///
    /// This enables support for:
    /// - Skills (from SkillsRegistry)
    /// - MCP tools (from MCP servers)
    /// - Custom commands (from routing rules)
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.command_parser = Some(parser);
        self
    }

    /// Update the command parser
    pub fn set_command_parser(&mut self, parser: Arc<CommandParser>) {
        self.command_parser = Some(parser);
    }

    /// Decide execution mode for user input
    ///
    /// This is the main entry point. It runs through L0-L4 layers in order
    /// and returns as soon as a decision is made.
    pub fn decide(&self, input: &str, context: Option<&ContextSignals>) -> DecisionResult {
        let start = std::time::Instant::now();

        // L0: Slash commands (direct tool invocation)
        if let Some(result) = self.check_slash_command(input) {
            return self.finalize_result(result, IntentLayer::SlashCommand, start, Some(input));
        }

        // L1: Regex pattern matching
        if let Some((mode, pattern)) = self.check_regex_patterns(input) {
            return self.finalize_result(mode, IntentLayer::PatternMatch, start, Some(&pattern));
        }

        // L2: Context signals
        if let Some(ctx) = context {
            if let Some(mode) = self.check_context_signals(input, ctx) {
                return self.finalize_result(mode, IntentLayer::ContextSignal, start, None);
            }
        }

        // L3: Semantic analysis (if enabled)
        // Note: This would require async and an AI provider, simplified here
        // In production, this would be: self.check_semantic(input).await

        // L4: Default fallback
        let default_mode = if self.config.default_to_execute {
            // Bias toward execution - assume user wants action
            ExecutionMode::Execute(TaskCategory::General)
        } else {
            ExecutionMode::Converse
        };

        self.finalize_result(default_mode, IntentLayer::DefaultFallback, start, None)
    }

    /// Check for slash command (L0)
    ///
    /// Uses CommandParser if available for dynamic command resolution,
    /// otherwise falls back to built-in command lookup.
    fn check_slash_command(&self, input: &str) -> Option<ExecutionMode> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        // If we have a command parser, use it for full resolution
        if let Some(ref parser) = self.command_parser {
            if let Some(parsed) = parser.parse(trimmed) {
                return Some(self.parsed_command_to_mode(parsed));
            }
        }

        // Fallback: Parse command and args manually
        let without_slash = &trimmed[1..];
        let (cmd_name, args) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.to_lowercase(), rest.trim().to_string()),
            None => (without_slash.to_lowercase(), String::new()),
        };

        // Check custom commands from config
        if let Some(tool_id) = self.config.custom_slash_commands.get(&cmd_name) {
            return Some(ExecutionMode::DirectTool(ToolInvocation {
                tool_id: tool_id.clone(),
                args,
            }));
        }

        // Check built-in commands
        if let Some(cmd) = SLASH_COMMANDS.get(cmd_name.as_str()) {
            return Some(ExecutionMode::DirectTool(ToolInvocation {
                tool_id: cmd.tool_id.to_string(),
                args,
            }));
        }

        None
    }

    /// Convert a ParsedCommand to ExecutionMode
    fn parsed_command_to_mode(&self, cmd: ParsedCommand) -> ExecutionMode {
        let args = cmd.arguments.clone().unwrap_or_default();

        match cmd.source_type {
            ToolSourceType::Builtin => {
                // Built-in commands map to DirectTool
                if let CommandContext::Builtin { tool_name } = cmd.context {
                    ExecutionMode::DirectTool(ToolInvocation {
                        tool_id: tool_name,
                        args,
                    })
                } else {
                    ExecutionMode::DirectTool(ToolInvocation {
                        tool_id: cmd.command_name,
                        args,
                    })
                }
            }

            ToolSourceType::Skill => {
                // Skills need their instructions injected
                if let CommandContext::Skill {
                    skill_id,
                    instructions,
                    display_name,
                } = cmd.context
                {
                    ExecutionMode::Skill(SkillInvocation {
                        skill_id,
                        display_name,
                        instructions,
                        args,
                    })
                } else {
                    // Fallback to general execution
                    ExecutionMode::Execute(TaskCategory::General)
                }
            }

            ToolSourceType::Mcp => {
                // MCP commands route to MCP server
                if let CommandContext::Mcp {
                    server_name,
                    tool_name,
                } = cmd.context
                {
                    ExecutionMode::Mcp(McpInvocation {
                        server_name,
                        tool_name,
                        args,
                    })
                } else {
                    ExecutionMode::Mcp(McpInvocation {
                        server_name: cmd.command_name,
                        tool_name: None,
                        args,
                    })
                }
            }

            ToolSourceType::Custom => {
                // Custom commands have system prompts
                if let CommandContext::Custom {
                    system_prompt,
                    provider,
                    ..
                } = cmd.context
                {
                    ExecutionMode::Custom(CustomInvocation {
                        command_name: cmd.command_name,
                        system_prompt,
                        provider,
                        args,
                    })
                } else {
                    ExecutionMode::Execute(TaskCategory::General)
                }
            }

            ToolSourceType::Native => {
                // Legacy native tools
                ExecutionMode::DirectTool(ToolInvocation {
                    tool_id: cmd.command_name,
                    args,
                })
            }
        }
    }

    /// Check regex patterns (L1)
    fn check_regex_patterns(&self, input: &str) -> Option<(ExecutionMode, String)> {
        // Check conversation patterns first (higher priority for explicit questions)
        for pattern in CONVERSE_PATTERNS.iter() {
            if pattern.is_match(input) {
                return Some((ExecutionMode::Converse, pattern.to_string()));
            }
        }

        // Check execution patterns
        for (pattern, category) in EXECUTE_PATTERNS.iter() {
            if pattern.is_match(input) {
                return Some((ExecutionMode::Execute(*category), pattern.to_string()));
            }
        }

        None
    }

    /// Check context signals (L2)
    fn check_context_signals(
        &self,
        _input: &str,
        context: &ContextSignals,
    ) -> Option<ExecutionMode> {
        // If user has a file selected, likely wants to operate on it
        if let Some(ref file_path) = context.selected_file {
            let lower = file_path.to_lowercase();
            if lower.ends_with(".jpg")
                || lower.ends_with(".png")
                || lower.ends_with(".gif")
                || lower.ends_with(".webp")
            {
                return Some(ExecutionMode::Execute(TaskCategory::ImageGeneration));
            }
            return Some(ExecutionMode::Execute(TaskCategory::FileOperation));
        }

        // If clipboard has an image, likely wants image-related action
        if context.clipboard_type.as_deref() == Some("image") {
            return Some(ExecutionMode::Execute(TaskCategory::ImageGeneration));
        }

        None
    }

    /// Finalize the result with metadata
    fn finalize_result(
        &self,
        mode: ExecutionMode,
        layer: IntentLayer,
        start: std::time::Instant,
        matched: Option<&str>,
    ) -> DecisionResult {
        let latency_us = start.elapsed().as_micros() as u64;
        let confidence = match layer {
            IntentLayer::SlashCommand => 1.0,
            IntentLayer::PatternMatch => 0.95,
            IntentLayer::ContextSignal => 0.8,
            IntentLayer::SemanticAnalysis => 0.7,
            IntentLayer::DefaultFallback => 0.5,
        };

        DecisionResult {
            mode,
            metadata: DecisionMetadata {
                layer,
                confidence,
                latency_us,
                matched_pattern: matched.map(|s| s.to_string()),
            },
        }
    }

    /// Get list of available slash commands
    pub fn list_slash_commands(&self) -> Vec<(&'static str, &'static str)> {
        SLASH_COMMANDS
            .iter()
            .map(|(name, cmd)| (*name, cmd.description))
            .collect()
    }
}

impl Default for ExecutionIntentDecider {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_command_screenshot() {
        let decider = ExecutionIntentDecider::new();
        let result = decider.decide("/screenshot", None);

        assert!(matches!(result.mode, ExecutionMode::DirectTool(_)));
        assert_eq!(result.metadata.layer, IntentLayer::SlashCommand);
        assert_eq!(result.metadata.confidence, 1.0);

        if let ExecutionMode::DirectTool(inv) = result.mode {
            assert_eq!(inv.tool_id, "screenshot");
        }
    }

    #[test]
    fn test_slash_command_with_args() {
        let decider = ExecutionIntentDecider::new();
        let result = decider.decide("/search how to learn rust", None);

        if let ExecutionMode::DirectTool(inv) = result.mode {
            assert_eq!(inv.tool_id, "search");
            assert_eq!(inv.args, "how to learn rust");
        } else {
            panic!("Expected DirectTool");
        }
    }

    #[test]
    fn test_execute_pattern_chinese() {
        let decider = ExecutionIntentDecider::new();

        let result = decider.decide("帮我整理下载文件夹", None);
        assert!(matches!(
            result.mode,
            ExecutionMode::Execute(TaskCategory::FileOrganize)
        ));
        assert_eq!(result.metadata.layer, IntentLayer::PatternMatch);
    }

    #[test]
    fn test_execute_pattern_english() {
        let decider = ExecutionIntentDecider::new();

        let result = decider.decide("organize my downloads folder", None);
        assert!(matches!(
            result.mode,
            ExecutionMode::Execute(TaskCategory::FileOrganize)
        ));
    }

    #[test]
    fn test_converse_pattern_question() {
        let decider = ExecutionIntentDecider::new();

        let result = decider.decide("什么是机器学习？", None);
        assert!(matches!(result.mode, ExecutionMode::Converse));
        assert_eq!(result.metadata.layer, IntentLayer::PatternMatch);
    }

    #[test]
    fn test_converse_pattern_english() {
        let decider = ExecutionIntentDecider::new();

        let result = decider.decide("What is machine learning?", None);
        assert!(matches!(result.mode, ExecutionMode::Converse));
    }

    #[test]
    fn test_context_signal_file_selected() {
        let decider = ExecutionIntentDecider::new();
        let context = ContextSignals {
            selected_file: Some("/Users/test/photo.jpg".to_string()),
            ..Default::default()
        };

        let result = decider.decide("处理这个", Some(&context));
        assert!(matches!(
            result.mode,
            ExecutionMode::Execute(TaskCategory::ImageGeneration)
        ));
        assert_eq!(result.metadata.layer, IntentLayer::ContextSignal);
    }

    #[test]
    fn test_default_fallback() {
        let decider = ExecutionIntentDecider::new();

        // Ambiguous input with no clear pattern
        let result = decider.decide("hello there", None);
        assert!(matches!(
            result.mode,
            ExecutionMode::Execute(TaskCategory::General)
        ));
        assert_eq!(result.metadata.layer, IntentLayer::DefaultFallback);
    }

    #[test]
    fn test_default_to_converse() {
        let config = DeciderConfig {
            default_to_execute: false,
            ..Default::default()
        };
        let decider = ExecutionIntentDecider::with_config(config);

        let result = decider.decide("hello there", None);
        assert!(matches!(result.mode, ExecutionMode::Converse));
    }

    #[test]
    fn test_latency_l0() {
        let decider = ExecutionIntentDecider::new();
        let result = decider.decide("/screenshot", None);

        // L0 should be fast (allow some slack for CI environments)
        assert!(result.metadata.latency_us < 10_000); // < 10ms
    }

    #[test]
    fn test_image_generation_patterns() {
        let decider = ExecutionIntentDecider::new();

        let inputs = vec![
            "生成一张猫的图片",
            "画一只狗",
            "create an image of a mountain",
            "draw a cartoon character",
        ];

        for input in inputs {
            let result = decider.decide(input, None);
            assert!(
                matches!(
                    result.mode,
                    ExecutionMode::Execute(TaskCategory::ImageGeneration)
                ),
                "Failed for: {}",
                input
            );
        }
    }

    #[test]
    fn test_web_search_patterns() {
        let decider = ExecutionIntentDecider::new();

        let inputs = vec!["搜索一下Rust教程", "search for python tutorials"];

        for input in inputs {
            let result = decider.decide(input, None);
            assert!(
                matches!(
                    result.mode,
                    ExecutionMode::Execute(TaskCategory::WebSearch)
                ),
                "Failed for: {}",
                input
            );
        }
    }
}
