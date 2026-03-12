//! Swarm Context Provider
//!
//! Implements ContextProvider trait for swarm intelligence integration.
//! Retrieves Tier 2 (Important) events from ContextInjector and formats
//! them as XML team communication protocol.

use crate::sync_primitives::Arc;
/// Trait for providing additional context to the agent loop
pub trait ContextProvider: Send + Sync {
    /// Get context to inject into the agent loop
    fn get_context(&self) -> Option<String>;
    /// Priority of this provider (higher = injected first)
    fn priority(&self) -> i32;
    /// Name of this provider for logging
    fn name(&self) -> &str;
}
use super::context_injector::ContextInjector;

/// Swarm context provider for team awareness injection
pub struct SwarmContextProvider {
    context_injector: Arc<ContextInjector>,
}

impl SwarmContextProvider {
    /// Create a new swarm context provider
    pub fn new(context_injector: Arc<ContextInjector>) -> Self {
        Self { context_injector }
    }

    /// Format swarm state as XML team communication protocol
    ///
    /// This format distinguishes team broadcast from agent's own memory.
    fn format_as_xml(&self, swarm_state: &str) -> String {
        if swarm_state.is_empty() {
            return String::new();
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let mut xml = format!(
            "<team_awareness timestamp=\"{}\">\n",
            timestamp
        );

        // Parse the swarm state (which is already formatted by ContextInjector)
        // and wrap it in XML structure
        xml.push_str("  <summary>Team activity in the last iteration</summary>\n");
        xml.push_str("  <updates>\n");

        // Add the swarm state content
        for line in swarm_state.lines() {
            if !line.trim().is_empty() && !line.starts_with("##") {
                xml.push_str("    ");
                xml.push_str(line.trim());
                xml.push('\n');
            }
        }

        xml.push_str("  </updates>\n");
        xml.push_str("</team_awareness>");
        xml
    }
}

impl ContextProvider for SwarmContextProvider {
    fn get_context(&self) -> Option<String> {
        // Call async method from sync context using block_in_place
        // This is safe and designed for calling async code from sync contexts
        // within a tokio runtime
        let injector = self.context_injector.clone();

        let swarm_state = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                injector.inject_swarm_state("").await
            })
        });

        if swarm_state.is_empty() {
            return None;
        }

        // Format as XML team communication protocol
        Some(self.format_as_xml(&swarm_state))
    }

    fn priority(&self) -> i32 {
        100  // High priority, but lower than Critical interruption (1000+)
    }

    fn name(&self) -> &str {
        "swarm_context"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_swarm_context_provider_empty() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus));
        let provider = SwarmContextProvider::new(injector);

        // No events yet, should return None
        assert_eq!(provider.get_context(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_swarm_context_provider_with_events() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus.clone()));

        // Start the injector to process events
        let injector_clone = injector.clone();
        let _handle = tokio::spawn(async move {
            injector_clone.run().await;
        });

        // Give the injector time to subscribe
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Publish an Important event through the bus
        let event = AgentEvent::Important(ImportantEvent::Hotspot {
            area: "src/auth/".to_string(),
            agent_count: 3,
            activity: "Refactoring".to_string(),
            timestamp: chrono::Utc::now()
                .timestamp()
                .try_into()
                .unwrap_or(0),
        });

        bus.publish(event).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let provider = SwarmContextProvider::new(injector);

        // Should return formatted context
        let context = provider.get_context();
        assert!(context.is_some());
        let ctx = context.unwrap();
        assert!(ctx.contains("<team_awareness"));
        assert!(ctx.contains("</team_awareness>"));
    }

    #[test]
    fn test_provider_priority() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus));
        let provider = SwarmContextProvider::new(injector);

        assert_eq!(provider.priority(), 100);
    }

    #[test]
    fn test_provider_name() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus));
        let provider = SwarmContextProvider::new(injector);

        assert_eq!(provider.name(), "swarm_context");
    }

    #[test]
    fn test_format_as_xml() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus));
        let provider = SwarmContextProvider::new(injector);

        let swarm_state = "\n## Swarm State (Team Awareness)\n[1m ago] Hotspot detected: 3 agents working on src/auth/ (Refactoring)\n\n";
        let xml = provider.format_as_xml(swarm_state);

        assert!(xml.contains("<team_awareness"));
        assert!(xml.contains("timestamp="));
        assert!(xml.contains("<summary>"));
        assert!(xml.contains("<updates>"));
        assert!(xml.contains("Hotspot detected"));
        assert!(xml.contains("</team_awareness>"));
    }

    #[test]
    fn test_format_empty_state() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = Arc::new(ContextInjector::new(bus));
        let provider = SwarmContextProvider::new(injector);

        let xml = provider.format_as_xml("");
        assert_eq!(xml, "");
    }
}


