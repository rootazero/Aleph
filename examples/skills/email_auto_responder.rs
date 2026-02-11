//! Email Auto-Responder Skill
//!
//! This skill demonstrates System State Bus usage by monitoring Mail.app
//! and automatically responding to urgent emails.
//!
//! # Features
//!
//! - Subscribes to Mail.app state changes
//! - Detects unread email count increases
//! - Identifies urgent emails (subject contains "URGENT")
//! - Automatically clicks "Reply" button
//! - Types pre-configured response
//! - Sends the email
//!
//! # Usage
//!
//! ```rust
//! use alephcore::gateway::Gateway;
//! use email_auto_responder::EmailAutoResponder;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let gateway = Gateway::new().await?;
//!     let responder = EmailAutoResponder::new(gateway);
//!     responder.run().await?;
//!     Ok(())
//! }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Email auto-responder configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponderConfig {
    /// Keywords that trigger auto-response
    pub urgent_keywords: Vec<String>,

    /// Auto-response template
    pub response_template: String,

    /// Maximum responses per hour
    pub rate_limit: u32,

    /// Enable/disable the responder
    pub enabled: bool,
}

impl Default for ResponderConfig {
    fn default() -> Self {
        Self {
            urgent_keywords: vec![
                "URGENT".to_string(),
                "ASAP".to_string(),
                "CRITICAL".to_string(),
            ],
            response_template: "Thank you for your email. I'm currently away but will respond as soon as possible.".to_string(),
            rate_limit: 10,
            enabled: true,
        }
    }
}

/// Email auto-responder skill
pub struct EmailAutoResponder {
    config: Arc<RwLock<ResponderConfig>>,
    response_count: Arc<RwLock<u32>>,
    subscription_id: Arc<RwLock<Option<String>>>,
}

impl EmailAutoResponder {
    /// Create a new email auto-responder
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(ResponderConfig::default())),
            response_count: Arc::new(RwLock::new(0)),
            subscription_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: ResponderConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            response_count: Arc::new(RwLock::new(0)),
            subscription_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Start the auto-responder
    ///
    /// This method subscribes to Mail.app state changes and processes
    /// incoming emails in real-time.
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Subscribe to Mail.app state
        let subscription_result = self.subscribe_to_mail().await?;

        // Store subscription ID
        *self.subscription_id.write().await = Some(subscription_result["subscription_id"].as_str().unwrap().to_string());

        // Start event processing loop
        self.process_events().await?;

        Ok(())
    }

    /// Stop the auto-responder
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(sub_id) = self.subscription_id.read().await.as_ref() {
            self.unsubscribe(sub_id).await?;
            *self.subscription_id.write().await = None;
        }

        Ok(())
    }

    /// Subscribe to Mail.app state changes
    async fn subscribe_to_mail(&self) -> Result<Value, Box<dyn std::error::Error>> {
        // In a real implementation, this would call gateway.rpc_call
        // For this example, we'll return a mock response
        Ok(json!({
            "subscription_id": "sub_mail_12345",
            "active_patterns": ["system.state.com.apple.mail.*"],
            "initial_snapshot": {
                "app_id": "com.apple.mail",
                "elements": [],
                "app_context": {
                    "unread_count": 0
                },
                "source": "accessibility",
                "confidence": 1.0
            }
        }))
    }

    /// Unsubscribe from state changes
    async fn unsubscribe(&self, subscription_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        // In a real implementation, this would call gateway.rpc_call
        println!("Unsubscribed: {}", subscription_id);
        Ok(())
    }

    /// Process incoming state change events
    async fn process_events(&self) -> Result<(), Box<dyn std::error::Error>> {
        // In a real implementation, this would listen to gateway.event_bus
        // For this example, we'll simulate event processing

        println!("Email Auto-Responder started. Monitoring Mail.app...");

        // Simulate receiving events
        loop {
            // In real implementation:
            // let event = gateway.event_bus.subscribe().recv().await?;
            // self.handle_event(event).await?;

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    /// Handle a single state change event
    async fn handle_event(&self, event: Value) -> Result<(), Box<dyn std::error::Error>> {
        let config = self.config.read().await;

        if !config.enabled {
            return Ok(());
        }

        // Check rate limit
        let mut count = self.response_count.write().await;
        if *count >= config.rate_limit {
            println!("Rate limit reached. Skipping auto-response.");
            return Ok(());
        }

        // Parse JSON Patch
        if let Some(patches) = event.get("patches").and_then(|p| p.as_array()) {
            for patch in patches {
                // Check if unread count increased
                if patch["path"] == "/app_context/unread_count" && patch["op"] == "replace" {
                    let new_count = patch["value"].as_u64().unwrap_or(0);

                    if new_count > 0 {
                        // Check if email is urgent
                        if self.is_urgent_email(&event).await? {
                            // Send auto-response
                            self.send_auto_response().await?;
                            *count += 1;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if the email contains urgent keywords
    async fn is_urgent_email(&self, _event: &Value) -> Result<bool, Box<dyn std::error::Error>> {
        // In a real implementation, this would check the email subject/body
        // For this example, we'll return a mock result
        Ok(true)
    }

    /// Send auto-response
    async fn send_auto_response(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config = self.config.read().await;

        println!("Sending auto-response...");

        // Step 1: Click "Reply" button
        self.execute_action(json!({
            "target_id": "btn_reply",
            "method": "click",
            "expect": {
                "condition": "element_appear",
                "target": "compose_window",
                "timeout_ms": 1000
            }
        })).await?;

        // Step 2: Type response
        self.execute_action(json!({
            "target_id": "compose_body",
            "method": "type",
            "params": {
                "text": config.response_template.clone()
            }
        })).await?;

        // Step 3: Click "Send" button
        self.execute_action(json!({
            "target_id": "btn_send",
            "method": "click",
            "expect": {
                "condition": "element_disappear",
                "target": "compose_window",
                "timeout_ms": 2000
            }
        })).await?;

        println!("Auto-response sent successfully!");

        Ok(())
    }

    /// Execute a UI action
    async fn execute_action(&self, action: Value) -> Result<(), Box<dyn std::error::Error>> {
        // In a real implementation, this would call gateway.rpc_call("system.action.execute", action)
        println!("Executing action: {:?}", action);
        Ok(())
    }

    /// Reset response count (call this hourly)
    pub async fn reset_rate_limit(&self) {
        *self.response_count.write().await = 0;
        println!("Rate limit reset");
    }

    /// Update configuration
    pub async fn update_config(&self, config: ResponderConfig) {
        *self.config.write().await = config;
        println!("Configuration updated");
    }

    /// Get current statistics
    pub async fn get_stats(&self) -> ResponderStats {
        ResponderStats {
            responses_sent: *self.response_count.read().await,
            rate_limit: self.config.read().await.rate_limit,
            enabled: self.config.read().await.enabled,
        }
    }
}

impl Default for EmailAutoResponder {
    fn default() -> Self {
        Self::new()
    }
}

/// Responder statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponderStats {
    pub responses_sent: u32,
    pub rate_limit: u32,
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_responder_creation() {
        let responder = EmailAutoResponder::new();
        let stats = responder.get_stats().await;

        assert_eq!(stats.responses_sent, 0);
        assert!(stats.enabled);
    }

    #[tokio::test]
    async fn test_custom_config() {
        let config = ResponderConfig {
            urgent_keywords: vec!["TEST".to_string()],
            response_template: "Test response".to_string(),
            rate_limit: 5,
            enabled: false,
        };

        let responder = EmailAutoResponder::with_config(config.clone());
        let stats = responder.get_stats().await;

        assert_eq!(stats.rate_limit, 5);
        assert!(!stats.enabled);
    }

    #[tokio::test]
    async fn test_rate_limit_reset() {
        let responder = EmailAutoResponder::new();

        // Simulate sending responses
        *responder.response_count.write().await = 5;

        // Reset
        responder.reset_rate_limit().await;

        let stats = responder.get_stats().await;
        assert_eq!(stats.responses_sent, 0);
    }

    #[tokio::test]
    async fn test_config_update() {
        let responder = EmailAutoResponder::new();

        let new_config = ResponderConfig {
            urgent_keywords: vec!["NEW".to_string()],
            response_template: "New template".to_string(),
            rate_limit: 20,
            enabled: false,
        };

        responder.update_config(new_config).await;

        let config = responder.config.read().await;
        assert_eq!(config.rate_limit, 20);
        assert!(!config.enabled);
    }
}
