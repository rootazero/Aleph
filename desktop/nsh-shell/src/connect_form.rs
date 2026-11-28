//! What the main window shows, and the native form for choosing the server:
//! an address field and the way back to this computer's own server. A remote
//! server must serve HTTPS with a certificate the system trusts, so the form
//! does not list the plain-HTTP addresses mDNS would find on the network.

use crate::gateway_probe::{probe_message, probe_target, Probe};
#[cfg(feature = "embedded-core")]
use crate::local_service::LocalService;
use crate::shell_host::HostSettings;
use crate::target_store;
use aleph_client::ConnectionTarget;
use nsh_base::{button, input, theme};
#[cfg(feature = "embedded-core")]
use nsh_core::prelude::blocking;
use nsh_core::prelude::{
    background, dynamic, px, signal, spawn, text, view, AnyElement, IntoElement, ParentElement,
    Signal, Styled,
};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum View {
    /// This computer's bundled server: its progress, then the Panel.
    Starting,
    /// Probing the remote server the app was started with.
    Checking,
    /// The connect form, with why the last attempt failed and the view to
    /// return to when the user cancels.
    Connect {
        address: String,
        error: Option<String>,
        back: Option<Box<View>>,
    },
    /// The Panel of a remote server that answered.
    Panel { target: ConnectionTarget },
}

/// Builds the initial view from the saved target, including a legacy HTTP target.
pub(crate) fn initial_view(target: &ConnectionTarget, rejected: Option<String>) -> View {
    if let Some(address) = rejected {
        return View::Connect {
            address,
            error: Some(HTTPS_REQUIRED.into()),
            back: None,
        };
    }
    match target {
        ConnectionTarget::Local => View::Starting,
        ConnectionTarget::Remote(_) => View::Checking,
    }
}

const HTTPS_REQUIRED: &str =
    "远程服务器必须使用 HTTPS。请填写带有系统信任证书的 HTTPS 地址，或选择使用本机服务。";

/// The view once `target` has been probed. An unreachable remote opens the
/// form with the reason, rather than a web view with a browser error page.
pub(crate) fn startup_view(target: &ConnectionTarget, probe: Probe) -> View {
    if *target == ConnectionTarget::Local {
        return View::Starting;
    }
    match probe_message(target, probe) {
        None => View::Panel {
            target: target.clone(),
        },
        Some(error) => View::Connect {
            address: target.panel_url(),
            error: Some(error),
            back: None,
        },
    }
}

/// Switches the window between servers: saves the choice, stops or starts
/// the bundled server, and sets the view.
#[derive(Clone)]
pub(crate) struct Switch {
    pub(crate) view: Signal<View>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) request: Signal<u64>,
    pub(crate) operation: Signal<bool>,
    #[cfg(feature = "embedded-core")]
    pub(crate) service: LocalService,
}

/// The server `view` shows or is starting; the form and the remote check
/// count as what Cancel goes back to, and as nothing when there is none.
fn shown_target(view: &View) -> Option<ConnectionTarget> {
    match view {
        View::Starting => Some(ConnectionTarget::Local),
        View::Panel { target } => Some(target.clone()),
        View::Connect {
            back: Some(back), ..
        } => shown_target(back),
        View::Checking | View::Connect { back: None, .. } => None,
    }
}

impl Switch {
    /// The server the window shows, or else the one saved as the choice.
    pub(crate) fn current_target(&self) -> ConnectionTarget {
        shown_target(&self.view.get_untracked()).unwrap_or_else(|| match &self.home {
            Some(home) => target_store::load_target(home),
            None => ConnectionTarget::Local,
        })
    }

    pub(crate) fn begin_operation(&self) -> bool {
        if self.operation.get_untracked() {
            return false;
        }
        self.operation.set(true);
        true
    }

    #[cfg(feature = "embedded-core")]
    pub(crate) async fn stop_local(&self) -> Result<(), String> {
        let service = self.service.clone();
        blocking(move || service.stop())
            .await
            .map_err(|error| format!("等待本机服务停止失败：{error}"))??;
        Ok(())
    }

    fn next_request(&self) -> u64 {
        let next = self.request.get_untracked().wrapping_add(1);
        self.request.set(next);
        next
    }

    pub(crate) fn open_form(&self) {
        let current = self.view.get_untracked();
        if self.operation.get_untracked() || matches!(current, View::Connect { .. }) {
            return;
        }
        self.next_request();
        let target = self.current_target();
        self.view.set(View::Connect {
            address: match target {
                ConnectionTarget::Local => String::new(),
                ConnectionTarget::Remote(_) => target.panel_url(),
            },
            error: None,
            back: Some(Box::new(current)),
        });
    }

    pub(crate) fn probe_startup(&self, target: ConnectionTarget) {
        let request = self.next_request();
        let switch = self.clone();
        spawn(async move {
            let probed = target.clone();
            let probe = background(async move { probe_target(&probed).await })
                .await
                .unwrap_or(Probe::Down);
            switch.complete_startup_probe(request, &target, probe);
        });
    }

    fn complete_startup_probe(&self, request: u64, target: &ConnectionTarget, probe: Probe) {
        if self.request.get_untracked() == request {
            self.view.try_set(startup_view(target, probe));
        }
    }

    fn use_local(&self) {
        if self.operation.get_untracked() {
            return;
        }
        self.next_request();
        self.save(&ConnectionTarget::Local);
        #[cfg(feature = "embedded-core")]
        self.service.start();
        self.view.set(View::Starting);
    }

    fn cancel(&self, back: View) {
        if self.operation.get_untracked() {
            return;
        }
        self.next_request();
        self.view.set(back.clone());
        if back == View::Checking {
            self.probe_startup(self.current_target());
        }
    }

    /// Connects to `raw` if a Gateway answers there; otherwise `problem`
    /// says why and nothing changes.
    fn connect(&self, raw: &str, problem: Signal<Option<String>>, busy: Signal<bool>) {
        if self.operation.get_untracked() {
            return;
        }
        let target = match ConnectionTarget::parse(raw.trim()) {
            Ok(target) => target,
            Err(_) => {
                problem.set(Some(
                    "地址无效。请输入服务器地址，例如 https://aleph.example.com。".into(),
                ));
                return;
            }
        };
        if target == ConnectionTarget::Local {
            self.use_local();
            return;
        }
        if HostSettings::for_target(target.clone(), None).is_err() {
            problem.set(Some(HTTPS_REQUIRED.into()));
            return;
        }
        let request = self.next_request();
        busy.set(true);
        problem.set(None);
        let switch = self.clone();
        spawn(async move {
            let probed = target.clone();
            let probe = background(async move { probe_target(&probed).await })
                .await
                .unwrap_or(Probe::Down);
            if switch.request.get_untracked() != request {
                return;
            }
            if let Some(error) = probe_message(&target, probe) {
                problem.try_set(Some(error));
                busy.try_set(false);
                return;
            }
            if !switch.begin_operation() {
                busy.try_set(false);
                return;
            }
            #[cfg(feature = "embedded-core")]
            if let Err(error) = switch.stop_local().await {
                switch.operation.set(false);
                problem.try_set(Some(error));
                busy.try_set(false);
                return;
            }
            switch.save(&target);
            switch.view.try_set(View::Panel { target });
            switch.operation.set(false);
        });
    }

    /// Saves the target for the next launch. A failure is logged and the
    /// switch still happens for this run.
    fn save(&self, target: &ConnectionTarget) {
        let Some(home) = &self.home else {
            eprintln!("no home directory: the connection target is not saved");
            return;
        };
        if let Err(error) = target_store::save_target(home, target) {
            eprintln!("could not save the connection target: {error}");
        }
    }
}

/// The connect form.
pub(crate) fn form(
    address: String,
    error: Option<String>,
    back: Option<View>,
    switch: Switch,
) -> AnyElement {
    let draft = signal(address);
    let problem = signal(error);
    let busy = signal(false);

    let submit = {
        let switch = switch.clone();
        move || {
            if !busy.get_untracked() {
                switch.connect(&draft.get_untracked(), problem, busy);
            }
        }
    };
    let local = switch.clone();
    let cancel = switch;

    let mut actions = view()
        .flex_row()
        .gap_2()
        .child(action("连接", true, submit.clone()))
        .child(action("使用本机服务", false, move || {
            local.use_local()
        }));
    if let Some(back) = back {
        actions = actions.child(action("取消", false, move || {
            cancel.cancel(back.clone());
        }));
    }

    view()
        .flex_1()
        .items_center()
        .justify_center()
        .p(px(32.0))
        .bg(move || theme().background)
        .child(
            view()
                .w(px(460.0))
                .gap_3()
                .child(text("连接到 Aleph 服务器").text_color(move || theme().foreground))
                .child(
                    input()
                        .value(move || draft.get())
                        .on_change(move |value: &str| draft.set(value.to_owned()))
                        .on_submit({
                            let submit = submit.clone();
                            move |_: &str| submit()
                        })
                        .placeholder("https://aleph.example.com")
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(move || theme().input)
                        .rounded(move || theme().radius)
                        .text_color(move || theme().foreground),
                )
                .child(actions)
                .child(dynamic(move || {
                    if busy.get() {
                        return text("正在连接…")
                            .text_color(move || theme().muted_foreground)
                            .into_element();
                    }
                    match problem.get() {
                        Some(message) => text(message)
                            .text_color(move || theme().destructive)
                            .into_element(),
                        None => view().into_element(),
                    }
                })),
        )
        .into_element()
}

fn action(label: &'static str, primary: bool, press: impl Fn() + 'static) -> AnyElement {
    button()
        .on_press(press)
        .px(px(16.0))
        .py(px(8.0))
        .rounded(move || theme().radius)
        .bg(move || {
            if primary {
                theme().primary
            } else {
                theme().muted
            }
        })
        .child(text(label).text_color(move || {
            if primary {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        }))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(raw: &str) -> ConnectionTarget {
        ConnectionTarget::parse(raw).unwrap()
    }

    #[test]
    fn a_local_target_starts_the_bundled_server() {
        assert_eq!(initial_view(&ConnectionTarget::Local, None), View::Starting);
        assert_eq!(
            startup_view(&ConnectionTarget::Local, Probe::Down),
            View::Starting
        );
    }

    #[test]
    fn a_remote_target_is_probed_before_the_panel() {
        let target = remote("https://gw.example.com");
        assert_eq!(initial_view(&target, None), View::Checking);
        assert_eq!(
            startup_view(&target, Probe::Ready),
            View::Panel {
                target: target.clone()
            }
        );
    }

    #[test]
    fn a_late_startup_probe_cannot_override_switching_to_local() {
        let runtime = nsh_core::element::reactive::Runtime::default();
        let _entered = runtime.enter();
        let switch = Switch {
            view: signal(View::Checking),
            request: signal(0),
            operation: signal(false),
            home: None,
            #[cfg(feature = "embedded-core")]
            service: crate::local_service::LocalService::new(
                Box::new(|| Ok(())),
                Box::new(|| Ok(false)),
            ),
        };
        let request = switch.next_request();
        switch.open_form();
        switch.use_local();
        switch.complete_startup_probe(request, &remote("https://gw.example.com"), Probe::Ready);
        assert_eq!(switch.view.get_untracked(), View::Starting);
    }

    #[test]
    fn an_unreachable_remote_opens_the_form_with_the_reason() {
        let target = remote("https://gw.example.com");
        let View::Connect {
            error: Some(error),
            back: None,
            ..
        } = startup_view(&target, Probe::Down)
        else {
            panic!("expected the connect form");
        };
        assert!(error.contains("https://gw.example.com:443"), "{error}");
        let View::Connect {
            error: Some(error), ..
        } = startup_view(&target, Probe::Untrusted)
        else {
            panic!("expected the connect form");
        };
        assert!(error.contains("trusted certificate"), "{error}");
    }

    #[test]
    fn the_shown_server_is_the_one_behind_the_form() {
        let target = remote("https://gw.example.com");
        assert_eq!(shown_target(&View::Starting), Some(ConnectionTarget::Local));
        let panel = View::Panel {
            target: target.clone(),
        };
        assert_eq!(shown_target(&panel), Some(target.clone()));
        let over_panel = View::Connect {
            address: target.panel_url(),
            error: None,
            back: Some(Box::new(panel)),
        };
        assert_eq!(shown_target(&over_panel), Some(target));
        // At startup, before any server shows, the saved choice decides.
        assert_eq!(shown_target(&View::Checking), None);
        let first = View::Connect {
            address: String::new(),
            error: Some("refused".into()),
            back: None,
        };
        assert_eq!(shown_target(&first), None);
    }
}
