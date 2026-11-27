//! The root component: one main window hosting the canonical Panel. The nsh
//! runner mounts it as nsh.json's `"root"`; the command-line entry mounts the
//! same component after presetting its launch.

use crate::connect_form::{self, Switch, View};
use crate::deep_link::{deep_link_action, DeepLink};
#[cfg(feature = "embedded-core")]
use crate::local_service::{LocalService, ServerPhase};
use crate::shell_host::HostSettings;
use crate::update::{self, InstallStep, Updater};
use crate::{autostart, credentials, launch, menu, panel_bridge};
use aleph_client::ConnectionTarget;
use nsh_base::theme;
use nsh_core::desktop::Shortcut;
#[cfg(target_os = "macos")]
use nsh_core::element::desktop::set_application_menu;
use nsh_core::element::desktop::{
    main_window, quit, register_global_shortcut, restart, set_tray, tray, WindowRef,
};
use nsh_core::notifications::Notification;
use nsh_core::prelude::{
    background, blocking, cleanup, component, dynamic, element_ref, next_notification_press,
    next_url_scheme_open, open_url, px, request_notification_permission, show_notification, signal,
    spawn, text, view, web_view, AnyElement, Element, IntoElement, ParentElement, Styled,
    WebNavigation,
};
#[cfg(feature = "embedded-core")]
use nsh_core::prelude::{when, Interactive, Signal};
use nsh_updater::Available;
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Instant;

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
        view: signal(connect_form::initial_view(
            &launch.settings.target,
            launch.rejected_address.clone(),
        )),
        request: signal(0),
        operation: signal(false),
        home: std::env::home_dir(),
        #[cfg(feature = "embedded-core")]
        service: LocalService::bundled(),
    };
    let window = main_window();
    #[cfg(all(feature = "tsx-panel", target_os = "macos"))]
    if crate::panel_surface::registered() {
        let observed_window = window.clone();
        let last_visible = Cell::new(None);
        // The Dock can reopen the window without using the shell's show action.
        nsh_core::element::effect(move || {
            if let Some(state) = observed_window.state() {
                let visible = Some(state.visible);
                if last_visible.replace(visible) != visible {
                    crate::panel_surface::set_voice_visible(state.visible);
                }
            }
        });
    }
    let hide_window = window.clone();
    let hide = move || {
        #[cfg(all(feature = "tsx-panel", target_os = "macos"))]
        crate::panel_surface::set_voice_visible(false);
        hide_window.hide();
    };
    let close = hide.clone();
    window.on_close_press(move |_| close());
    let shortcut_window = window.clone();
    let bridge_window = window.clone();
    let show = move || {
        #[cfg(all(feature = "tsx-panel", target_os = "macos"))]
        crate::panel_surface::set_voice_visible(true);
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
    // A QA run checks the window it was given, not the update feed.
    let updates = launch
        .qa
        .is_none()
        .then(Updater::current)
        .flatten()
        .map(|updater| Updates::new(updater, switch.clone()));
    let open_in_browser = {
        let switch = switch.clone();
        move || {
            let opened = open_url(switch.current_target().panel_url());
            spawn(async move {
                if !opened.await {
                    eprintln!("could not open the Panel in the browser");
                }
            });
        }
    };
    let quit_and_stop = {
        let switch = switch.clone();
        let qa = launch.qa.is_some();
        move || quit_and_stop(&switch, qa)
    };
    let commands = menu::Commands {
        show: Rc::new(show.clone()),
        open_in_browser: Rc::new(open_in_browser),
        connect: Rc::new(connect),
        check_for_updates: updates
            .clone()
            .map(|updates| -> menu::Command { Rc::new(move || updates.check(true)) }),
        reload: Rc::new(panel_bridge::reload_current_panel),
        enable_notifications: Rc::new(|| {
            let permission = request_notification_permission();
            spawn(async move {
                eprintln!("NSH notification permission: {:?}", permission.await);
            });
        }),
        quit_and_stop: Rc::new(quit_and_stop),
    };
    set_tray(
        tray()
            .icon(nsh_core::prelude::asset!("aleph-tray.png"))
            .tooltip("Aleph")
            .menu(commands.tray()),
    );
    #[cfg(target_os = "macos")]
    set_application_menu(commands.menu_bar());
    #[cfg(all(target_os = "macos", feature = "embedded-core"))]
    if launch.qa.is_none() {
        crate::perm_monitor::start(switch.service.clone(), switch.home.clone());
    }
    listen_for_intents(show.clone(), updates.clone());
    if let Some(updates) = updates {
        updates.schedule();
    }
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
                hide();
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
    match switch.view.get_untracked() {
        View::Starting => {
            #[cfg(feature = "embedded-core")]
            switch.service.start();
        }
        View::Checking => switch.probe_startup(launch.settings.target.clone()),
        _ => {}
    }
    let shown = switch.view;
    view()
        .flex_1()
        .child(dynamic(move || match shown.get() {
            View::Starting => local(&switch, bridge_window.clone()),
            View::Checking => status_screen()
                .child(muted("正在连接服务器…"))
                .into_element(),
            View::Connect {
                address,
                error,
                back,
            } => connect_form::form(address, error, back.map(|view| *view), switch.clone()),
            View::Panel { target } => match HostSettings::for_target(target, None) {
                Ok(settings) => panel(
                    &launch::Launch {
                        settings,
                        qa: None,
                        rejected_address: None,
                    },
                    bridge_window.clone(),
                ),
                Err(error) => connect_form::form(String::new(), Some(error), None, switch.clone()),
            },
        }))
        .into_element()
}

/// Quits; when the window shows this computer's server, stops it first. A
/// QA run's server is not the bundled one, so a QA run only quits.
fn quit_and_stop(switch: &Switch, qa: bool) {
    if !switch.begin_operation() {
        return;
    }
    let switch = switch.clone();
    spawn(async move {
        #[cfg(feature = "embedded-core")]
        let stopped = if qa {
            Ok(())
        } else {
            switch.stop_local().await
        };
        #[cfg(feature = "embedded-core")]
        if let Err(error) = stopped {
            switch.operation.set(false);
            eprintln!("cannot quit and stop Aleph: {error}");
            notify("aleph.local-stop", "本机服务未停止".into(), error);
            return;
        }
        let _ = (switch, qa);
        #[cfg(all(feature = "tsx-panel", target_os = "macos"))]
        crate::panel_surface::set_voice_visible(false);
        quit();
    });
}

/// The links and notification presses the app receives. nsh holds one
/// waiter for each, so this is the app's only listener; it routes each to
/// the Panel shown now or to the updater.
fn listen_for_intents(show: impl Fn() + 'static, updates: Option<Updates>) {
    spawn(async move {
        let mut next_url = next_url_scheme_open();
        let mut next_press = next_notification_press();
        loop {
            tokio::select! {
                // nsh keeps the links that came before this listener, the
                // one that launched the app included.
                url = &mut next_url => {
                    if deep_link_action(&url) == Some(DeepLink::Open) {
                        show();
                        panel_bridge::open_chat_in_current_panel();
                    }
                    next_url = next_url_scheme_open();
                }
                press = &mut next_press => {
                    if press == panel_bridge::CHAT_NOTIFICATION {
                        show();
                        panel_bridge::open_chat_in_current_panel();
                    } else if press == update::NOTIFICATION {
                        if let Some(updates) = &updates {
                            updates.install();
                        }
                    }
                    next_press = next_notification_press();
                }
            }
        }
    });
}

/// The app's updates: a check 90 s after launch and every 6 h after, and on
/// demand from the tray. A notification offers what a check finds; pressing
/// it installs that version and restarts.
#[derive(Clone)]
struct Updates {
    updater: Updater,
    switch: Switch,
    /// The version the last notification offered.
    offered: Rc<RefCell<Option<Available>>>,
    installing: Rc<Cell<bool>>,
}

impl Updates {
    fn new(updater: Updater, switch: Switch) -> Self {
        Self {
            updater,
            switch,
            offered: Rc::default(),
            installing: Rc::default(),
        }
    }

    fn schedule(&self) {
        let updates = self.clone();
        let launched = Instant::now();
        spawn(async move {
            let mut last_check = None;
            loop {
                let delay = update::next_check_delay(launched.elapsed(), last_check);
                let _ = background(tokio::time::sleep(delay)).await;
                last_check = Some(launched.elapsed());
                updates.run_check(false).await;
            }
        });
    }

    /// Checks now. `asked`: the user asked, so say what came of it even when
    /// there is nothing new.
    fn check(&self, asked: bool) {
        let updates = self.clone();
        spawn(async move { updates.run_check(asked).await });
    }

    async fn run_check(&self, asked: bool) {
        let updater = self.updater.clone();
        let result = background(async move { updater.check().await })
            .await
            .unwrap_or_else(|_| Err("the update check stopped".into()));
        match result {
            Ok(Some(available)) => {
                let repeated = self
                    .offered
                    .borrow()
                    .as_ref()
                    .is_some_and(|offered| offered.version == available.version);
                if asked || !repeated {
                    notify(
                        update::NOTIFICATION,
                        format!("Aleph {} 可以更新", available.version),
                        "点按此通知以安装并重新启动 Aleph。".into(),
                    );
                }
                *self.offered.borrow_mut() = Some(available);
            }
            Ok(None) => {
                if asked {
                    notify(
                        update::NOTIFICATION,
                        "Aleph 已是最新版本".into(),
                        format!("当前版本 {}。", self.updater.version()),
                    );
                }
            }
            Err(error) => {
                eprintln!("update check failed: {error}");
                if asked {
                    notify(update::NOTIFICATION, "检查更新失败".into(), error);
                }
            }
        }
    }

    /// Installs the version offered last, then restarts. Pressing the
    /// notification again while it runs does nothing.
    fn install(&self) {
        let Some(available) = self.offered.borrow().clone() else {
            return;
        };
        if self.installing.replace(true) {
            return;
        }
        let updates = self.clone();
        spawn(async move {
            if let Err(error) = updates.run_install(available).await {
                eprintln!("update failed: {error}");
                notify(update::NOTIFICATION, "更新失败".into(), error);
            }
            updates.installing.set(false);
        });
    }

    /// Downloads and verifies while the app and its server keep running;
    /// only the swap itself waits for the server to stop. A failed swap
    /// leaves the installed app as it was and starts the server again.
    async fn run_install(&self, available: Available) -> Result<(), String> {
        notify(
            update::NOTIFICATION,
            format!("正在更新到 Aleph {}", available.version),
            "下载并校验完成后，Aleph 会重新启动。".into(),
        );
        let updater = self.updater.clone();
        let mut staged = Some(
            background(async move { updater.prepare(&available).await })
                .await
                .map_err(|_| "the update download stopped".to_owned())??,
        );
        if !self.switch.begin_operation() {
            return Err("连接正在切换，请稍后重试更新。".into());
        }
        let result = self.install_staged(&mut staged).await;
        self.switch.operation.set(false);
        result
    }

    async fn install_staged(
        &self,
        staged: &mut Option<nsh_updater::install::Staged>,
    ) -> Result<(), String> {
        let target = self.switch.current_target();
        for step in update::install_plan(&target) {
            match step {
                InstallStep::StopLocalServer => {
                    #[cfg(feature = "embedded-core")]
                    self.switch.stop_local().await?;
                }
                InstallStep::Install => {
                    let staged = staged.take().expect("one install step");
                    let finished = blocking(move || staged.finish())
                        .await
                        .map_err(|_| "the update swap stopped".to_owned())
                        .and_then(|result| result.map_err(|error| error.to_string()));
                    if let Err(error) = finished {
                        self.resume_local_server(&target);
                        return Err(error);
                    }
                }
                InstallStep::Restart => restart(),
            }
        }
        Ok(())
    }

    /// After a failed swap the installed app is unchanged, so its bundled
    /// server, stopped for the swap, starts again.
    fn resume_local_server(&self, target: &ConnectionTarget) {
        #[cfg(feature = "embedded-core")]
        if *target == ConnectionTarget::Local {
            self.switch.service.start();
        }
        #[cfg(not(feature = "embedded-core"))]
        let _ = target;
    }
}

/// Shows a system notification; one the system refuses is only logged.
fn notify(id: &str, title: String, body: String) {
    let shown = show_notification(Notification::new(id, title.clone(), body));
    spawn(async move {
        if !shown.await {
            eprintln!("notification not shown: {title}");
        }
    });
}

/// This computer's server: the bundled one once it is ready, or, in a build
/// without it, whatever already listens on the local port.
fn local(switch: &Switch, window: WindowRef) -> AnyElement {
    let settings = HostSettings::for_target(aleph_client::ConnectionTarget::Local, None)
        .expect("the local target is a loopback HTTP origin");
    let launch = launch::Launch {
        settings,
        qa: None,
        rejected_address: None,
    };
    #[cfg(feature = "embedded-core")]
    return local_panel(launch, switch.service.clone(), switch.operation, window);
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
fn local_panel(
    launch: launch::Launch,
    service: LocalService,
    operation: Signal<bool>,
    window: WindowRef,
) -> AnyElement {
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
                dynamic(move || local_status(phase.get(), &service, operation))
            }),
        )
        .into_element()
}

#[cfg(feature = "embedded-core")]
fn local_status(phase: ServerPhase, service: &LocalService, operation: Signal<bool>) -> AnyElement {
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
                .on_press(move || {
                    if !operation.get_untracked() {
                        retry.start();
                    }
                })
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
        launch.settings.target.clone(),
    );
    let retiring = bridge.clone();
    cleanup(move || retiring.retire());
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
