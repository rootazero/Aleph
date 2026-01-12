//! Dispatcher configuration types
//!
//! Contains Dispatcher Layer (Aether Cortex) configuration:
//! - DispatcherConfigToml: Multi-layer routing and confirmation settings
//! - AgentConfigToml: L3 Agent (multi-step planning) settings

use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// DispatcherConfigToml
// =============================================================================

/// Configuration for the Dispatcher Layer (Aether Cortex)
///
/// The Dispatcher Layer provides intelligent tool routing through three layers:
/// - L1: Regex-based pattern matching (highest confidence)
/// - L2: Semantic keyword matching (medium confidence)
/// - L3: AI-powered inference (variable confidence)
///
/// When a tool match has low confidence, the system can show a confirmation
/// dialog to the user before execution.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher]
/// enabled = true
/// l3_enabled = true
/// l3_timeout_ms = 5000
/// confirmation_threshold = 0.7
/// confirmation_timeout_ms = 30000
///
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfigToml {
    /// Whether the dispatcher is enabled (default: true)
    #[serde(default = "default_dispatcher_enabled")]
    pub enabled: bool,

    /// Whether L3 AI inference is enabled (default: true)
    #[serde(default = "default_dispatcher_l3_enabled")]
    pub l3_enabled: bool,

    /// L3 routing timeout in milliseconds (default: 5000)
    #[serde(default = "default_dispatcher_l3_timeout")]
    pub l3_timeout_ms: u64,

    /// Confidence threshold below which confirmation is required (0.0-1.0, default: 0.7)
    /// - Values >= 1.0 disable confirmation entirely
    /// - Values <= 0.0 always require confirmation
    #[serde(default = "default_dispatcher_confirmation_threshold")]
    pub confirmation_threshold: f32,

    /// Confirmation dialog timeout in milliseconds (default: 30000)
    #[serde(default = "default_dispatcher_confirmation_timeout")]
    pub confirmation_timeout_ms: u64,

    /// Whether confirmation dialogs are enabled (default: true)
    #[serde(default = "default_dispatcher_confirmation_enabled")]
    pub confirmation_enabled: bool,

    /// L3 Agent configuration for multi-step planning
    #[serde(default)]
    pub agent: AgentConfigToml,
}

pub fn default_dispatcher_enabled() -> bool {
    true
}

pub fn default_dispatcher_l3_enabled() -> bool {
    true
}

pub fn default_dispatcher_l3_timeout() -> u64 {
    5000 // 5 seconds
}

pub fn default_dispatcher_confirmation_threshold() -> f32 {
    0.7 // Require confirmation if confidence < 70%
}

pub fn default_dispatcher_confirmation_timeout() -> u64 {
    30000 // 30 seconds
}

pub fn default_dispatcher_confirmation_enabled() -> bool {
    true
}

impl Default for DispatcherConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_dispatcher_enabled(),
            l3_enabled: default_dispatcher_l3_enabled(),
            l3_timeout_ms: default_dispatcher_l3_timeout(),
            confirmation_threshold: default_dispatcher_confirmation_threshold(),
            confirmation_timeout_ms: default_dispatcher_confirmation_timeout(),
            confirmation_enabled: default_dispatcher_confirmation_enabled(),
            agent: AgentConfigToml::default(),
        }
    }
}

// =============================================================================
// AgentConfigToml - L3 Agent (Multi-step Planning) Configuration
// =============================================================================

/// Configuration for L3 Agent multi-step planning and execution
///
/// The L3 Agent enables intelligent multi-step task planning where the AI
/// decomposes complex requests into sequential tool invocations.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigToml {
    /// Whether agent mode is enabled (default: true)
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,

    /// Maximum number of steps allowed in a plan (default: 10)
    #[serde(default = "default_agent_max_steps")]
    pub max_steps: u32,

    /// Timeout for each step in milliseconds (default: 30000)
    #[serde(default = "default_agent_step_timeout")]
    pub step_timeout_ms: u64,

    /// Whether to attempt rollback on failure (default: true)
    #[serde(default = "default_agent_enable_rollback")]
    pub enable_rollback: bool,

    /// Whether to require user confirmation before executing plans (default: true)
    #[serde(default = "default_agent_plan_confirmation_required")]
    pub plan_confirmation_required: bool,

    /// Whether irreversible steps can run without additional confirmation (default: false)
    /// When false, plans with irreversible steps will show a warning.
    #[serde(default = "default_agent_allow_irreversible")]
    pub allow_irreversible_without_confirmation: bool,

    /// Heuristics threshold for triggering planning (default: 2)
    /// Number of action verbs/connectors needed to trigger multi-step planning
    #[serde(default = "default_agent_heuristics_threshold")]
    pub heuristics_threshold: u32,
}

pub fn default_agent_enabled() -> bool {
    true
}

pub fn default_agent_max_steps() -> u32 {
    10
}

pub fn default_agent_step_timeout() -> u64 {
    30000 // 30 seconds per step
}

pub fn default_agent_enable_rollback() -> bool {
    true
}

pub fn default_agent_plan_confirmation_required() -> bool {
    true
}

pub fn default_agent_allow_irreversible() -> bool {
    false
}

pub fn default_agent_heuristics_threshold() -> u32 {
    2 // At least 2 action signals to trigger planning
}

impl Default for AgentConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_agent_enabled(),
            max_steps: default_agent_max_steps(),
            step_timeout_ms: default_agent_step_timeout(),
            enable_rollback: default_agent_enable_rollback(),
            plan_confirmation_required: default_agent_plan_confirmation_required(),
            allow_irreversible_without_confirmation: default_agent_allow_irreversible(),
            heuristics_threshold: default_agent_heuristics_threshold(),
        }
    }
}

impl AgentConfigToml {
    /// Validate the agent configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_steps == 0 {
            return Err("agent.max_steps must be > 0".to_string());
        }
        if self.max_steps > 50 {
            warn!(
                max_steps = self.max_steps,
                "agent.max_steps > 50 may cause excessive processing"
            );
        }

        if self.step_timeout_ms == 0 {
            return Err("agent.step_timeout_ms must be > 0".to_string());
        }
        if self.step_timeout_ms > 120000 {
            warn!(
                timeout = self.step_timeout_ms,
                "agent.step_timeout_ms > 120000ms may cause poor user experience"
            );
        }

        Ok(())
    }
}

impl DispatcherConfigToml {
    /// Validate the configuration values
    ///
    /// # Returns
    /// * `Ok(())` - Configuration is valid
    /// * `Err(String)` - Validation error message
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Validate confirmation threshold range
        if self.confirmation_threshold < 0.0 {
            return Err(format!(
                "confirmation_threshold must be >= 0.0, got {}",
                self.confirmation_threshold
            ));
        }
        if self.confirmation_threshold > 1.0 {
            warn!(
                threshold = self.confirmation_threshold,
                "confirmation_threshold > 1.0 will disable confirmation entirely"
            );
        }

        // Validate L3 timeout
        if self.l3_timeout_ms == 0 {
            return Err("l3_timeout_ms must be > 0".to_string());
        }
        if self.l3_timeout_ms > 60000 {
            warn!(
                timeout = self.l3_timeout_ms,
                "l3_timeout_ms > 60000ms may cause poor user experience"
            );
        }

        // Validate confirmation timeout
        if self.confirmation_timeout_ms == 0 {
            return Err("confirmation_timeout_ms must be > 0".to_string());
        }

        // Validate agent configuration
        self.agent.validate()?;

        Ok(())
    }

    /// Convert to internal DispatcherConfig
    pub fn to_dispatcher_config(&self) -> crate::dispatcher::DispatcherConfig {
        use crate::dispatcher::{ConfirmationConfig, DispatcherConfig};

        DispatcherConfig {
            enabled: self.enabled,
            l3_enabled: self.l3_enabled,
            l3_timeout_ms: self.l3_timeout_ms,
            l3_confidence_threshold: self.confirmation_threshold,
            confirmation: ConfirmationConfig {
                enabled: self.confirmation_enabled,
                threshold: self.confirmation_threshold,
                timeout_ms: self.confirmation_timeout_ms,
                show_parameters: true,
                skip_native_tools: false,
            },
        }
    }
}
