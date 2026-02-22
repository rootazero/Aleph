//! Governance Module
//!
//! Provides resource governance capabilities for the Multi-Agent Resilience architecture.
//!
//! # Components
//!
//! - `governor`: ResourceGovernor with Lane-based priority isolation
//! - `sentry`: Recursive Sentry for depth limiting
//! - `quota`: QuotaManager for concurrency and resource limits

mod governor;
mod quota;
mod sentry;

pub use governor::{GovernorConfig, GovernorStats, ResourceGovernor, ResourcePermit};
pub use quota::{
    QuotaCheckResult, QuotaConfig, QuotaManager, QuotaUsage, QuotaViolation, RemainingCapacity,
};
pub use sentry::{RecursionLimitExceeded, RecursiveSentry};
