mod connection;
mod runtime;
mod supervisor;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::*;
    use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

    #[component]
    fn App() -> impl IntoElement {
        use connection::{spawn_connection_loop, ShellStatus};

        let status = signal(ShellStatus::Connecting);

        // Spawn connection loop on background thread
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ShellStatus>(8);
        let _handle = spawn_connection_loop(tx);

        // Update status signal from connection events
        spawn(async move {
            while let Some(new_status) = rx.recv().await {
                status.set(new_status);
            }
        });

        // Render status
        text(move || match status.get() {
            ShellStatus::Connecting => "Connecting to Aleph server...".to_string(),
            ShellStatus::Connected {
                server_version,
                session_count,
            } => {
                format!(
                    "Connected: server v{}, {} sessions",
                    server_version, session_count
                )
            }
            ShellStatus::Error(e) => format!("Error: {}", e),
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
