//! Factory for assembling MinimalAgentLoop from existing Aleph services.
//!
//! Bridges the "old world" (AiProvider, AlephToolServer, SoulManifest) to the
//! "new world" (MinimalAgentLoop with flat tool registry and prompt builder).

use std::sync::Arc;

use crate::providers::AiProvider;
use crate::thinker::soul::SoulManifest;
use crate::tools::AlephToolDyn;

use super::adapters::BuiltinToolAdapter;
use super::loop_core::{LoopConfig, MinimalAgentLoop};
use super::prompt_builder::MinimalPromptBuilder;
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::MinimalToolRegistry;

/// Factory that assembles a `MinimalAgentLoop` from existing Aleph services.
pub struct MinimalLoopFactory;

impl MinimalLoopFactory {
    /// Build a `MinimalAgentLoop` from existing Aleph services.
    ///
    /// # Arguments
    ///
    /// * `provider` — AI provider (from ProviderRegistry::default_provider())
    /// * `tools` — All registered tools as Arc<dyn AlephToolDyn>
    /// * `soul` — Optional SoulManifest for identity/personality
    /// * `config` — Loop configuration (max iterations, timeout, etc.)
    pub fn build(
        provider: Arc<dyn AiProvider>,
        tools: Vec<Arc<dyn AlephToolDyn>>,
        soul: Option<&SoulManifest>,
        config: LoopConfig,
    ) -> MinimalAgentLoop<AiProviderBridge> {
        // Wrap provider via bridge
        let bridge = AiProviderBridge::new(provider);

        // Adapt existing tools into MinimalTool
        let mut registry = MinimalToolRegistry::new();
        for tool_dyn in tools {
            registry.register(Box::new(BuiltinToolAdapter::new(tool_dyn)));
        }

        // Build prompt from soul or defaults
        let prompt_builder = match soul {
            Some(s) => MinimalPromptBuilder::from_soul(s),
            None => MinimalPromptBuilder::new(),
        };

        // Safety guard with sensible defaults
        let safety = SafetyGuard::default_guard();

        MinimalAgentLoop::new(bridge, registry, prompt_builder, safety, config)
    }

    /// Build a `MinimalAgentLoop` from an `AlephToolServer`.
    ///
    /// Convenience wrapper that fetches tools from the server (async).
    pub async fn build_from_server(
        provider: Arc<dyn AiProvider>,
        tool_server: &crate::tools::AlephToolServer,
        soul: Option<&SoulManifest>,
        config: LoopConfig,
    ) -> MinimalAgentLoop<AiProviderBridge> {
        let tools = tool_server.list_tools_arc().await;
        Self::build(provider, tools, soul, config)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{ToolCategory, ToolDefinition as DispatcherToolDefinition};
    use crate::tools::AlephToolDyn;
    use serde_json::{json, Value};
    use std::future::Future;
    use std::pin::Pin;

    /// Minimal fake provider for factory tests.
    struct FakeProvider;

    impl AiProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn color(&self) -> &str {
            "#000000"
        }

        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
            Box::pin(async { Ok("response".into()) })
        }
    }

    /// Minimal fake tool for factory tests.
    struct FakeTool {
        tool_name: String,
    }

    impl AlephToolDyn for FakeTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn definition(&self) -> DispatcherToolDefinition {
            DispatcherToolDefinition::new(
                &self.tool_name,
                &format!("Fake tool: {}", self.tool_name),
                json!({"type": "object", "properties": {}}),
                ToolCategory::Builtin,
            )
        }

        fn call(
            &self,
            _args: Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<Value>> + Send + '_>> {
            Box::pin(async { Ok(json!({"ok": true})) })
        }
    }

    #[test]
    fn test_factory_build_empty_tools() {
        let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider);
        let config = LoopConfig {
            max_iterations: 5,
            token_budget: 10000,
            timeout_secs: 30,
        };

        let loop_instance = MinimalLoopFactory::build(provider, vec![], None, config);
        // Should create a valid loop with empty registry
        let defs = loop_instance.tool_definitions();
        assert!(defs.is_empty());
    }

    #[test]
    fn test_factory_build_with_tools() {
        let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider);
        let tools: Vec<Arc<dyn AlephToolDyn>> = vec![
            Arc::new(FakeTool {
                tool_name: "search".into(),
            }),
            Arc::new(FakeTool {
                tool_name: "memory".into(),
            }),
        ];
        let config = LoopConfig {
            max_iterations: 10,
            token_budget: 50000,
            timeout_secs: 60,
        };

        let loop_instance = MinimalLoopFactory::build(provider, tools, None, config);
        let defs = loop_instance.tool_definitions();
        assert_eq!(defs.len(), 2);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"search"));
        assert!(names.contains(&"memory"));
    }

    #[test]
    fn test_factory_build_with_soul() {
        use crate::thinker::soul::{SoulManifest, SoulVoice};

        let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider);
        let soul = SoulManifest {
            identity: "I am Aleph, a personal AI.".into(),
            voice: SoulVoice {
                tone: "warm and concise".into(),
                ..Default::default()
            },
            directives: vec!["Be helpful".into()],
            ..Default::default()
        };
        let config = LoopConfig {
            max_iterations: 10,
            token_budget: 50000,
            timeout_secs: 60,
        };

        let _loop_instance = MinimalLoopFactory::build(provider, vec![], Some(&soul), config);
        // Factory should succeed — prompt builder was configured from soul
    }

    #[tokio::test]
    async fn test_factory_build_from_server() {
        let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider);
        let server = crate::tools::AlephToolServer::new();

        // Add tools to server
        server
            .add_tool(FakeTool {
                tool_name: "file_read".into(),
            })
            .await;
        server
            .add_tool(FakeTool {
                tool_name: "web_search".into(),
            })
            .await;

        let config = LoopConfig {
            max_iterations: 10,
            token_budget: 50000,
            timeout_secs: 60,
        };

        let loop_instance =
            MinimalLoopFactory::build_from_server(provider, &server, None, config).await;
        let defs = loop_instance.tool_definitions();
        assert_eq!(defs.len(), 2);
    }
}
