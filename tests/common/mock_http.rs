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

pub struct MattermostApiMock;

impl MattermostApiMock {
    pub async fn users_me(server: &MockServer) {
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/v4/users/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "user-bot-123",
                "username": "aleph-bot",
                "email": "bot@example.com",
            })))
            .mount(server)
            .await;
    }

    pub async fn users_me_unauthorized(server: &MockServer) {
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/v4/users/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "id": "",
                "message": "Invalid or expired session token",
                "request_id": "req-123",
                "status_code": 401,
            })))
            .mount(server)
            .await;
    }

    pub async fn create_post(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/v4/posts"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "post-abc-123",
                "channel_id": "ch-789",
                "message": "Hello from Mattermost!",
                "user_id": "user-bot-123",
                "create_at": 1700000000000_i64,
            })))
            .mount(server)
            .await;
    }

    pub async fn create_post_with_root_id(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/v4/posts"))
            .and(matchers::body_json(serde_json::json!({
                "channel_id": "ch-789",
                "message": "Thread reply",
                "root_id": "post-root-456",
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "post-reply-789",
                "channel_id": "ch-789",
                "message": "Thread reply",
                "user_id": "user-bot-123",
                "root_id": "post-root-456",
                "create_at": 1700000000000_i64,
            })))
            .mount(server)
            .await;
    }

    pub async fn edit_post(server: &MockServer) {
        Mock::given(matchers::method("PUT"))
            .and(matchers::path_regex("/api/v4/posts/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "post-abc-123",
                "message": "Edited message",
            })))
            .mount(server)
            .await;
    }

    pub async fn delete_post(server: &MockServer) {
        Mock::given(matchers::method("DELETE"))
            .and(matchers::path_regex("/api/v4/posts/.*"))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    pub async fn create_reaction(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/v4/reactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user_id": "user-bot-123",
                "post_id": "post-abc-123",
                "emoji_name": "thumbsup",
                "create_at": 1700000000000_i64,
            })))
            .mount(server)
            .await;
    }

    pub async fn typing(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/v4/users/me/typing"))
            .respond_with(ResponseTemplate::new(200))
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
