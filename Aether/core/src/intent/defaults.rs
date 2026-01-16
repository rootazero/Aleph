//! DefaultsResolver for smart parameter resolution.
//!
//! Resolves default parameters using a 3-tier strategy:
//! 1. User preferences (stored in config)
//! 2. Preset scenarios (hardcoded defaults)
//! 3. Context inference (based on file analysis)

use super::classifier::ExecutableTask;
use super::parameters::{ParameterSource, TaskParameters};
use super::presets::PresetRegistry;

/// Resolves default parameters for executable tasks
///
/// Three-tier resolution:
/// 1. User preferences (stored in config) - TODO: implement PreferenceStore
/// 2. Preset scenarios (hardcoded defaults)
/// 3. Context inference (based on file analysis)
pub struct DefaultsResolver {
    presets: PresetRegistry,
    // preferences: PreferenceStore, // TODO: Implement in future
}

impl DefaultsResolver {
    /// Create a new defaults resolver
    pub fn new() -> Self {
        Self {
            presets: PresetRegistry::default(),
        }
    }

    /// Resolve parameters using 3-tier strategy
    pub async fn resolve(&self, task: &ExecutableTask) -> TaskParameters {
        // Tier 1: Check user preferences (TODO: implement PreferenceStore)
        // if let Some(params) = self.preferences.get_for_task(&task.category, &task.target) {
        //     return params;
        // }

        // Tier 2: Match preset scenario
        if let Some(preset) = self.presets.match_scenario(task) {
            return preset.parameters.clone();
        }

        // Tier 3: Context inference (simplified for now)
        self.infer_from_context(task).await
    }

    /// Infer parameters from context (simplified implementation)
    async fn infer_from_context(&self, _task: &ExecutableTask) -> TaskParameters {
        // TODO: Implement file scanning and inference
        // For now, return defaults with Inference source
        TaskParameters::default().with_source(ParameterSource::Inference)
    }
}

impl Default for DefaultsResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::TaskCategory;

    #[tokio::test]
    async fn test_defaults_resolver_preset() {
        let resolver = DefaultsResolver::new();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: Some("/tmp/test".to_string()),
            confidence: 0.9,
        };
        let params = resolver.resolve(&task).await;
        assert_eq!(params.source, ParameterSource::Preset);
    }

    #[tokio::test]
    async fn test_defaults_resolver_inference_fallback() {
        let resolver = DefaultsResolver::new();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "做一些事情".to_string(), // No matching preset
            target: None,
            confidence: 0.9,
        };
        let params = resolver.resolve(&task).await;
        assert_eq!(params.source, ParameterSource::Inference);
    }

    #[tokio::test]
    async fn test_defaults_resolver_photos_by_date() {
        let resolver = DefaultsResolver::new();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理照片".to_string(),
            target: None,
            confidence: 0.9,
        };
        let params = resolver.resolve(&task).await;
        assert_eq!(
            params.organize_method,
            super::super::parameters::OrganizeMethod::ByDate
        );
    }

    #[tokio::test]
    async fn test_defaults_resolver_downloads_by_category() {
        let resolver = DefaultsResolver::new();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "清理下载文件夹".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.9,
        };
        let params = resolver.resolve(&task).await;
        assert_eq!(
            params.organize_method,
            super::super::parameters::OrganizeMethod::ByCategory
        );
    }
}
