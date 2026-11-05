mod connection;
mod runtime;
mod session;
mod supervisor;

#[cfg(any(target_os = "macos", test))]
mod components;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::*;
    use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

    #[component]
    fn App() -> impl IntoElement {
        use components::{input_field::input_field, message_list::message_list};
        use connection::{spawn_connection_loop, ShellStatus};
        use session::{Message, Role, SessionEvent, SessionManager};
        use std::sync::Arc;

        let connection_status = signal(ShellStatus::Connecting);
        let session_info = signal(String::from("No session"));
        let messages = signal(Vec::<Message>::new());
        let draft = signal(String::new());
        let is_generating = signal(false);
        let send_error = signal(None::<String>);
        let active_session = signal_with(None::<Arc<SessionManager>>, SignalEquals::Never);
        let streaming_id = signal(None::<String>);

        let (conn_tx, mut conn_rx) = tokio::sync::mpsc::channel::<ShellStatus>(8);
        let _handle = spawn_connection_loop(conn_tx);

        let connection_for_events = connection_status;
        spawn(async move {
            while let Some(new_status) = conn_rx.recv().await {
                connection_for_events.set(new_status);
            }
        });

        let session_info_for_setup = session_info;
        let connection_for_setup = connection_status;
        let manager_for_setup = active_session;
        let messages_for_setup = messages;
        let stream_for_setup = streaming_id;
        let generating_for_setup = is_generating;

        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if !matches!(connection_for_setup.get(), ShellStatus::Connected { .. }) {
                    continue;
                }

                if session_info_for_setup.get() == "No session" {
                    let mut manager = SessionManager::new();
                    match manager.create_session().await {
                        Ok(session_id) => {
                            session_info_for_setup.set(format!("Session: {session_id}"));
                            let mut events = manager.subscribe_events();
                            manager_for_setup.set(Some(Arc::new(manager)));

                            spawn(async move {
                                while let Some(event) = events.recv().await {
                                    match event {
                                        SessionEvent::MessageAdded(message) => {
                                            let current = messages_for_setup.get();
                                            if message.role == Role::Assistant {
                                                if let Some(stream_id) = stream_for_setup.get() {
                                                    messages_for_setup.set(
                                                        components::message_list::finish_stream(
                                                            &current, &stream_id, message,
                                                        ),
                                                    );
                                                    stream_for_setup.set(None);
                                                } else {
                                                    messages_for_setup
                                                        .set([current, vec![message]].concat());
                                                }
                                            } else {
                                                messages_for_setup
                                                    .set([current, vec![message]].concat());
                                            }
                                        }
                                        SessionEvent::MessageDelta { delta } => {
                                            let stream_id =
                                                stream_for_setup.get().unwrap_or_else(|| {
                                                    let id = format!(
                                                        "streaming-{}",
                                                        uuid::Uuid::new_v4()
                                                    );
                                                    stream_for_setup.set(Some(id.clone()));
                                                    id
                                                });
                                            let current = messages_for_setup.get();
                                            messages_for_setup.set(
                                                components::message_list::append_stream_delta(
                                                    &current,
                                                    &stream_id,
                                                    &chrono::Utc::now().to_rfc3339(),
                                                    &delta,
                                                ),
                                            );
                                        }
                                        SessionEvent::GenerationStarted => {
                                            generating_for_setup.set(true);
                                            let stream_id =
                                                format!("streaming-{}", uuid::Uuid::new_v4());
                                            stream_for_setup.set(Some(stream_id.clone()));
                                            let current = messages_for_setup.get();
                                            messages_for_setup.set(
                                                components::message_list::append_stream_delta(
                                                    &current,
                                                    &stream_id,
                                                    &chrono::Utc::now().to_rfc3339(),
                                                    "",
                                                ),
                                            );
                                        }
                                        SessionEvent::MessageComplete => {}
                                        SessionEvent::GenerationComplete => {
                                            generating_for_setup.set(false);
                                        }
                                    }
                                }
                                generating_for_setup.set(false);
                            });
                        }
                        Err(error) => {
                            session_info_for_setup.set(format!("Session error: {error}"));
                        }
                    }
                }

                break;
            }
        });

        let draft_for_change = draft;
        let draft_for_submit = draft;
        let manager_for_submit = active_session;
        let generating_for_submit = is_generating;
        let error_for_submit = send_error;
        let submit_message = move || {
            if generating_for_submit.get() {
                return;
            }
            let content = draft_for_submit.get();
            if content.trim().is_empty() {
                return;
            }
            let Some(manager) = manager_for_submit.get() else {
                error_for_submit.set(Some("No active session".to_string()));
                return;
            };

            draft_for_submit.set(String::new());
            error_for_submit.set(None);
            generating_for_submit.set(true);
            spawn(async move {
                if let Err(error) = manager.send_message(content).await {
                    error_for_submit.set(Some(error));
                    generating_for_submit.set(false);
                }
            });
        };

        view()
            .child(text(move || {
                let status = match connection_status.get() {
                    ShellStatus::Connecting => "Connecting to Aleph server...".to_string(),
                    ShellStatus::Connected {
                        server_version,
                        session_count,
                    } => format!(
                        "Connected: server v{server_version}, {session_count} sessions\n{}",
                        session_info.get()
                    ),
                    ShellStatus::Error(error) => format!("Connection error: {error}"),
                };
                match send_error.get() {
                    Some(error) => format!("{status}\n{error}"),
                    None => status,
                }
            }))
            .child(message_list(messages))
            .child(input_field(
                draft,
                move |value| draft_for_change.set(value),
                submit_message,
                is_generating,
            ))
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
