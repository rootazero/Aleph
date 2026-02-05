//! Recovery Module
//!
//! Provides task recovery capabilities for the Multi-Agent Resilience architecture:
//! - Shadow Replay: Deterministic task recovery without LLM calls
//! - Graceful Shutdown: Checkpoint running tasks on SIGTERM
//! - Recovery Manager: Startup recovery with risk-aware decisions
//! - Risk Adapter: Bridge between dispatcher and persistence risk levels

mod graceful_shutdown;
mod manager;
mod risk_adapter;
mod shadow_replay;

pub use graceful_shutdown::{GracefulShutdown, ShutdownSignal};
pub use manager::{RecoveryDecision, RecoveryManager, RecoverySummary};
pub use risk_adapter::TaskRiskAdapter;
pub use shadow_replay::{DivergenceStatus, ReplayResult, ShadowReplayEngine};
