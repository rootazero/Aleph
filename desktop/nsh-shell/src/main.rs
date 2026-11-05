#[allow(dead_code)]
mod connection;
#[allow(dead_code)]
mod runtime;
mod supervisor;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::*;
    use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

    #[component]
    fn App() -> impl IntoElement {
        let status = signal("Connecting...".to_string());

        spawn(async move {
            match supervisor::ensure_ready().await {
                Ok(_) => {
                    status.set("Connected to server".to_string());
                }
                Err(e) => {
                    status.set(format!("Error: {e}"));
                }
            }
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
