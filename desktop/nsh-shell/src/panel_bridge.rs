use crate::credentials::CredentialService;
use aleph_protocol::panel_shell::{
    valid_document, valid_notification, CredentialCall, CredentialReply, PanelReply, ShellRequest,
};
use nsh_core::element::desktop::WindowRef;
use nsh_core::notifications::{Notification, NotificationPermission};
use nsh_core::prelude::{notification_permission, set_timeout, show_notification, spawn, ElementRef};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};

/// The id of the notifications the Panel asks for; pressing one opens the
/// chat.
pub(crate) const CHAT_NOTIFICATION: &str = "aleph-panel-chat";
const MESSAGE_LIMIT: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct Bridge(Rc<Inner>);

thread_local! {
    /// The Panel the window shows now: the last bridge started. Switching
    /// servers starts a new one, which takes its place here.
    static CURRENT: RefCell<Weak<Inner>> = const { RefCell::new(Weak::new()) };
    /// A chat asked for while no Panel showed (a link that launched the
    /// app, say, while the local server was starting): the next Panel
    /// opens it.
    static CHAT_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Opens the chat in the Panel the window shows, or in the next one when
/// none shows yet (the connect form, the local server's progress).
pub(crate) fn open_chat_in_current_panel() {
    match CURRENT.with(|current| current.borrow().upgrade()) {
        Some(inner) => Bridge(inner).open_chat(),
        None => CHAT_PENDING.set(true),
    }
}

struct Inner {
    view: ElementRef,
    prefix: String,
    state: RefCell<Page>,
    work: Notify,
    reply: Notify,
    notifications: mpsc::Sender<(String, String)>,
    credentials: CredentialService,
    autostart: crate::autostart::Reader,
}

#[derive(Default)]
struct Page {
    /// The web view is gone (the window shows another server or the
    /// connect form): the worker stops.
    retired: bool,
    loading: bool,
    needs_probe: bool,
    open_chat: bool,
    generation: u64,
    request: u32,
    document: Option<String>,
    exchange: Option<Exchange>,
}

struct Exchange {
    request: u32,
    target: Option<String>,
    result: Option<String>,
}

pub(crate) fn bootstrap(url: &str, prefix: &str) -> String {
    format!(
        "{}\ninstallAlephShellBridge({}, {});",
        include_str!("panel_bridge.js"),
        serde_json::json!(url.trim_end_matches('/')),
        serde_json::json!(prefix),
    )
}

impl Bridge {
    pub(crate) fn start(
        view: ElementRef,
        prefix: String,
        window: WindowRef,
        credentials: CredentialService,
        autostart: crate::autostart::Reader,
    ) -> Self {
        let (notifications, mut pending) = mpsc::channel::<(String, String)>(1);
        let bridge = Self(Rc::new(Inner {
            view,
            prefix,
            state: RefCell::new(Page::default()),
            work: Notify::new(),
            reply: Notify::new(),
            notifications,
            credentials,
            autostart,
        }));
        let worker = bridge.clone();
        spawn(async move { worker.navigate().await });

        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&bridge.0));
        if CHAT_PENDING.take() {
            bridge.open_chat();
        }
        spawn(async move {
            while let Some((title, body)) = pending.recv().await {
                if window
                    .state()
                    .is_some_and(|state| state.focused && state.visible)
                {
                    continue;
                }
                let permission = notification_permission().await;
                eprintln!("NSH notification permission: {permission:?}");
                if permission == NotificationPermission::Granted
                    && !window
                        .state()
                        .is_some_and(|state| state.focused && state.visible)
                {
                    let shown =
                        show_notification(Notification::new(CHAT_NOTIFICATION, title, body)).await;
                    eprintln!("NSH notification admitted by OS: {shown}");
                }
            }
        });
        bridge
    }

    pub(crate) fn loading(&self, loading: bool) {
        let mut state = self.0.state.borrow_mut();
        if state.loading == loading && (loading || state.generation != 0) {
            return;
        }
        self.0.credentials.invalidate();
        state.loading = loading;
        state.generation = state.generation.wrapping_add(1);
        state.document = None;
        state.exchange = None;
        state.needs_probe = !loading;
        self.0.reply.notify_one();
        self.0.work.notify_one();
    }

    pub(crate) fn receive(&self, raw: &str) -> bool {
        let Some(payload) = raw.strip_prefix(&self.0.prefix) else {
            return false;
        };
        if payload.len() > MESSAGE_LIMIT {
            return true;
        }
        let Ok(message) = serde_json::from_str::<PanelReply>(payload) else {
            return true;
        };
        let mut state = self.0.state.borrow_mut();
        if state.loading {
            return true;
        }
        let autostart = match &message {
            PanelReply::AutostartRequest { request, document } => {
                Some((*request, document.clone(), None))
            }
            PanelReply::AutostartSet {
                request,
                document,
                enabled,
            } => Some((*request, document.clone(), Some(*enabled))),
            _ => None,
        };
        if let Some((request, document, write)) = autostart {
            let generation = state.generation;
            let accepted =
                state.document.as_deref() == Some(document.as_str()) && valid_document(&document);
            drop(state);
            if accepted {
                let bridge = self.clone();
                spawn(async move {
                    let status = match write {
                        Some(enabled) => bridge.0.autostart.set(enabled).await,
                        None => bridge.0.autostart.query().await,
                    };
                    let page = bridge.0.state.borrow();
                    if !page.loading
                        && page.generation == generation
                        && page.document.as_deref() == Some(document.as_str())
                    {
                        let message = ShellRequest::AutostartReply {
                            request,
                            document,
                            status,
                        };
                        bridge.0.view.post_message(
                            serde_json::to_string(&message).expect("fixed status reply"),
                        );
                    }
                });
            }
            return true;
        }
        if let PanelReply::CredentialRequest {
            request,
            document,
            call,
        } = message
        {
            let generation = state.generation;
            let accepted =
                state.document.as_deref() == Some(document.as_str()) && valid_document(&document);
            drop(state);
            if accepted {
                self.invoke_credential(generation, request, document, call);
            }
            return true;
        }
        let (request, document, opened) = match message {
            PanelReply::Ready { request, document } => (request, document, false),
            PanelReply::ChatOpened { request, document } => (request, document, true),
            PanelReply::Notify {
                document,
                title,
                body,
            } => {
                if state.document.as_deref() == Some(document.as_str())
                    && valid_notification(&title, &body)
                {
                    if let Err(mpsc::error::TrySendError::Full(_)) =
                        self.0.notifications.try_send((title, body))
                    {
                        eprintln!("NSH notification queue full");
                    }
                }
                return true;
            }
            PanelReply::CredentialRequest { .. }
            | PanelReply::AutostartRequest { .. }
            | PanelReply::AutostartSet { .. } => unreachable!(),
        };
        if let Some(exchange) = &mut state.exchange {
            if exchange.request == request
                && valid_document(&document)
                && (if opened {
                    exchange.target.as_deref() == Some(document.as_str())
                } else {
                    exchange.target.is_none()
                })
            {
                exchange.result = Some(document);
                self.0.reply.notify_one();
            }
        }
        true
    }

    fn invoke_credential(
        &self,
        generation: u64,
        request: u32,
        document: String,
        call: CredentialCall,
    ) {
        let service = self.0.credentials.clone();
        let credential_generation = service.generation();
        let bridge = self.clone();
        spawn(async move {
            let reply = service.invoke(credential_generation, call).await;
            bridge.respond_credential(generation, request, document, reply);
        });
    }

    fn respond_credential(
        &self,
        generation: u64,
        request: u32,
        document: String,
        reply: CredentialReply,
    ) {
        let state = self.0.state.borrow();
        if state.loading
            || state.generation != generation
            || state.document.as_deref() != Some(document.as_str())
        {
            return;
        }
        let message = ShellRequest::CredentialReply {
            request,
            document,
            reply,
        };
        self.0
            .view
            .post_message(serde_json::to_string(&message).expect("fixed credential reply"));
    }

    /// Called when the web view goes: the bridge stops working and stops
    /// being the Panel links and notification presses go to.
    pub(crate) fn retire(&self) {
        self.0.state.borrow_mut().retired = true;
        self.0.work.notify_one();
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if current.as_ptr() == Rc::as_ptr(&self.0) {
                *current = Weak::new();
            }
        });
    }

    fn open_chat(&self) {
        self.0.state.borrow_mut().open_chat = true;
        self.0.work.notify_one();
    }

    async fn navigate(&self) {
        loop {
            self.0.work.notified().await;
            if self.0.state.borrow().retired {
                return;
            }
            loop {
                let (generation, document) = {
                    let mut state = self.0.state.borrow_mut();
                    if state.loading || (!state.needs_probe && !state.open_chat) {
                        break;
                    }
                    state.needs_probe = false;
                    (state.generation, state.document.clone())
                };
                if document.is_none() {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    while Instant::now() < deadline
                        && self.0.state.borrow().generation == generation
                    {
                        let result = self.exchange(None, Duration::from_millis(350)).await;
                        if let Some(document) = result {
                            let mut state = self.0.state.borrow_mut();
                            if state.generation != generation || state.loading {
                                break;
                            }
                            state.document = Some(document.clone());
                            drop(state);
                            self.0.view.post_message(
                                serde_json::to_string(&ShellRequest::CredentialsReady { document })
                                    .expect("fixed document readiness"),
                            );
                            break;
                        }
                    }
                    let mut state = self.0.state.borrow_mut();
                    if state.generation == generation && state.document.is_none() {
                        state.open_chat = false;
                        eprintln!("NSH Panel did not acknowledge the document probe");
                    }
                    continue;
                }
                if !std::mem::take(&mut self.0.state.borrow_mut().open_chat) {
                    continue;
                }
                if self
                    .exchange(document, Duration::from_secs(2))
                    .await
                    .is_none()
                {
                    eprintln!(
                        "NSH Panel did not confirm Chat navigation; not replaying the action"
                    );
                }
            }
        }
    }

    async fn exchange(&self, target: Option<String>, timeout: Duration) -> Option<String> {
        let message = {
            let mut state = self.0.state.borrow_mut();
            state.request = state.request.wrapping_add(1);
            let request = state.request;
            let message = match &target {
                Some(document) => ShellRequest::OpenChat {
                    request,
                    document: document.clone(),
                },
                None => ShellRequest::Probe { request },
            };
            state.exchange = Some(Exchange {
                request,
                target,
                result: None,
            });
            message
        };
        self.0
            .view
            .post_message(serde_json::to_string(&message).expect("fixed Panel message"));
        let mut timer = set_timeout(timeout, || ());
        let result = loop {
            {
                let mut state = self.0.state.borrow_mut();
                let Some(exchange) = state.exchange.as_mut() else {
                    break None;
                };
                if let Some(result) = exchange.result.take() {
                    state.exchange = None;
                    break Some(result);
                }
            }
            tokio::select! {
                _ = self.0.reply.notified() => {},
                _ = &mut timer => {
                    self.0.state.borrow_mut().exchange = None;
                    return None;
                }
            }
        };
        timer.cancel();
        result
    }
}
