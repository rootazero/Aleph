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
        error: Option<String>,
        back: Option<Box<View>>,
    },
    /// The Panel of a remote server that answered.
    Panel { target: ConnectionTarget },
}

/// The view before any probe: a remote target is probed first.
pub(crate) fn initial_view(target: &ConnectionTarget) -> View {
    match target {
        ConnectionTarget::Local => View::Starting,
        ConnectionTarget::Remote(_) => View::Checking,
    }
}

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
    #[cfg(feature = "embedded-core")]
    pub(crate) service: LocalService,
}

impl Switch {
    /// Opens the form over whatever is shown, remembering it for Cancel.
    pub(crate) fn open_form(&self) {
        let current = self.view.get_untracked();
        if matches!(current, View::Connect { .. }) {
            return;
        }
        self.view.set(View::Connect {
            error: None,
            back: Some(Box::new(current)),
        });
    }

    /// Probes the remote server the app was started with.
    pub(crate) fn probe_startup(&self, target: ConnectionTarget) {
        let view = self.view;
        spawn(async move {
            let probed = target.clone();
            let probe = background(async move { probe_target(&probed).await })
                .await
                .unwrap_or(Probe::Down);
            view.try_set(startup_view(&target, probe));
        });
    }

    fn use_local(&self) {
        self.save(&ConnectionTarget::Local);
        self.view.set(View::Starting);
    }

    /// Connects to `raw` if a Gateway answers there; otherwise `problem`
    /// says why and nothing changes.
    fn connect(&self, raw: &str, problem: Signal<Option<String>>, busy: Signal<bool>) {
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
            problem.set(Some(
                "远程服务器必须使用 HTTPS：只有本机地址可以用 HTTP。\
                 局域网中的服务器请通过带受信任证书的 HTTPS 地址访问。"
                    .into(),
            ));
            return;
        }
        busy.set(true);
        problem.set(None);
        let switch = self.clone();
        spawn(async move {
            let probed = target.clone();
            let probe = background(async move { probe_target(&probed).await })
                .await
                .unwrap_or(Probe::Down);
            if let Some(error) = probe_message(&target, probe) {
                problem.try_set(Some(error));
                busy.try_set(false);
                return;
            }
            switch.save(&target);
            // The bundled server is not needed while a remote one serves
            // the Panel; it starts again when the user switches back.
            #[cfg(feature = "embedded-core")]
            {
                let service = switch.service.clone();
                let _ = blocking(move || service.stop()).await;
            }
            switch.view.try_set(View::Panel { target });
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
pub(crate) fn form(error: Option<String>, back: Option<View>, switch: Switch) -> AnyElement {
    let draft = signal(String::new());
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
            cancel.view.set(back.clone());
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
        assert_eq!(initial_view(&ConnectionTarget::Local), View::Starting);
        assert_eq!(
            startup_view(&ConnectionTarget::Local, Probe::Down),
            View::Starting
        );
    }

    #[test]
    fn a_remote_target_is_probed_before_the_panel() {
        let target = remote("https://gw.example.com");
        assert_eq!(initial_view(&target), View::Checking);
        assert_eq!(
            startup_view(&target, Probe::Ready),
            View::Panel {
                target: target.clone()
            }
        );
    }

    #[test]
    fn an_unreachable_remote_opens_the_form_with_the_reason() {
        let target = remote("https://gw.example.com");
        let View::Connect {
            error: Some(error),
            back: None,
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
}
