//! Opt-in acceptance through the real TUI transport, reducer, and renderer.
//! Uses only an existing loopback Gateway and a fresh acceptance conversation.

use super::*;
use ratatui::backend::TestBackend;

#[tokio::test]
#[ignore = "requires ALEPH_TUI_LIVE_ACCEPTANCE=1 and an existing local Gateway/model"]
async fn subscribed_reply_reaches_screen_after_startup_and_reconnect() {
    assert_eq!(
        std::env::var("ALEPH_TUI_LIVE_ACCEPTANCE").as_deref(),
        Ok("1")
    );
    let config = CliConfig {
        server: "ws://127.0.0.1:18790/ws".into(),
        device_name: "tui-stream-acceptance".into(),
        ..CliConfig::default()
    };
    let (client, mut events) = AlephClient::connect(&config.server, &config)
        .await
        .expect("local Gateway");
    let mut state = AppState::new(String::new(), "acceptance".into());
    for reconnect in [false, true] {
        if reconnect {
            client.close().await.expect("close old socket");
            client.reconnect(&config).await.expect("replace socket");
        }
        subscribe_browser_live_topics(&client)
            .await
            .expect("browser subscription");
        let marker = if reconnect {
            "TUI_RECONNECTED_OK"
        } else {
            "TUI_STARTED_OK"
        };
        commands::send_to_agent(
            &mut state,
            &client,
            &format!("本地界面验收：请只原样回复 {marker}，不调用工具。"),
            "Acceptance send failed",
        )
        .await;
        let mut received = 0;
        let mut completed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
            let mine = match &event {
                StreamEvent::RunAccepted { session_key, .. } => session_key == &state.session_key,
                _ => state
                    .current_run
                    .as_deref()
                    .is_some_and(|id| event.run_id() == id),
            };
            if !mine {
                continue;
            }
            received += 1;
            let terminal = matches!(
                &event,
                StreamEvent::RunComplete { .. } | StreamEvent::RunError { .. }
            );
            state.handle_gateway_event(event);
            if terminal {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "no terminal event after reconnect={reconnect}; received={received}"
        );
        assert!(
            state.messages.iter().any(|entry| match entry {
                app::TranscriptEntry::AssistantText { markdown, .. } => markdown.contains(marker),
                app::TranscriptEntry::Step(step) => step
                    .text
                    .as_deref()
                    .is_some_and(|text| text.contains(marker)),
                _ => false,
            }),
            "model reply missing from assistant entries after reconnect={reconnect}"
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).expect("test terminal");
        let textarea = TextArea::default();
        terminal
            .draw(|frame| render::render(frame, &mut state, &textarea))
            .expect("render reply");
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(
            text.contains(marker),
            "reply did not reach rendered screen after reconnect={reconnect}"
        );
        println!("reconnect={reconnect}: received={received}, reply visible=true");
    }
    client.close().await.expect("close acceptance connection");
}
