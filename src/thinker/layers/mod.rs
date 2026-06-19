//! Prompt layers — each file implements one `PromptLayer`.

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
mod mcp_instructions;
mod runtime_capabilities;
mod skill_instructions;

// --- Behavior layers ---
mod skill_mode;
mod thinking_guidance;

// --- Identity layer ---
mod soul;

// --- Agent role layer ---
mod agent_role;

// --- Profile layer ---
pub mod profile;

// --- Tool layers ---
mod tool_runtime_state;
mod tool_usage_grammar;
mod tools;
pub use tool_runtime_state::ToolRuntimeStateLayer;

// --- Execution-plan layer (active scratchpad checklist, re-surfaced per turn) ---
mod execution_plan;
pub use execution_plan::ExecutionPlanLayer;

// --- Standing-goal layer (active goal objective, re-surfaced per turn) ---
mod standing_goal;
pub use standing_goal::StandingGoalLayer;

// --- Strategy layer (welded <strategy> envelope, Stable prefix, prio 70) ---
mod strategy;
pub use strategy::StrategyLayer;

// --- Strategy pointer layer (guardrail echo, Dynamic, prio 1756) ---
mod strategy_pointer;
pub use strategy_pointer::StrategyPointerLayer;

// --- Context layers ---
mod chain_context;
mod environment;
mod heartbeat;
mod inbound_context;
mod operational_guidelines;
mod protocol_tokens;
mod provider_guidance;
mod runtime_context;
mod security;
mod session_budget;
mod doctor_repair_hint;
mod voice_mode;

// --- Identity files layer ---
mod identity_files;

// --- Extra files layer ([prompt.extra_files]) ---
mod extra_files;

// --- Curated memory layer ---
mod curated_memory;

// --- Memory augmentation layer ---
mod memory_augmentation;

// --- Memory protocol guidance layer ---
mod memory_protocol;

// --- Session context guide layer ---
mod session_context_guide;

// --- Session resume layer ---
mod session_resume;

// --- Re-exports ---
pub use citation_standards::CitationStandardsLayer;
pub use guidelines::GuidelinesLayer;
pub use role::RoleLayer;
pub use special_actions::SpecialActionsLayer;

pub use agent_catalog::AgentCatalogLayer;
pub use custom_instructions::CustomInstructionsLayer;
pub use generation_models::GenerationModelsLayer;
pub use language::LanguageLayer;
pub use mcp_instructions::McpInstructionsLayer;
pub use runtime_capabilities::RuntimeCapabilitiesLayer;
pub use skill_instructions::SkillInstructionsLayer;

pub use skill_mode::SkillModeLayer;
pub use thinking_guidance::ThinkingGuidanceLayer;

pub use agent_role::AgentRoleLayer;
pub use profile::ProfileLayer;
pub use soul::SoulLayer;

pub use tool_usage_grammar::ToolUsageGrammarLayer;
pub use tools::HydratedToolsLayer;
pub use tools::ToolsLayer;

pub use chain_context::ChainContextLayer;
pub use curated_memory::CuratedMemoryLayer;
pub use environment::EnvironmentLayer;
pub use extra_files::ExtraFilesLayer;
pub use heartbeat::HeartbeatLayer;
pub use identity_files::IdentityFilesLayer;
pub use inbound_context::InboundContextLayer;
pub use memory_augmentation::MemoryAugmentationLayer;
pub use memory_protocol::MemoryProtocolLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
pub use protocol_tokens::ProtocolTokensLayer;
pub use provider_guidance::ProviderGuidanceLayer;
pub use runtime_context::RuntimeContextLayer;
pub use security::SecurityLayer;
pub use session_budget::SessionBudgetLayer;
pub use session_context_guide::SessionContextGuideLayer;
pub use session_resume::SessionResumeLayer;
pub use doctor_repair_hint::DoctorRepairHintLayer;
pub use voice_mode::VoiceModeLayer;
