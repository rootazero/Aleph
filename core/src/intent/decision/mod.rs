//! Decision layer for intent routing.
//!
//! This module provides unified execution decision making, signal aggregation,
//! confidence calibration, and routing for the Agent Loop.

pub mod aggregator;
pub mod calibrator;
pub mod execution_decider;
pub mod router;

pub use aggregator::{
    AggregatedIntent, AggregatorConfig, IntentAction, IntentAggregator, MissingParameter,
};
pub use calibrator::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
pub use execution_decider::{
    ContextSignals, CustomInvocation, DeciderConfig, DecisionMetadata,
    DecisionResult, ExecutionIntentDecider, ExecutionMode, IntentLayer, McpInvocation,
    SkillInvocation, SlashCommand, ToolInvocation,
};
// Backward compatibility: DecisionLayer is deprecated, use IntentLayer instead
#[allow(deprecated)]
pub use execution_decider::DecisionLayer;
pub use router::{DirectMode, DirectRouteInfo, IntentRouter, RouteResult, ThinkingContext};
