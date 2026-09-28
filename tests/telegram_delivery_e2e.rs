//! Wiremock-backed integration tests for the Telegram delivery layer.
//!
//! These tests exercise `delivery::send_message`, `send_attachments`, and
//! `send_attachment` through a real `teloxide::Bot` whose API URL is
//! pointed at a `wiremock::MockServer`. No real Telegram round-trip is
//! performed; the mock's matchers let us assert WHICH endpoint the
//! channel chose (e.g. `sendMediaGroup` for ≥2 images, `sendVoice` for
//! `is_voice_note = true`, `sendPhoto` for single-image, `sendMessage`
//! for plain text).
//!
//! The three gaps these tests close vs `tests/telegram_e2e.rs`:
//!
//! 1. **P0-C error taxonomy** — `classify_error` splits Forbidden into
//!    `BotBlocked` / `UserNotFound` / `ChatNotFound` (Permanent) vs
//!    `MessageNotFound` (Retryable). The split prevents a missing edit
//!    target from parking a healthy conversation for hours. Asserted by
//!    observing `ErrorCooldown::check()` state after a failure.
//!
//! 2. **P2-A photo albums** — ≥2 image attachments must reach
//!    `sendMediaGroup`, not `sendPhoto` repeated N times. Single-image
//!    must reach `sendPhoto` (preserves the byline on some clients).
//!
//! 3. **P2-B voice / video notes** — `Attachment::is_voice_note` and
//!    `is_video_note` route to `sendVoice` / `sendVideoNote` instead of
//!    the generic `sendAudio` / `sendVideo` / `sendDocument`.
//!
//! `Bot::set_api_url` is how teloxide 0.17 lets us redirect; teloxide then
//! issues `<mock_uri>/bot<TOKEN>/<method>` (see teloxide-core
//! `net::method_url`). `TelegramBotMock`'s helpers match on the full
//! `/botTEST_TOKEN/<method>` path because the token is fixed below.
//!
//! The `#[doc(hidden)]` test-only exports (`send_message`,
//! `send_attachments`, `send_attachment`, `classify_error`, `ErrorCooldown`,
//! `ErrorKind`) are deliberate: they keep the public API surface narrow
//! while still letting us hit the inner retry / dispatch loop.

#![cfg(test)]

mod common;

use std::time::Duration;

use alephcore::gateway::channel::{Attachment, OutboundMessage};
use alephcore::gateway::interfaces::telegram::config_resolver::ResolvedConfig;
use alephcore::gateway::interfaces::telegram::config_v2::{
    DmPolicy, ErrorPolicy, ErrorPolicyMode, GroupPolicy, LinkPreviewMode, StreamingOptions,
};
use alephcore::gateway::interfaces::telegram::delivery::{
    send_attachment, send_attachments, send_message,
};
use alephcore::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;

use common::mock_http::TelegramBotMock;
use teloxide::Bot;
use wiremock::MockServer;

/// Token used by every test below. Pinned here so the `path("/bot.../...")`
/// matchers in `TelegramBotMock` are stable.
const TEST_TOKEN: &str = "TEST_TOKEN";

/// Build a `Bot` whose `api.telegram.org` calls are redirected at the mock.
async fn bot_for(server: &MockServer) -> Bot {
    let api_url = reqwest::Url::parse(&server.uri()).expect("mock uri parses");
    Bot::new(TEST_TOKEN).set_api_url(api_url)
}

/// Minimal `ResolvedConfig` — disable typing / chunking so the call path is
/// a single `sendMessage` round-trip with no test noise.
fn silent_config() -> ResolvedConfig {
    ResolvedConfig {
        account_id: "test".to_string(),
        bot_token: TEST_TOKEN.to_string(),
        bot_username: None,
        default_agent: None,
        dm_policy: DmPolicy::Open,
        group_policy: GroupPolicy::Disabled,
        send_typing: false, // skip the chat_action call
        allowed_users: vec![],
        allowed_groups: vec![],
        streaming: StreamingOptions::default(),
        error_policy: ErrorPolicy {
            mode: ErrorPolicyMode::Silent,
            template: None,
            max_retries: 3,
        },
        max_retries: 3,
        html_fallback: false,
        link_preview: LinkPreviewMode::Disabled,
    }
}

fn text_message(conv: &str, body: &str) -> OutboundMessage {
    OutboundMessage::text(conv, body)
}

fn png_attachment(id: &str) -> Attachment {
    Attachment {
        id: id.to_string(),
        mime_type: "image/png".to_string(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![0x89, 0x50, 0x4E, 0x47]),
        is_voice_note: false,
        is_video_note: false,
    }
}

/// Send a text message through a mock that returns 200. Asserts the mock
/// saw exactly one `sendMessage` call (no retry storm).
#[tokio::test]
async fn send_message_text_succeeds() {
    let server = MockServer::start().await;
    TelegramBotMock::send_message_ok(&server).await;

    let bot = bot_for(&server).await;
    let config = silent_config();
    let cooldown = ErrorCooldown::new();
    let msg = text_message("-100123", "hello");

    let result = send_message(&bot, &config, &msg, &cooldown).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "1001");

    // Conversation is healthy — no cooldown recorded.
    assert!(
        cooldown.check("-100123").is_ok(),
        "successful send must not park the conversation"
    );
}

/// P0-C rate limit: the first call returns 429 with `retry_after`, the
/// second returns 200. The `send_message` retry loop should drain the
/// 429 and succeed; the conversation must NOT be parked (Retryable).
#[tokio::test]
async fn send_message_retries_then_succeeds_on_429() {
    let server = MockServer::start().await;
    TelegramBotMock::send_message_rate_limited(&server, 1).await;
    TelegramBotMock::send_message_ok(&server).await;

    let bot = bot_for(&server).await;
    let config = silent_config();
    let cooldown = ErrorCooldown::new();
    let msg = text_message("-100123", "with rate limit");

    // Sleep 1s before retrying per `retry_after`. We pass 1s above; the
    // test sleeps a hair longer to avoid clock-skew flakes on slow CI.
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        send_message(&bot, &config, &msg, &cooldown),
    )
    .await
    .expect("send_message must drain the 429 within 8s");
    assert!(result.is_ok(), "expected Ok after retry, got {result:?}");

    // A retryable failure must not park the conversation — only the
    // first attempt fails, the second succeeds, so check() should be Ok.
    assert!(
        cooldown.check("-100123").is_ok(),
        "Retryable → permanent-cooldown path must not be entered"
    );
}

/// P0-C `Forbidden(BotBlocked)` is Permanent. The conversation must end
/// up in cooldown (a future send within the cooldown window must fail
/// the precheck). Verifies the operator's diagnostic story: "the bot was
/// blocked, this chat is parked for 4h, a DIFFERENT chat on the same
/// account is unaffected" (P0-C contract).
#[tokio::test]
async fn send_message_forbidden_bot_blocked_marks_permanent() {
    let server = MockServer::start().await;
    TelegramBotMock::send_message_bot_blocked(&server).await;

    let bot = bot_for(&server).await;
    let config = silent_config();
    let cooldown = ErrorCooldown::new();
    let msg = text_message("-100123", "this will fail");

    let result = send_message(&bot, &config, &msg, &cooldown).await;
    assert!(result.is_err(), "expected Err on BotBlocked");

    // Permanent failure must put the conversation in cooldown.
    assert!(
        cooldown.check("-100123").is_err(),
        "Forbidden::BotBlocked is Permanent; the conversation must be parked"
    );
}

/// P2-A photo albums: ≥2 image attachments must reach `sendMediaGroup`.
/// Verifies the `flush_image_album` path is actually wired through.
#[tokio::test]
async fn send_message_two_images_use_sendmediagroup() {
    let server = MockServer::start().await;
    TelegramBotMock::send_message_ok(&server).await;
    TelegramBotMock::send_media_group_ok(&server).await;
    // Catch-all so any accidental `sendPhoto` round-trip fails the test
    // loudly instead of silently passing through MockServer's default.
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let config = silent_config();
    let cooldown = ErrorCooldown::new();

    let mut msg = text_message("-100123", "album");
    msg.attachments = vec![png_attachment("img-1"), png_attachment("img-2")];

    let result = send_message(&bot, &config, &msg, &cooldown).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");

    // Both endpoints were hit exactly the right number of times — wiremock
    // counts requests per match spec, so the failure mode is "extra /
    // missing call", not "wrong call".
}

/// P2-A photo albums: a SINGLE image must NOT collapse into `sendMediaGroup`
/// (which would lose the byline on some clients). It must use `sendPhoto`.
#[tokio::test]
async fn send_message_single_image_uses_sendphoto_not_mediagroup() {
    let server = MockServer::start().await;
    TelegramBotMock::send_message_ok(&server).await;
    TelegramBotMock::send_photo_ok(&server).await;
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let config = silent_config();
    let cooldown = ErrorCooldown::new();

    let mut msg = text_message("-100123", "single");
    msg.attachments = vec![png_attachment("img-1")];

    let result = send_message(&bot, &config, &msg, &cooldown).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

/// P2-B voice notes: an audio/* attachment with `is_voice_note = true`
/// must reach `sendVoice` (round bubble), not `sendAudio` / `sendDocument`.
#[tokio::test]
async fn send_attachment_voice_note_routes_to_sendvoice() {
    let server = MockServer::start().await;
    TelegramBotMock::send_voice_ok(&server).await;
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let att = Attachment {
        id: "v1".to_string(),
        mime_type: "audio/ogg".to_string(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![0x4F, 0x67, 0x67, 0x53]), // Ogg magic
        is_voice_note: true,
        is_video_note: false,
    };

    let result = send_attachment(&bot, teloxide::types::ChatId(-100_123), None, &att).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

/// P2-B video notes: a video/* attachment with `is_video_note = true`
/// must reach `sendVideoNote`, not `sendVideo`.
#[tokio::test]
async fn send_attachment_video_note_routes_to_sendvideonote() {
    let server = MockServer::start().await;
    TelegramBotMock::send_video_note_ok(&server).await;
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let att = Attachment {
        id: "vn1".to_string(),
        mime_type: "video/mp4".to_string(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![0x00, 0x00, 0x00, 0x18]), // MP4-ish
        is_voice_note: false,
        is_video_note: true,
    };

    let result = send_attachment(&bot, teloxide::types::ChatId(-100_123), None, &att).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

/// P2-B: a video/* attachment WITHOUT the `is_video_note` hint must reach
/// the generic `sendVideo`. Verifies the hint is actually a switch, not
/// decorative.
#[tokio::test]
async fn send_attachment_plain_video_does_not_use_sendvideonote() {
    let server = MockServer::start().await;
    mock_server_video_ok(&server).await;
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let att = Attachment {
        id: "v1".to_string(),
        mime_type: "video/mp4".to_string(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![0x00, 0x00, 0x00, 0x18]),
        is_voice_note: false,
        is_video_note: false,
    };

    let result = send_attachment(&bot, teloxide::types::ChatId(-100_123), None, &att).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

/// `send_attachments` route: the standalone helper, called directly so a
/// future refactor that pulls it out of `send_message` keeps the same
/// dispatch surface.
#[tokio::test]
async fn send_attachments_routes_album_correctly() {
    let server = MockServer::start().await;
    TelegramBotMock::send_media_group_ok(&server).await;
    TelegramBotMock::catch_other_send(&server).await;

    let bot = bot_for(&server).await;
    let atts = vec![png_attachment("a"), png_attachment("b")];

    let result = send_attachments(&bot, teloxide::types::ChatId(-100_123), None, &atts).await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

/// Local mock helper for the "plain video" case — `TelegramBotMock` only
/// ships the specialised endpoints (`sendVoice` / `sendVideoNote` /
/// `sendMediaGroup`), and the catch-all deliberately 500s on the generic
/// paths, so the positive case needs an explicit mount.
async fn mock_server_video_ok(server: &MockServer) {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN/SendVideo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 1001, "date": 1_700_000_000,
                "chat": {"id": 123, "type": "private"},
            },
        })))
        .mount(server)
        .await;
}
