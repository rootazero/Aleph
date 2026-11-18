mod shell_host;

#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../../../qa/phase9_clock.rs"]
mod clock;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::prelude::{IntoElement, Prop, Props, Styled, component, web_view};
    use nsh_platform_macos::{AppOptions, AppSource, Root, quit_app, run_app};
    use std::io::Write;
    use std::sync::Mutex;

    let endpoint = std::env::var("ALEPH_NSH_QA_URL").ok();
    let nonce = std::env::var("ALEPH_NSH_QA_NONCE").ok();
    let settings =
        match shell_host::HostSettings::from_inputs(endpoint.as_deref(), nonce.as_deref()) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("NSH host refused configuration: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
    let qa_run_id = std::env::var("ALEPH_PANEL_QA_RUN_ID").unwrap_or_default();
    let message = std::env::var("ALEPH_PANEL_QA_MESSAGE").unwrap_or_default();
    let script = settings
        .qa_nonce
        .as_ref()
        .map(|nonce| {
            format!(
            "{}\ninstallAlephPanelQa({}, event => window.nsh.postMessage(JSON.stringify(event)));",
            include_str!("../../../qa/nsh_panel_driver.js"),
            serde_json::json!({"nonce": nonce, "qa_run_id": qa_run_id, "message": message})
        )
        })
        .unwrap_or_default();
    // This single-window debug trace is not a Chat or Artifacts projection.
    static LAST_EVENT: Mutex<Option<serde_json::Value>> = Mutex::new(None);
    #[derive(Clone, PartialEq, Props)]
    struct AppProps {
        url: Prop<String>,
        script: Prop<String>,
        nonce: Prop<Option<String>>,
        qa_run_id: Prop<String>,
    }
    #[component]
    fn App(props: AppProps) -> impl IntoElement {
        web_view(props.url)
            .flex_1()
            .injected_java_script_before_content(props.script.get_untracked())
            .on_message(move |raw: &str| {
                let nonce = props.nonce.get_untracked();
                let Some(nonce) = nonce.as_deref() else {
                    return;
                };
                let Ok(mut event) = serde_json::from_str::<serde_json::Value>(raw) else {
                    return;
                };
                if event["nonce"].as_str() != Some(nonce)
                    || event["qa_run_id"].as_str() != Some(props.qa_run_id.get_untracked().as_str())
                {
                    return;
                }
                #[cfg(debug_assertions)]
                clock::annotate(&mut event);
                println!("ALEPH_PANEL_QA {event}");
                let _ = std::io::stdout().flush();
                let close = event["kind"].as_str() == Some("close-requested");
                *LAST_EVENT
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(event);
                if close {
                    quit_app();
                }
            })
    }
    let root = Root::new::<App>(move || AppProps {
        url: settings.url.clone().into(),
        script: script.clone().into(),
        nonce: settings.qa_nonce.clone().into(),
        qa_run_id: qa_run_id.clone().into(),
    });
    #[cfg(debug_assertions)]
    clock::start();
    let exit = run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Aleph".to_string(),
            size: (1180, 800),
            app_id: "com.aleph.nsh-shell".to_string(),
            ..AppOptions::default()
        },
    );
    // run_app has returned: the owned native engine has stopped.
    if let Some(mut event) = LAST_EVENT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
    {
        event["kind"] = "closed".into();
        event["payload"] =
            serde_json::json!({"owned_drained": exit == std::process::ExitCode::SUCCESS});
        #[cfg(debug_assertions)]
        clock::annotate(&mut event);
        println!("ALEPH_PANEL_QA {event}");
        let _ = std::io::stdout().flush();
    }
    exit
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("The NSH host is currently supported on macOS only");
    std::process::ExitCode::FAILURE
}
