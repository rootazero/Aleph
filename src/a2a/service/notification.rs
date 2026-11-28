use serde::{Deserialize, Serialize};

use crate::a2a::domain::{A2AError, UpdateEvent};
use crate::a2a::port::A2AResult;
use crate::security::ssrf::{SsrfPolicy, validate_url_with_pinned};

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

/// Validates and delivers callbacks owned by the task store.
pub struct NotificationService {
    ssrf_policy: SsrfPolicy,
}

impl NotificationService {
    /// Construct with the built-in default SSRF policy.
    ///
    /// The operator's outbound-host policy (`webhook_ssrf_policy` in the
    /// server config) is **not** applied on this path. `SsrfPolicy::default()`
    /// blocks private/internal address ranges, but it carries none of the
    /// operator's `allowed_hosts` / `blocked_hosts` lists, so an operator who
    /// restricted outbound hosts would have that restriction silently ignored
    /// for A2A push notifications. The service therefore warns at construction
    /// instead of pretending the operator policy is honoured; wire
    /// [`NotificationService::with_ssrf_policy`] to apply it.
    #[must_use]
    pub fn new() -> Self {
        tracing::warn!(
            "A2A push notifications are using the default SSRF policy — the operator webhook SSRF policy is NOT applied on this path. Wire NotificationService::with_ssrf_policy(...) to honour it."
        );
        Self {
            ssrf_policy: SsrfPolicy::default(),
        }
    }

    /// Apply the operator's outbound policy at registration and delivery.
    #[must_use]
    pub fn with_ssrf_policy(ssrf_policy: SsrfPolicy) -> Self {
        Self { ssrf_policy }
    }

    pub(crate) async fn validate_config(&self, config: &PushNotificationConfig) -> A2AResult<()> {
        // A webhook target is delivered over HTTP, so anything else is a mistake
        // at best and an SSRF-via-alternate-scheme vector at worst
        // (`ftp://internal/…`, `gopher://internal:6379/…`). Screened here rather
        // than left to the SSRF engine so hostless schemes (`file:///etc/passwd`)
        // get a scheme-specific message instead of an incidental "no host".
        match url::Url::parse(&config.url) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {}
            Ok(parsed) => {
                return Err(A2AError::InvalidParams(format!(
                    "pushNotificationConfig.url has unsupported scheme '{}': \
                     push notifications allow only http/https",
                    parsed.scheme()
                )));
            }
            Err(e) => {
                return Err(A2AError::InvalidParams(format!(
                    "pushNotificationConfig.url is not a valid URL: {e}"
                )));
            }
        }

        // Validate at accept time, not only at send time. `send_webhook` does
        // run `safe_fetch`, but that is a different behaviour, not a duplicate
        // one: without this the registering client gets `Ok` for a webhook that
        // can only ever fail, and the failure surfaces as a `tracing::warn!` in
        // the daemon it will never read. Rejecting here is the only synchronous
        // signal the caller gets.
        validate_url_with_pinned(&config.url, &self.ssrf_policy)
            .await
            .map(|(_, _pinned)| ())
            .map_err(|e| {
                A2AError::InvalidParams(format!(
                    "pushNotificationConfig.url rejected by SSRF policy: {e}"
                ))
            })
    }

    pub(crate) async fn notify(&self, config: PushNotificationConfig, event: UpdateEvent) {
        let (event_type, data) = match event {
            UpdateEvent::StatusUpdate(update) => ("status-update", serde_json::json!(update)),
            UpdateEvent::ArtifactUpdate(update) => ("artifact-update", serde_json::json!(update)),
        };
        if config.events.is_empty() || config.events.iter().any(|e| e == event_type) {
            self.send_webhook(
                &config,
                &serde_json::json!({"type": event_type, "data": data}),
            )
            .await;
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
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }

        let fetch_request =
            crate::security::ssrf::SafeFetchRequest::post(body, std::time::Duration::from_secs(10))
                .with_headers(headers);

        match crate::security::ssrf::safe_fetch(&config.url, &self.ssrf_policy, fetch_request).await
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
    async fn validate_config_rejects_non_http_scheme() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "ftp://example.com/webhook"))
            .await
            .expect_err("non-http scheme must be rejected");
        match err {
            crate::a2a::domain::A2AError::InvalidParams(_) => {}
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_config_rejects_file_scheme() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "file:///etc/passwd"))
            .await
            .expect_err("file scheme must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    // Accept-time SSRF rejection. These cover a boundary `send_webhook`'s own
    // `safe_fetch` does not: the caller must be told synchronously, rather than
    // getting `Ok` for a webhook that silently never fires. All IP literals, so
    // the decision needs no DNS and the tests stay hermetic.

    #[tokio::test]
    async fn validate_config_rejects_loopback_ip() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "http://127.0.0.1/hook"))
            .await
            .expect_err("loopback IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn validate_config_rejects_localhost_hostname() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "http://localhost/hook"))
            .await
            .expect_err("localhost hostname must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn validate_config_rejects_metadata_endpoint() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url(
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
    async fn validate_config_rejects_private_ip() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "http://10.0.0.1/hook"))
            .await
            .expect_err("private 10.0.0.0/8 IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));

        let err = svc
            .validate_config(&config_with_url("t2", "http://192.168.1.1/hook"))
            .await
            .expect_err("private 192.168.0.0/16 IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn validate_config_rejects_link_local_ip() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "http://169.254.1.1/hook"))
            .await
            .expect_err("link-local IP must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn validate_config_rejects_url_with_credentials() {
        let svc = NotificationService::new();
        let err = svc
            .validate_config(&config_with_url("t1", "https://user:pass@example.com/hook"))
            .await
            .expect_err("URL with embedded credentials must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }

    #[tokio::test]
    async fn validate_config_accepts_public_https() {
        let svc = NotificationService::new();
        let result = svc
            .validate_config(&config_with_url("t1", "https://8.8.8.8/hook"))
            .await;
        assert!(
            result.is_ok(),
            "public HTTPS URL must be accepted, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn send_webhook_blocks_private_ip_destination() {
        use crate::a2a::domain::{TaskState, TaskStatusUpdateEvent};
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
        svc.notify(config, UpdateEvent::StatusUpdate(event)).await;

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

    #[tokio::test]
    async fn validate_config_applies_operator_ssrf_policy() {
        let svc = NotificationService::with_ssrf_policy(SsrfPolicy {
            blocked_hosts: vec!["blocked.example.com".to_string()],
            ..SsrfPolicy::default()
        });
        let err = svc
            .validate_config(&config_with_url("t1", "https://blocked.example.com/hook"))
            .await
            .expect_err("operator-blocked host must be rejected");
        assert!(matches!(
            err,
            crate::a2a::domain::A2AError::InvalidParams(_)
        ));
    }
}
