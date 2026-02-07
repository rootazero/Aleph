//! Integration layer for PriorityScheduler with Dispatcher and BrowserPool
//!
//! This module provides helper functions and types for integrating the
//! PriorityScheduler with the existing Dispatcher and future BrowserPool.

use super::priority::{PriorityTier, RiskLevel, TaskMetadata};
use crate::dispatcher::agent_types::{Task, TaskType};

/// Detect if a task is a browser task
///
/// Note: BrowserAction and WebScraping task types will be added in future phases
/// of the Liquid Hub architecture. This function currently returns false for all
/// tasks but provides the framework for future browser task detection.
pub fn is_browser_task(task: &Task) -> bool {
    // TODO: When browser task types are added, update this to:
    // matches!(
    //     task.task_type,
    //     TaskType::BrowserAction { .. } | TaskType::WebScraping { .. }
    // )

    // For now, check task description for browser-related keywords
    if let Some(desc) = &task.description {
        desc.contains("browser") || desc.contains("web") || desc.contains("navigate")
    } else {
        false
    }
}

/// Extract task metadata for priority scheduling
pub fn extract_task_metadata(task: &Task) -> TaskMetadata {
    // Determine priority tier based on task type and description
    let tier = match &task.task_type {
        // AI inference tasks get user priority (interactive)
        TaskType::AiInference(_) => PriorityTier::User,

        // Code execution and app automation get financial priority (important)
        TaskType::CodeExecution(_) | TaskType::AppAutomation(_) => PriorityTier::Financial,

        // Everything else is background
        _ => PriorityTier::Background,
    };

    // Extract domain from task metadata
    let domain = extract_domain_from_task(task);

    // Determine risk level
    let risk_level = determine_risk_level(task);

    TaskMetadata::new(tier, domain, risk_level)
}

/// Extract domain from task
fn extract_domain_from_task(task: &Task) -> Option<String> {
    // Try to extract domain from task description or metadata
    // This is a simplified implementation
    if let Some(desc) = &task.description {
        // Look for domain patterns in description
        if desc.contains("example.com") {
            return Some("example.com".to_string());
        }
        if desc.contains("bank.com") {
            return Some("bank.com".to_string());
        }
    }

    None
}

/// Determine risk level for a task
fn determine_risk_level(task: &Task) -> RiskLevel {
    // Check task description for risk indicators
    if let Some(desc) = &task.description {
        if desc.contains("payment")
            || desc.contains("transaction")
            || desc.contains("transfer")
            || desc.contains("delete")
        {
            return RiskLevel::High;
        }

        if desc.contains("form")
            || desc.contains("submit")
            || desc.contains("login")
            || desc.contains("write")
        {
            return RiskLevel::Medium;
        }
    }

    // Code execution is inherently risky
    if matches!(task.task_type, TaskType::CodeExecution(_)) {
        return RiskLevel::High;
    }

    RiskLevel::Low
}

/// Browser task execution context
#[derive(Debug, Clone)]
pub struct BrowserTaskContext {
    /// Task ID
    pub task_id: String,

    /// Browser context ID (for CDP operations)
    pub context_id: Option<String>,

    /// Whether the task is currently frozen
    pub is_frozen: bool,
}

impl BrowserTaskContext {
    /// Create a new browser task context
    pub fn new(task_id: String) -> Self {
        Self {
            task_id,
            context_id: None,
            is_frozen: false,
        }
    }

    /// Set the browser context ID
    pub fn with_context_id(mut self, context_id: String) -> Self {
        self.context_id = Some(context_id);
        self
    }
}

/// BrowserPool integration stubs
///
/// These methods will be implemented when BrowserPool is integrated
pub mod browser_pool_stubs {
    use super::BrowserTaskContext;
    use crate::error::Result;

    /// Execute a task with priority scheduling
    ///
    /// This is a stub that will be implemented when BrowserPool is integrated
    pub async fn execute_task_with_priority(
        _context: &BrowserTaskContext,
        _task_id: &str,
    ) -> Result<()> {
        // TODO: Implement actual browser task execution
        // This will:
        // 1. Get or create a browser context
        // 2. Execute the task in the context
        // 3. Handle errors and retries
        Ok(())
    }

    /// Freeze a browser context (suspend execution)
    ///
    /// This is a stub that will be implemented when BrowserPool is integrated
    /// Uses CDP Debugger.pause to freeze JavaScript execution
    pub async fn freeze_context(_context_id: &str) -> Result<()> {
        // TODO: Implement CDP Debugger.pause
        // This will:
        // 1. Send Debugger.pause command to the context
        // 2. Wait for confirmation
        // 3. Mark context as frozen
        Ok(())
    }

    /// Resume a frozen browser context
    ///
    /// This is a stub that will be implemented when BrowserPool is integrated
    /// Uses CDP Debugger.resume to resume JavaScript execution
    pub async fn resume_context(_context_id: &str) -> Result<()> {
        // TODO: Implement CDP Debugger.resume
        // This will:
        // 1. Send Debugger.resume command to the context
        // 2. Wait for confirmation
        // 3. Mark context as active
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::AiTask;

    #[test]
    fn test_is_browser_task() {
        let browser_task = Task::new(
            "task1",
            "Navigate to example.com",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Navigate browser to example.com");

        assert!(is_browser_task(&browser_task));

        let file_task = Task::new(
            "task2",
            "List files",
            TaskType::FileOperation(crate::dispatcher::agent_types::FileOp::List {
                path: std::path::PathBuf::from("/tmp"),
            }),
        );
        assert!(!is_browser_task(&file_task));
    }

    #[test]
    fn test_extract_task_metadata_ai_task() {
        let task = Task::new(
            "task1",
            "AI inference",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let metadata = extract_task_metadata(&task);
        assert_eq!(metadata.tier, PriorityTier::User);
        assert_eq!(metadata.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_extract_task_metadata_financial() {
        let task = Task::new(
            "task1",
            "Process payment",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Process payment transaction");

        let metadata = extract_task_metadata(&task);
        assert_eq!(metadata.tier, PriorityTier::User);
        assert_eq!(metadata.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_determine_risk_level() {
        let high_risk = Task::new(
            "task1",
            "Submit payment",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Submit payment form");
        assert_eq!(determine_risk_level(&high_risk), RiskLevel::High);

        let medium_risk = Task::new(
            "task2",
            "Fill form",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Fill login form");
        assert_eq!(determine_risk_level(&medium_risk), RiskLevel::Medium);

        let low_risk = Task::new(
            "task3",
            "Read content",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Read page content");
        assert_eq!(determine_risk_level(&low_risk), RiskLevel::Low);
    }

    #[test]
    fn test_browser_task_context() {
        let context = BrowserTaskContext::new("task1".to_string())
            .with_context_id("ctx_123".to_string());

        assert_eq!(context.task_id, "task1");
        assert_eq!(context.context_id, Some("ctx_123".to_string()));
        assert!(!context.is_frozen);
    }

    #[test]
    fn test_extract_domain() {
        let task = Task::new(
            "task1",
            "Navigate",
            TaskType::AiInference(AiTask {
                prompt: "Test".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Navigate to example.com");

        let domain = extract_domain_from_task(&task);
        assert_eq!(domain, Some("example.com".to_string()));
    }
}
