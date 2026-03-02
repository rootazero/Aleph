//! Prompt layers — each file implements one PromptLayer.

// --- Always-on layers ---
mod role;
mod guidelines;
mod special_actions;
mod citation_standards;

// --- Config-gated layers ---
mod generation_models;
mod runtime_capabilities;
mod custom_instructions;
mod language;
mod skill_instructions;

// --- Behavior layers ---
mod skill_mode;
mod thinking_guidance;
mod response_format;

// --- Identity layer ---
mod soul;

// --- Profile layer ---
pub mod profile;

// --- Tool layers ---
mod tools;

// --- Context layers ---
mod runtime_context;
mod environment;
mod security;
mod protocol_tokens;
mod operational_guidelines;

// --- Re-exports ---
pub use role::RoleLayer;
pub use guidelines::GuidelinesLayer;
pub use special_actions::SpecialActionsLayer;
pub use citation_standards::CitationStandardsLayer;

pub use generation_models::GenerationModelsLayer;
pub use runtime_capabilities::RuntimeCapabilitiesLayer;
pub use custom_instructions::CustomInstructionsLayer;
pub use language::LanguageLayer;
pub use skill_instructions::SkillInstructionsLayer;

pub use skill_mode::SkillModeLayer;
pub use thinking_guidance::ThinkingGuidanceLayer;
pub use response_format::ResponseFormatLayer;

pub use soul::SoulLayer;
pub use profile::ProfileLayer;

pub use tools::ToolsLayer;
pub use tools::HydratedToolsLayer;

pub use runtime_context::RuntimeContextLayer;
pub use environment::EnvironmentLayer;
pub use security::SecurityLayer;
pub use protocol_tokens::ProtocolTokensLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
