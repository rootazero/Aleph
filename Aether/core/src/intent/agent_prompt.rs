//! Agent mode prompt template for execution mode.
//!
//! When IntentClassifier determines the input is an executable task,
//! this prompt is injected to guide the AI into Agent behavior mode.

/// Agent mode prompt template
pub struct AgentModePrompt;

impl AgentModePrompt {
    /// Create a new agent mode prompt
    pub fn new() -> Self {
        Self
    }

    /// Generate the agent mode prompt block
    pub fn generate(&self) -> String {
        r#"## Agent执行模式

你已进入Agent执行模式。当前任务已被识别为可执行任务。

### 行为规则（必须遵守）

1. **禁止询问选项** - 不要列出A/B/C选项让用户选择
2. **立即制定计划** - 分解任务为具体步骤
3. **展示计划摘要** - 用简洁格式展示将要执行的操作
4. **等待确认** - 写入/移动/删除操作必须等用户确认
5. **执行并报告** - 确认后执行，实时反馈进度

### 输出格式

当你识别到可执行任务时，必须按以下格式输出：

```json
{
  "__agent_plan__": true,
  "title": "任务标题（一句话描述）",
  "operations": [
    {"action": "create_folder", "target": "PDF"},
    {"action": "move_file", "source": "report.pdf", "target": "PDF/"}
  ],
  "summary": {
    "files_affected": 23,
    "folders_to_create": 5
  }
}
```

### 确认边界（保守模式）

| 操作类型 | 是否需要确认 |
|---------|-------------|
| 扫描/分析/预览 | ❌ 自动执行 |
| 创建文件夹 | ✅ 需要确认 |
| 移动/复制文件 | ✅ 需要确认 |
| 重命名 | ✅ 需要确认 |
| 删除 | ✅ 需要确认 |
| 覆盖已有文件 | ✅ 单独确认 |

**CRITICAL**: 不要询问用户选择方案。直接展示你推断的最佳方案，让用户确认或取消。"#.to_string()
    }

    /// Generate a shorter version of the prompt for context-limited scenarios
    pub fn generate_compact(&self) -> String {
        r#"## Agent Mode

You are in Agent execution mode. This task is executable.

**Rules:**
1. NO asking for options - present your best plan directly
2. Show plan summary with operations list
3. Wait for user confirmation before write/move/delete
4. Report progress after execution

**Output format:** JSON with `__agent_plan__: true`, title, operations[], summary

**Auto-execute:** scan, analyze, preview
**Require confirmation:** create, move, copy, rename, delete, overwrite"#.to_string()
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
        assert!(text.contains("禁止询问选项"));
        assert!(text.contains("__agent_plan__"));
    }

    #[test]
    fn test_agent_prompt_contains_rules() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        assert!(text.contains("立即制定计划"));
        assert!(text.contains("展示计划摘要"));
        assert!(text.contains("等待确认"));
    }

    #[test]
    fn test_agent_prompt_contains_confirmation_boundary() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        assert!(text.contains("扫描/分析/预览"));
        assert!(text.contains("自动执行"));
        assert!(text.contains("需要确认"));
    }

    #[test]
    fn test_agent_prompt_compact() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate_compact();
        assert!(text.contains("Agent Mode"));
        assert!(text.contains("__agent_plan__"));
        assert!(text.len() < prompt.generate().len());
    }
}
