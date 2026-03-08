//! A2A Discover Tool
//!
//! Discovers a remote A2A agent by fetching its Agent Card from a URL
//! and registers it in the local CardRegistry for future use.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::a2a::adapter::client::A2AClient;
use crate::a2a::domain::TrustLevel;
use crate::a2a::port::AgentResolver;
use crate::a2a::service::CardRegistry;
use crate::error::AlephError;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2ADiscoverArgs {
    /// URL of the remote A2A agent to discover (e.g. "http://localhost:9000")
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2ADiscoverResult {
    pub agent_id: String,
    pub name: String,
    pub description: Option<String>,
    pub skills: Vec<String>,
    pub trust_level: String,
}

#[derive(Clone)]
pub struct A2ADiscoverTool {
    registry: Arc<CardRegistry>,
}

impl A2ADiscoverTool {
    pub fn new(registry: Arc<CardRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl AlephTool for A2ADiscoverTool {
    const NAME: &'static str = "a2a_discover";
    const DESCRIPTION: &'static str =
        "Discover a remote A2A agent by URL, fetch its Agent Card, and register it for future use.";

    type Args = A2ADiscoverArgs;
    type Output = A2ADiscoverResult;

    async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
        // 1. Create temp client to fetch card
        let client = A2AClient::new(&args.url);
        let card = client
            .fetch_agent_card()
            .await
            .map_err(|e| AlephError::tool(format!("Failed to fetch agent card: {}", e)))?;

        // 2. Register with inferred trust level
        let trust_level = TrustLevel::infer_from_url(&args.url);
        self.registry
            .register(card.clone(), &args.url, trust_level)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to register agent: {}", e)))?;

        Ok(A2ADiscoverResult {
            agent_id: card.id.clone(),
            name: card.name,
            description: card.description,
            skills: card.skills.iter().map(|s| s.name.clone()).collect(),
            trust_level: format!("{:?}", trust_level),
        })
    }
}
