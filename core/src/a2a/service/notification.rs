use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::a2a::domain::{TaskArtifactUpdateEvent, TaskStatusUpdateEvent};
use crate::a2a::port::A2AResult;

/// Configuration for push notifications on a task
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationConfig {
    pub task_id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub events: Vec<String>, // "status-update", "artifact-update"
}

/// Push notification service — manages webhook configs and fires notifications
pub struct NotificationService {
    configs: RwLock<HashMap<String, PushNotificationConfig>>,
    http_client: reqwest::Client,
}

impl NotificationService {
    pub fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Store or overwrite a push notification config for a task
    pub async fn set_config(
        &self,
        config: PushNotificationConfig,
    ) -> A2AResult<PushNotificationConfig> {
        let mut configs = self.configs.write().await;
        configs.insert(config.task_id.clone(), config.clone());
        Ok(config)
    }

    /// Retrieve the push notification config for a task, if any
    pub async fn get_config(&self, task_id: &str) -> A2AResult<Option<PushNotificationConfig>> {
        let configs = self.configs.read().await;
        Ok(configs.get(task_id).cloned())
    }

    /// Remove the push notification config for a task
    pub async fn delete_config(&self, task_id: &str) -> A2AResult<()> {
        let mut configs = self.configs.write().await;
        configs.remove(task_id);
        Ok(())
    }

    /// List all registered push notification configs
    pub async fn list_configs(&self) -> A2AResult<Vec<PushNotificationConfig>> {
        let configs = self.configs.read().await;
        Ok(configs.values().cloned().collect())
    }

    /// Send push notification for a status update
    pub async fn notify_status_update(&self, task_id: &str, event: &TaskStatusUpdateEvent) {
        let config = {
            let configs = self.configs.read().await;
            configs.get(task_id).cloned()
        };

        if let Some(config) = config {
            if config.events.is_empty()
                || config.events.contains(&"status-update".to_string())
            {
                self.send_webhook(
                    &config,
                    &serde_json::json!({
                        "type": "status-update",
                        "data": event,
                    }),
                )
                .await;
            }
        }
    }

    /// Send push notification for an artifact update
    pub async fn notify_artifact_update(&self, task_id: &str, event: &TaskArtifactUpdateEvent) {
        let config = {
            let configs = self.configs.read().await;
            configs.get(task_id).cloned()
        };

        if let Some(config) = config {
            if config.events.is_empty()
                || config.events.contains(&"artifact-update".to_string())
            {
                self.send_webhook(
                    &config,
                    &serde_json::json!({
                        "type": "artifact-update",
                        "data": event,
                    }),
                )
                .await;
            }
        }
    }

    /// Send webhook POST request (fire-and-forget, log errors)
    async fn send_webhook(&self, config: &PushNotificationConfig, payload: &serde_json::Value) {
        let mut builder = self
            .http_client
            .post(&config.url)
            .json(payload)
            .timeout(std::time::Duration::from_secs(10));

        if let Some(ref token) = config.token {
            builder = builder.bearer_auth(token);
        }

        match builder.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(
                    task_id = %config.task_id,
                    url = %config.url,
                    "Push notification sent"
                );
            }
            Ok(resp) => {
                tracing::warn!(
                    task_id = %config.task_id,
                    url = %config.url,
                    status = %resp.status(),
                    "Push notification failed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %config.task_id,
                    url = %config.url,
                    error = %e,
                    "Push notification error"
                );
            }
        }
    }
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(task_id: &str, events: Vec<&str>) -> PushNotificationConfig {
        PushNotificationConfig {
            task_id: task_id.to_string(),
            url: "https://example.com/webhook".to_string(),
            token: Some("test-token".to_string()),
            events: events.into_iter().map(String::from).collect(),
        }
    }

    #[tokio::test]
    async fn set_and_get_config() {
        let svc = NotificationService::new();
        let config = make_config("task-1", vec!["status-update"]);

        let result = svc.set_config(config.clone()).await.unwrap();
        assert_eq!(result.task_id, "task-1");
        assert_eq!(result.url, "https://example.com/webhook");

        let fetched = svc.get_config("task-1").await.unwrap().unwrap();
        assert_eq!(fetched.task_id, "task-1");
        assert_eq!(fetched.events, vec!["status-update"]);
        assert_eq!(fetched.token, Some("test-token".to_string()));
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let svc = NotificationService::new();
        let result = svc.get_config("no-such-task").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_config_removes_entry() {
        let svc = NotificationService::new();
        svc.set_config(make_config("task-1", vec![])).await.unwrap();

        svc.delete_config("task-1").await.unwrap();
        assert!(svc.get_config("task-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let svc = NotificationService::new();
        // Should not error
        svc.delete_config("ghost").await.unwrap();
    }

    #[tokio::test]
    async fn list_configs_returns_all() {
        let svc = NotificationService::new();
        svc.set_config(make_config("task-1", vec!["status-update"]))
            .await
            .unwrap();
        svc.set_config(make_config("task-2", vec!["artifact-update"]))
            .await
            .unwrap();

        let mut configs = svc.list_configs().await.unwrap();
        configs.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].task_id, "task-1");
        assert_eq!(configs[1].task_id, "task-2");
    }

    #[tokio::test]
    async fn set_config_overwrites_previous() {
        let svc = NotificationService::new();
        svc.set_config(make_config("task-1", vec!["status-update"]))
            .await
            .unwrap();

        let updated = PushNotificationConfig {
            task_id: "task-1".to_string(),
            url: "https://new-url.com/hook".to_string(),
            token: None,
            events: vec!["artifact-update".to_string()],
        };
        svc.set_config(updated).await.unwrap();

        let fetched = svc.get_config("task-1").await.unwrap().unwrap();
        assert_eq!(fetched.url, "https://new-url.com/hook");
        assert!(fetched.token.is_none());
        assert_eq!(fetched.events, vec!["artifact-update"]);
    }

    #[test]
    fn push_notification_config_serde_roundtrip() {
        let config = make_config("task-1", vec!["status-update", "artifact-update"]);
        let json = serde_json::to_string(&config).unwrap();
        let back: PushNotificationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "task-1");
        assert_eq!(back.events.len(), 2);

        // Verify camelCase
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("taskId").is_some());
    }

    #[test]
    fn push_notification_config_skips_none_token() {
        let config = PushNotificationConfig {
            task_id: "t1".to_string(),
            url: "https://example.com".to_string(),
            token: None,
            events: vec![],
        };
        let value = serde_json::to_value(&config).unwrap();
        assert!(value.get("token").is_none());
    }
}
