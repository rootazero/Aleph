fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        use nsh_core::prelude::*;
        use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

        #[component]
        fn App() -> impl IntoElement {
            text("NSH Spike: compiles OK")
        }

        let root = Root::of::<App>();
        run_app(
            AppSource::Rust(root),
            AppOptions {
                title: "Aleph NSH Spike".to_string(),
                size: (800, 600),
                app_id: "com.aleph.nsh-spike".to_string(),
                ..AppOptions::default()
            },
        )
    }
    #[cfg(not(target_os = "macos"))]
    std::process::ExitCode::SUCCESS
}
