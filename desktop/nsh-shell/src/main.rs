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
        use session::{Artifact, Message, Role, SessionEvent, SessionManager};
        use std::sync::Arc;

        let connection_status = signal(ShellStatus::Connecting);
        let session_info = signal(String::from("No session"));
        let messages = signal(Vec::<Message>::new());
        let artifacts = signal(Vec::<Artifact>::new());
        let queued = signal(None::<String>);
        let active_run = signal(None::<String>);
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

        let connection_for_setup = connection_status;
        let manager_for_setup = active_session;
        let messages_for_setup = messages;
        let artifacts_for_setup = artifacts;
        let stream_for_setup = streaming_id;
        let queued_for_setup = queued;
        let active_run_for_setup = active_run;
        let generating_for_setup = is_generating;
        let error_for_setup = send_error;

        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if !matches!(connection_for_setup.get(), ShellStatus::Connected { .. }) {
                    continue;
                }

                match SessionManager::connect().await {
                    Ok(mut manager) => {
                        let mut events = manager.subscribe_events();
                        manager_for_setup.set(Some(Arc::new(manager)));

                        spawn(async move {
                            while let Some(event) = events.recv().await {
                                match event {
                                    SessionEvent::SessionReady { session_id } => {
                                        session_info.set(format!("Session: {session_id}"));
                                    }
                                    SessionEvent::Queued { run_id, ahead } => {
                                        queued_for_setup.set(Some(format!(
                                            "Queued run {run_id} ({ahead} ahead)"
                                        )));
                                    }
                                    SessionEvent::RunStarted { run_id } => {
                                        queued_for_setup.set(None);
                                        active_run_for_setup.set(Some(run_id));
                                    }
                                    SessionEvent::HistoryLoaded(history) => {
                                        stream_for_setup.set(None);
                                        messages_for_setup.set(history);
                                    }
                                    SessionEvent::ArtifactsLoaded(items) => {
                                        artifacts_for_setup.set(items);
                                    }
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
                                        let stream_id = stream_for_setup
                                            .get()
                                            .unwrap_or_else(|| "streaming-active-run".to_string());
                                        stream_for_setup.set(Some(stream_id.clone()));
                                        let current = messages_for_setup.get();
                                        messages_for_setup.set(
                                            components::message_list::append_stream_delta(
                                                &current, &stream_id, "", &delta,
                                            ),
                                        );
                                    }
                                    SessionEvent::GenerationStarted => {
                                        generating_for_setup.set(true);
                                        let stream_id = "streaming-active-run".to_string();
                                        stream_for_setup.set(Some(stream_id.clone()));
                                        let current = messages_for_setup.get();
                                        messages_for_setup.set(
                                            components::message_list::append_stream_delta(
                                                &current, &stream_id, "", "",
                                            ),
                                        );
                                    }
                                    SessionEvent::MessageComplete => {}
                                    SessionEvent::GenerationComplete => {
                                        generating_for_setup.set(false);
                                        active_run_for_setup.set(None);
                                    }
                                    SessionEvent::Error(error) => {
                                        error_for_setup.set(Some(error));
                                        generating_for_setup.set(false);
                                    }
                                }
                            }
                            generating_for_setup.set(false);
                        });
                    }
                    Err(error) => {
                        error_for_setup.set(Some(error));
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
                        "Connected: server v{server_version}, {session_count} sessions\n{}\n{}\n{}",
                        session_info.get(),
                        queued.get().unwrap_or_else(|| "Queue: idle".to_string()),
                        active_run
                            .get()
                            .map(|run_id| format!("Run: {run_id}"))
                            .unwrap_or_else(|| "Run: idle".to_string())
                    ),
                    ShellStatus::Error(error) => format!("Connection error: {error}"),
                };
                match send_error.get() {
                    Some(error) => format!("{status}\n{error}"),
                    None => status,
                }
            }))
            .child(message_list(messages))
            .child(text(move || {
                let items = artifacts
                    .get()
                    .into_iter()
                    .map(|item| format!("{} ({} bytes)", item.filename, item.size))
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    "Artifacts: none".to_string()
                } else {
                    format!("Artifacts:\n{}", items.join("\n"))
                }
            }))
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
