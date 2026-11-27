//! The root component: one main window hosting the canonical Panel. The nsh
//! runner mounts it as nsh.json's `"root"`; the command-line entry mounts the
//! same component after presetting its launch.

use crate::connect_form::{self, Switch, View};
#[cfg(feature = "embedded-core")]
use crate::local_service::{LocalService, ServerPhase};
use crate::shell_host::HostSettings;
use crate::{autostart, credentials, launch, panel_bridge};
use nsh_base::theme;
use nsh_core::desktop::{Shortcut, TrayTitleRun};
use nsh_core::element::desktop::{
    main_window, quit, register_global_shortcut, set_tray, tray, MenuItem, WindowRef,
};
use nsh_core::prelude::{
    component, dynamic, element_ref, px, request_notification_permission, signal, spawn, text,
    view, web_view, AnyElement, Element, IntoElement, ParentElement, Styled, WebNavigation,
};
#[cfg(feature = "embedded-core")]
use nsh_core::prelude::{when, Interactive};
use std::io::Write;
use std::sync::Mutex;

// This single-window debug trace is not a Chat or Artifacts projection.
static LAST_QA_EVENT: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// The last QA event the Panel reported, taken once the app has stopped.
pub(crate) fn take_last_qa_event() -> Option<serde_json::Value> {
    LAST_QA_EVENT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
}

/// The script injected before the Panel's content: the shell bridge, then
/// the QA driver in a QA run.
fn injected_script(launch: &launch::Launch, bridge_prefix: &str) -> String {
    let bootstrap = panel_bridge::bootstrap(&launch.settings.url, bridge_prefix);
    let (Some(qa), Some(nonce)) = (&launch.qa, &launch.settings.qa_nonce) else {
        return bootstrap;
    };
    let driver = if qa.status {
        concat!(
            include_str!("../../../qa/phase11_nsh_panel_status.js"),
            "\n",
            include_str!("../../../qa/phase11_nsh_pairing.js")
        )
    } else {
        include_str!("../../../qa/nsh_panel_driver.js")
    };
    let options = serde_json::json!({"nonce": nonce, "qa_run_id": qa.run_id,
        "message": qa.message, "status_labels": qa.status_labels});
    format!(
        "{bootstrap}\n{driver}\ninstallAlephPanelQa({options}, event => window.nsh.postMessage(JSON.stringify(event)));"
    )
}

#[component]
pub fn App() -> impl IntoElement {
    let launch = launch::current();
    let switch = Switch {
        view: signal(connect_form::initial_view(&launch.settings.target)),
        home: std::env::home_dir(),
        #[cfg(feature = "embedded-core")]
        service: LocalService::bundled(),
    };
    let window = main_window();
    let close_window = window.clone();
    window.on_close_press(move |_| close_window.hide());
    let shortcut_window = window.clone();
    let bridge_window = window.clone();
    let show = move || {
        window.show();
        window.focus();
    };
    let connect = {
        let switch = switch.clone();
        let show = show.clone();
        move || {
            switch.open_form();
            show();
        }
    };
    set_tray(
        tray()
            .title([TrayTitleRun::new("Aleph")])
            .tooltip("Aleph")
            .menu([
                MenuItem::new("Show Aleph").on_select(show.clone()),
                MenuItem::new("Connect to Server…").on_select(connect),
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
    #[cfg(feature = "tsx-panel")]
    if crate::panel_surface::registered() {
        return nsh_core::prelude::native(crate::panel_surface::SURFACE)
            .flex_1()
            .into_element();
    }
    // A QA run shows its isolated endpoint as it is: no probe, no form.
    if launch.qa.is_some() {
        return panel(&launch, bridge_window);
    }
    if let target @ aleph_client::ConnectionTarget::Remote(_) = &launch.settings.target {
        switch.probe_startup(target.clone());
    }
    let shown = switch.view;
    view()
        .flex_1()
        .child(dynamic(move || match shown.get() {
            View::Starting => local(&switch, bridge_window.clone()),
            View::Checking => status_screen()
                .child(muted("正在连接服务器…"))
                .into_element(),
            View::Connect { error, back } => {
                connect_form::form(error, back.map(|view| *view), switch.clone())
            }
            View::Panel { target } => match HostSettings::for_target(target, None) {
                Ok(settings) => panel(
                    &launch::Launch { settings, qa: None },
                    bridge_window.clone(),
                ),
                Err(error) => connect_form::form(Some(error), None, switch.clone()),
            },
        }))
        .into_element()
}

/// This computer's server: the bundled one once it is ready, or, in a build
/// without it, whatever already listens on the local port.
fn local(switch: &Switch, window: WindowRef) -> AnyElement {
    let settings = HostSettings::for_target(aleph_client::ConnectionTarget::Local, None)
        .expect("the local target is a loopback HTTP origin");
    let launch = launch::Launch { settings, qa: None };
    #[cfg(feature = "embedded-core")]
    return local_panel(launch, switch.service.clone(), window);
    #[cfg(not(feature = "embedded-core"))]
    {
        let _ = switch;
        panel(&launch, window)
    }
}

fn status_screen() -> nsh_core::prelude::View {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p(px(32.0))
        .bg(move || theme().background)
}

fn muted(message: impl Into<String>) -> impl IntoElement {
    text(message.into()).text_color(move || theme().muted_foreground)
}

/// The Panel once the bundled server is ready; until then, its progress or
/// its failure with a retry.
#[cfg(feature = "embedded-core")]
fn local_panel(launch: launch::Launch, service: LocalService, window: WindowRef) -> AnyElement {
    // Start first, so the phase read below is this attempt's, not the
    // Ready left over from before a switch to a remote server.
    service.start();
    let phase = signal(service.phase());
    let mut changes = service.subscribe();
    spawn(async move {
        loop {
            // The view that owns `phase` is gone after a switch away.
            if !phase.try_set(changes.borrow_and_update().clone()) {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    });
    view()
        .flex_1()
        .child(
            when(
                move || phase.get() == ServerPhase::Ready,
                move || panel(&launch, window.clone()),
            )
            .otherwise(move || {
                let service = service.clone();
                dynamic(move || local_status(phase.get(), &service))
            }),
        )
        .into_element()
}

#[cfg(feature = "embedded-core")]
fn local_status(phase: ServerPhase, service: &LocalService) -> AnyElement {
    let screen = status_screen();
    let ServerPhase::Failed(error) = phase else {
        return screen.child(muted("正在启动本地服务…")).into_element();
    };
    let retry = service.clone();
    screen
        .child(text("本地服务启动失败").text_color(move || theme().foreground))
        .child(muted(error))
        .child(
            view()
                .px(px(20.0))
                .py(px(8.0))
                .rounded(move || theme().radius)
                .bg(move || theme().primary)
                .on_press(move || retry.start())
                .child(text("重试").text_color(move || theme().primary_foreground)),
        )
        .into_element()
}

/// The canonical Panel in a web view, with the shell bridge and, in a QA
/// run, the QA driver.
fn panel(launch: &launch::Launch, window: WindowRef) -> AnyElement {
    let bridge_prefix = format!("aleph-shell-v1:{}:", uuid::Uuid::new_v4().simple());
    let script = injected_script(launch, &bridge_prefix);
    let nonce = launch.settings.qa_nonce.clone();
    let qa_run_id = launch
        .qa
        .as_ref()
        .map(|qa| qa.run_id.clone())
        .unwrap_or_default();
    let view = element_ref();
    let credentials =
        credentials::CredentialService::new(&launch.settings.gateway, nonce.as_deref());
    let bridge = panel_bridge::Bridge::start(
        view,
        bridge_prefix,
        window,
        credentials,
        autostart::Reader::new(nonce.as_deref()),
    );
    let navigation = bridge.clone();
    let terminated = bridge.clone();
    web_view(launch.settings.url.clone())
        .flex_1()
        .element_ref(&view)
        .injected_java_script_before_content(script)
        .on_navigation_state_change(move |state: &WebNavigation| navigation.loading(state.loading))
        .on_process_terminated(move |_: &str| terminated.loading(true))
        .on_message(move |raw: &str| {
            if bridge.receive(raw) {
                return;
            }
            let Some(nonce) = nonce.as_deref() else {
                return;
            };
            let Ok(event) = serde_json::from_str::<serde_json::Value>(raw) else {
                return;
            };
            if event["nonce"].as_str() != Some(nonce)
                || event["qa_run_id"].as_str() != Some(qa_run_id.as_str())
            {
                return;
            }
            #[cfg(all(target_os = "macos", debug_assertions))]
            let event = {
                let mut event = event;
                crate::clock::annotate(&mut event);
                event
            };
            println!("ALEPH_PANEL_QA {event}");
            let _ = std::io::stdout().flush();
            let close = event["kind"].as_str() == Some("close-requested");
            *LAST_QA_EVENT
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(event);
            if close {
                quit();
            }
        })
        .into_element()
}
