use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::a2a::domain::{A2AError, TaskArtifactUpdateEvent, TaskStatusUpdateEvent};
use crate::a2a::port::A2AResult;
use crate::security::ssrf::{validate_url_async, SsrfPolicy};
use crate::sync_primitives::AsyncRwLock;

/// Configuration for push notifications on a task
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationConfig {
    pub task_id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub events: Vec<String>, // "status-update", "artifact-update"
}

impl std::fmt::Debug for PushNotificationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushNotificationConfig")
            .field("task_id", &self.task_id)
            .field("url", &self.url)
            .field("token", &self.token.as_deref().map(|_| "[REDACTED]"))
            .field("events", &self.events)
            .finish()
    }
}

/// Push notification service — manages webhook configs and fires notifications
pub struct NotificationService {
    configs: AsyncRwLock<HashMap<String, PushNotificationConfig>>,
}

impl NotificationService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            configs: AsyncRwLock::new(HashMap::new()),
        }
    }

    /// Store or overwrite a push notification config for a task
    pub async fn set_config(
        &self,
        config: PushNotificationConfig,
    ) -> A2AResult<PushNotificationConfig> {
        validate_url_async(&config.url, &SsrfPolicy::default())
            .await
            .map_err(|e| {
                A2AError::InvalidParams(format!(
                    "pushNotificationConfig.url rejected by SSRF policy: {e}"
                ))
            })?;
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
        let mut result: Vec<_> = configs.values().cloned().collect();
        result.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(result)
    }

    /// Send push notification for a status update
    pub async fn notify_status_update(&self, task_id: &str, event: &TaskStatusUpdateEvent) {
        let config = {
            let configs = self.configs.read().await;
            configs.get(task_id).cloned()
        };

        if let Some(config) = config {
            if config.events.is_empty() || config.events.iter().any(|e| e == "status-update") {
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
            if config.events.is_empty() || config.events.iter().any(|e| e == "artifact-update") {
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
        let body = match serde_json::to_vec(payload) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    task_id = %config.task_id,
                    error = %e,
                    "Push notification payload serialization failed"
                );
                return;
            }
        };

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        if let Some(ref token) = config.token {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            {
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }

        let fetch_request = crate::security::ssrf::SafeFetchRequest::post(
            body,
            std::time::Duration::from_secs(10),
        )
        .with_headers(headers);

        match crate::security::ssrf::safe_fetch(
            &config.url,
            &SsrfPolicy::default(),
            fetch_request,
        )
        .await
        {
            Ok(resp) if resp.status.is_success() => {
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
                    status = %resp.status,
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
            url: "https://8.8.8.8/webhook".to_string(),
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
        assert_eq!(result.url, "https://8.8.8.8/webhook");

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

        let configs = svc.list_configs().await.unwrap();
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
            url: "https://1.1.1.1/hook".to_string(),
            token: None,
            events: vec!["artifact-update".to_string()],
        };
        svc.set_config(updated).await.unwrap();

        let fetched = svc.get_config("task-1").await.unwrap().unwrap();
        assert_eq!(fetched.url, "https://1.1.1.1/hook");
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

    fn config_with_url(task_id: &str, url: &str) -> PushNotificationConfig {
        PushNotificationConfig {
            task_id: task_id.to_string(),
            url: url.to_string(),
            token: None,
            events: vec![],
        }
    }

    #[tokio::test]
    async fn set_config_rejects_non_http_scheme() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "ftp://example.com/webhook"))
            .await
            .expect_err("non-http scheme must be rejected");
        match err {
            crate::a2a::domain::A2AError::InvalidParams(_) => {}
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_config_rejects_file_scheme() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "file:///etc/passwd"))
            .await
            .expect_err("file scheme must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_rejects_loopback_ip() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "http://127.0.0.1/hook"))
            .await
            .expect_err("loopback IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_rejects_localhost_hostname() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "http://localhost/hook"))
            .await
            .expect_err("localhost hostname must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_rejects_metadata_endpoint() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url(
                "t1",
                "http://169.254.169.254/latest/meta-data/",
            ))
            .await
            .expect_err("cloud metadata endpoint must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_rejects_private_ip() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "http://10.0.0.1/hook"))
            .await
            .expect_err("private 10.0.0.0/8 IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));

        let err = svc
            .set_config(config_with_url("t2", "http://192.168.1.1/hook"))
            .await
            .expect_err("private 192.168.0.0/16 IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_rejects_link_local_ip() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url("t1", "http://169.254.1.1/hook"))
            .await
            .expect_err("link-local IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn set_config_accepts_public_https() {
        let svc = NotificationService::new();
        let result = svc
            .set_config(config_with_url("t1", "https://8.8.8.8/hook"))
            .await;
        assert!(
            result.is_ok(),
            "public HTTPS URL must be accepted, got: {:?}",
            result
        );
        let stored = svc.get_config("t1").await.unwrap().unwrap();
        assert_eq!(stored.url, "https://8.8.8.8/hook");
    }

    #[tokio::test]
    async fn set_config_rejects_url_with_credentials() {
        let svc = NotificationService::new();
        let err = svc
            .set_config(config_with_url(
                "t1",
                "https://user:pass@example.com/hook",
            ))
            .await
            .expect_err("URL with embedded credentials must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn send_webhook_blocks_private_ip_destination() {
        use crate::a2a::domain::TaskState;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let svc = NotificationService::new();
        let config = PushNotificationConfig {
            task_id: "task-sendblock".to_string(),
            url: format!("{}/hook", server.uri()),
            token: None,
            events: vec![],
        };
        {
            let mut configs = svc.configs.write().await;
            configs.insert("task-sendblock".to_string(), config);
        }

        let event = TaskStatusUpdateEvent {
            task_id: "task-sendblock".to_string(),
            context_id: "ctx".to_string(),
            status: crate::a2a::domain::task::TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: chrono::Utc::now(),
            },
            is_final: false,
            metadata: None,
        };
        svc.notify_status_update("task-sendblock", &event).await;

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let received = server
            .received_requests()
            .await
            .expect("wiremock must record requests");
        assert!(
            received.is_empty(),
            "send_webhook must not POST to a private/loopback URL, got {} request(s)",
            received.len()
        );
    }
}
