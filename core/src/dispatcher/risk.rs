//! Risk evaluation for task execution
//!
//! This module provides risk assessment for tasks and task graphs,
//! determining whether user confirmation is required before execution.

use regex::Regex;
use std::sync::OnceLock;

use crate::dispatcher::agent_types::{FileOp, Task, TaskGraph, TaskType};

/// Risk level for task execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Low risk - can be executed automatically
    Low,
    /// High risk - requires user confirmation
    High,
}

/// Risk evaluator for tasks and task graphs
#[derive(Debug, Clone)]
pub struct RiskEvaluator {
    /// Whether to use pattern-based evaluation
    use_patterns: bool,
}

/// Get high-risk patterns (lazily initialized)
fn get_high_risk_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // Network/API patterns
            Regex::new(r"(?i)(api|http|request|fetch|curl|wget)").unwrap(),
            // Execution patterns
            Regex::new(r"(?i)(execute|run|eval|shell|command|exec)").unwrap(),
            // File modification patterns
            Regex::new(r"(?i)(write|delete|remove|modify|create)\s*(file|文件)").unwrap(),
            // Send/Upload patterns
            Regex::new(r"(?i)(send|post|upload|publish|发送|上传)").unwrap(),
            // Financial patterns
            Regex::new(r"(?i)(pay|purchase|transaction|transfer|支付|购买|转账)").unwrap(),
            // Chinese API patterns
            Regex::new(r"(?i)(调用|请求|接口)").unwrap(),
        ]
    })
}

impl RiskEvaluator {
    /// Create a new risk evaluator
    pub fn new() -> Self {
        Self { use_patterns: true }
    }

    /// Evaluate the risk level of a single task
    pub fn evaluate(&self, task: &Task) -> RiskLevel {
        // First check task type
        let type_risk = self.evaluate_task_type(&task.task_type);
        if type_risk == RiskLevel::High {
            return RiskLevel::High;
        }

        // Then check patterns in name and description
        if self.use_patterns {
            if self.matches_high_risk_pattern(&task.name) {
                return RiskLevel::High;
            }
            if let Some(ref desc) = task.description {
                if self.matches_high_risk_pattern(desc) {
                    return RiskLevel::High;
                }
            }
        }

        RiskLevel::Low
    }

    /// Evaluate whether the task graph contains any high-risk tasks
    pub fn evaluate_graph(&self, graph: &TaskGraph) -> bool {
        graph.tasks.iter().any(|task| self.evaluate(task) == RiskLevel::High)
    }

    /// Get all high-risk tasks from a task graph
    pub fn get_high_risk_tasks<'a>(&self, graph: &'a TaskGraph) -> Vec<&'a Task> {
        graph
            .tasks
            .iter()
            .filter(|task| self.evaluate(task) == RiskLevel::High)
            .collect()
    }

    /// Evaluate risk based on task type
    fn evaluate_task_type(&self, task_type: &TaskType) -> RiskLevel {
        match task_type {
            // Code execution is always high risk
            TaskType::CodeExecution(_) => RiskLevel::High,

            // App automation is always high risk
            TaskType::AppAutomation(_) => RiskLevel::High,

            // File operations depend on the specific operation
            TaskType::FileOperation(file_op) => self.evaluate_file_op(file_op),

            // AI inference is low risk
            TaskType::AiInference(_) => RiskLevel::Low,

            // Document generation is low risk
            TaskType::DocumentGeneration(_) => RiskLevel::Low,

            // Generation tasks are low risk (they don't modify files or execute code)
            TaskType::ImageGeneration(_) => RiskLevel::Low,
            TaskType::VideoGeneration(_) => RiskLevel::Low,
            TaskType::AudioGeneration(_) => RiskLevel::Low,
        }
    }

    /// Evaluate risk based on file operation type
    fn evaluate_file_op(&self, file_op: &FileOp) -> RiskLevel {
        match file_op {
            // Write, delete, move operations are high risk
            FileOp::Write { .. } => RiskLevel::High,
            FileOp::Delete { .. } => RiskLevel::High,
            FileOp::Move { .. } => RiskLevel::High,
            FileOp::BatchMove { .. } => RiskLevel::High,

            // Read, list, search, copy operations are low risk
            FileOp::Read { .. } => RiskLevel::Low,
            FileOp::List { .. } => RiskLevel::Low,
            FileOp::Search { .. } => RiskLevel::Low,
            FileOp::Copy { .. } => RiskLevel::Low,
        }
    }

    /// Check if text matches any high-risk pattern
    fn matches_high_risk_pattern(&self, text: &str) -> bool {
        get_high_risk_patterns()
            .iter()
            .any(|pattern| pattern.is_match(text))
    }
}

impl Default for RiskEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{AiTask, AppAuto, CodeExec, DocGen, Language};
    use std::path::PathBuf;

    fn create_task(id: &str, name: &str, task_type: TaskType) -> Task {
        Task::new(id, name, task_type)
    }

    #[test]
    fn test_evaluate_low_risk() {
        let evaluator = RiskEvaluator::new();

        // AI inference is low risk
        let task = create_task(
            "ai_1",
            "Analyze text",
            TaskType::AiInference(AiTask {
                prompt: "Summarize this document".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);

        // Document generation is low risk
        let task = create_task(
            "doc_1",
            "Generate report",
            TaskType::DocumentGeneration(DocGen::Pdf {
                style: None,
                output: PathBuf::from("/tmp/report.pdf"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);

        // File read is low risk
        let task = create_task(
            "file_1",
            "Read config",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/etc/config"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_high_risk_api() {
        let evaluator = RiskEvaluator::new();

        // Task with API in name
        let task = create_task(
            "api_1",
            "Call external API",
            TaskType::AiInference(AiTask {
                prompt: "test".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Task with HTTP in name
        let task = create_task(
            "http_1",
            "Make HTTP request",
            TaskType::AiInference(AiTask {
                prompt: "test".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Task with Chinese API pattern
        let task = create_task(
            "cn_api_1",
            "调用第三方接口",
            TaskType::AiInference(AiTask {
                prompt: "test".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_high_risk_execute() {
        let evaluator = RiskEvaluator::new();

        // Code execution is always high risk
        let task = create_task(
            "exec_1",
            "Run script",
            TaskType::CodeExecution(CodeExec::Script {
                code: "print('hello')".into(),
                language: Language::Python,
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Shell command is high risk
        let task = create_task(
            "cmd_1",
            "Execute command",
            TaskType::CodeExecution(CodeExec::Command {
                cmd: "ls".into(),
                args: vec!["-la".into()],
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // App automation is high risk
        let task = create_task(
            "auto_1",
            "Launch app",
            TaskType::AppAutomation(AppAuto::Launch {
                bundle_id: "com.apple.Safari".into(),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_file_operations() {
        let evaluator = RiskEvaluator::new();

        // Write is high risk
        let task = create_task(
            "write_1",
            "Save file",
            TaskType::FileOperation(FileOp::Write {
                path: PathBuf::from("/tmp/test.txt"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Delete is high risk
        let task = create_task(
            "delete_1",
            "Remove file",
            TaskType::FileOperation(FileOp::Delete {
                path: PathBuf::from("/tmp/test.txt"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Move is high risk
        let task = create_task(
            "move_1",
            "Move file",
            TaskType::FileOperation(FileOp::Move {
                from: PathBuf::from("/tmp/a.txt"),
                to: PathBuf::from("/tmp/b.txt"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);

        // Read is low risk
        let task = create_task(
            "read_1",
            "Read file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/tmp/test.txt"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);

        // List is low risk
        let task = create_task(
            "list_1",
            "List directory",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);

        // Search is low risk
        let task = create_task(
            "search_1",
            "Find files",
            TaskType::FileOperation(FileOp::Search {
                pattern: "*.txt".into(),
                dir: PathBuf::from("/tmp"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);

        // Copy is low risk
        let task = create_task(
            "copy_1",
            "Copy file",
            TaskType::FileOperation(FileOp::Copy {
                from: PathBuf::from("/tmp/a.txt"),
                to: PathBuf::from("/tmp/b.txt"),
            }),
        );
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_graph() {
        let evaluator = RiskEvaluator::new();

        // Graph with only low-risk tasks
        let mut graph = TaskGraph::new("graph_1", "Test Graph");
        graph.add_task(create_task(
            "task_1",
            "Read file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/tmp/test.txt"),
            }),
        ));
        graph.add_task(create_task(
            "task_2",
            "Analyze content",
            TaskType::AiInference(AiTask {
                prompt: "Analyze".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        ));
        assert!(!evaluator.evaluate_graph(&graph));
        assert!(evaluator.get_high_risk_tasks(&graph).is_empty());

        // Graph with high-risk tasks
        graph.add_task(create_task(
            "task_3",
            "Execute script",
            TaskType::CodeExecution(CodeExec::Script {
                code: "print('test')".into(),
                language: Language::Python,
            }),
        ));
        assert!(evaluator.evaluate_graph(&graph));

        let high_risk_tasks = evaluator.get_high_risk_tasks(&graph);
        assert_eq!(high_risk_tasks.len(), 1);
        assert_eq!(high_risk_tasks[0].id, "task_3");
    }
}
