//! MCP Context Injector
//!
//! Injects context from MCP servers into sampling requests.
//! Supports both single-server and all-server context modes.

use crate::mcp::client::McpClient;
use crate::mcp::jsonrpc::mcp::{IncludeContext, PromptRole, SamplingContent, SamplingMessage};

/// Context that can be injected into sampling requests
#[derive(Debug, Clone)]
pub struct InjectedContext {
    /// Server name that provided the context
    pub server_name: String,
    /// Resources from the server
    pub resources: Vec<ResourceContext>,
    /// Available tools from the server
    pub tools: Vec<ToolContext>,
}

/// Resource context summary
#[derive(Debug, Clone)]
pub struct ResourceContext {
    /// Resource URI
    pub uri: String,
    /// Resource name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
}

/// Tool context summary
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
}

/// Injects context from MCP servers into sampling messages
pub struct ContextInjector;

impl ContextInjector {
    /// Gather context from MCP client based on include mode
    pub async fn gather_context(
        client: &McpClient,
        mode: &IncludeContext,
        requesting_server: &str,
    ) -> Vec<InjectedContext> {
        match mode {
            IncludeContext::ThisServer => Self::gather_server_context(client, requesting_server)
                .await
                .map(|ctx| vec![ctx])
                .unwrap_or_default(),
            IncludeContext::AllServers => Self::gather_all_context(client).await,
        }
    }

    /// Gather context from a specific server
    async fn gather_server_context(
        client: &McpClient,
        server_name: &str,
    ) -> Option<InjectedContext> {
        let all_resources = client.list_resources().await;
        let all_tools = client.list_tools().await;

        // Filter to resources/tools from this server (prefixed with server_name:)
        let resources: Vec<ResourceContext> = all_resources
            .into_iter()
            .filter(|r| {
                r.uri.starts_with(&format!("{}:", server_name))
                    || r.name.starts_with(&format!("{}:", server_name))
            })
            .map(|r| ResourceContext {
                uri: r.uri,
                name: r.name,
                description: r.description,
            })
            .collect();

        let tools: Vec<ToolContext> = all_tools
            .into_iter()
            .filter(|t| t.name.starts_with(&format!("{}:", server_name)))
            .map(|t| ToolContext {
                name: t.name,
                description: t.description,
            })
            .collect();

        if resources.is_empty() && tools.is_empty() {
            None
        } else {
            Some(InjectedContext {
                server_name: server_name.to_string(),
                resources,
                tools,
            })
        }
    }

    /// Gather context from all connected servers
    async fn gather_all_context(client: &McpClient) -> Vec<InjectedContext> {
        let server_names = client.service_names().await;
        let mut contexts = Vec::new();

        for name in server_names {
            if let Some(ctx) = Self::gather_server_context(client, &name).await {
                contexts.push(ctx);
            }
        }

        contexts
    }

    /// Format context as a system message for injection
    pub fn format_as_system_message(contexts: &[InjectedContext]) -> Option<SamplingMessage> {
        if contexts.is_empty() {
            return None;
        }

        let mut parts = vec!["Available MCP context:".to_string()];

        for ctx in contexts {
            parts.push(format!("\n## Server: {}", ctx.server_name));

            if !ctx.resources.is_empty() {
                parts.push("\n### Resources:".to_string());
                for r in &ctx.resources {
                    let desc = r.description.as_deref().unwrap_or("No description");
                    parts.push(format!("- {} ({}): {}", r.name, r.uri, desc));
                }
            }

            if !ctx.tools.is_empty() {
                parts.push("\n### Tools:".to_string());
                for t in &ctx.tools {
                    parts.push(format!("- {}: {}", t.name, t.description));
                }
            }
        }

        Some(SamplingMessage {
            role: PromptRole::System,
            content: SamplingContent::Text {
                text: parts.join("\n"),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_context() {
        let result = ContextInjector::format_as_system_message(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_format_context_with_resources() {
        let context = InjectedContext {
            server_name: "test-server".to_string(),
            resources: vec![ResourceContext {
                uri: "test-server:file:///test.txt".to_string(),
                name: "test.txt".to_string(),
                description: Some("A test file".to_string()),
            }],
            tools: vec![],
        };

        let result = ContextInjector::format_as_system_message(&[context]);
        assert!(result.is_some());

        if let Some(msg) = result {
            if let SamplingContent::Text { text } = msg.content {
                assert!(text.contains("test-server"));
                assert!(text.contains("test.txt"));
            }
        }
    }

    #[test]
    fn test_format_context_with_tools() {
        let context = InjectedContext {
            server_name: "tools-server".to_string(),
            resources: vec![],
            tools: vec![ToolContext {
                name: "tools-server:search".to_string(),
                description: "Search for files".to_string(),
            }],
        };

        let result = ContextInjector::format_as_system_message(&[context]);
        assert!(result.is_some());

        if let Some(msg) = result {
            if let SamplingContent::Text { text } = msg.content {
                assert!(text.contains("tools-server"));
                assert!(text.contains("search"));
                assert!(text.contains("Search for files"));
            }
        }
    }

    #[test]
    fn test_format_context_with_resources_and_tools() {
        let context = InjectedContext {
            server_name: "full-server".to_string(),
            resources: vec![
                ResourceContext {
                    uri: "full-server:file:///doc.md".to_string(),
                    name: "doc.md".to_string(),
                    description: Some("Documentation".to_string()),
                },
                ResourceContext {
                    uri: "full-server:file:///config.json".to_string(),
                    name: "config.json".to_string(),
                    description: None,
                },
            ],
            tools: vec![
                ToolContext {
                    name: "full-server:read".to_string(),
                    description: "Read a file".to_string(),
                },
                ToolContext {
                    name: "full-server:write".to_string(),
                    description: "Write a file".to_string(),
                },
            ],
        };

        let result = ContextInjector::format_as_system_message(&[context]);
        assert!(result.is_some());

        if let Some(msg) = result {
            assert!(matches!(msg.role, PromptRole::System));
            if let SamplingContent::Text { text } = msg.content {
                // Check server name
                assert!(text.contains("## Server: full-server"));
                // Check resources section
                assert!(text.contains("### Resources:"));
                assert!(text.contains("doc.md"));
                assert!(text.contains("Documentation"));
                assert!(text.contains("config.json"));
                assert!(text.contains("No description")); // Default for None
                // Check tools section
                assert!(text.contains("### Tools:"));
                assert!(text.contains("full-server:read"));
                assert!(text.contains("Read a file"));
                assert!(text.contains("full-server:write"));
                assert!(text.contains("Write a file"));
            } else {
                panic!("Expected Text content");
            }
        }
    }

    #[test]
    fn test_format_multiple_server_contexts() {
        let contexts = vec![
            InjectedContext {
                server_name: "server-a".to_string(),
                resources: vec![ResourceContext {
                    uri: "server-a:file:///a.txt".to_string(),
                    name: "a.txt".to_string(),
                    description: Some("File A".to_string()),
                }],
                tools: vec![],
            },
            InjectedContext {
                server_name: "server-b".to_string(),
                resources: vec![],
                tools: vec![ToolContext {
                    name: "server-b:tool".to_string(),
                    description: "Tool B".to_string(),
                }],
            },
        ];

        let result = ContextInjector::format_as_system_message(&contexts);
        assert!(result.is_some());

        if let Some(msg) = result {
            if let SamplingContent::Text { text } = msg.content {
                assert!(text.contains("## Server: server-a"));
                assert!(text.contains("## Server: server-b"));
                assert!(text.contains("a.txt"));
                assert!(text.contains("server-b:tool"));
            }
        }
    }

    #[test]
    fn test_injected_context_clone() {
        let context = InjectedContext {
            server_name: "test".to_string(),
            resources: vec![ResourceContext {
                uri: "test:uri".to_string(),
                name: "name".to_string(),
                description: None,
            }],
            tools: vec![ToolContext {
                name: "tool".to_string(),
                description: "desc".to_string(),
            }],
        };

        let cloned = context.clone();
        assert_eq!(cloned.server_name, context.server_name);
        assert_eq!(cloned.resources.len(), context.resources.len());
        assert_eq!(cloned.tools.len(), context.tools.len());
    }
}
