//! Ripple module for local knowledge graph exploration
//!
//! RippleTask enables local exploration around retrieved facts to expand knowledge context.
//! When a fact is retrieved, it explores related facts within N hops in the knowledge graph,
//! enriching the context with connected information.

mod config;
mod task;

#[cfg(test)]
mod tests;

pub use config::{RippleConfig, RippleResult};
pub use task::RippleTask;
