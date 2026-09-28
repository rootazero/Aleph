#![allow(dead_code, unused_imports)]

use serde_json::json;
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

pub struct LineApiMock;

impl LineApiMock {
    pub async fn push_message(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v2/bot/message/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sentMessages": [
                    { "id": "line-msg-123", "quoteToken": "quote-abc" }
                ]
            })))
            .mount(server)
            .await;
    }

    pub async fn reply_message(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v2/bot/message/reply"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sentMessages": [
                    { "id": "line-msg-456", "quoteToken": "quote-def" }
                ]
            })))
            .mount(server)
            .await;
    }

    pub async fn delete_message(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path_regex("/v2/bot/message/.*/delete"))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    pub async fn get_profile(server: &MockServer) {
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex("/v2/bot/profile/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "displayName": "Test User",
                "userId": "U123456",
                "pictureUrl": "https://example.com/photo.jpg",
                "statusMessage": "Hello!"
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

/// Telegram Bot API mocks.
///
/// Telegram routes every call through `<api.telegram.org>/bot<TOKEN>/<method>`.
/// `Bot::set_api_url(mock_uri)` makes teloxide 0.17 issue `<mock_uri>/bot<TOKEN>/<method>`
/// (see teloxide-core `net::method_url`), so every helper here matches on the
/// full `/bot<TOKEN>/<method>` path — the token is fixed to `TEST_TOKEN` in
/// `telegram_delivery_e2e.rs::bot_for`, so the paths below are stable.
///
/// **Path casing**: teloxide's `impl_payload!` macro derives `Payload::NAME`
/// from `stringify!($Method)` of the PascalCase token (`SendMessage`,
/// `SendMediaGroup`, ...). So the wire-level path is `/botTOKEN/SendMessage`,
/// not `/botTOKEN/sendMessage`. Telegram's HTTP API happens to be
/// case-insensitive on method names — that's why the production call works
/// even though Rust-side `stringify!` produces PascalCase — but wiremock
/// matches paths case-sensitively, so the helpers below use PascalCase too.
///
/// All responses use the `{"ok": ..., "result": ...}` envelope (or the
/// `{"ok": false, "error_code": ..., "description": "..."}` shape that
/// teloxide's `ApiError::Xxx` matches against). Returning well-formed error
/// bodies is what lets `classify_error` exercise the full error taxonomy
/// (Network / Forbidden / RateLimited / Rejected) without a real Telegram.
pub struct TelegramBotMock;

impl TelegramBotMock {
    /// Successful `sendMessage`. `result` defaults to a minimal Message-shaped
    /// object so `delivery::send_message` can build its `SendResult`.
    pub async fn send_message_ok(server: &MockServer) {
        Self::send_message_ok_with(
            server,
            json!({
                "message_id": 1001,
                "date": 1_700_000_000,
                "chat": {"id": 123, "type": "private"},
                "text": "ok",
            }),
        )
        .await;
    }

    /// `sendMessage` returning a specific `result` body.
    pub async fn send_message_ok_with(server: &MockServer, result: serde_json::Value) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": result,
            })))
            .mount(server)
            .await;
    }

    /// `sendMessage` returning 429 with `retry_after`. Counts as one call;
    /// the test must `mount` exactly one follow-up 200 to drain the retry.
    pub async fn send_message_rate_limited(server: &MockServer, retry_after_secs: u64) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendMessage"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "ok": false,
                "error_code": 429,
                "description": format!("Too Many Requests: retry after {retry_after_secs}"),
                "parameters": {"retry_after": retry_after_secs},
            })))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }

    /// `sendMessage` returning 403 with the exact description teloxide parses
    /// as `ApiError::BotBlocked`. Triggers `ErrorClass::Forbidden(BotBlocked)`
    /// → `ErrorKind::Permanent` (P0-C: this used to be lumped into `Rejected`
    /// and the conversation was parked correctly, but with the wrong label).
    pub async fn send_message_bot_blocked(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendMessage"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "ok": false,
                "error_code": 403,
                "description": "Forbidden: bot was blocked by the user",
            })))
            .mount(server)
            .await;
    }

    /// `sendMessage` returning the exact description teloxide parses as
    /// `ApiError::MessageToEditNotFound`. Triggers
    /// `ErrorClass::Forbidden(MessageNotFound)` → `ErrorKind::Retryable` —
    /// the W2 fix is that a missing edit target does NOT park the
    /// conversation for hours.
    pub async fn send_message_to_edit_not_found(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/EditMessageText"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false,
                "error_code": 400,
                "description": "Bad Request: message to edit not found",
            })))
            .mount(server)
            .await;
    }

    /// `sendMediaGroup` (Telegram's photo album endpoint) succeeds. P2-A:
    /// ≥2 image attachments must reach here, not `sendPhoto` repeated N times.
    pub async fn send_media_group_ok(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendMediaGroup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": [
                    {"message_id": 1001, "date": 1_700_000_000, "chat": {"id": 123, "type": "private"}},
                    {"message_id": 1002, "date": 1_700_000_001, "chat": {"id": 123, "type": "private"}},
                ],
            })))
            .mount(server)
            .await;
    }

    /// `sendPhoto` succeeds. Used to assert the SINGLE-image path did NOT
    /// fall through to `sendMediaGroup` (which would lose the byline).
    pub async fn send_photo_ok(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendPhoto"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {
                    "message_id": 1001, "date": 1_700_000_000,
                    "chat": {"id": 123, "type": "private"},
                },
            })))
            .mount(server)
            .await;
    }

    /// `sendVoice` succeeds. P2-B: an attachment with
    /// `is_voice_note = true` (or the audio/* MIME on a voice note hint)
    /// must reach `sendVoice`, not `sendAudio` / `sendDocument`.
    pub async fn send_voice_ok(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendVoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {
                    "message_id": 1001, "date": 1_700_000_000,
                    "chat": {"id": 123, "type": "private"},
                },
            })))
            .mount(server)
            .await;
    }

    /// `sendVideoNote` succeeds. P2-B: `is_video_note = true` on a video/*
    /// attachment must reach `sendVideoNote` (round bubble), not `sendVideo`.
    pub async fn send_video_note_ok(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/botTEST_TOKEN/SendVideoNote"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {
                    "message_id": 1001, "date": 1_700_000_000,
                    "chat": {"id": 123, "type": "private"},
                },
            })))
            .mount(server)
            .await;
    }

    /// `sendAudio` / `sendVideo` / `sendDocument` catch-all for the
    /// "did NOT route to the right endpoint" assertion. Tests register
    /// this with a strict `not()` matcher on the right endpoint to
    /// verify the dispatch chose the specific endpoint, not a generic one.
    pub async fn catch_other_send(server: &MockServer) {
        Mock::given(matchers::method("POST"))
            .and(matchers::path_regex(
                r"^/botTEST_TOKEN/(SendAudio|SendVideo|SendDocument)$",
            ))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "ok": false,
                "error_code": 500,
                "description": "unexpected endpoint — test expected a different method",
            })))
            .mount(server)
            .await;
    }
}
