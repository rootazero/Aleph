// Aether/core/src/permission/mod.rs
//! Unified Permission System
//!
//! This module implements a rule-based permission system inspired by OpenCode,
//! replacing the previous confidence-based confirmation flow.
//!
//! # Architecture
//!
//! ```text
//! Tool Execution Request
//!       ↓
//! ┌───────────────────────────────────────┐
//! │      PermissionManager                │
//! │                                       │
//! │  1. Get permission type from tool     │
//! │  2. Evaluate against rulesets         │
//! │  3. Action: allow → proceed           │
//! │            deny  → PermissionError    │
//! │            ask   → emit event, wait   │
//! └───────────────────────────────────────┘
//!       ↓
//! EventBus (PermissionAsked) → UI → reply()
//! ```
//!
//! # Configuration
//!
//! ```json
//! {
//!   "permission": {
//!     "edit": "allow",
//!     "bash": {
//!       "git *": "allow",
//!       "rm -rf *": "deny",
//!       "*": "ask"
//!     }
//!   }
//! }
//! ```

mod config;
mod error;
mod manager;
mod rule;

pub use config::{PermissionConfig, PermissionConfigMap};
pub use error::PermissionError;
pub use manager::{PendingPermission, PermissionManager, PermissionManagerConfig};
pub use rule::{PermissionEvaluator, PermissionMapping, PermissionRule, Ruleset};

// Re-export from event module for convenience
pub use crate::event::permission::{PermissionAction, PermissionReply, PermissionRequest};
