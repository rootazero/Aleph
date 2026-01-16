//! Task parameters for executable task defaults.

/// How to organize files
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrganizeMethod {
    /// Group by file extension (pdf, jpg, etc.)
    #[default]
    ByExtension,
    /// Group by category (Documents, Images, Videos, etc.)
    ByCategory,
    /// Group by date (Year/Month)
    ByDate,
}

/// How to handle file conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictResolution {
    /// Skip conflicting files
    Skip,
    /// Rename with suffix (file_1.txt)
    #[default]
    Rename,
    /// Overwrite existing files
    Overwrite,
}

/// Source of parameters (for debugging/transparency)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterSource {
    /// From user's stored preferences
    UserPreference,
    /// From preset scenario
    Preset,
    /// Inferred from context
    Inference,
    /// Default fallback
    #[default]
    Default,
}

/// Parameters for task execution
#[derive(Debug, Clone)]
pub struct TaskParameters {
    /// How to organize files
    pub organize_method: OrganizeMethod,
    /// How to handle conflicts
    pub conflict_resolution: ConflictResolution,
    /// Where these parameters came from
    pub source: ParameterSource,
}

impl Default for TaskParameters {
    fn default() -> Self {
        Self {
            organize_method: OrganizeMethod::ByExtension,
            conflict_resolution: ConflictResolution::Rename,
            source: ParameterSource::Default,
        }
    }
}

impl TaskParameters {
    /// Create parameters for organizing by extension
    pub fn file_organize_by_extension() -> Self {
        Self {
            organize_method: OrganizeMethod::ByExtension,
            ..Default::default()
        }
    }

    /// Create parameters for organizing by category
    pub fn file_organize_by_category() -> Self {
        Self {
            organize_method: OrganizeMethod::ByCategory,
            ..Default::default()
        }
    }

    /// Create parameters for organizing by date
    pub fn file_organize_by_date() -> Self {
        Self {
            organize_method: OrganizeMethod::ByDate,
            ..Default::default()
        }
    }

    /// Set the parameter source
    pub fn with_source(mut self, source: ParameterSource) -> Self {
        self.source = source;
        self
    }

    /// Set the conflict resolution strategy
    pub fn with_conflict_resolution(mut self, resolution: ConflictResolution) -> Self {
        self.conflict_resolution = resolution;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_parameters_default() {
        let params = TaskParameters::default();
        assert_eq!(params.organize_method, OrganizeMethod::ByExtension);
        assert_eq!(params.conflict_resolution, ConflictResolution::Rename);
        assert_eq!(params.source, ParameterSource::Default);
    }

    #[test]
    fn test_task_parameters_by_extension() {
        let params = TaskParameters::file_organize_by_extension();
        assert_eq!(params.organize_method, OrganizeMethod::ByExtension);
    }

    #[test]
    fn test_task_parameters_by_category() {
        let params = TaskParameters::file_organize_by_category();
        assert_eq!(params.organize_method, OrganizeMethod::ByCategory);
    }

    #[test]
    fn test_task_parameters_by_date() {
        let params = TaskParameters::file_organize_by_date();
        assert_eq!(params.organize_method, OrganizeMethod::ByDate);
    }

    #[test]
    fn test_task_parameters_with_source() {
        let params = TaskParameters::default().with_source(ParameterSource::Preset);
        assert_eq!(params.source, ParameterSource::Preset);
    }

    #[test]
    fn test_task_parameters_with_conflict_resolution() {
        let params = TaskParameters::default().with_conflict_resolution(ConflictResolution::Skip);
        assert_eq!(params.conflict_resolution, ConflictResolution::Skip);
    }
}
