//! Configuration type definitions for Aether
//!
//! This module contains all struct definitions used in the configuration system.
//! Types are organized by domain:
//!
//! - `general`: Core settings (GeneralConfig, ShortcutsConfig, BehaviorConfig, TriggerConfig)
//! - `provider`: AI provider settings (ProviderConfig, ProviderConfigEntry, TestConnectionResult)
//! - `routing`: Routing rules (RoutingRuleConfig)
//! - `memory`: Memory/RAG settings (MemoryConfig)
//! - `search`: Search capability settings (SearchConfigInternal, SearchConfig, PIIConfig)
//! - `smart_flow`: Intent detection and matching (SmartFlowConfig, SmartMatchingConfig)
//! - `tools`: Native and MCP tools (ToolsConfig, UnifiedToolsConfig)
//! - `video`: Video transcript settings (VideoConfig)
//! - `skills`: Claude Agent Skills settings (SkillsConfig)
//! - `dispatcher`: Dispatcher Layer settings (DispatcherConfigToml)
//! - `agent`: Agent task orchestration settings (AgentConfigToml)
//! - `orchestrator`: Three-Layer Control orchestrator settings (OrchestratorConfig, OrchestratorGuards)
//! - `typo_correction`: Quick typo correction settings (TypoCorrectionConfig)

pub mod agent;
pub mod dispatcher;
pub mod general;
pub mod generation;
pub mod memory;
pub mod orchestrator;
pub mod policies;
pub mod provider;
pub mod routing;
pub mod search;
pub mod skills;
pub mod smart_flow;
pub mod tools;
pub mod typo_correction;
pub mod video;

// Re-export all types for backward compatibility
// Users can still use `use crate::config::XXX` instead of `use crate::config::types::XXX`
pub use agent::*;
pub use dispatcher::*;
pub use general::*;
pub use generation::*;
pub use memory::*;
pub use orchestrator::*;
pub use policies::*;
pub use provider::*;
pub use routing::*;
pub use search::*;
pub use skills::*;
pub use smart_flow::*;
pub use tools::*;
pub use typo_correction::*;
pub use video::*;
