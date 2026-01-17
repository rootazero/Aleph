//! Agent mode prompt template for execution mode.
//!
//! When IntentClassifier determines the input is an executable task,
//! this prompt is injected to guide the AI into Agent behavior mode.

/// Tool description for prompt generation
#[derive(Debug, Clone)]
pub struct ToolDescription {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
}

impl ToolDescription {
    /// Create a new tool description
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Agent mode prompt template
pub struct AgentModePrompt {
    /// Available tools
    tools: Vec<ToolDescription>,
}

impl AgentModePrompt {
    /// Create a new agent mode prompt without tools
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Create a new agent mode prompt with tools
    pub fn with_tools(tools: Vec<ToolDescription>) -> Self {
        Self { tools }
    }

    /// Generate the agent mode prompt block
    ///
    /// Includes available tools list so AI knows what it can use.
    pub fn generate(&self) -> String {
        let tools_section = if self.tools.is_empty() {
            String::new()
        } else {
            let tool_list: Vec<String> = self
                .tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect();
            format!("\n\n### 可用工具\n\n{}", tool_list.join("\n"))
        };

        format!(
            r#"## Agent执行模式

你是一个能够执行任务的AI助手。你必须使用工具来完成用户请求。{}

### 行为规则（必须严格遵守）

1. **先分析** - 使用 file_ops 的 list 操作查看文件夹内容
2. **展示计划并等待确认** - 分析完成后，必须：
   - 列出将要执行的操作（如：将 X 个图片移动到 Images 文件夹）
   - 明确询问用户"是否执行？(Y/N)"
   - **等待用户回复确认后才能执行**
3. **用户确认后执行** - 只有收到用户明确确认（如"Y"、"是"、"确认"、"执行"）后，才能调用 organize/batch_move/move/delete 等操作
4. **批量操作** - 执行时优先使用:
   - `organize`: 一键按类型整理到 Images/Documents/Videos/Audio/Archives/Code/Others
   - `batch_move`: 批量移动匹配模式的文件
5. **报告结果** - 执行后报告整理结果

### 重要提示

- 你可以直接访问用户的本地文件系统
- **禁止未经确认直接执行文件移动/删除操作**
- 必须先展示计划，等用户说"Y"或"确认"后才能执行
- 如果用户说"N"或"取消"，则放弃操作"#,
            tools_section
        )
    }

    /// Generate a shorter version of the prompt for context-limited scenarios
    pub fn generate_compact(&self) -> String {
        r#"## Agent Mode

You are a task-executing AI assistant with available tools. You MUST use tools to complete tasks.

**Available Tools:**
- file_ops: File operations (list, read, write, move, copy, delete, mkdir, search, batch_move, organize)
- search: Web search
- web_fetch: Fetch web page content

**Rules:**
1. Use tools proactively - don't just describe
2. Analyze first (file_ops list), then execute
3. **Use batch operations**: 'organize' for auto-sorting, 'batch_move' for pattern-based moves
4. Confirm before delete operations
5. Report results after execution

**Important:** You CAN access local files. Use 'organize' to auto-sort files by type in one call!"#.to_string()
    }
}

impl Default for AgentModePrompt {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_prompt_generation() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        assert!(text.contains("Agent执行模式"));
        assert!(text.contains("使用工具来完成用户请求"));
        assert!(text.contains("行为规则"));
    }

    #[test]
    fn test_agent_prompt_contains_behavior_rules() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        assert!(text.contains("先分析"));
        assert!(text.contains("批量操作"));
        assert!(text.contains("organize"));
        assert!(text.contains("报告结果"));
        // Verify confirmation workflow
        assert!(text.contains("等待确认"));
        assert!(text.contains("禁止未经确认"));
    }

    #[test]
    fn test_agent_prompt_compact() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate_compact();
        assert!(text.contains("Agent Mode"));
        assert!(text.contains("Use tools proactively"));
        // Compact version has hardcoded tool list
        assert!(text.contains("file_ops"));
    }

    #[test]
    fn test_agent_prompt_with_tools() {
        let tools = vec![
            ToolDescription::new("test_tool", "A test tool for testing"),
            ToolDescription::new("another_tool", "Another tool"),
        ];
        let prompt = AgentModePrompt::with_tools(tools);
        let text = prompt.generate();

        // Should contain tool section
        assert!(text.contains("可用工具"));
        assert!(text.contains("test_tool"));
        assert!(text.contains("A test tool for testing"));
        assert!(text.contains("another_tool"));
    }

    #[test]
    fn test_agent_prompt_without_tools() {
        // When no tools provided, should not have tool section
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // Should still have important instructions
        assert!(text.contains("你可以直接访问用户的本地文件系统"));
        assert!(text.contains("file_ops"));
    }

    #[test]
    fn test_agent_prompt_no_parameter_details() {
        // Prompt should NOT contain detailed tool parameter descriptions
        // Parameter details are handled by rig-core function calling
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // Should not have JSON schema style parameter descriptions
        assert!(!text.contains("\"type\": \"string\""));
        assert!(!text.contains("\"required\":"));
    }
}
