//! The `aleph-nsh-shell` command: read-only status queries, the `--connect`
//! override and debug QA runs. It validates everything before the native
//! event loop, presets the launch, then runs the same root `App` the nsh
//! runner mounts in the packaged app.

use crate::app::{take_last_qa_event, App};
use crate::launch::{self, Launch, QaLaunch};
use crate::platform::{run_app, AppOptions, AppSource, Root};
use crate::shell_host::{self, APP_ID};
use crate::{autostart, ipc_status};
use nsh_core::desktop::MainWindowOptions;
use std::io::Write;
use std::process::ExitCode;

const HELP: &str = "Usage: aleph-nsh-shell [--connect <origin> | --autostart-status | --desktop-ipc-status <socket>]\n\nConnects to an existing Gateway; does not start, stop or reconfigure a service.\nUse HTTPS for a remote origin. No target or credential URL is persisted.\n--autostart-status reads the current app login item without changing it.\n--desktop-ipc-status reads health over Unix IPC from an already-running Gateway, without opening the Panel or falling back to WebSocket.";

pub fn run() -> ExitCode {
    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>()
    else {
        eprintln!("NSH host refused non-Unicode launch arguments");
        return ExitCode::FAILURE;
    };
    if args.as_slice() == ["--help"] {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == ipc_status::FLAG) {
        let qa = [
            "ALEPH_NSH_QA_URL",
            "ALEPH_NSH_QA_NONCE",
            "ALEPH_PANEL_QA_STATUS",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
        return ipc_status::run(&args, qa);
    }
    let qa = ["ALEPH_NSH_QA_URL", "ALEPH_NSH_QA_NONCE"].map(|name| {
        std::env::var_os(name)
            .map(|value| value.into_string())
            .transpose()
    });
    let [Ok(endpoint), Ok(nonce)] = qa else {
        eprintln!("NSH host refused non-Unicode QA configuration");
        return ExitCode::FAILURE;
    };
    if args.as_slice() == ["--autostart-status"] {
        if endpoint.is_some() {
            eprintln!("NSH host refused status query combined with QA endpoint");
            return ExitCode::FAILURE;
        }
        let identity = match autostart::expected_identity(nonce.as_deref()) {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("NSH host refused status query: {error}");
                return ExitCode::FAILURE;
            }
        };
        println!(
            "{}",
            serde_json::to_string(&autostart::read(&identity)).expect("closed status JSON")
        );
        return ExitCode::SUCCESS;
    }
    let status_qa = match std::env::var_os("ALEPH_PANEL_QA_STATUS") {
        None => false,
        Some(value)
            if value == "1"
                && cfg!(all(debug_assertions, feature = "qa-service"))
                && endpoint.is_some()
                && nonce.is_some() =>
        {
            true
        }
        Some(_) => {
            eprintln!("NSH host refused Panel status QA without its isolated debug context");
            return ExitCode::FAILURE;
        }
    };
    let settings =
        match shell_host::HostSettings::from_launch(&args, endpoint.as_deref(), nonce.as_deref()) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("NSH host refused configuration: {error}");
                return ExitCode::FAILURE;
            }
        };
    match std::env::var_os("ALEPH_SHELL_PANEL") {
        None => {}
        Some(value)
            if value == "tsx" && cfg!(feature = "tsx-panel") && settings.qa_nonce.is_none() =>
        {
            #[cfg(feature = "tsx-panel")]
            crate::panel_surface::register(crate::panel_host::Target::resolve(
                args.iter().any(|arg| arg == "--connect"),
                std::env::var_os(crate::panel_host::IPC_ENV),
            ));
        }
        Some(_) => {
            eprintln!("NSH host refused ALEPH_SHELL_PANEL: only \"tsx\", in a tsx-panel build, without QA");
            return ExitCode::FAILURE;
        }
    }
    let qa = settings.qa_nonce.is_some().then(|| QaLaunch {
        run_id: std::env::var("ALEPH_PANEL_QA_RUN_ID").unwrap_or_default(),
        message: std::env::var("ALEPH_PANEL_QA_MESSAGE").unwrap_or_default(),
        status: status_qa,
        status_labels: std::env::var("ALEPH_PANEL_QA_STATUS_LABELS").unwrap_or_default(),
        window_data_dir: cfg!(debug_assertions)
            .then(|| std::env::var_os("ALEPH_NSH_QA_WINDOW_DATA_DIR"))
            .flatten()
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir()),
    });
    let mut placement = MainWindowOptions::default();
    // Use the public hide command so NSH reports visibility to the shortcut.
    placement.attributes.close_on_press = Some(false);
    let window_data_dir = qa.as_ref().and_then(|qa| qa.window_data_dir.clone());
    placement.restore_bounds = qa.is_none() || window_data_dir.is_some();
    launch::preset(Launch {
        settings,
        qa,
        rejected_address: None,
    });
    let root = Root::of::<App>()
        .app_id(APP_ID)
        .url_schemes(["aleph"])
        .main_window(placement);
    #[cfg(all(target_os = "macos", debug_assertions))]
    crate::clock::start();
    let exit = run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Aleph".to_string(),
            size: (1180, 800),
            app_id: APP_ID.to_string(),
            #[cfg(target_os = "macos")]
            keep_alive_on_close: true,
            notifications: true,
            data_dir: window_data_dir,
            ..AppOptions::default()
        },
    );
    #[cfg(target_os = "linux")]
    let exit = ExitCode::from(exit);
    // run_app has returned: the owned native engine has stopped.
    if let Some(mut event) = take_last_qa_event() {
        event["kind"] = "closed".into();
        event["payload"] = serde_json::json!({"owned_drained": exit == ExitCode::SUCCESS});
        #[cfg(all(target_os = "macos", debug_assertions))]
        crate::clock::annotate(&mut event);
        println!("ALEPH_PANEL_QA {event}");
        let _ = std::io::stdout().flush();
    }
    exit
}
