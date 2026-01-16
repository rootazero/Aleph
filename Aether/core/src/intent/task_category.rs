//! Task category definitions for executable task classification.

/// Categories of executable tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCategory {
    /// File organization (sort, classify)
    FileOrganize,
    /// File transfer (move, copy)
    FileTransfer,
    /// File cleanup (delete, archive)
    FileCleanup,
    /// Code execution
    CodeExecution,
    /// Application automation
    AppAutomation,
    /// Document generation
    DocumentGenerate,
    /// Data processing
    DataProcess,
}

impl TaskCategory {
    /// Returns the string representation of the category
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileOrganize => "file_organize",
            Self::FileTransfer => "file_transfer",
            Self::FileCleanup => "file_cleanup",
            Self::CodeExecution => "code_execution",
            Self::AppAutomation => "app_automation",
            Self::DocumentGenerate => "document_generate",
            Self::DataProcess => "data_process",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_category_display() {
        assert_eq!(TaskCategory::FileOrganize.as_str(), "file_organize");
        assert_eq!(TaskCategory::FileTransfer.as_str(), "file_transfer");
        assert_eq!(TaskCategory::FileCleanup.as_str(), "file_cleanup");
        assert_eq!(TaskCategory::CodeExecution.as_str(), "code_execution");
        assert_eq!(TaskCategory::AppAutomation.as_str(), "app_automation");
        assert_eq!(TaskCategory::DocumentGenerate.as_str(), "document_generate");
        assert_eq!(TaskCategory::DataProcess.as_str(), "data_process");
    }
}
