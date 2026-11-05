mod connection;
mod runtime;
mod session;
mod supervisor;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::*;
    use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

    #[component]
    fn App() -> impl IntoElement {
        use connection::{spawn_connection_loop, ShellStatus};
        use session::SessionManager;

        let connection_status = signal(ShellStatus::Connecting);
        let session_info = signal(String::from("No session"));
        let messages_display = signal(String::from("No messages yet"));

        // Phase 2: Connection loop
        let (conn_tx, mut conn_rx) = tokio::sync::mpsc::channel::<ShellStatus>(8);
        let _handle = spawn_connection_loop(conn_tx);

        let conn_status_clone = connection_status;
        spawn(async move {
            while let Some(new_status) = conn_rx.recv().await {
                conn_status_clone.set(new_status);
            }
        });

        // Phase 3: Session creation when connected
        let session_info_clone = session_info;
        let messages_clone = messages_display;
        let conn_status_for_session = conn_status_clone;

        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if matches!(conn_status_for_session.get(), ShellStatus::Connected { .. })
                {
                    // Create session once
                    if session_info_clone.get() == "No session" {
                        let mut manager = SessionManager::new();

                        match manager.create_session().await {
                            Ok(session_id) => {
                                session_info_clone.set(format!("Session: {}", session_id));

                                // Subscribe to events
                                let mut rx = manager.subscribe_events();
                                let messages_display_clone = messages_clone;

                                spawn(async move {
                                    while let Some(event) = rx.recv().await {
                                        use session::{SessionEvent, Role};

                                        match event {
                                            SessionEvent::MessageAdded(msg) => {
                                                let role_name = match msg.role {
                                                    Role::User => "You",
                                                    Role::Assistant => "Assistant",
                                                };
                                                let timestamp = msg.timestamp.split('T')
                                                    .next()
                                                    .and_then(|d| d.split('-').next_back())
                                                    .unwrap_or("");

                                                let display = format!(
                                                    "[{}] {}: {}",
                                                    role_name,
                                                    timestamp,
                                                    msg.content
                                                );
                                                messages_display_clone.set(display);
                                            }
                                            SessionEvent::MessageDelta { delta } => {
                                                let current = messages_display_clone.get();
                                                messages_display_clone.set(format!("{}{}", current, delta));
                                            }
                                            SessionEvent::GenerationStarted => {
                                                messages_display_clone.set("Assistant is typing...".to_string());
                                            }
                                            SessionEvent::GenerationComplete => {
                                                // Final message will come via MessageAdded
                                            }
                                            SessionEvent::MessageComplete => {
                                                // Streaming complete
                                            }
                                        }
                                    }
                                });

                                // Demonstrate sending a message
                                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                                let _ = manager.send_message("Hello from Phase 3!".to_string()).await;
                            }
                            Err(e) => {
                                session_info_clone.set(format!("Session error: {}", e));
                            }
                        }
                    }

                    break; // Only try once
                }
            }
        });

        // Render
        text(move || {
            let conn_status = connection_status.get();
            let session = session_info.get();
            let messages = messages_display.get();

            match conn_status {
                ShellStatus::Connecting => {
                    "Connecting to Aleph server...".to_string()
                }
                ShellStatus::Connected { server_version, session_count } => {
                    format!(
                        "Connected: server v{}, {} sessions\n\n{}\n\nMessages:\n{}\n\n(Phase 3 Demo: Mock session auto-sends 'Hello from Phase 3!' after 2s)",
                        server_version,
                        session_count,
                        session,
                        messages
                    )
                }
                ShellStatus::Error(e) => {
                    format!("Error: {}", e)
                }
            }
        })
    }

    let root = Root::of::<App>();
    run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Aleph NSH Desktop".to_string(),
            size: (800, 600),
            app_id: "com.aleph.nsh-shell".to_string(),
            ..AppOptions::default()
        },
    )
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
