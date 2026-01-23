//! Tool execution implementations for builtin tools

use rig::tool::Tool;
use serde_json::Value;
use tracing::info;

use crate::agents::sub_agents::{DelegateTool, DelegateArgs};
use crate::error::{AetherError, Result};
use crate::rig_tools::meta_tools::{ListToolsTool, GetToolSchemaTool, ListToolsArgs, GetToolSchemaArgs};
use crate::rig_tools::skill_reader::{ReadSkillArgs, ListSkillsArgs as SkillListArgs};

use super::BuiltinToolRegistry;

impl BuiltinToolRegistry {
    /// Execute the search tool
    pub(crate) async fn execute_search(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::SearchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid search arguments: {}", e))
            })?;

        let result = self.search_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Search failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the web fetch tool
    pub(crate) async fn execute_web_fetch(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::WebFetchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid web_fetch arguments: {}", e))
            })?;

        let result = self.web_fetch_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Web fetch failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the YouTube tool
    pub(crate) async fn execute_youtube(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::YouTubeArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid youtube arguments: {}", e))
            })?;

        let result = self.youtube_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("YouTube tool failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the file operations tool
    pub(crate) async fn execute_file_ops(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::FileOpsArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid file_ops arguments: {}", e))
            })?;

        let result = self.file_ops_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("File operations failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the code execution tool
    pub(crate) async fn execute_code_exec(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::CodeExecArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid code_exec arguments: {}", e))
            })?;

        let result = self.code_exec_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Code execution failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the PDF generation tool
    pub(crate) async fn execute_pdf_generate(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::PdfGenerateArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid pdf_generate arguments: {}", e))
            })?;

        let result = self.pdf_generate_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("PDF generation failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the image generation tool
    pub(crate) async fn execute_image_generate(&self, arguments: Value) -> Result<Value> {
        let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
            AetherError::tool("Image generation not available: no generation registry configured")
        })?;

        let args: crate::rig_tools::ImageGenerateArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid generate_image arguments: {}", e))
            })?;

        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Image generation failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the video generation tool
    pub(crate) async fn execute_video_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AetherError::tool("Video generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AetherError::tool("Invalid generate_video arguments: expected object")
        })?;

        let prompt = obj.get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AetherError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AetherError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Video) {
                    return Err(AetherError::tool(format!(
                        "Provider '{}' does not support video generation", pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Video)
                    .ok_or_else(|| AetherError::tool("No video generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing video generation");

        // Create request and generate
        let request = GenerationRequest::video(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AetherError::tool(format!("Video generation failed: {}", e))
        })?;

        // Build result
        let result = serde_json::json!({
            "provider": name,
            "prompt": prompt,
            "data": match &output.data {
                crate::generation::GenerationData::Url(url) => serde_json::json!({"type": "url", "value": url}),
                crate::generation::GenerationData::LocalPath(path) => serde_json::json!({"type": "file", "value": path}),
                crate::generation::GenerationData::Bytes(bytes) => serde_json::json!({"type": "bytes", "size": bytes.len()}),
            },
            "model": output.metadata.model,
            "duration_ms": output.metadata.duration.map(|d| d.as_millis()),
        });

        Ok(result)
    }

    /// Execute the audio generation tool
    pub(crate) async fn execute_audio_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AetherError::tool("Audio generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AetherError::tool("Invalid generate_audio arguments: expected object")
        })?;

        let prompt = obj.get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AetherError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AetherError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Audio) {
                    return Err(AetherError::tool(format!(
                        "Provider '{}' does not support audio generation", pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Audio)
                    .ok_or_else(|| AetherError::tool("No audio generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing audio generation");

        // Create request and generate
        let request = GenerationRequest::audio(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AetherError::tool(format!("Audio generation failed: {}", e))
        })?;

        // Build result
        let result = serde_json::json!({
            "provider": name,
            "prompt": prompt,
            "data": match &output.data {
                crate::generation::GenerationData::Url(url) => serde_json::json!({"type": "url", "value": url}),
                crate::generation::GenerationData::LocalPath(path) => serde_json::json!({"type": "file", "value": path}),
                crate::generation::GenerationData::Bytes(bytes) => serde_json::json!({"type": "bytes", "size": bytes.len()}),
            },
            "model": output.metadata.model,
            "duration_ms": output.metadata.duration.map(|d| d.as_millis()),
        });

        Ok(result)
    }

    /// Execute the list_tools meta tool
    pub(crate) async fn execute_list_tools(&self, arguments: Value) -> Result<Value> {
        let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
            AetherError::tool("list_tools not available: no dispatcher registry configured")
        })?;

        let args: ListToolsArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid list_tools arguments: {}", e))
        })?;

        // Create a temporary ListToolsTool and execute
        let tool = ListToolsTool::new(std::sync::Arc::clone(registry));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("list_tools failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the get_tool_schema meta tool
    pub(crate) async fn execute_get_tool_schema(&self, arguments: Value) -> Result<Value> {
        let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
            AetherError::tool("get_tool_schema not available: no dispatcher registry configured")
        })?;

        let args: GetToolSchemaArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid get_tool_schema arguments: {}", e))
        })?;

        // Create a temporary GetToolSchemaTool and execute
        let tool = GetToolSchemaTool::new(std::sync::Arc::clone(registry));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("get_tool_schema failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the delegate tool for sub-agent delegation
    pub(crate) async fn execute_delegate(&self, arguments: Value) -> Result<Value> {
        let dispatcher = self.sub_agent_dispatcher.as_ref().ok_or_else(|| {
            AetherError::tool("delegate not available: no sub_agent_dispatcher configured")
        })?;

        let args: DelegateArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid delegate arguments: {}", e))
        })?;

        // Create a temporary DelegateTool and execute
        let tool = DelegateTool::new(std::sync::Arc::clone(dispatcher));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("delegate failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the read_skill tool
    pub(crate) async fn execute_read_skill(&self, arguments: Value) -> Result<Value> {
        let args: ReadSkillArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid read_skill arguments: {}", e))
        })?;

        let result = self.read_skill_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("read_skill failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the list_skills tool
    pub(crate) async fn execute_list_skills(&self, arguments: Value) -> Result<Value> {
        let args: SkillListArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid list_skills arguments: {}", e))
        })?;

        let result = self.list_skills_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("list_skills failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }
}
