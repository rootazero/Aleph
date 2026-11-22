mod autostart;
mod credentials;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod panel_bridge;
mod platform;
mod shell_host;

#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../../../qa/phase9_clock.rs"]
mod clock;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    use crate::platform::{run_app, AppOptions, AppSource, Root};
    use nsh_core::desktop::{MainWindowOptions, Shortcut, TrayTitleRun};
    use nsh_core::element::desktop::{
        main_window, quit, register_global_shortcut, set_tray, tray, MenuItem,
    };
    use nsh_core::prelude::{
        component, element_ref, request_notification_permission, spawn, web_view, Element,
        IntoElement, Prop, Props, Styled, WebNavigation,
    };
    use std::io::Write;
    use std::sync::Mutex;

    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>()
    else {
        eprintln!("NSH host refused non-Unicode launch arguments");
        return std::process::ExitCode::FAILURE;
    };
    if args.as_slice() == ["--help"] {
        println!("Usage: aleph-nsh-shell [--connect <origin> | --autostart-status]\n\nConnects to an existing Gateway; does not start, stop or reconfigure a service.\nUse HTTPS for a remote origin. No target or credential URL is persisted.\n--autostart-status reads the current app login item without changing it.");
        return std::process::ExitCode::SUCCESS;
    }
    let qa = ["ALEPH_NSH_QA_URL", "ALEPH_NSH_QA_NONCE"].map(|name| {
        std::env::var_os(name)
            .map(|value| value.into_string())
            .transpose()
    });
    let [Ok(endpoint), Ok(nonce)] = qa else {
        eprintln!("NSH host refused non-Unicode QA configuration");
        return std::process::ExitCode::FAILURE;
    };
    if args.as_slice() == ["--autostart-status"] {
        if endpoint.is_some() {
            eprintln!("NSH host refused status query combined with QA endpoint");
            return std::process::ExitCode::FAILURE;
        }
        let identity = match autostart::expected_identity(nonce.as_deref()) {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("NSH host refused status query: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
        println!(
            "{}",
            serde_json::to_string(&autostart::read(&identity)).expect("closed status JSON")
        );
        return std::process::ExitCode::SUCCESS;
    }
    let settings =
        match shell_host::HostSettings::from_launch(&args, endpoint.as_deref(), nonce.as_deref()) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("NSH host refused configuration: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
    let qa_run_id = std::env::var("ALEPH_PANEL_QA_RUN_ID").unwrap_or_default();
    let message = std::env::var("ALEPH_PANEL_QA_MESSAGE").unwrap_or_default();
    let bridge_prefix = format!("aleph-shell-v1:{}:", uuid::Uuid::new_v4().simple());
    let bootstrap = panel_bridge::bootstrap(&settings.url, &bridge_prefix);
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
    let script = format!("{bootstrap}\n{script}");
    // This single-window debug trace is not a Chat or Artifacts projection.
    static LAST_EVENT: Mutex<Option<serde_json::Value>> = Mutex::new(None);
    use shell_host::APP_ID;

    #[derive(Clone, PartialEq, Props)]
    struct AppProps {
        url: Prop<String>,
        gateway: Prop<String>,
        script: Prop<String>,
        bridge_prefix: Prop<String>,
        nonce: Prop<Option<String>>,
        qa_run_id: Prop<String>,
    }
    #[component]
    fn App(props: AppProps) -> impl IntoElement {
        let window = main_window();
        let close_window = window.clone();
        window.on_close_press(move |_| close_window.hide());
        let view = element_ref();
        let credentials = credentials::CredentialService::new(
            &props.gateway.get_untracked(),
            props.nonce.get_untracked().as_deref(),
        );
        let bridge = panel_bridge::Bridge::start(
            view,
            props.bridge_prefix.get_untracked(),
            window.clone(),
            credentials,
            autostart::Reader::new(props.nonce.get_untracked().as_deref()),
        );
        let navigation = bridge.clone();
        let terminated = bridge.clone();
        let shortcut_window = window.clone();
        let show = move || {
            window.show();
            window.focus();
        };
        set_tray(
            tray()
                .title([TrayTitleRun::new("Aleph")])
                .tooltip("Aleph")
                .menu([
                    MenuItem::new("Show Aleph").on_select(show.clone()),
                    MenuItem::new("Enable Notifications").on_select(|| {
                        let permission = request_notification_permission();
                        spawn(async move {
                            eprintln!("NSH notification permission: {:?}", permission.await);
                        });
                    }),
                    MenuItem::new("Quit Aleph").on_select(quit),
                ]),
        );
        let hotkey = std::env::var("ALEPH_SHELL_HOTKEY").unwrap_or_else(|_| "mod+shift+a".into());
        if hotkey == "disabled" {
            eprintln!("NSH summon hotkey disabled");
        } else if let Err(error) = Shortcut::parse(&hotkey, true) {
            eprintln!("NSH summon hotkey invalid: {error}");
        } else {
            let registration = register_global_shortcut(&hotkey, move || {
                if shortcut_window
                    .state()
                    .is_some_and(|state| state.visible && state.focused)
                {
                    shortcut_window.hide();
                } else {
                    show();
                }
            });
            spawn(async move {
                if registration.await {
                    eprintln!("NSH summon hotkey registered");
                } else {
                    eprintln!("NSH summon hotkey refused");
                }
            });
        }
        web_view(props.url)
            .flex_1()
            .element_ref(&view)
            .injected_java_script_before_content(props.script.get_untracked())
            .on_navigation_state_change(move |state: &WebNavigation| {
                navigation.loading(state.loading)
            })
            .on_process_terminated(move |_: &str| terminated.loading(true))
            .on_message(move |raw: &str| {
                if bridge.receive(raw) {
                    return;
                }
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
                #[cfg(all(target_os = "macos", debug_assertions))]
                clock::annotate(&mut event);
                println!("ALEPH_PANEL_QA {event}");
                let _ = std::io::stdout().flush();
                let close = event["kind"].as_str() == Some("close-requested");
                *LAST_EVENT
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(event);
                if close {
                    quit();
                }
            })
    }
    let mut placement = MainWindowOptions::default();
    // Use the public hide command so NSH reports visibility to the shortcut.
    placement.attributes.close_on_press = Some(false);
    let placement_data = if cfg!(debug_assertions) && settings.qa_nonce.is_some() {
        std::env::var_os("ALEPH_NSH_QA_WINDOW_DATA_DIR")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir())
    } else {
        None
    };
    placement.restore_bounds = settings.qa_nonce.is_none() || placement_data.is_some();
    let root = Root::new::<App>(move || AppProps {
        url: settings.url.clone().into(),
        gateway: settings.gateway.clone().into(),
        script: script.clone().into(),
        bridge_prefix: bridge_prefix.clone().into(),
        nonce: settings.qa_nonce.clone().into(),
        qa_run_id: qa_run_id.clone().into(),
    })
    .app_id(APP_ID)
    .url_schemes(["aleph"])
    .main_window(placement);
    #[cfg(all(target_os = "macos", debug_assertions))]
    clock::start();
    let exit = run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Aleph".to_string(),
            size: (1180, 800),
            app_id: APP_ID.to_string(),
            #[cfg(target_os = "macos")]
            keep_alive_on_close: true,
            notifications: true,
            data_dir: placement_data,
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
        #[cfg(all(target_os = "macos", debug_assertions))]
        clock::annotate(&mut event);
        println!("ALEPH_PANEL_QA {event}");
        let _ = std::io::stdout().flush();
    }
    exit
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() -> std::process::ExitCode {
    eprintln!("The NSH host is unsupported on this target");
    std::process::ExitCode::FAILURE
}
