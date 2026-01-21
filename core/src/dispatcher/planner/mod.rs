//! Task Planner module
//!
//! This module provides LLM-driven task decomposition, converting natural
//! language requests into structured TaskGraphs.

mod llm;
mod prompt;

pub use llm::LlmTaskPlanner;
pub use prompt::GenerationProviders;

use async_trait::async_trait;

use crate::dispatcher::agent_types::TaskGraph;
use crate::error::Result;

/// Trait for task planners
///
/// A task planner converts natural language requests into structured
/// TaskGraphs that can be executed by the scheduler.
#[async_trait]
pub trait TaskPlanner: Send + Sync {
    /// Plan a task from a natural language request
    ///
    /// # Arguments
    ///
    /// * `request` - The user's natural language request
    ///
    /// # Returns
    ///
    /// * `Ok(TaskGraph)` - A structured task graph
    /// * `Err` - If planning fails
    async fn plan(&self, request: &str) -> Result<TaskGraph>;

    /// Plan a task with available generation providers
    ///
    /// # Arguments
    ///
    /// * `request` - The user's natural language request
    /// * `providers` - Available generation providers (image, video, audio)
    ///
    /// # Returns
    ///
    /// * `Ok(TaskGraph)` - A structured task graph
    /// * `Err` - If planning fails
    async fn plan_with_providers(
        &self,
        request: &str,
        providers: &GenerationProviders,
    ) -> Result<TaskGraph> {
        // Default implementation ignores providers
        let _ = providers;
        self.plan(request).await
    }

    /// Get the name of this planner
    fn name(&self) -> &str;
}
