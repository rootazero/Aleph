//! DecisionEngine - Unified configuration management
//!
//! This module provides configuration infrastructure for the Cortex dispatcher,
//! including routing thresholds, confirmation policies, and execution parameters.
//!
//! # Architecture
//!
//! The decision module uses a two-tier configuration system:
//!
//! 1. **Global Configuration** (`DecisionConfig`) - Persisted settings loaded
//!    from a TOML file, defining default behavior for all sessions.
//!
//! 2. **Session Overrides** (`SessionOverride`) - Temporary per-session
//!    adjustments that override specific global settings.
//!
//! # Decision Flow
//!
//! ```text
//! Confidence Score → DecisionConfig::decide() → DecisionAction
//!         │                                           │
//!         │   ┌───────────────────────────────────────┘
//!         │   │
//!         v   v
//!     NoMatch (< 0.3)
//!     RequiresConfirmation (0.3 - 0.5)
//!     OptionalConfirmation (0.5 - 0.9)
//!     AutoExecute (>= 0.9)
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::dispatcher::cortex::decision::{
//!     DecisionConfig, SessionOverride, ConfirmationOverride, merge_config
//! };
//!
//! // Load global config
//! let global = DecisionConfig::default();
//!
//! // Apply session-specific overrides
//! let session = SessionOverride {
//!     confirmation: Some(ConfirmationOverride {
//!         enabled: Some(false), // Disable confirmations for this session
//!         ..Default::default()
//!     }),
//!     ..Default::default()
//! };
//!
//! let config = merge_config(&global, &session);
//!
//! // Make a decision based on confidence
//! let action = config.decide(0.75);
//! ```

pub mod config;
pub mod session_override;

pub use config::{
    ConfirmationConfig, DecisionAction, DecisionConfig, ExecutionConfig, RoutingConfig,
};
pub use session_override::{merge_config, ConfirmationOverride, RoutingOverride, SessionOverride};
