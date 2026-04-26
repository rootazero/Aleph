use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

pub struct SlackApiMock;

impl SlackApiMock {
    pub async fn auth_test(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "user_id": "U123456",
                "user": "testbot",
                "team": "T123456",
            })))
            .mount(server)
            .await;
    }

    pub async fn chat_post_message(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1234567890.123456",
                "channel": "C12345",
                "message": {
                    "type": "message",
                    "user": "U123456",
                    "text": "Hello",
                    "ts": "1234567890.123456",
                }
            })))
            .mount(server)
            .await;
    }

    pub async fn chat_post_message_rate_limit(server: &MockServer, retry_after: u64) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/chat.postMessage"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", retry_after.to_string())
                    .set_body_json(serde_json::json!({
                        "ok": false,
                        "error": "rate_limited",
                    })),
            )
            .mount(server)
            .await;
    }

    pub async fn chat_post_typing(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/chat.postTyping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
            })))
            .mount(server)
            .await;
    }

    pub async fn reactions_add(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/reactions.add"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
            })))
            .mount(server)
            .await;
    }
}

pub struct WebhookMock;

impl WebhookMock {
    pub async fn callback_ok(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::header_exists("X-Webhook-Signature"))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    pub async fn callback_error(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(server)
            .await;
    }
}
