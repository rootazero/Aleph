//! Tool Parameter Types with schemars Integration
//!
//! This module provides type-safe tool parameter definitions using schemars
//! for automatic JSON Schema generation.
//!
//! # Design
//!
//! The `ToolParams` trait acts as a marker trait for tool parameters.
//! Types implementing this trait can be automatically serialized to JSON Schema
//! using the `schemars::schema_for!()` macro.
//!
//! # Example
//!
//! ```rust,ignore
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//! use crate::tools::params::ToolParams;
//!
//! #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
//! pub struct SearchParams {
//!     /// The search query string
//!     pub query: String,
//!
//!     /// Maximum number of results to return (1-20)
//!     #[serde(default)]
//!     pub max_results: Option<u32>,
//! }
//!
//! impl ToolParams for SearchParams {}
//!
//! // Generate JSON Schema
//! let schema = SearchParams::json_schema();
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// ToolParams Trait
// =============================================================================

/// Marker trait for tool parameter types
///
/// Types implementing this trait can be used with the `ToolHandler` trait
/// and provide automatic JSON Schema generation for LLM function calling.
///
/// # Requirements
///
/// Implementors must also derive:
/// - `serde::Serialize` and `serde::Deserialize` for JSON parsing
/// - `schemars::JsonSchema` for schema generation
/// - `Debug` and `Clone` for standard operations
pub trait ToolParams:
    Serialize + for<'de> Deserialize<'de> + JsonSchema + std::fmt::Debug + Clone + Send + Sync
{
    /// Generate JSON Schema for this parameter type as a serde_json::Value
    ///
    /// Uses schemars to produce a JSON Schema Draft 7 compliant schema.
    fn schema_value() -> Value
    where
        Self: Sized,
    {
        let schema = schemars::schema_for!(Self);
        serde_json::to_value(schema).unwrap_or_default()
    }

    /// Get the root schema without wrapper
    ///
    /// Returns the inner schema object, stripping the outer metadata.
    fn schema_object() -> Value
    where
        Self: Sized,
    {
        let full_schema = Self::schema_value();
        // Extract just the schema part, removing the $schema and $defs
        match full_schema {
            Value::Object(obj) => {
                let mut result = serde_json::Map::<String, Value>::new();
                if let Some(v) = obj.get("type") {
                    result.insert("type".to_string(), v.clone());
                }
                if let Some(v) = obj.get("properties") {
                    result.insert("properties".to_string(), v.clone());
                }
                if let Some(v) = obj.get("required") {
                    result.insert("required".to_string(), v.clone());
                }
                if let Some(v) = obj.get("description") {
                    result.insert("description".to_string(), v.clone());
                }
                Value::Object(result)
            }
            _ => full_schema,
        }
    }
}

// =============================================================================
// ToolOutput
// =============================================================================

/// Standardized output type for tool execution results
///
/// This provides a consistent structure for all tool outputs,
/// making it easier to pass results between steps in a multi-step plan.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolOutput {
    /// Whether the tool execution was successful
    pub success: bool,

    /// The output data (can be any JSON value)
    #[serde(default)]
    pub data: Value,

    /// Human-readable output message
    #[serde(default)]
    pub message: String,

    /// Error message if the tool failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolOutput {
    /// Create a successful output
    pub fn success(data: impl Into<Value>, message: impl Into<String>) -> Self {
        Self {
            success: true,
            data: data.into(),
            message: message.into(),
            error: None,
        }
    }

    /// Create a failed output
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            data: Value::Null,
            message: String::new(),
            error: Some(error.into()),
        }
    }

    /// Create a simple text output
    pub fn text(content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            success: true,
            data: Value::String(content.clone()),
            message: content,
            error: None,
        }
    }
}

impl Default for ToolOutput {
    fn default() -> Self {
        Self {
            success: false,
            data: Value::Null,
            message: String::new(),
            error: None,
        }
    }
}

// =============================================================================
// Common Parameter Types
// =============================================================================

/// Search tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The search query string
    pub query: String,

    /// Maximum number of results to return (1-20)
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

fn default_max_results() -> u32 {
    5
}

impl ToolParams for SearchParams {}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            max_results: default_max_results(),
        }
    }
}

/// Translation tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranslateParams {
    /// The text content to translate
    pub content: String,

    /// Target language code (e.g., "en", "zh", "ja")
    pub target_language: String,

    /// Source language code (optional, auto-detect if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_language: Option<String>,
}

impl ToolParams for TranslateParams {}

impl Default for TranslateParams {
    fn default() -> Self {
        Self {
            content: String::new(),
            target_language: "en".to_string(),
            source_language: None,
        }
    }
}

/// Summarization tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeParams {
    /// The content to summarize
    pub content: String,

    /// Maximum length of the summary in words
    #[serde(default = "default_max_length")]
    pub max_length: u32,

    /// Format of the summary (e.g., "bullet_points", "paragraph", "key_points")
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_max_length() -> u32 {
    100
}

fn default_format() -> String {
    "paragraph".to_string()
}

impl ToolParams for SummarizeParams {}

impl Default for SummarizeParams {
    fn default() -> Self {
        Self {
            content: String::new(),
            max_length: default_max_length(),
            format: default_format(),
        }
    }
}

/// File read tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileReadParams {
    /// Path to the file to read
    pub path: String,

    /// Starting line number (1-indexed, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,

    /// Ending line number (1-indexed, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

impl ToolParams for FileReadParams {}

impl Default for FileReadParams {
    fn default() -> Self {
        Self {
            path: String::new(),
            start_line: None,
            end_line: None,
        }
    }
}

/// File write tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileWriteParams {
    /// Path to the file to write
    pub path: String,

    /// Content to write to the file
    pub content: String,

    /// Whether to append to the file instead of overwriting
    #[serde(default)]
    pub append: bool,

    /// Whether to create parent directories if they don't exist
    #[serde(default = "default_create_parents")]
    pub create_parents: bool,
}

fn default_create_parents() -> bool {
    true
}

impl ToolParams for FileWriteParams {}

impl Default for FileWriteParams {
    fn default() -> Self {
        Self {
            path: String::new(),
            content: String::new(),
            append: false,
            create_parents: default_create_parents(),
        }
    }
}

/// Shell execute tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShellExecuteParams {
    /// The command to execute
    pub command: String,

    /// Working directory for the command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,

    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    60
}

impl ToolParams for ShellExecuteParams {}

impl Default for ShellExecuteParams {
    fn default() -> Self {
        Self {
            command: String::new(),
            working_directory: None,
            timeout_seconds: default_timeout(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_params_schema() {
        let schema = SearchParams::schema_value();
        assert!(schema.is_object());

        let obj = schema.as_object().unwrap();
        assert!(obj.contains_key("properties"));

        let props = obj.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("query"));
        assert!(props.contains_key("max_results"));
    }

    #[test]
    fn test_translate_params_schema() {
        let schema = TranslateParams::schema_value();
        let obj = schema.as_object().unwrap();
        let props = obj.get("properties").unwrap().as_object().unwrap();

        assert!(props.contains_key("content"));
        assert!(props.contains_key("target_language"));
        assert!(props.contains_key("source_language"));
    }

    #[test]
    fn test_summarize_params_default() {
        let params = SummarizeParams::default();
        assert_eq!(params.max_length, 100);
        assert_eq!(params.format, "paragraph");
    }

    #[test]
    fn test_tool_output_success() {
        let output = ToolOutput::success(serde_json::json!({"count": 5}), "Found 5 results");
        assert!(output.success);
        assert!(output.error.is_none());
        assert_eq!(output.message, "Found 5 results");
    }

    #[test]
    fn test_tool_output_failure() {
        let output = ToolOutput::failure("Connection timeout");
        assert!(!output.success);
        assert_eq!(output.error, Some("Connection timeout".to_string()));
    }

    #[test]
    fn test_tool_output_text() {
        let output = ToolOutput::text("Hello, World!");
        assert!(output.success);
        assert_eq!(output.message, "Hello, World!");
    }

    #[test]
    fn test_schema_object_extraction() {
        let schema = SearchParams::schema_object();
        let obj = schema.as_object().unwrap();

        // Should have core schema elements
        assert!(obj.contains_key("type") || obj.contains_key("properties"));
    }

    #[test]
    fn test_file_read_params_serialization() {
        let params = FileReadParams {
            path: "/tmp/test.txt".to_string(),
            start_line: Some(1),
            end_line: Some(10),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("/tmp/test.txt"));
        assert!(json.contains("start_line"));

        let parsed: FileReadParams = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, "/tmp/test.txt");
    }

    #[test]
    fn test_shell_execute_default_timeout() {
        let params = ShellExecuteParams::default();
        assert_eq!(params.timeout_seconds, 60);
    }
}
