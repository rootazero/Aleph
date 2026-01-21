//! FFI types and conversions for Agent Execution Mode.
//!
//! These types are exposed through UniFFI to Swift/Kotlin clients.
//! Type definitions are in aether.udl, this file provides the Rust implementations.

use super::task_category::TaskCategory;
use crate::intent::detection::{ExecutableTask, ExecutionIntent};
use crate::intent::parameters::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};

// ===== TaskCategory FFI =====
// Type defined in aether.udl

/// FFI-safe task category enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCategoryFFI {
    General,
    FileOrganize,
    FileOperation,
    FileTransfer,
    FileCleanup,
    CodeExecution,
    AppLaunch,
    AppAutomation,
    DocumentGeneration,
    DocumentGenerate,
    ImageGeneration,
    VideoGeneration,
    AudioGeneration,
    SpeechGeneration,
    WebSearch,
    WebFetch,
    SystemInfo,
    MediaDownload,
    TextProcessing,
    DataProcess,
}

impl From<TaskCategory> for TaskCategoryFFI {
    fn from(cat: TaskCategory) -> Self {
        match cat {
            TaskCategory::General => Self::General,
            TaskCategory::FileOrganize => Self::FileOrganize,
            TaskCategory::FileOperation => Self::FileOperation,
            TaskCategory::FileTransfer => Self::FileTransfer,
            TaskCategory::FileCleanup => Self::FileCleanup,
            TaskCategory::CodeExecution => Self::CodeExecution,
            TaskCategory::AppLaunch => Self::AppLaunch,
            TaskCategory::AppAutomation => Self::AppAutomation,
            TaskCategory::DocumentGeneration => Self::DocumentGeneration,
            TaskCategory::DocumentGenerate => Self::DocumentGenerate,
            TaskCategory::ImageGeneration => Self::ImageGeneration,
            TaskCategory::VideoGeneration => Self::VideoGeneration,
            TaskCategory::AudioGeneration => Self::AudioGeneration,
            TaskCategory::SpeechGeneration => Self::SpeechGeneration,
            TaskCategory::WebSearch => Self::WebSearch,
            TaskCategory::WebFetch => Self::WebFetch,
            TaskCategory::SystemInfo => Self::SystemInfo,
            TaskCategory::MediaDownload => Self::MediaDownload,
            TaskCategory::TextProcessing => Self::TextProcessing,
            TaskCategory::DataProcess => Self::DataProcess,
        }
    }
}

impl From<TaskCategoryFFI> for TaskCategory {
    fn from(cat: TaskCategoryFFI) -> Self {
        match cat {
            TaskCategoryFFI::General => Self::General,
            TaskCategoryFFI::FileOrganize => Self::FileOrganize,
            TaskCategoryFFI::FileOperation => Self::FileOperation,
            TaskCategoryFFI::FileTransfer => Self::FileTransfer,
            TaskCategoryFFI::FileCleanup => Self::FileCleanup,
            TaskCategoryFFI::CodeExecution => Self::CodeExecution,
            TaskCategoryFFI::AppLaunch => Self::AppLaunch,
            TaskCategoryFFI::AppAutomation => Self::AppAutomation,
            TaskCategoryFFI::DocumentGeneration => Self::DocumentGeneration,
            TaskCategoryFFI::DocumentGenerate => Self::DocumentGenerate,
            TaskCategoryFFI::ImageGeneration => Self::ImageGeneration,
            TaskCategoryFFI::VideoGeneration => Self::VideoGeneration,
            TaskCategoryFFI::AudioGeneration => Self::AudioGeneration,
            TaskCategoryFFI::SpeechGeneration => Self::SpeechGeneration,
            TaskCategoryFFI::WebSearch => Self::WebSearch,
            TaskCategoryFFI::WebFetch => Self::WebFetch,
            TaskCategoryFFI::SystemInfo => Self::SystemInfo,
            TaskCategoryFFI::MediaDownload => Self::MediaDownload,
            TaskCategoryFFI::TextProcessing => Self::TextProcessing,
            TaskCategoryFFI::DataProcess => Self::DataProcess,
        }
    }
}

// ===== ExecutionIntent FFI =====
// Type defined in aether.udl

/// FFI-safe execution intent type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionIntentTypeFFI {
    Executable,
    Ambiguous,
    Conversational,
}

impl From<&ExecutionIntent> for ExecutionIntentTypeFFI {
    fn from(intent: &ExecutionIntent) -> Self {
        match intent {
            ExecutionIntent::Executable(_) => Self::Executable,
            ExecutionIntent::Ambiguous { .. } => Self::Ambiguous,
            ExecutionIntent::Conversational => Self::Conversational,
        }
    }
}

// ===== ExecutableTask FFI =====
// Type defined in aether.udl

/// FFI-safe executable task
#[derive(Debug, Clone)]
pub struct ExecutableTaskFFI {
    pub category: TaskCategoryFFI,
    pub action: String,
    pub target: Option<String>,
    pub confidence: f32,
}

impl From<ExecutableTask> for ExecutableTaskFFI {
    fn from(task: ExecutableTask) -> Self {
        Self {
            category: task.category.into(),
            action: task.action,
            target: task.target,
            confidence: task.confidence,
        }
    }
}

impl From<&ExecutableTask> for ExecutableTaskFFI {
    fn from(task: &ExecutableTask) -> Self {
        Self {
            category: task.category.into(),
            action: task.action.clone(),
            target: task.target.clone(),
            confidence: task.confidence,
        }
    }
}

// ===== AmbiguousTask FFI =====
// Type defined in aether.udl

/// FFI-safe ambiguous task info
#[derive(Debug, Clone)]
pub struct AmbiguousTaskFFI {
    pub task_hint: String,
    pub clarification: String,
}

// ===== ParameterSource FFI =====
// Type defined in aether.udl

/// FFI-safe parameter source enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSourceFFI {
    UserPreference,
    Preset,
    Inference,
    Default,
}

impl From<ParameterSource> for ParameterSourceFFI {
    fn from(src: ParameterSource) -> Self {
        match src {
            ParameterSource::UserPreference => Self::UserPreference,
            ParameterSource::Preset => Self::Preset,
            ParameterSource::Inference => Self::Inference,
            ParameterSource::Default => Self::Default,
        }
    }
}

impl From<ParameterSourceFFI> for ParameterSource {
    fn from(src: ParameterSourceFFI) -> Self {
        match src {
            ParameterSourceFFI::UserPreference => Self::UserPreference,
            ParameterSourceFFI::Preset => Self::Preset,
            ParameterSourceFFI::Inference => Self::Inference,
            ParameterSourceFFI::Default => Self::Default,
        }
    }
}

// ===== OrganizeMethod FFI =====
// Type defined in aether.udl

/// FFI-safe organize method enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizeMethodFFI {
    ByExtension,
    ByCategory,
    ByDate,
}

impl From<OrganizeMethod> for OrganizeMethodFFI {
    fn from(method: OrganizeMethod) -> Self {
        match method {
            OrganizeMethod::ByExtension => Self::ByExtension,
            OrganizeMethod::ByCategory => Self::ByCategory,
            OrganizeMethod::ByDate => Self::ByDate,
        }
    }
}

impl From<OrganizeMethodFFI> for OrganizeMethod {
    fn from(method: OrganizeMethodFFI) -> Self {
        match method {
            OrganizeMethodFFI::ByExtension => Self::ByExtension,
            OrganizeMethodFFI::ByCategory => Self::ByCategory,
            OrganizeMethodFFI::ByDate => Self::ByDate,
        }
    }
}

// ===== ConflictResolution FFI =====
// Type defined in aether.udl

/// FFI-safe conflict resolution enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolutionFFI {
    Skip,
    Rename,
    Overwrite,
}

impl From<ConflictResolution> for ConflictResolutionFFI {
    fn from(res: ConflictResolution) -> Self {
        match res {
            ConflictResolution::Skip => Self::Skip,
            ConflictResolution::Rename => Self::Rename,
            ConflictResolution::Overwrite => Self::Overwrite,
        }
    }
}

impl From<ConflictResolutionFFI> for ConflictResolution {
    fn from(res: ConflictResolutionFFI) -> Self {
        match res {
            ConflictResolutionFFI::Skip => Self::Skip,
            ConflictResolutionFFI::Rename => Self::Rename,
            ConflictResolutionFFI::Overwrite => Self::Overwrite,
        }
    }
}

// ===== TaskParameters FFI =====
// Type defined in aether.udl

/// FFI-safe task parameters
#[derive(Debug, Clone)]
pub struct TaskParametersFFI {
    pub organize_method: OrganizeMethodFFI,
    pub conflict_resolution: ConflictResolutionFFI,
    pub source: ParameterSourceFFI,
}

impl From<TaskParameters> for TaskParametersFFI {
    fn from(params: TaskParameters) -> Self {
        Self {
            organize_method: params.organize_method.into(),
            conflict_resolution: params.conflict_resolution.into(),
            source: params.source.into(),
        }
    }
}

impl From<&TaskParameters> for TaskParametersFFI {
    fn from(params: &TaskParameters) -> Self {
        Self {
            organize_method: params.organize_method.into(),
            conflict_resolution: params.conflict_resolution.into(),
            source: params.source.into(),
        }
    }
}

impl From<TaskParametersFFI> for TaskParameters {
    fn from(params: TaskParametersFFI) -> Self {
        Self {
            organize_method: params.organize_method.into(),
            conflict_resolution: params.conflict_resolution.into(),
            source: params.source.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_category_ffi_conversion() {
        let rust_cat = TaskCategory::FileOrganize;
        let ffi_cat: TaskCategoryFFI = rust_cat.into();
        assert_eq!(ffi_cat, TaskCategoryFFI::FileOrganize);

        let back: TaskCategory = ffi_cat.into();
        assert_eq!(back, TaskCategory::FileOrganize);
    }

    #[test]
    fn test_executable_task_ffi_conversion() {
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.95,
        };

        let ffi: ExecutableTaskFFI = task.into();
        assert_eq!(ffi.category, TaskCategoryFFI::FileOrganize);
        assert_eq!(ffi.action, "整理文件");
        assert_eq!(ffi.target, Some("/Downloads".to_string()));
        assert_eq!(ffi.confidence, 0.95);
    }

    #[test]
    fn test_task_parameters_ffi_conversion() {
        let params = TaskParameters {
            organize_method: OrganizeMethod::ByDate,
            conflict_resolution: ConflictResolution::Skip,
            source: ParameterSource::Preset,
        };

        let ffi: TaskParametersFFI = params.into();
        assert_eq!(ffi.organize_method, OrganizeMethodFFI::ByDate);
        assert_eq!(ffi.conflict_resolution, ConflictResolutionFFI::Skip);
        assert_eq!(ffi.source, ParameterSourceFFI::Preset);

        let back: TaskParameters = ffi.into();
        assert_eq!(back.organize_method, OrganizeMethod::ByDate);
    }

    #[test]
    fn test_execution_intent_type_ffi() {
        let executable = ExecutionIntent::Executable(ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "test".to_string(),
            target: None,
            confidence: 1.0,
        });
        assert_eq!(
            ExecutionIntentTypeFFI::from(&executable),
            ExecutionIntentTypeFFI::Executable
        );

        let ambiguous = ExecutionIntent::Ambiguous {
            task_hint: "hint".to_string(),
            clarification: "question".to_string(),
        };
        assert_eq!(
            ExecutionIntentTypeFFI::from(&ambiguous),
            ExecutionIntentTypeFFI::Ambiguous
        );

        let conversational = ExecutionIntent::Conversational;
        assert_eq!(
            ExecutionIntentTypeFFI::from(&conversational),
            ExecutionIntentTypeFFI::Conversational
        );
    }
}
