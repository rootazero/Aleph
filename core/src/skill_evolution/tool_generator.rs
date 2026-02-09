//! Tool-backed skill generation.
//!
//! This module generates executable tool packages from skill suggestions:
//! 1. Analyze skill instructions to determine tool requirements
//! 2. Generate tool_definition.json with input schema
//! 3. Generate entrypoint script (Python by default)
//! 4. Support self-test before registration
//!
//! ## Tool Package Structure
//!
//! ```text
//! ~/.aleph/tools/compiled/<tool-name>/
//! ├── tool_definition.json    # Tool metadata and schema
//! ├── entrypoint.py           # Main execution script
//! ├── requirements.txt        # Python dependencies (if any)
//! └── README.md               # Auto-generated documentation
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};
use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

use super::types::SolidificationSuggestion;

/// Tool definition for generated tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedToolDefinition {
    /// Tool name (snake_case)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
    /// Runtime to use (python, node, etc.)
    pub runtime: String,
    /// Entrypoint file
    pub entrypoint: String,
    /// Whether the tool has been self-tested
    pub self_tested: bool,
    /// Whether the tool requires confirmation before first use
    pub requires_confirmation: bool,
    /// Sandbox capabilities required by this tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_capabilities: Option<RequiredCapabilities>,
    /// Generation metadata
    pub generated: GenerationMetadata,
}

/// Metadata about the generation process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    /// Pattern ID from the suggestion
    pub pattern_id: String,
    /// Confidence score
    pub confidence: f32,
    /// Generation timestamp
    pub generated_at: i64,
    /// Version of the generator
    pub generator_version: String,
}

/// Result of tool generation
#[derive(Debug, Clone)]
pub struct ToolGenerationResult {
    /// The generated tool definition
    pub definition: GeneratedToolDefinition,
    /// Path to the tool package directory
    pub package_dir: PathBuf,
    /// Path to the generated entrypoint
    pub entrypoint_path: PathBuf,
    /// Preview of generated code
    pub code_preview: String,
}

/// Configuration for tool generation
#[derive(Debug, Clone)]
pub struct ToolGeneratorConfig {
    /// Output directory for tool packages
    pub output_dir: PathBuf,
    /// Runtime to use (python, node, etc.)
    pub runtime: String,
    /// Require confirmation for first run
    pub require_confirmation: bool,
}

impl Default for ToolGeneratorConfig {
    fn default() -> Self {
        let output_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("tools")
            .join("compiled");

        Self {
            output_dir,
            runtime: "python".to_string(),
            require_confirmation: true,
        }
    }
}

/// Generator for tool-backed skills
pub struct ToolGenerator {
    config: ToolGeneratorConfig,
}

impl ToolGenerator {
    /// Create a new tool generator with default config
    pub fn new() -> Self {
        Self {
            config: ToolGeneratorConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: ToolGeneratorConfig) -> Self {
        Self { config }
    }

    /// Generate a tool package from a skill suggestion
    pub fn generate(&self, suggestion: &SolidificationSuggestion) -> Result<ToolGenerationResult> {
        let tool_name = to_tool_name(&suggestion.suggested_name);
        let package_dir = self.config.output_dir.join(&tool_name);

        // Check if already exists
        if package_dir.exists() {
            return Err(AlephError::Other {
                message: format!("Tool '{}' already exists at {}", tool_name, package_dir.display()),
                suggestion: Some("Delete the existing tool or use a different name".to_string()),
            });
        }

        // Create package directory
        fs::create_dir_all(&package_dir).map_err(|e| AlephError::Other {
            message: format!("Failed to create tool directory: {}", e),
            suggestion: None,
        })?;

        // Generate tool definition
        let input_schema = generate_input_schema(&suggestion.instructions_preview);
        let definition = GeneratedToolDefinition {
            name: tool_name.clone(),
            description: suggestion.suggested_description.clone(),
            input_schema: input_schema.clone(),
            runtime: self.config.runtime.clone(),
            entrypoint: format!("entrypoint.{}", get_extension(&self.config.runtime)),
            self_tested: false,
            requires_confirmation: self.config.require_confirmation,
            required_capabilities: Some(Self::generate_required_capabilities(&suggestion)),
            generated: GenerationMetadata {
                pattern_id: suggestion.pattern_id.clone(),
                confidence: suggestion.confidence,
                generated_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                generator_version: "1.0.0".to_string(),
            },
        };

        // Write tool_definition.json
        let definition_path = package_dir.join("tool_definition.json");
        let definition_json = serde_json::to_string_pretty(&definition).map_err(|e| {
            AlephError::Other {
                message: format!("Failed to serialize definition: {}", e),
                suggestion: None,
            }
        })?;
        fs::write(&definition_path, &definition_json).map_err(|e| AlephError::Other {
            message: format!("Failed to write definition: {}", e),
            suggestion: None,
        })?;

        // Generate entrypoint
        let (entrypoint_content, code_preview) = match self.config.runtime.as_str() {
            "python" => generate_python_entrypoint(suggestion, &input_schema),
            "node" => generate_node_entrypoint(suggestion, &input_schema),
            _ => generate_python_entrypoint(suggestion, &input_schema), // Default to Python
        };

        let entrypoint_path = package_dir.join(&definition.entrypoint);
        fs::write(&entrypoint_path, &entrypoint_content).map_err(|e| AlephError::Other {
            message: format!("Failed to write entrypoint: {}", e),
            suggestion: None,
        })?;

        // Generate README
        let readme = generate_readme(suggestion, &definition);
        fs::write(package_dir.join("README.md"), &readme).map_err(|e| AlephError::Other {
            message: format!("Failed to write README: {}", e),
            suggestion: None,
        })?;

        // Generate requirements.txt for Python
        if self.config.runtime == "python" {
            fs::write(package_dir.join("requirements.txt"), "# Add dependencies here\n")
                .map_err(|e| AlephError::Other {
                    message: format!("Failed to write requirements: {}", e),
                    suggestion: None,
                })?;
        }

        info!(
            tool_name = %tool_name,
            package_dir = %package_dir.display(),
            "Generated tool package"
        );

        Ok(ToolGenerationResult {
            definition,
            package_dir,
            entrypoint_path,
            code_preview,
        })
    }

    /// Preview what would be generated without writing
    pub fn preview(&self, suggestion: &SolidificationSuggestion) -> String {
        let input_schema = generate_input_schema(&suggestion.instructions_preview);
        let (_, preview) = generate_python_entrypoint(suggestion, &input_schema);
        preview
    }

    /// Load a generated tool definition from disk
    pub fn load_definition(&self, tool_name: &str) -> Result<GeneratedToolDefinition> {
        let package_dir = self.config.output_dir.join(tool_name);
        let definition_path = package_dir.join("tool_definition.json");

        if !definition_path.exists() {
            return Err(AlephError::Other {
                message: format!("Tool '{}' not found", tool_name),
                suggestion: None,
            });
        }

        let content = fs::read_to_string(&definition_path).map_err(|e| AlephError::Other {
            message: format!("Failed to read definition: {}", e),
            suggestion: None,
        })?;

        serde_json::from_str(&content).map_err(|e| AlephError::Other {
            message: format!("Failed to parse definition: {}", e),
            suggestion: None,
        })
    }

    /// List all generated tools
    pub fn list_tools(&self) -> Result<Vec<GeneratedToolDefinition>> {
        if !self.config.output_dir.exists() {
            return Ok(vec![]);
        }

        let mut tools = Vec::new();

        for entry in fs::read_dir(&self.config.output_dir)
            .map_err(|e| AlephError::Other {
                message: format!("Failed to read tools directory: {}", e),
                suggestion: None,
            })?
            .flatten()
        {
            if !entry.path().is_dir() {
                continue;
            }

            if let Some(name) = entry.file_name().to_str() {
                match self.load_definition(name) {
                    Ok(def) => tools.push(def),
                    Err(e) => {
                        warn!(tool = %name, error = %e, "Failed to load tool definition");
                    }
                }
            }
        }

        Ok(tools)
    }

    /// Delete a generated tool
    pub fn delete_tool(&self, tool_name: &str) -> Result<()> {
        let package_dir = self.config.output_dir.join(tool_name);

        if !package_dir.exists() {
            return Err(AlephError::Other {
                message: format!("Tool '{}' not found", tool_name),
                suggestion: None,
            });
        }

        fs::remove_dir_all(&package_dir).map_err(|e| AlephError::Other {
            message: format!("Failed to delete tool: {}", e),
            suggestion: None,
        })?;

        info!(tool_name = %tool_name, "Deleted tool package");
        Ok(())
    }

    /// Mark a tool as self-tested
    pub fn mark_self_tested(&self, tool_name: &str, passed: bool) -> Result<()> {
        let package_dir = self.config.output_dir.join(tool_name);
        let definition_path = package_dir.join("tool_definition.json");

        let mut definition = self.load_definition(tool_name)?;
        definition.self_tested = passed;

        let content = serde_json::to_string_pretty(&definition).map_err(|e| AlephError::Other {
            message: format!("Failed to serialize: {}", e),
            suggestion: None,
        })?;

        fs::write(&definition_path, content).map_err(|e| AlephError::Other {
            message: format!("Failed to write definition: {}", e),
            suggestion: None,
        })?;

        debug!(tool_name = %tool_name, passed = passed, "Updated self-test status");
        Ok(())
    }

    /// Get the package directory for a tool
    pub fn get_package_dir(&self, tool_name: &str) -> PathBuf {
        self.config.output_dir.join(tool_name)
    }

    /// Get the output directory
    pub fn output_dir(&self) -> &Path {
        &self.config.output_dir
    }

    /// Generate required capabilities for a tool
    fn generate_required_capabilities(suggestion: &SolidificationSuggestion) -> crate::exec::sandbox::parameter_binding::RequiredCapabilities {
        use crate::exec::sandbox::parameter_binding::{RequiredCapabilities, CapabilityOverrides};
        use crate::skill_evolution::sandbox_integration::infer_preset_from_purpose;

        let base_preset = infer_preset_from_purpose(&suggestion.instructions_preview);

        RequiredCapabilities {
            base_preset,
            description: format!("Capabilities for {}", suggestion.suggested_name),
            overrides: CapabilityOverrides::default(),
            parameter_bindings: Default::default(),
        }
    }
}

impl Default for ToolGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Convert a skill name to a valid tool name (snake_case)
fn to_tool_name(skill_name: &str) -> String {
    skill_name
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_lowercase()
}

/// Get file extension for a runtime
fn get_extension(runtime: &str) -> &str {
    match runtime {
        "python" => "py",
        "node" => "js",
        _ => "py",
    }
}

/// Generate input schema from instructions
fn generate_input_schema(instructions: &str) -> serde_json::Value {
    // Simple heuristic: if instructions mention "input", "text", "content", etc.
    // create corresponding properties
    let lower = instructions.to_lowercase();

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    // Common input patterns
    if lower.contains("input") || lower.contains("text") || lower.contains("content") {
        properties.insert(
            "input".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "The input text to process"
            }),
        );
        required.push("input");
    }

    if lower.contains("file") || lower.contains("path") {
        properties.insert(
            "file_path".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Path to the file to process"
            }),
        );
    }

    if lower.contains("output") || lower.contains("destination") {
        properties.insert(
            "output_path".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Path for the output"
            }),
        );
    }

    // Default to a generic input if no patterns matched
    if properties.is_empty() {
        properties.insert(
            "input".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "The input to process"
            }),
        );
        required.push("input");
    }

    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

/// Generate Python entrypoint
fn generate_python_entrypoint(
    suggestion: &SolidificationSuggestion,
    input_schema: &serde_json::Value,
) -> (String, String) {
    let properties = input_schema
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();

    let params: Vec<String> = properties
        .keys()
        .map(|k| format!("{}: str", k))
        .collect();

    let param_docs: Vec<String> = properties
        .iter()
        .map(|(k, v)| {
            let desc = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
            format!("        {}: {}", k, desc)
        })
        .collect();

    let code = format!(
        r#"#!/usr/bin/env python3
"""
{name}: {description}

Auto-generated by Aleph Skill Compiler
Pattern ID: {pattern_id}
Confidence: {confidence:.0}%
"""

import json
import sys
from typing import Any, Dict


def main({params}) -> Dict[str, Any]:
    """
    Execute the skill logic.

    Args:
{param_docs}

    Returns:
        A dictionary with the result
    """
    # TODO: Implement the actual logic based on the skill instructions:
    #
{instructions}
    #
    # This is a placeholder implementation
    result = {{
        "status": "success",
        "message": "Processed successfully",
        "input_received": locals()
    }}
    return result


if __name__ == "__main__":
    # Read input from stdin (JSON)
    input_data = json.loads(sys.stdin.read())

    # Execute and output result
    result = main(**input_data)
    print(json.dumps(result, indent=2))
"#,
        name = suggestion.suggested_name,
        description = suggestion.suggested_description,
        pattern_id = suggestion.pattern_id,
        confidence = suggestion.confidence * 100.0,
        params = params.join(", "),
        param_docs = param_docs.join("\n"),
        instructions = suggestion
            .instructions_preview
            .lines()
            .map(|l| format!("    # {}", l))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let preview = format!(
        "```python\n{}\n```\n\nInput Schema:\n```json\n{}\n```",
        code.lines().take(30).collect::<Vec<_>>().join("\n"),
        serde_json::to_string_pretty(input_schema).unwrap_or_default()
    );

    (code, preview)
}

/// Generate Node.js entrypoint
fn generate_node_entrypoint(
    suggestion: &SolidificationSuggestion,
    input_schema: &serde_json::Value,
) -> (String, String) {
    let code = format!(
        r#"#!/usr/bin/env node
/**
 * {name}: {description}
 *
 * Auto-generated by Aleph Skill Compiler
 * Pattern ID: {pattern_id}
 * Confidence: {confidence:.0}%
 */

async function main(input) {{
    // TODO: Implement the actual logic based on the skill instructions:
    //
{instructions}
    //
    // This is a placeholder implementation
    return {{
        status: "success",
        message: "Processed successfully",
        input_received: input
    }};
}}

// Read input from stdin and execute
let data = "";
process.stdin.on("data", chunk => data += chunk);
process.stdin.on("end", async () => {{
    try {{
        const input = JSON.parse(data);
        const result = await main(input);
        console.log(JSON.stringify(result, null, 2));
    }} catch (error) {{
        console.error(JSON.stringify({{ error: error.message }}));
        process.exit(1);
    }}
}});
"#,
        name = suggestion.suggested_name,
        description = suggestion.suggested_description,
        pattern_id = suggestion.pattern_id,
        confidence = suggestion.confidence * 100.0,
        instructions = suggestion
            .instructions_preview
            .lines()
            .map(|l| format!("    // {}", l))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let preview = format!(
        "```javascript\n{}\n```\n\nInput Schema:\n```json\n{}\n```",
        code.lines().take(30).collect::<Vec<_>>().join("\n"),
        serde_json::to_string_pretty(input_schema).unwrap_or_default()
    );

    (code, preview)
}

/// Generate README for the tool
fn generate_readme(
    suggestion: &SolidificationSuggestion,
    definition: &GeneratedToolDefinition,
) -> String {
    format!(
        r#"# {name}

{description}

## Usage

This tool was auto-generated from repeated successful execution patterns.

### Input Schema

```json
{schema}
```

### Example

```bash
echo '{{"input": "example"}}' | python entrypoint.py
```

## Generation Info

- **Pattern ID**: {pattern_id}
- **Confidence**: {confidence:.0}%
- **Runtime**: {runtime}
- **Generated**: {generated_at}

## Original Instructions

{instructions}

---

*Auto-generated by Aleph Skill Compiler*
"#,
        name = definition.name,
        description = definition.description,
        schema = serde_json::to_string_pretty(&definition.input_schema).unwrap_or_default(),
        pattern_id = definition.generated.pattern_id,
        confidence = definition.generated.confidence * 100.0,
        runtime = definition.runtime,
        generated_at = definition.generated.generated_at,
        instructions = suggestion.instructions_preview,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;
    use tempfile::TempDir;

    fn create_test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "test-pattern".to_string(),
            suggested_name: "text-processor".to_string(),
            suggested_description: "Process text input".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("test-pattern"),
            sample_contexts: vec!["process text".to_string()],
            instructions_preview: "# Text Processor\n\nProcess the input text and return result."
                .to_string(),
        }
    }

    #[test]
    fn test_to_tool_name() {
        assert_eq!(to_tool_name("text-processor"), "text_processor");
        assert_eq!(to_tool_name("MyTool"), "mytool");
        assert_eq!(to_tool_name("tool_name"), "tool_name");
    }

    #[test]
    fn test_generate_tool() {
        let temp_dir = TempDir::new().unwrap();
        let config = ToolGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        };

        let generator = ToolGenerator::with_config(config);
        let suggestion = create_test_suggestion();

        let result = generator.generate(&suggestion).unwrap();

        assert_eq!(result.definition.name, "text_processor");
        assert!(result.package_dir.exists());
        assert!(result.entrypoint_path.exists());
    }

    #[test]
    fn test_list_tools() {
        let temp_dir = TempDir::new().unwrap();
        let config = ToolGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        };

        let generator = ToolGenerator::with_config(config);
        let suggestion = create_test_suggestion();

        generator.generate(&suggestion).unwrap();

        let tools = generator.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "text_processor");
    }

    #[test]
    fn test_delete_tool() {
        let temp_dir = TempDir::new().unwrap();
        let config = ToolGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        };

        let generator = ToolGenerator::with_config(config);
        let suggestion = create_test_suggestion();

        generator.generate(&suggestion).unwrap();
        assert_eq!(generator.list_tools().unwrap().len(), 1);

        generator.delete_tool("text_processor").unwrap();
        assert_eq!(generator.list_tools().unwrap().len(), 0);
    }

    #[test]
    fn test_mark_self_tested() {
        let temp_dir = TempDir::new().unwrap();
        let config = ToolGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        };

        let generator = ToolGenerator::with_config(config);
        let suggestion = create_test_suggestion();

        generator.generate(&suggestion).unwrap();

        // Initially not tested
        let def = generator.load_definition("text_processor").unwrap();
        assert!(!def.self_tested);

        // Mark as tested
        generator.mark_self_tested("text_processor", true).unwrap();

        let def = generator.load_definition("text_processor").unwrap();
        assert!(def.self_tested);
    }

    #[test]
    fn test_generate_input_schema() {
        let schema = generate_input_schema("Process the input text");
        assert!(schema.get("properties").unwrap().get("input").is_some());

        let schema = generate_input_schema("Read from file path");
        assert!(schema.get("properties").unwrap().get("file_path").is_some());
    }

    #[test]
    fn test_preview() {
        let generator = ToolGenerator::new();
        let suggestion = create_test_suggestion();

        let preview = generator.preview(&suggestion);
        assert!(preview.contains("python"));
        // Preview shows the original suggested name (with hyphen)
        assert!(preview.contains("text-processor"));
    }

    #[test]
    fn test_tool_definition_with_capabilities() {
        let def = GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(crate::exec::sandbox::parameter_binding::RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test capabilities".to_string(),
                overrides: Default::default(),
                parameter_bindings: Default::default(),
            }),
            generated: GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        };

        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("required_capabilities"));
    }
}
