//! Dispatcher FFI Methods for AetherCore
//!
//! This module contains FFI methods for the Dispatcher layer:
//! - Task orchestration: agent_plan, agent_execute, etc.
//! - Model routing: get_model_profiles, update_routing_rule, etc.
//! - Budget management: get_budget_status, etc.
//! - A/B testing and ensemble: get_ab_testing_status, get_ensemble_status, etc.
//!
//! # Module Organization
//!
//! - `core`: Agent engine lifecycle and basic task orchestration
//! - `state`: State management (pause, resume, cancel, subscribe)
//! - `config`: Code execution and file operations configuration
//! - `model_routing`: Model profiles, routing rules, and health monitoring
//! - `budget`: Budget status and limit management
//! - `ab_testing`: Experiment management
//! - `ensemble`: Ensemble status and configuration
//! - `confirmation`: DAG plan confirmation

mod ab_testing;
mod budget;
mod config;
mod confirmation;
mod core;
mod ensemble;
mod model_routing;
mod state;
