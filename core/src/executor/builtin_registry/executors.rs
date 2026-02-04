//! Tool execution implementations for builtin tools
//!
//! Note: Most tools now use AlephTool::call_json directly from registry.rs.
//! This file only contains execute_* methods for tools that haven't been
//! migrated to AlephTool (video/audio generation, delegate).

use serde_json::Value;
use tracing::info;

use crate::agents::sub_agents::{DelegateArgs, DelegateTool};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

use super::BuiltinToolRegistry;

impl BuiltinToolRegistry {
    /// Execute the video generation tool
    ///
    /// Note: Video generation has not been migrated to AlephTool yet
    /// as it uses the generation registry directly.
    pub(crate) async fn execute_video_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AlephError::tool("Video generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AlephError::tool("Invalid generate_video arguments: expected object")
        })?;

        let prompt = obj
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AlephError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AlephError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AlephError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Video) {
                    return Err(AlephError::tool(format!(
                        "Provider '{}' does not support video generation",
                        pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Video)
                    .ok_or_else(|| AlephError::tool("No video generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing video generation");

        // Create request and generate
        let request = GenerationRequest::video(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AlephError::tool(format!("Video generation failed: {}", e))
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
    ///
    /// Note: Audio generation has not been migrated to AlephTool yet
    /// as it uses the generation registry directly.
    pub(crate) async fn execute_audio_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AlephError::tool("Audio generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AlephError::tool("Invalid generate_audio arguments: expected object")
        })?;

        let prompt = obj
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AlephError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AlephError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AlephError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Audio) {
                    return Err(AlephError::tool(format!(
                        "Provider '{}' does not support audio generation",
                        pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Audio)
                    .ok_or_else(|| AlephError::tool("No audio generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing audio generation");

        // Create request and generate
        let request = GenerationRequest::audio(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AlephError::tool(format!("Audio generation failed: {}", e))
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

    /// Execute the delegate tool for sub-agent delegation
    pub(crate) async fn execute_delegate(&self, arguments: Value) -> Result<Value> {
        let dispatcher = self.sub_agent_dispatcher.as_ref().ok_or_else(|| {
            AlephError::tool("delegate not available: no sub_agent_dispatcher configured")
        })?;

        let args: DelegateArgs = serde_json::from_value(arguments).map_err(|e| {
            AlephError::tool(format!("Invalid delegate arguments: {}", e))
        })?;

        // Create a temporary DelegateTool and execute via AlephTool trait
        let tool = DelegateTool::new(std::sync::Arc::clone(dispatcher));
        let result = AlephTool::call(&tool, args).await?;

        serde_json::to_value(result)
            .map_err(|e| AlephError::tool(format!("Failed to serialize result: {}", e)))
    }
}
