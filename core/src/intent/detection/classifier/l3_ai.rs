//! L3 AI-based classification (1-3s).

use super::l2_keywords::intent_type_to_category;
use super::types::ExecutableTask;
use crate::intent::detection::ai_detector::AiIntentResult;
use crate::intent::types::TaskCategory;

/// Convert AiIntentResult to ExecutableTask
pub fn convert_ai_result(result: &AiIntentResult, input: &str) -> Option<ExecutableTask> {
    // Map AI intent types to TaskCategory
    let category = match result.intent.as_str() {
        "file_organize" => Some(TaskCategory::FileOrganize),
        "file_cleanup" => Some(TaskCategory::FileCleanup),
        "code_execution" => Some(TaskCategory::CodeExecution),
        "file_transfer" => Some(TaskCategory::FileTransfer),
        "document_generate" => Some(TaskCategory::DocumentGenerate),
        _ => None,
    }?;

    Some(ExecutableTask {
        category,
        action: input.to_string(),
        target: result.params.get("path").cloned(),
        confidence: result.confidence as f32,
    })
}

/// Convert AI intent string to TaskCategory using both formats
pub fn ai_intent_to_category(intent: &str) -> Option<TaskCategory> {
    // Try snake_case format first (AI result format)
    match intent {
        "file_organize" => Some(TaskCategory::FileOrganize),
        "file_cleanup" => Some(TaskCategory::FileCleanup),
        "code_execution" => Some(TaskCategory::CodeExecution),
        "file_transfer" => Some(TaskCategory::FileTransfer),
        "document_generate" => Some(TaskCategory::DocumentGenerate),
        _ => {
            // Fall back to PascalCase format (keyword index format)
            intent_type_to_category(intent)
        }
    }
}
