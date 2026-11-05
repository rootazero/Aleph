#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::*;
    use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

    #[component]
    fn App() -> impl IntoElement {
        let status = signal("Connecting...".to_string());

        spawn(async move {
            // Placeholder for Phase 1: show connection status
            status.set("Connecting to server...".to_string());
            // Tasks 3-5 will wire in: supervisor → connection → status updates
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            status.set("Status: Connected (Phase 1 skeleton)".to_string());
        });

        text(move || status.get())
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
