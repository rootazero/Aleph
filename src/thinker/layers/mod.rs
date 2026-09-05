//! Prompt layers — each file implements one `PromptLayer`.

// --- Always-on layers ---
mod citation_standards;
mod guidelines;
mod role;
mod special_actions;

// --- Config-gated layers ---
mod agent_catalog;
mod language;
mod mcp_instructions;
mod runtime_capabilities;
mod skill_instructions;

// --- Identity layer ---
mod soul;

// --- Agent role layer ---
mod agent_role;

// --- Project-room layer ---
pub(crate) mod room_roster;

// --- Profile layer ---
pub mod profile;

// --- Tool layers ---
mod tool_runtime_state;
pub use tool_runtime_state::ToolRuntimeStateLayer;

// --- Execution-plan layer (active scratchpad checklist, re-surfaced per turn) ---
mod execution_plan;
pub use execution_plan::ExecutionPlanLayer;

// --- Standing-goal layer (active goal objective, re-surfaced per turn) ---
mod graph_topology;
mod standing_goal;
pub use graph_topology::GraphTopologyLayer;
pub use standing_goal::StandingGoalLayer;

// --- Timer-loop layer (active watch loop status, re-surfaced per turn) ---
mod timer_loop;
pub use timer_loop::TimerLoopLayer;

// --- Strategy layer (welded <strategy> envelope, Stable prefix, prio 70) ---
mod strategy;
pub use strategy::StrategyLayer;

// --- Strategy pointer layer (guardrail echo, Dynamic, prio 1757) ---
mod strategy_pointer;
pub use strategy_pointer::StrategyPointerLayer;

// --- Operating envelope (approval tier + usage mode, Dynamic, prio 1758) ---
mod operating_envelope;
pub use operating_envelope::OperatingEnvelopeLayer;

// --- Context layers ---
mod chain_context;
mod doctor_repair_hint;
mod environment;
mod multi_step_conduct;
mod operational_guidelines;
mod protocol_tokens;
mod provider_guidance;
mod runtime_context;
mod security;
mod session_budget;
mod voice_mode;

// --- Identity files layer ---
mod identity_files;

// --- Extra files layer ([prompt.extra_files]) ---
mod extra_files;

// --- Curated memory layer ---
mod curated_memory;

// --- Memory augmentation layer ---

// --- Memory protocol guidance layer (constant destination ladder, Stable, prio 1105) ---
mod memory_protocol;

// --- Memory window claim (which memory blocks are already in front of the
// model this turn; the per-turn half split out of `memory_protocol`, Dynamic,
// prio 1745) ---
mod memory_window;

// --- Session context guide layer ---
mod session_context_guide;

// --- Re-exports ---
pub use citation_standards::CitationStandardsLayer;
pub use guidelines::GuidelinesLayer;
pub use role::RoleLayer;
pub use room_roster::RoomRosterLayer;
// Both `ResolvedContext` construction sites pre-render the member line through
// this ONE resolver, so the scope predicate, the room-of-one rule, the cap, the
// owner mark and the display-name sanitiser keep a single owner — same shape as
// `sanitize_identity_content`. `render_members` is deliberately NOT re-exported
// beside it: a caller that could reach the renderer without the resolver is a
// caller that can re-derive "is this run in a room", which is the second answer
// this seam exists to prevent.
pub(crate) use room_roster::ambient_line as ambient_room_roster_line;
pub use special_actions::SpecialActionsLayer;

pub use agent_catalog::AgentCatalogLayer;
pub use language::LanguageLayer;
pub use mcp_instructions::McpInstructionsLayer;
pub use runtime_capabilities::RuntimeCapabilitiesLayer;
pub use skill_instructions::SkillInstructionsLayer;

pub use agent_role::AgentRoleLayer;
pub use profile::ProfileLayer;
pub use soul::SoulLayer;

pub use chain_context::ChainContextLayer;
pub use curated_memory::CuratedMemoryLayer;
pub use doctor_repair_hint::DoctorRepairHintLayer;
pub use environment::EnvironmentLayer;
pub use extra_files::ExtraFilesLayer;
/// The injection-pattern + invisible-Unicode scanner every prompt surface that
/// carries **files from a folder the user opened** must pass its text through.
/// Re-exported so the gateway's project-skill advertiser uses the same one as
/// `ExtraFilesLayer`, instead of a weaker local sanitizer.
pub(crate) use identity_files::sanitize_identity_content;
pub use identity_files::IdentityFilesLayer;
pub use memory_protocol::MemoryProtocolLayer;
pub use memory_window::MemoryWindowLayer;
pub use multi_step_conduct::MultiStepConductLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
pub use protocol_tokens::ProtocolTokensLayer;
pub use provider_guidance::ProviderGuidanceLayer;
pub use runtime_context::RuntimeContextLayer;
pub use security::SecurityLayer;
pub use session_budget::SessionBudgetLayer;
pub use session_context_guide::SessionContextGuideLayer;
pub use voice_mode::VoiceModeLayer;
