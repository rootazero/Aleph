//! Prompt layers — each file implements one PromptLayer.

// --- Always-on layers ---
mod citation_standards;
mod guidelines;
mod role;
mod special_actions;

// --- Config-gated layers ---
mod agent_catalog;
mod custom_instructions;
mod generation_models;
mod language;
mod runtime_capabilities;
mod mcp_instructions;
mod mcp_tool_index;
mod skill_instructions;

// --- Behavior layers ---
mod response_format;
mod skill_mode;
mod thinking_guidance;

// --- Identity layer ---
mod soul;

// --- Agent role layer ---
mod agent_role;

// --- Profile layer ---
pub mod profile;

// --- Tool layers ---
mod tool_usage_grammar;
mod tools;

// --- Context layers ---
mod environment;
mod heartbeat;
mod inbound_context;
mod operational_guidelines;
mod protocol_tokens;
mod runtime_context;
mod security;
mod voice_mode;

// --- Identity files layer ---
mod identity_files;

// --- Memory augmentation layer ---
mod memory_augmentation;

// --- Session context guide layer ---
mod session_context_guide;

// --- Session resume layer ---
mod session_resume;

// --- Bootstrap layer ---
pub mod bootstrap;

// --- Re-exports ---
pub use citation_standards::CitationStandardsLayer;
pub use guidelines::GuidelinesLayer;
pub use role::RoleLayer;
pub use special_actions::SpecialActionsLayer;

pub use agent_catalog::AgentCatalogLayer;
pub use custom_instructions::CustomInstructionsLayer;
pub use generation_models::GenerationModelsLayer;
pub use language::LanguageLayer;
pub use mcp_tool_index::McpToolIndexLayer;
pub use runtime_capabilities::RuntimeCapabilitiesLayer;
pub use mcp_instructions::McpInstructionsLayer;
pub use skill_instructions::SkillInstructionsLayer;

pub use response_format::ResponseFormatLayer;
pub use skill_mode::SkillModeLayer;
pub use thinking_guidance::ThinkingGuidanceLayer;

pub use agent_role::AgentRoleLayer;
pub use profile::ProfileLayer;
pub use soul::SoulLayer;

pub use tool_usage_grammar::ToolUsageGrammarLayer;
pub use tools::HydratedToolsLayer;
pub use tools::ToolsLayer;

pub use bootstrap::BootstrapLayer;
pub use environment::EnvironmentLayer;
pub use heartbeat::HeartbeatLayer;
pub use identity_files::IdentityFilesLayer;
pub use inbound_context::InboundContextLayer;
pub use memory_augmentation::MemoryAugmentationLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
pub use protocol_tokens::ProtocolTokensLayer;
pub use runtime_context::RuntimeContextLayer;
pub use security::SecurityLayer;
pub use session_context_guide::SessionContextGuideLayer;
pub use session_resume::SessionResumeLayer;
pub use voice_mode::VoiceModeLayer;
