//! Context Aggregator for Channel Capability Awareness
//!
//! This module RECONCILES the interaction layer (what's technically possible)
//! with the security layer (what's allowed by policy) through two-phase filtering.
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────────┐
//! │                         ContextAggregator                                  │
//! │                                                                            │
//! │  ┌──────────────────────┐        ┌──────────────────────┐                  │
//! │  │ InteractionManifest  │        │   SecurityContext    │                  │
//! │  │ (technical caps)     │        │   (policy rules)     │                  │
//! │  └──────────┬───────────┘        └──────────┬───────────┘                  │
//! │             │                               │                              │
//! │             ▼                               ▼                              │
//! │  ┌─────────────────────────────────────────────────────────────────────┐   │
//! │  │                    Two-Phase Filtering                              │   │
//! │  │                                                                     │   │
//! │  │  Phase 1: InteractionManifest.supports_tool()                       │   │
//! │  │           → UnsupportedByChannel (silent filter)                    │   │
//! │  │                                                                     │   │
//! │  │  Phase 2: SecurityContext.check_tool()                              │   │
//! │  │           → BlockedByPolicy / RequiresApproval (transparent)        │   │
//! │  └─────────────────────────────────────────────────────────────────────┘   │
//! │                                    │                                       │
//! │                                    ▼                                       │
//! │  ┌─────────────────────────────────────────────────────────────────────┐   │
//! │  │                       ResolvedContext                               │   │
//! │  │  • available_tools: Vec<ToolInfo>                                   │   │
//! │  │  • disabled_tools: Vec<DisabledTool>                                │   │
//! │  │  • environment_contract: EnvironmentContract                        │   │
//! │  └─────────────────────────────────────────────────────────────────────┘   │
//! └────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//! use alephcore::thinker::{
//!     ContextAggregator, InteractionManifest, InteractionParadigm,
//!     SecurityContext, ResolvedContext,
//! };
//! use alephcore::agent_loop::ToolInfo;
//!
//! // Create manifests for WebRich channel with permissive security
//! let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
//! let security = SecurityContext::permissive();
//!
//! // Sample tools
//! let tools = vec![
//!     ToolInfo {
//!         name: "web_search".to_string(),
//!         description: "Search the web".to_string(),
//!         parameters_schema: "{}".to_string(),
//!         category: Some("search".to_string()),
//!     },
//! ];
//!
//! // Resolve context
//! let resolved = ContextAggregator::resolve(&interaction, &security, &tools);
//! assert_eq!(resolved.available_tools.len(), 1);
//! assert!(resolved.disabled_tools.is_empty());
//! ```

use serde::{Deserialize, Serialize};

use super::interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
use super::security_context::{SecurityContext, ToolPermission};
use crate::agent_loop::ToolInfo;

/// Reason why a tool is disabled
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DisableReason {
    /// Tool is not supported by the current channel (silent filter)
    ///
    /// This is used for interaction-layer filtering. The AI should not
    /// mention these tools as they are technically unavailable.
    UnsupportedByChannel,

    /// Tool is blocked by security policy (transparent to AI)
    ///
    /// The AI is informed about this restriction so it can explain
    /// to the user why the tool is unavailable.
    BlockedByPolicy {
        /// Reason for the policy block
        reason: String,
    },

    /// Tool requires user approval before execution
    ///
    /// The tool is available but execution requires explicit approval.
    /// The AI should inform the user before attempting to use it.
    RequiresApproval {
        /// Prompt to display for approval
        prompt: String,
    },
}

/// A tool that is disabled with a specific reason
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisabledTool {
    /// Tool name
    pub name: String,
    /// Reason for disabling
    pub reason: DisableReason,
}

/// Contract describing the current environment for the AI
///
/// This struct provides a unified view of what the AI can do in the
/// current environment, combining interaction capabilities with
/// security constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentContract {
    /// The interaction paradigm (CLI, WebRich, Messaging, etc.)
    pub paradigm: InteractionParadigm,
    /// Active capabilities in this environment
    pub active_capabilities: Vec<Capability>,
    /// Interaction constraints (output limits, streaming, etc.)
    pub constraints: InteractionConstraints,
    /// Security notes to include in system prompt
    pub security_notes: Vec<String>,
}

impl EnvironmentContract {
    /// Generate a description suitable for system prompt injection
    pub fn to_prompt_description(&self) -> String {
        let mut parts = Vec::new();

        // Paradigm description
        parts.push(format!("Environment: {}", self.paradigm.description()));

        // Active capabilities
        if !self.active_capabilities.is_empty() {
            let cap_hints: Vec<String> = self.active_capabilities
                .iter()
                .map(|c| {
                    let (name, hint) = c.prompt_hint();
                    format!("- {}: {}", name, hint)
                })
                .collect();
            parts.push(format!("Capabilities:\n{}", cap_hints.join("\n")));
        }

        // Constraints
        let mut constraint_notes = Vec::new();
        if let Some(max_chars) = self.constraints.max_output_chars {
            constraint_notes.push(format!("Max output: {} characters", max_chars));
        }
        if self.constraints.prefer_compact {
            constraint_notes.push("Prefer compact responses".to_string());
        }
        if !constraint_notes.is_empty() {
            parts.push(format!("Constraints: {}", constraint_notes.join(", ")));
        }

        // Security notes
        if !self.security_notes.is_empty() {
            parts.push(format!("Security:\n{}", self.security_notes.iter()
                .map(|n| format!("- {}", n))
                .collect::<Vec<_>>()
                .join("\n")));
        }

        parts.join("\n\n")
    }
}

/// Resolved context after two-phase filtering
///
/// This is the final output of the ContextAggregator, containing:
/// - Tools that are fully available for use
/// - Tools that are disabled (with reasons)
/// - The environment contract describing the AI's working context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedContext {
    /// Tools available for the AI to use
    pub available_tools: Vec<ToolInfo>,
    /// Tools that are disabled with reasons
    pub disabled_tools: Vec<DisabledTool>,
    /// Environment contract describing the working context
    pub environment_contract: EnvironmentContract,
}

/// Context Aggregator for reconciling interaction and security layers
///
/// The ContextAggregator performs two-phase filtering:
///
/// 1. **Interaction Phase**: Filter based on channel capabilities
///    - Tools unsupported by the channel are silently removed
///
/// 2. **Security Phase**: Filter based on security policy
///    - Tools blocked by policy are disabled with transparent reasons
///    - Tools requiring approval are marked but still available
pub struct ContextAggregator;

impl ContextAggregator {
    /// Resolve the final context from interaction manifest, security context, and tools
    ///
    /// This performs two-phase filtering:
    /// 1. Check if tool is supported by interaction layer (silent filter)
    /// 2. Check security policy (transparent filter with reasons)
    ///
    /// Tools requiring approval are added to BOTH available_tools AND disabled_tools
    /// so the AI knows they exist but also knows approval is needed.
    pub fn resolve(
        interaction: &InteractionManifest,
        security: &SecurityContext,
        all_tools: &[ToolInfo],
    ) -> ResolvedContext {
        let mut available_tools = Vec::new();
        let mut disabled_tools = Vec::new();

        for tool in all_tools {
            // Phase 1: Interaction layer filter (silent)
            if !interaction.supports_tool(&tool.name) {
                disabled_tools.push(DisabledTool {
                    name: tool.name.clone(),
                    reason: DisableReason::UnsupportedByChannel,
                });
                continue;
            }

            // Phase 2: Security layer filter (transparent)
            match security.check_tool(&tool.name) {
                ToolPermission::Allowed => {
                    available_tools.push(tool.clone());
                }
                ToolPermission::Denied { reason } => {
                    disabled_tools.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::BlockedByPolicy { reason },
                    });
                }
                ToolPermission::RequiresApproval { prompt } => {
                    // Tool is available but marked as requiring approval
                    // Add to BOTH lists so AI knows it exists and knows approval is needed
                    available_tools.push(tool.clone());
                    disabled_tools.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::RequiresApproval { prompt },
                    });
                }
            }
        }

        let environment_contract = Self::build_contract(interaction, security);

        ResolvedContext {
            available_tools,
            disabled_tools,
            environment_contract,
        }
    }

    /// Build the environment contract from interaction and security contexts
    fn build_contract(
        interaction: &InteractionManifest,
        security: &SecurityContext,
    ) -> EnvironmentContract {
        EnvironmentContract {
            paradigm: interaction.paradigm,
            active_capabilities: interaction.capabilities.iter().cloned().collect(),
            constraints: interaction.constraints.clone(),
            security_notes: security.security_notes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_tool(name: &str) -> ToolInfo {
        ToolInfo {
            name: name.to_string(),
            description: format!("{} tool description", name),
            parameters_schema: "{}".to_string(),
            category: None,
        }
    }

    #[test]
    fn test_all_tools_available_in_permissive() {
        // WebRich + permissive = all tools should be available
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();

        let tools = vec![
            make_tool("web_search"),
            make_tool("file_ops"),
            make_tool("bash"),
            make_tool("canvas"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // All tools should be available
        assert_eq!(resolved.available_tools.len(), 4);

        // No tools should be disabled
        assert!(resolved.disabled_tools.is_empty());
    }

    #[test]
    fn test_security_blocks_tool() {
        // CLI + strict_readonly should block file_ops and exec tools
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let tools = vec![
            make_tool("web_search"),
            make_tool("file_ops"),
            make_tool("exec"),
            make_tool("read_skill"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // Only read_skill should be available (web_search blocked by network policy)
        let available_names: Vec<&str> = resolved.available_tools.iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"read_skill"));
        assert!(!available_names.contains(&"file_ops"));
        assert!(!available_names.contains(&"exec"));

        // file_ops, exec should be in disabled list with BlockedByPolicy
        let file_ops_disabled = resolved.disabled_tools.iter()
            .find(|d| d.name == "file_ops");
        assert!(file_ops_disabled.is_some());
        assert!(matches!(
            file_ops_disabled.unwrap().reason,
            DisableReason::BlockedByPolicy { .. }
        ));

        let exec_disabled = resolved.disabled_tools.iter()
            .find(|d| d.name == "exec");
        assert!(exec_disabled.is_some());
        assert!(matches!(
            exec_disabled.unwrap().reason,
            DisableReason::BlockedByPolicy { .. }
        ));
    }

    #[test]
    fn test_requires_approval_shows_both() {
        // Standard sandbox requires approval for exec tools
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));

        let tools = vec![
            make_tool("web_search"),
            make_tool("bash"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // bash should be in available_tools
        let available_names: Vec<&str> = resolved.available_tools.iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"bash"));
        assert!(available_names.contains(&"web_search"));

        // bash should ALSO be in disabled_tools with RequiresApproval
        let bash_disabled = resolved.disabled_tools.iter()
            .find(|d| d.name == "bash");
        assert!(bash_disabled.is_some());
        assert!(matches!(
            bash_disabled.unwrap().reason,
            DisableReason::RequiresApproval { .. }
        ));

        // web_search should NOT be in disabled_tools
        let web_disabled = resolved.disabled_tools.iter()
            .find(|d| d.name == "web_search");
        assert!(web_disabled.is_none());
    }

    #[test]
    fn test_environment_contract() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        let contract = &resolved.environment_contract;

        // Check paradigm
        assert_eq!(contract.paradigm, InteractionParadigm::CLI);

        // Check capabilities (CLI has RichText, CodeHighlight, Streaming)
        assert!(contract.active_capabilities.contains(&Capability::RichText));
        assert!(contract.active_capabilities.contains(&Capability::Streaming));
        assert!(!contract.active_capabilities.contains(&Capability::Canvas));

        // Check constraints
        assert!(!contract.constraints.prefer_compact);

        // Check security notes (strict mode should have several notes)
        assert!(!contract.security_notes.is_empty());
        assert!(contract.security_notes.iter().any(|n| n.contains("Strict")));
        assert!(contract.security_notes.iter().any(|n| n.contains("Network Access: Disabled")));
    }

    #[test]
    fn test_canvas_filtered_by_interaction() {
        // CLI doesn't support canvas capability
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::permissive();

        let tools = vec![
            make_tool("web_search"),
            make_tool("canvas"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // web_search should be available
        let available_names: Vec<&str> = resolved.available_tools.iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"web_search"));

        // canvas should NOT be in available_tools (filtered by interaction layer)
        assert!(!available_names.contains(&"canvas"));

        // canvas should be in disabled_tools with UnsupportedByChannel
        let canvas_disabled = resolved.disabled_tools.iter()
            .find(|d| d.name == "canvas");
        assert!(canvas_disabled.is_some());
        assert!(matches!(
            canvas_disabled.unwrap().reason,
            DisableReason::UnsupportedByChannel
        ));
    }

    #[test]
    fn test_environment_contract_prompt_description() {
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        let description = resolved.environment_contract.to_prompt_description();

        // Should contain environment info
        assert!(description.contains("Environment:"));
        assert!(description.contains("Web"));

        // Should contain capabilities
        assert!(description.contains("Capabilities:"));

        // Should contain security info
        assert!(description.contains("Security:"));
        assert!(description.contains("Strict"));
    }
}
