//! Dispatcher Module
//!
//! Phase 4: Dispatcher - Proactive Action Decision System

pub mod config;
pub mod mode;
pub mod policy;
pub mod policies;

pub use config::DispatcherConfig;
pub use mode::DispatcherMode;
pub use policy::{
    ActionType, NotificationPriority, Policy, PolicyEngine, ProposedAction, RiskLevel,
};
