//! Preset scenarios for default task parameters.

use super::classifier::ExecutableTask;
use super::parameters::{ParameterSource, TaskParameters};
use super::task_category::TaskCategory;

/// A preset scenario with default parameters
#[derive(Debug, Clone)]
pub struct ScenarioPreset {
    /// Keywords that trigger this preset
    pub keywords: Vec<String>,
    /// Task category this preset applies to
    pub category: TaskCategory,
    /// Default parameters for this scenario
    pub parameters: TaskParameters,
}

/// Registry of preset scenarios
pub struct PresetRegistry {
    presets: Vec<ScenarioPreset>,
}

impl Default for PresetRegistry {
    fn default() -> Self {
        // Order matters: more specific presets should come first
        Self {
            presets: vec![
                // 照片整理 → 按日期分组 (specific, must come before generic "整理")
                ScenarioPreset {
                    keywords: vec![
                        "照片".to_string(),
                        "图片".to_string(),
                        "photos".to_string(),
                        "pictures".to_string(),
                        "images".to_string(),
                    ],
                    category: TaskCategory::FileOrganize,
                    parameters: TaskParameters::file_organize_by_date()
                        .with_source(ParameterSource::Preset),
                },
                // 清理下载 → 按大类分组 (specific, must come before generic "清理")
                ScenarioPreset {
                    keywords: vec![
                        "清理下载".to_string(),
                        "clean downloads".to_string(),
                        "下载".to_string(),
                        "downloads".to_string(),
                    ],
                    category: TaskCategory::FileOrganize,
                    parameters: TaskParameters::file_organize_by_category()
                        .with_source(ParameterSource::Preset),
                },
                // 整理文件 → 按扩展名分组 (generic fallback)
                ScenarioPreset {
                    keywords: vec![
                        "整理".to_string(),
                        "organize".to_string(),
                        "sort".to_string(),
                        "清理".to_string(),
                    ],
                    category: TaskCategory::FileOrganize,
                    parameters: TaskParameters::file_organize_by_extension()
                        .with_source(ParameterSource::Preset),
                },
            ],
        }
    }
}

impl PresetRegistry {
    /// Create a new preset registry with default presets
    pub fn new() -> Self {
        Self::default()
    }

    /// Match a task to a preset scenario
    pub fn match_scenario(&self, task: &ExecutableTask) -> Option<&ScenarioPreset> {
        let action_lower = task.action.to_lowercase();
        self.presets.iter().find(|p| {
            p.category == task.category
                && p.keywords
                    .iter()
                    .any(|k| action_lower.contains(&k.to_lowercase()))
        })
    }

    /// Get all presets
    pub fn presets(&self) -> &[ScenarioPreset] {
        &self.presets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_match_file_organize() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: None,
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_some());
        assert_eq!(
            preset.unwrap().parameters.organize_method,
            super::super::parameters::OrganizeMethod::ByExtension
        );
    }

    #[test]
    fn test_preset_match_clean_downloads() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "清理下载文件夹".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_some());
        assert_eq!(
            preset.unwrap().parameters.organize_method,
            super::super::parameters::OrganizeMethod::ByCategory
        );
    }

    #[test]
    fn test_preset_match_photos() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理照片".to_string(),
            target: None,
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_some());
        assert_eq!(
            preset.unwrap().parameters.organize_method,
            super::super::parameters::OrganizeMethod::ByDate
        );
    }

    #[test]
    fn test_preset_no_match_wrong_category() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileTransfer, // Different category
            action: "整理文件".to_string(),
            target: None,
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_none());
    }

    #[test]
    fn test_preset_no_match_no_keywords() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "做一些事情".to_string(), // No matching keywords
            target: None,
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_none());
    }

    #[test]
    fn test_preset_match_english() {
        let registry = PresetRegistry::default();
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "organize files".to_string(),
            target: None,
            confidence: 0.9,
        };
        let preset = registry.match_scenario(&task);
        assert!(preset.is_some());
    }
}
