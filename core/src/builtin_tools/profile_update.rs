//! Profile Update Tool — Learn and remember user preferences
//!
//! Allows the AI to create and update the user profile at ~/.aleph/user_profile.md
//! based on information discovered through conversation.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::thinker::user_profile::UserProfile;
use crate::tools::AlephTool;

/// Which field of the UserProfile to update
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileField {
    /// User's full name
    Name,
    /// Preferred name / nickname
    PreferredName,
    /// Timezone (e.g., "Asia/Shanghai")
    Timezone,
    /// Preferred language for responses
    Language,
    /// Context notes about the user (list)
    ContextNotes,
    /// Custom addendum text
    Addendum,
}

impl std::fmt::Display for ProfileField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name => write!(f, "name"),
            Self::PreferredName => write!(f, "preferred_name"),
            Self::Timezone => write!(f, "timezone"),
            Self::Language => write!(f, "language"),
            Self::ContextNotes => write!(f, "context_notes"),
            Self::Addendum => write!(f, "addendum"),
        }
    }
}

/// What operation to perform on the field
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOperation {
    /// Replace the field value entirely
    Set,
    /// Append to list fields (context_notes)
    Append,
    /// Remove an item from list fields (context_notes)
    Remove,
}

impl std::fmt::Display for ProfileOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Set => write!(f, "set"),
            Self::Append => write!(f, "append"),
            Self::Remove => write!(f, "remove"),
        }
    }
}

/// Arguments for the profile_update tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUpdateArgs {
    /// Which field to update
    pub field: ProfileField,
    /// What operation to perform
    pub operation: ProfileOperation,
    /// The value to set, append, or remove
    pub value: String,
    /// Reason for this change (for auditability)
    pub reason: String,
}

/// Output from the profile_update tool
#[derive(Debug, Clone, Serialize)]
pub struct ProfileUpdateOutput {
    /// Whether the update succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// Which field was updated
    pub field: String,
    /// What operation was performed
    pub operation: String,
}

/// Tool that allows the AI to update the user profile
#[derive(Clone)]
pub struct ProfileUpdateTool {
    profile_path: std::path::PathBuf,
}

impl ProfileUpdateTool {
    /// Create a new ProfileUpdateTool pointing at the given profile file
    pub fn new(profile_path: std::path::PathBuf) -> Self {
        Self { profile_path }
    }

    /// Apply an operation to the user profile
    fn apply_operation(
        profile: &mut UserProfile,
        field: &ProfileField,
        operation: &ProfileOperation,
        value: &str,
    ) -> std::result::Result<String, String> {
        match (field, operation) {
            // Name: string field
            (ProfileField::Name, ProfileOperation::Set) => {
                profile.name = value.to_string();
                Ok("Name updated".to_string())
            }
            (ProfileField::Name, _) => {
                Err("Name only supports 'set' operation".to_string())
            }

            // PreferredName: optional string field
            (ProfileField::PreferredName, ProfileOperation::Set) => {
                profile.preferred_name = Some(value.to_string());
                Ok("Preferred name updated".to_string())
            }
            (ProfileField::PreferredName, ProfileOperation::Remove) => {
                profile.preferred_name = None;
                Ok("Preferred name cleared".to_string())
            }
            (ProfileField::PreferredName, ProfileOperation::Append) => {
                Err("Preferred name only supports 'set' or 'remove' operations".to_string())
            }

            // Timezone: optional string field
            (ProfileField::Timezone, ProfileOperation::Set) => {
                profile.timezone = Some(value.to_string());
                Ok("Timezone updated".to_string())
            }
            (ProfileField::Timezone, ProfileOperation::Remove) => {
                profile.timezone = None;
                Ok("Timezone cleared".to_string())
            }
            (ProfileField::Timezone, ProfileOperation::Append) => {
                Err("Timezone only supports 'set' or 'remove' operations".to_string())
            }

            // Language: optional string field
            (ProfileField::Language, ProfileOperation::Set) => {
                profile.language = Some(value.to_string());
                Ok("Language updated".to_string())
            }
            (ProfileField::Language, ProfileOperation::Remove) => {
                profile.language = None;
                Ok("Language cleared".to_string())
            }
            (ProfileField::Language, ProfileOperation::Append) => {
                Err("Language only supports 'set' or 'remove' operations".to_string())
            }

            // ContextNotes: list field
            (ProfileField::ContextNotes, ProfileOperation::Set) => {
                profile.context_notes = vec![value.to_string()];
                Ok("Context notes set to single item".to_string())
            }
            (ProfileField::ContextNotes, ProfileOperation::Append) => {
                if profile.context_notes.contains(&value.to_string()) {
                    return Ok("Context note already exists, skipping".to_string());
                }
                profile.context_notes.push(value.to_string());
                Ok(format!(
                    "Context note appended (now {} total)",
                    profile.context_notes.len()
                ))
            }
            (ProfileField::ContextNotes, ProfileOperation::Remove) => {
                let before = profile.context_notes.len();
                profile.context_notes.retain(|n| n != value);
                let after = profile.context_notes.len();
                if before == after {
                    Ok("Context note not found, no change".to_string())
                } else {
                    Ok(format!(
                        "Context note removed (now {} total)",
                        profile.context_notes.len()
                    ))
                }
            }

            // Addendum: optional string field
            (ProfileField::Addendum, ProfileOperation::Set) => {
                profile.addendum = Some(value.to_string());
                Ok("Addendum updated".to_string())
            }
            (ProfileField::Addendum, ProfileOperation::Append) => {
                match &mut profile.addendum {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(value);
                    }
                    None => {
                        profile.addendum = Some(value.to_string());
                    }
                }
                Ok("Addendum appended".to_string())
            }
            (ProfileField::Addendum, ProfileOperation::Remove) => {
                profile.addendum = None;
                Ok("Addendum cleared".to_string())
            }
        }
    }
}

#[async_trait]
impl AlephTool for ProfileUpdateTool {
    const NAME: &'static str = "profile_update";
    const DESCRIPTION: &'static str =
        "Update the user profile. Use when you learn the user's name, timezone, \
         language preference, or other personal context through conversation. \
         The profile persists across sessions to personalize interactions.";

    type Args = ProfileUpdateArgs;
    type Output = ProfileUpdateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "profile_update(field='name', operation='set', value='Alice', reason='User introduced themselves')"
                .to_string(),
            "profile_update(field='timezone', operation='set', value='Asia/Shanghai', reason='User mentioned being in Shanghai')"
                .to_string(),
            "profile_update(field='context_notes', operation='append', value='Works on Rust/AI projects', reason='Discovered from conversation topics')"
                .to_string(),
            "profile_update(field='language', operation='set', value='Chinese', reason='User prefers Chinese responses')"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            field = %args.field,
            operation = %args.operation,
            reason = %args.reason,
            "Profile update requested"
        );

        // Load existing profile or use default
        let mut profile = if self.profile_path.exists() {
            UserProfile::load_from_file(&self.profile_path).unwrap_or_default()
        } else {
            UserProfile::default()
        };

        // Apply the operation
        let result = Self::apply_operation(&mut profile, &args.field, &args.operation, &args.value);

        match result {
            Ok(message) => {
                // Save back to file
                if let Err(e) = profile.save_to_file(&self.profile_path) {
                    return Ok(ProfileUpdateOutput {
                        success: false,
                        message: format!("Operation succeeded but save failed: {}", e),
                        field: args.field.to_string(),
                        operation: args.operation.to_string(),
                    });
                }

                info!(
                    field = %args.field,
                    operation = %args.operation,
                    message = %message,
                    "Profile updated successfully"
                );

                Ok(ProfileUpdateOutput {
                    success: true,
                    message,
                    field: args.field.to_string(),
                    operation: args.operation.to_string(),
                })
            }
            Err(message) => Ok(ProfileUpdateOutput {
                success: false,
                message,
                field: args.field.to_string(),
                operation: args.operation.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(ProfileUpdateTool::NAME, "profile_update");
        assert!(ProfileUpdateTool::DESCRIPTION.contains("user profile"));
    }

    #[test]
    fn test_tool_examples() {
        let tool = ProfileUpdateTool::new(PathBuf::from("/tmp/test.md"));
        let examples = tool.examples();
        assert!(examples.is_some());
        assert_eq!(examples.unwrap().len(), 4);
    }

    #[test]
    fn test_apply_set_name() {
        let mut profile = UserProfile::default();
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::Name,
            &ProfileOperation::Set,
            "Alice",
        );
        assert!(result.is_ok());
        assert_eq!(profile.name, "Alice");
    }

    #[test]
    fn test_apply_set_timezone() {
        let mut profile = UserProfile::default();
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::Timezone,
            &ProfileOperation::Set,
            "Asia/Shanghai",
        );
        assert!(result.is_ok());
        assert_eq!(profile.timezone, Some("Asia/Shanghai".to_string()));
    }

    #[test]
    fn test_apply_set_language() {
        let mut profile = UserProfile::default();
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::Language,
            &ProfileOperation::Set,
            "Chinese",
        );
        assert!(result.is_ok());
        assert_eq!(profile.language, Some("Chinese".to_string()));
    }

    #[test]
    fn test_apply_append_context_note() {
        let mut profile = UserProfile {
            name: "User".to_string(),
            context_notes: vec!["Note 1".to_string()],
            ..Default::default()
        };
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::ContextNotes,
            &ProfileOperation::Append,
            "Note 2",
        );
        assert!(result.is_ok());
        assert_eq!(profile.context_notes.len(), 2);
    }

    #[test]
    fn test_apply_append_duplicate_context_note_skips() {
        let mut profile = UserProfile {
            name: "User".to_string(),
            context_notes: vec!["Note 1".to_string()],
            ..Default::default()
        };
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::ContextNotes,
            &ProfileOperation::Append,
            "Note 1",
        );
        assert!(result.is_ok());
        assert_eq!(profile.context_notes.len(), 1);
        assert!(result.unwrap().contains("already exists"));
    }

    #[test]
    fn test_apply_remove_context_note() {
        let mut profile = UserProfile {
            name: "User".to_string(),
            context_notes: vec!["A".to_string(), "B".to_string()],
            ..Default::default()
        };
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::ContextNotes,
            &ProfileOperation::Remove,
            "A",
        );
        assert!(result.is_ok());
        assert_eq!(profile.context_notes, vec!["B".to_string()]);
    }

    #[test]
    fn test_apply_remove_preferred_name() {
        let mut profile = UserProfile {
            name: "User".to_string(),
            preferred_name: Some("Al".to_string()),
            ..Default::default()
        };
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::PreferredName,
            &ProfileOperation::Remove,
            "",
        );
        assert!(result.is_ok());
        assert!(profile.preferred_name.is_none());
    }

    #[test]
    fn test_apply_invalid_name_append() {
        let mut profile = UserProfile::default();
        let result = ProfileUpdateTool::apply_operation(
            &mut profile,
            &ProfileField::Name,
            &ProfileOperation::Append,
            "anything",
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_async_create_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user_profile.md");

        let tool = ProfileUpdateTool::new(path.clone());
        let output = tool
            .call(ProfileUpdateArgs {
                field: ProfileField::Name,
                operation: ProfileOperation::Set,
                value: "Alice".to_string(),
                reason: "User introduced themselves".to_string(),
            })
            .await
            .unwrap();

        assert!(output.success);
        assert!(path.exists());

        let reloaded = UserProfile::load_from_file(&path).unwrap();
        assert_eq!(reloaded.name, "Alice");
    }

    #[tokio::test]
    async fn test_async_update_existing_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user_profile.md");

        // Create initial profile
        let initial = UserProfile {
            name: "Alice".to_string(),
            ..Default::default()
        };
        initial.save_to_file(&path).unwrap();

        // Update timezone
        let tool = ProfileUpdateTool::new(path.clone());
        let output = tool
            .call(ProfileUpdateArgs {
                field: ProfileField::Timezone,
                operation: ProfileOperation::Set,
                value: "Asia/Shanghai".to_string(),
                reason: "User mentioned timezone".to_string(),
            })
            .await
            .unwrap();

        assert!(output.success);

        let reloaded = UserProfile::load_from_file(&path).unwrap();
        assert_eq!(reloaded.name, "Alice"); // preserved
        assert_eq!(reloaded.timezone, Some("Asia/Shanghai".to_string()));
    }
}
