//! Plugin system integration
//!
//! Integrates plugins with Aether's core systems (Thinker, EventBus, AgentRegistry, MCP).

use std::path::Path;
use std::sync::Arc;

use crate::agents::{AgentDef, AgentMode, AgentRegistry};
use crate::plugins::components::hook::{matches_pattern, substitute_variables};
use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::{HookAction, HookEvent, LoadedPlugin, PluginAgent, PluginSkill};
use crate::runtimes::RuntimeRegistry;

// ============================================================================
// Skill Integration
// ============================================================================

/// Build skill instructions for prompt injection
///
/// This generates a markdown section describing available plugin skills
/// that can be appended to the system prompt.
pub fn build_skill_instructions(skills: &[PluginSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    output.push_str("\n\n## Available Plugin Skills\n\n");
    output.push_str("You have access to the following plugin skills. ");
    output.push_str("Use them when they match the user's intent:\n\n");

    for skill in skills {
        output.push_str(&format!(
            "### /{}\n**Description**: {}\n\n{}\n\n---\n\n",
            skill.qualified_name(),
            skill.description,
            skill.content
        ));
    }

    output
}

/// Build a single skill prompt for execution
///
/// Substitutes $ARGUMENTS and prepares the skill for LLM processing.
pub fn build_skill_prompt(skill: &PluginSkill, arguments: &str) -> String {
    let content = crate::plugins::components::skill::substitute_arguments(&skill.content, arguments);

    format!(
        "Execute the following skill instruction:\n\n---\n\n{}\n\n---\n\nUser context: {}",
        content,
        if arguments.is_empty() { "(none)" } else { arguments }
    )
}

// ============================================================================
// Agent Integration
// ============================================================================

/// Convert a plugin agent to Aether's AgentDef format
pub fn plugin_agent_to_agent_def(agent: &PluginAgent) -> AgentDef {
    // Build a system prompt that includes description and capabilities
    let mut full_prompt = format!(
        "# Agent: {}\n\n**Description**: {}\n\n",
        agent.agent_name, agent.description
    );

    if !agent.capabilities.is_empty() {
        full_prompt.push_str("**Capabilities**:\n");
        for cap in &agent.capabilities {
            full_prompt.push_str(&format!("- {}\n", cap));
        }
        full_prompt.push_str("\n");
    }

    full_prompt.push_str(&agent.system_prompt);

    AgentDef {
        id: agent.qualified_name(),
        mode: AgentMode::SubAgent,
        system_prompt: full_prompt,
        // Plugin agents get all tools by default
        allowed_tools: vec!["*".into()],
        denied_tools: vec![],
        max_iterations: Some(10),
    }
}

/// Register plugin agents with the agent registry
pub fn register_plugin_agents(
    registry: &AgentRegistry,
    agents: &[PluginAgent],
) -> PluginResult<Vec<String>> {
    let mut registered = Vec::new();

    for agent in agents {
        let agent_def = plugin_agent_to_agent_def(agent);
        let agent_id = agent_def.id.clone();

        // Check if agent already exists
        if registry.get(&agent_id).is_some() {
            tracing::warn!("Agent '{}' already exists, skipping", agent_id);
            continue;
        }

        registry.register(agent_def);
        tracing::info!("Registered plugin agent: {}", agent_id);
        registered.push(agent_id);
    }

    Ok(registered)
}

// ============================================================================
// Hook Integration
// ============================================================================

/// Hook executor for running hook actions
pub struct HookExecutor {
    plugin_root: std::path::PathBuf,
    runtime: Option<Arc<RuntimeRegistry>>,
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new(plugin_root: impl AsRef<Path>) -> Self {
        Self {
            plugin_root: plugin_root.as_ref().to_path_buf(),
            runtime: None,
        }
    }

    /// Set the runtime registry
    pub fn with_runtime(mut self, runtime: Arc<RuntimeRegistry>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Execute a hook action
    pub async fn execute(&self, action: &HookAction, context: &HookContext) -> PluginResult<HookResult> {
        match action {
            HookAction::Command { command } => {
                self.execute_command(command, context).await
            }
            HookAction::Prompt { prompt } => {
                self.execute_prompt(prompt, context).await
            }
            HookAction::Agent { agent } => {
                self.execute_agent(agent, context).await
            }
        }
    }

    /// Execute a shell command hook
    async fn execute_command(&self, command: &str, context: &HookContext) -> PluginResult<HookResult> {
        let resolved = substitute_variables(
            command,
            &self.plugin_root,
            context.arguments.as_deref(),
            context.file.as_deref(),
        );

        tracing::debug!("Executing hook command: {}", resolved);

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&resolved)
            .current_dir(&self.plugin_root)
            .output()
            .await
            .map_err(|e| PluginError::HookExecutionError(format!("Command failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            tracing::warn!("Hook command failed: {}", stderr);
        }

        Ok(HookResult {
            success: output.status.success(),
            output: Some(stdout),
            error: if stderr.is_empty() { None } else { Some(stderr) },
        })
    }

    /// Execute a prompt hook (returns prompt for LLM evaluation)
    async fn execute_prompt(&self, prompt: &str, context: &HookContext) -> PluginResult<HookResult> {
        let resolved = substitute_variables(
            prompt,
            &self.plugin_root,
            context.arguments.as_deref(),
            context.file.as_deref(),
        );

        // Prompt hooks return the prompt for the caller to send to LLM
        Ok(HookResult {
            success: true,
            output: Some(resolved),
            error: None,
        })
    }

    /// Execute an agent hook (returns agent name for the caller to invoke)
    async fn execute_agent(&self, agent: &str, _context: &HookContext) -> PluginResult<HookResult> {
        // Agent hooks return the agent name for the caller to invoke
        Ok(HookResult {
            success: true,
            output: Some(agent.to_string()),
            error: None,
        })
    }
}

/// Context for hook execution
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// Arguments/context for the hook
    pub arguments: Option<String>,
    /// File path if applicable
    pub file: Option<String>,
    /// Tool name for tool-related hooks
    pub tool_name: Option<String>,
}

/// Result of hook execution
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Whether the hook succeeded
    pub success: bool,
    /// Output from the hook
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

/// Hook handler for event bus integration
pub struct PluginHookHandler {
    /// Plugin name
    plugin_name: String,
    /// Plugin root path
    plugin_root: std::path::PathBuf,
    /// Hooks configuration
    hooks: crate::plugins::types::PluginHooksConfig,
}

impl PluginHookHandler {
    /// Create a new hook handler
    pub fn new(plugin: &LoadedPlugin) -> Self {
        Self {
            plugin_name: plugin.manifest.name.clone(),
            plugin_root: plugin.path.clone(),
            hooks: plugin.hooks.clone(),
        }
    }

    /// Get the plugin name
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    /// Get the plugin root path
    pub fn plugin_root(&self) -> &std::path::Path {
        &self.plugin_root
    }

    /// Get matchers for a specific event
    pub fn get_matchers(&self, event: HookEvent) -> Option<&Vec<crate::plugins::types::HookMatcher>> {
        self.hooks.hooks.get(&event)
    }

    /// Check if an event should trigger hooks
    pub fn should_trigger(&self, event: HookEvent, value: Option<&str>) -> bool {
        if let Some(matchers) = self.get_matchers(event) {
            for matcher in matchers {
                if let Some(v) = value {
                    if matches_pattern(&matcher.matcher, v) {
                        return true;
                    }
                } else if matcher.matcher.is_none() {
                    return true;
                }
            }
        }
        false
    }

    /// Create a HookExecutor for this plugin
    pub fn create_executor(&self) -> HookExecutor {
        HookExecutor::new(&self.plugin_root)
    }
}

// ============================================================================
// MCP Integration
// ============================================================================

/// Resolve MCP server command to use Aether's runtime
pub fn resolve_mcp_command(
    command: &str,
    runtime: Option<&RuntimeRegistry>,
) -> std::path::PathBuf {
    match command {
        "npx" | "node" => {
            if let Some(rt) = runtime {
                if let Some(fnm) = rt.get("fnm") {
                    // Get the executable path from fnm runtime
                    let exec_path = fnm.executable_path();
                    if exec_path.exists() {
                        // For npx, try to find it in the same directory
                        if command == "npx" {
                            if let Some(parent) = exec_path.parent() {
                                let npx_path = parent.join("npx");
                                if npx_path.exists() {
                                    return npx_path;
                                }
                            }
                        }
                        return exec_path;
                    }
                }
            }
            // Fallback to PATH lookup
            find_in_path(command)
        }
        "uvx" | "python" | "python3" => {
            if let Some(rt) = runtime {
                if let Some(uv) = rt.get("uv") {
                    let exec_path = uv.executable_path();
                    if exec_path.exists() {
                        // For uvx, try to find it in the same directory
                        if command == "uvx" {
                            if let Some(parent) = exec_path.parent() {
                                let uvx_path = parent.join("uvx");
                                if uvx_path.exists() {
                                    return uvx_path;
                                }
                            }
                        }
                        return exec_path;
                    }
                }
            }
            // Fallback to PATH lookup
            find_in_path(command)
        }
        other => {
            // For other commands, try to find in PATH
            find_in_path(other)
        }
    }
}

/// Find a command in PATH or return as-is
fn find_in_path(command: &str) -> std::path::PathBuf {
    // Try common locations first
    let common_paths = [
        "/usr/local/bin",
        "/usr/bin",
        "/opt/homebrew/bin",
    ];

    for dir in common_paths {
        let path = std::path::Path::new(dir).join(command);
        if path.exists() {
            return path;
        }
    }

    // Return as-is, let the OS figure it out
    std::path::PathBuf::from(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::SkillType;

    #[test]
    fn test_build_skill_instructions_empty() {
        let result = build_skill_instructions(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_skill_instructions() {
        let skills = vec![
            PluginSkill {
                plugin_name: "test".to_string(),
                skill_name: "hello".to_string(),
                skill_type: SkillType::Skill,
                description: "Say hello".to_string(),
                content: "Greet the user".to_string(),
                disable_model_invocation: false,
            },
        ];

        let result = build_skill_instructions(&skills);
        assert!(result.contains("test:hello"));
        assert!(result.contains("Say hello"));
        assert!(result.contains("Greet the user"));
    }

    #[test]
    fn test_build_skill_prompt() {
        let skill = PluginSkill {
            plugin_name: "test".to_string(),
            skill_name: "greet".to_string(),
            skill_type: SkillType::Command,
            description: "Greet someone".to_string(),
            content: "Say hello to $ARGUMENTS".to_string(),
            disable_model_invocation: true,
        };

        let result = build_skill_prompt(&skill, "World");
        assert!(result.contains("Say hello to World"));
        assert!(result.contains("User context: World"));
    }

    #[test]
    fn test_plugin_agent_to_agent_def() {
        let agent = PluginAgent {
            plugin_name: "my-plugin".to_string(),
            agent_name: "reviewer".to_string(),
            description: "Code reviewer".to_string(),
            capabilities: vec!["review".to_string()],
            system_prompt: "You are a code reviewer.".to_string(),
        };

        let def = plugin_agent_to_agent_def(&agent);
        assert_eq!(def.id, "my-plugin:reviewer");
        assert!(matches!(def.mode, AgentMode::SubAgent));
        assert!(def.system_prompt.contains("Code reviewer"));
        assert!(def.system_prompt.contains("You are a code reviewer"));
    }

    #[test]
    fn test_hook_context_default() {
        let ctx = HookContext::default();
        assert!(ctx.arguments.is_none());
        assert!(ctx.file.is_none());
        assert!(ctx.tool_name.is_none());
    }
}
