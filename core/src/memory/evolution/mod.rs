//! Evolution module for fact contradiction detection and resolution
//!
//! When new facts contradict existing facts, this module creates an evolution chain
//! that records the progression of knowledge, allowing the system to understand how
//! beliefs changed over time.

mod chain;
mod detector;
mod resolver;

#[cfg(test)]
mod tests;

pub use chain::{EvolutionChain, EvolutionNode, FactEvolution};
pub use detector::ContradictionDetector;
pub use resolver::{EvolutionResolver, ResolutionStrategy};
