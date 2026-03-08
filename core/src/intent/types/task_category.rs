//! Task category definitions for executable task classification.

/// Categories of executable tasks
///
/// Used by the intent classifier to determine which tools to inject
/// and which prompt guidelines to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCategory {
    /// General task (explicit /agent command, category TBD)
    General,
    /// File organization (sort, classify)
    FileOrganize,
    /// File operations (read, write, search)
    FileOperation,
    /// File transfer (move, copy)
    FileTransfer,
    /// File cleanup (delete, archive)
    FileCleanup,
    /// Code execution
    CodeExecution,
    /// Application launch
    AppLaunch,
    /// Application automation
    AppAutomation,
    /// Document generation
    DocumentGeneration,
    /// Document generation (alias for compatibility)
    DocumentGenerate,
    /// Image generation
    ImageGeneration,
    /// Video generation
    VideoGeneration,
    /// Audio generation
    AudioGeneration,
    /// Speech generation (TTS)
    SpeechGeneration,
    /// Web search
    WebSearch,
    /// Web fetch (page content)
    WebFetch,
    /// System information queries
    SystemInfo,
    /// Media download (YouTube, etc.)
    MediaDownload,
    /// Text processing (translation, summarization)
    TextProcessing,
    /// Data processing
    DataProcess,
}

impl TaskCategory {
    /// Returns the string representation of the category
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::FileOrganize => "file_organize",
            Self::FileOperation => "file_operation",
            Self::FileTransfer => "file_transfer",
            Self::FileCleanup => "file_cleanup",
            Self::CodeExecution => "code_execution",
            Self::AppLaunch => "app_launch",
            Self::AppAutomation => "app_automation",
            Self::DocumentGeneration | Self::DocumentGenerate => "document_generation",
            Self::ImageGeneration => "image_generation",
            Self::VideoGeneration => "video_generation",
            Self::AudioGeneration => "audio_generation",
            Self::SpeechGeneration => "speech_generation",
            Self::WebSearch => "web_search",
            Self::WebFetch => "web_fetch",
            Self::SystemInfo => "system_info",
            Self::MediaDownload => "media_download",
            Self::TextProcessing => "text_processing",
            Self::DataProcess => "data_process",
        }
    }

    /// Check if this category involves file operations
    pub fn is_file_related(&self) -> bool {
        matches!(
            self,
            Self::FileOrganize | Self::FileOperation | Self::FileTransfer | Self::FileCleanup
        )
    }

    /// Check if this category involves content generation
    pub fn is_generation(&self) -> bool {
        matches!(
            self,
            Self::ImageGeneration
                | Self::VideoGeneration
                | Self::AudioGeneration
                | Self::SpeechGeneration
                | Self::DocumentGeneration
                | Self::DocumentGenerate
        )
    }

    /// Check if this category is read-only (no side effects)
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::WebSearch | Self::WebFetch | Self::SystemInfo | Self::TextProcessing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_category_display() {
        assert_eq!(TaskCategory::General.as_str(), "general");
        assert_eq!(TaskCategory::FileOrganize.as_str(), "file_organize");
        assert_eq!(TaskCategory::FileOperation.as_str(), "file_operation");
        assert_eq!(TaskCategory::FileTransfer.as_str(), "file_transfer");
        assert_eq!(TaskCategory::FileCleanup.as_str(), "file_cleanup");
        assert_eq!(TaskCategory::CodeExecution.as_str(), "code_execution");
        assert_eq!(TaskCategory::AppLaunch.as_str(), "app_launch");
        assert_eq!(TaskCategory::AppAutomation.as_str(), "app_automation");
        // Both DocumentGeneration and DocumentGenerate map to same string
        assert_eq!(
            TaskCategory::DocumentGeneration.as_str(),
            "document_generation"
        );
        assert_eq!(
            TaskCategory::DocumentGenerate.as_str(),
            "document_generation"
        );
        assert_eq!(TaskCategory::ImageGeneration.as_str(), "image_generation");
        assert_eq!(TaskCategory::WebSearch.as_str(), "web_search");
        assert_eq!(TaskCategory::DataProcess.as_str(), "data_process");
    }

    #[test]
    fn test_task_category_is_file_related() {
        assert!(TaskCategory::FileOrganize.is_file_related());
        assert!(TaskCategory::FileOperation.is_file_related());
        assert!(TaskCategory::FileTransfer.is_file_related());
        assert!(TaskCategory::FileCleanup.is_file_related());
        assert!(!TaskCategory::CodeExecution.is_file_related());
        assert!(!TaskCategory::ImageGeneration.is_file_related());
    }

    #[test]
    fn test_task_category_is_generation() {
        assert!(TaskCategory::ImageGeneration.is_generation());
        assert!(TaskCategory::VideoGeneration.is_generation());
        assert!(TaskCategory::AudioGeneration.is_generation());
        assert!(TaskCategory::SpeechGeneration.is_generation());
        assert!(TaskCategory::DocumentGeneration.is_generation());
        assert!(!TaskCategory::FileOrganize.is_generation());
        assert!(!TaskCategory::WebSearch.is_generation());
    }

    #[test]
    fn test_task_category_is_read_only() {
        assert!(TaskCategory::WebSearch.is_read_only());
        assert!(TaskCategory::WebFetch.is_read_only());
        assert!(TaskCategory::SystemInfo.is_read_only());
        assert!(TaskCategory::TextProcessing.is_read_only());
        assert!(!TaskCategory::FileOrganize.is_read_only());
        assert!(!TaskCategory::CodeExecution.is_read_only());
    }
}
