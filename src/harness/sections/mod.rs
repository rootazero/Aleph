//! Prompt sections — modular content for PromptBuilder.
//!
//! Static sections are compiled into the binary via `include_str!`.
//! Dynamic sections are rendered at runtime from `SessionContext`.

use std::collections::HashSet;

// =============================================================================
// Static section content (compiled-in .md files)
// =============================================================================

pub const TASK_PHILOSOPHY: &str = include_str!("task_philosophy.md");
pub const RISK_ACTIONS: &str = include_str!("risk_actions.md");
pub const TOOL_GRAMMAR: &str = include_str!("tool_grammar.md");
pub const OUTPUT_STYLE: &str = include_str!("output_style.md");
pub const PERSISTENCE: &str = include_str!("persistence.md");

// =============================================================================
// Conditional guidance content
// =============================================================================

const BROWSER_GUIDANCE: &str = include_str!("guidance/browser.md");
const CODE_EXEC_GUIDANCE: &str = include_str!("guidance/code_exec.md");
const SUBAGENT_GUIDANCE: &str = include_str!("guidance/subagent.md");

// =============================================================================
// SessionContext
// =============================================================================

/// Runtime context injected into dynamic prompt sections.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    /// Operating system name (e.g. "macos", "linux")
    pub os: String,
    /// User's shell (e.g. "/bin/zsh")
    pub shell: String,
    /// Current working directory
    pub cwd: String,
    /// Current git branch, if in a git repository
    pub git_branch: Option<String>,
    /// User's preferred language (e.g. "zh-CN", "en")
    pub language: String,
}

// =============================================================================
// Dynamic section renderers
// =============================================================================

use crate::agent_loop::ToolInfo;

/// Render environment info section from SessionContext.
pub fn render_environment(ctx: &SessionContext) -> String {
    let mut lines = vec![
        format!("- OS: {}", ctx.os),
        format!("- Shell: {}", ctx.shell),
        format!("- Working Directory: {}", ctx.cwd),
    ];
    if let Some(ref branch) = ctx.git_branch {
        lines.push(format!("- Git Branch: {}", branch));
    }
    if !ctx.language.is_empty() {
        lines.push(format!("- Language: {}", ctx.language));
    }
    lines.join("\n")
}

/// Render session-specific guidance based on available tools.
///
/// Returns `None` if no tool-specific guidance applies.
pub fn render_session_guidance(tools: &[ToolInfo]) -> Option<String> {
    let tool_names: HashSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let mut parts: Vec<&str> = Vec::new();

    if tool_names.iter().any(|name| name.starts_with("browser_")) {
        parts.push(BROWSER_GUIDANCE);
    }
    if tool_names.contains("code_exec") || tool_names.contains("bash") {
        parts.push(CODE_EXEC_GUIDANCE);
    }
    if tool_names.contains("subagent") {
        parts.push(SUBAGENT_GUIDANCE);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_sections_are_non_empty() {
        assert!(!TASK_PHILOSOPHY.is_empty());
        assert!(!RISK_ACTIONS.is_empty());
        assert!(!TOOL_GRAMMAR.is_empty());
        assert!(!OUTPUT_STYLE.is_empty());
        assert!(!PERSISTENCE.is_empty());
    }

    #[test]
    fn render_environment_basic() {
        let ctx = SessionContext {
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            cwd: "/home/user/project".into(),
            git_branch: Some("main".into()),
            language: "zh-CN".into(),
        };
        let result = render_environment(&ctx);
        assert!(result.contains("macos"));
        assert!(result.contains("/bin/zsh"));
        assert!(result.contains("main"));
        assert!(result.contains("zh-CN"));
    }

    #[test]
    fn render_environment_no_git() {
        let ctx = SessionContext {
            os: "linux".into(),
            shell: "/bin/bash".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            language: String::new(),
        };
        let result = render_environment(&ctx);
        assert!(!result.contains("Git Branch"));
        assert!(!result.contains("Language"));
    }

    #[test]
    fn session_guidance_empty_when_no_matching_tools() {
        let tools = vec![ToolInfo {
            name: "memory_store".into(),
            description: "Store memory".into(),
            parameters_schema: None,
            usage_hint: None,
        }];
        assert!(render_session_guidance(&tools).is_none());
    }

    #[test]
    fn session_guidance_includes_browser_when_present() {
        let tools = vec![ToolInfo {
            name: "browser_open".into(),
            description: "Open browser".into(),
            parameters_schema: None,
            usage_hint: None,
        }];
        let result = render_session_guidance(&tools).unwrap();
        assert!(result.contains("browser") || result.contains("Browser"));
    }

    #[test]
    fn session_guidance_includes_subagent_when_present() {
        let tools = vec![ToolInfo {
            name: "subagent".into(),
            description: "Run subagent".into(),
            parameters_schema: None,
            usage_hint: None,
        }];
        let result = render_session_guidance(&tools).unwrap();
        assert!(
            result.contains("subagent")
                || result.contains("Subagent")
                || result.contains("sub-agent")
        );
    }
}
