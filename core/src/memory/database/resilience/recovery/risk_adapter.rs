//! Task Risk Adapter
//!
//! Bridges the dispatcher's RiskEvaluator with the resilience layer's RiskLevel.
//! Provides unified risk evaluation for task creation and recovery decisions.

use crate::dispatcher::agent_types::Task;
use crate::dispatcher::risk::RiskEvaluator as DispatcherRiskEvaluator;
use crate::dispatcher::risk::RiskLevel as DispatcherRiskLevel;
use crate::memory::database::resilience::RiskLevel;

/// Adapter for evaluating task risk and converting to persistence-compatible format
pub struct TaskRiskAdapter {
    evaluator: DispatcherRiskEvaluator,
}

impl TaskRiskAdapter {
    /// Create a new task risk adapter
    pub fn new() -> Self {
        Self {
            evaluator: DispatcherRiskEvaluator::new(),
        }
    }

    /// Evaluate a dispatcher Task and return persistence-compatible RiskLevel
    pub fn evaluate_task(&self, task: &Task) -> RiskLevel {
        match self.evaluator.evaluate(task) {
            DispatcherRiskLevel::High => RiskLevel::High,
            DispatcherRiskLevel::Low => RiskLevel::Low,
        }
    }

    /// Evaluate risk level from task prompt text using pattern matching
    pub fn evaluate_prompt(&self, prompt: &str) -> RiskLevel {
        // Use heuristics based on common high-risk patterns
        let high_risk_patterns = [
            // File modification
            "write", "delete", "remove", "modify", "create file",
            "写入", "删除", "修改", "创建文件",
            // Execution
            "execute", "run command", "shell", "bash", "exec",
            "执行", "运行",
            // Network/API
            "send", "post", "upload", "api call",
            "发送", "上传", "调用",
            // Financial
            "pay", "purchase", "transfer",
            "支付", "购买", "转账",
        ];

        let prompt_lower = prompt.to_lowercase();
        for pattern in high_risk_patterns {
            if prompt_lower.contains(pattern) {
                return RiskLevel::High;
            }
        }

        RiskLevel::Low
    }

    /// Evaluate risk from a list of tool names
    /// Tools that modify state are considered high risk
    pub fn evaluate_tools(&self, tool_names: &[&str]) -> RiskLevel {
        let high_risk_tools = [
            "write_file", "edit_file", "delete_file",
            "bash", "shell", "execute",
            "send_message", "send_email",
            "browser_action", "click", "type",
        ];

        for tool in tool_names {
            let tool_lower = tool.to_lowercase();
            for high_risk in high_risk_tools {
                if tool_lower.contains(high_risk) {
                    return RiskLevel::High;
                }
            }
        }

        RiskLevel::Low
    }
}

impl Default for TaskRiskAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_prompt_low_risk() {
        let adapter = TaskRiskAdapter::new();
        assert_eq!(adapter.evaluate_prompt("search for files"), RiskLevel::Low);
        assert_eq!(adapter.evaluate_prompt("analyze the code"), RiskLevel::Low);
        assert_eq!(adapter.evaluate_prompt("read the document"), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_prompt_high_risk() {
        let adapter = TaskRiskAdapter::new();
        assert_eq!(adapter.evaluate_prompt("delete all files"), RiskLevel::High);
        assert_eq!(adapter.evaluate_prompt("execute bash command"), RiskLevel::High);
        assert_eq!(adapter.evaluate_prompt("send email to user"), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_tools_low_risk() {
        let adapter = TaskRiskAdapter::new();
        assert_eq!(adapter.evaluate_tools(&["read_file", "search"]), RiskLevel::Low);
        assert_eq!(adapter.evaluate_tools(&["grep", "list_files"]), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_tools_high_risk() {
        let adapter = TaskRiskAdapter::new();
        assert_eq!(adapter.evaluate_tools(&["read_file", "write_file"]), RiskLevel::High);
        assert_eq!(adapter.evaluate_tools(&["bash"]), RiskLevel::High);
        assert_eq!(adapter.evaluate_tools(&["send_message"]), RiskLevel::High);
    }
}
