use crate::components::mode_sidebar::PanelMode;
use crate::context::DashboardState;
use aleph_protocol::panel_shell::{PanelReply, ShellRequest, valid_document, valid_notification};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use wasm_bindgen::{JsCast, JsValue};

const BRIDGE: &str = "__alephShellBridge";
const NOTIFICATION_TOPIC: &str = "surface.notify";

#[component]
pub(crate) fn ShellBridge() -> impl IntoView {
    let window = leptos::prelude::window();
    let Ok(bridge) = js_sys::Reflect::get(&window, &BRIDGE.into()) else {
        return;
    };
    if bridge.is_undefined() {
        return;
    }
    let Some(document) = js_sys::Reflect::get(&bridge, &"document".into())
        .ok()
        .and_then(|value| value.as_string())
        .filter(|document| valid_document(document))
    else {
        return;
    };

    let chat_path = PanelMode::Chat.path();
    let location = use_location();
    let pathname = location.pathname;
    let navigate = use_navigate();
    let pending_chat = RwSignal::new(None::<u32>);
    let ack_document = document.clone();
    Effect::new(move |_| {
        if pathname.get() == chat_path {
            if let Some(request) = pending_chat.get() {
                pending_chat.set(None);
                send(PanelReply::ChatOpened {
                    request,
                    document: ack_document.clone(),
                });
            }
        }
    });
    let message_document = document.clone();
    let listener = window_event_listener_untyped("message", move |event| {
        let Ok(event) = event.dyn_into::<web_sys::MessageEvent>() else {
            return;
        };
        if event.source().is_some() || !event.origin().is_empty() {
            return;
        }
        let Some(raw) = event.data().as_string().filter(|raw| raw.len() <= 256) else {
            return;
        };
        match serde_json::from_str::<ShellRequest>(&raw) {
            Ok(ShellRequest::Probe { request }) => {
                send(PanelReply::Ready {
                    request,
                    document: message_document.clone(),
                });
            }
            Ok(ShellRequest::OpenChat { request, document }) if document == message_document => {
                pending_chat.set(Some(request));
                if pathname.get_untracked() != chat_path {
                    navigate(chat_path, Default::default());
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || listener.remove());

    let state = expect_context::<DashboardState>();
    let subscription = state.subscribe_events(move |event| {
        if event.topic != NOTIFICATION_TOPIC {
            return;
        }
        if let (Some(title), Some(body)) =
            (event.data["title"].as_str(), event.data["body"].as_str())
        {
            if valid_notification(title, body) {
                send(PanelReply::Notify {
                    document: document.clone(),
                    title: title.to_owned(),
                    body: body.to_owned(),
                });
            }
        }
    });
    on_cleanup(move || state.unsubscribe_events(subscription));
    // This listener has the same lifetime as the document's Gateway connection.
    spawn_local(async move {
        if state.subscribe_topic(NOTIFICATION_TOPIC).await.is_err() {
            leptos::logging::warn!("Could not subscribe to desktop notification events");
        }
    });
}

fn send(message: PanelReply) {
    fn send_to_shell(message: PanelReply) -> Result<(), JsValue> {
        let window = leptos::prelude::window();
        let bridge = js_sys::Reflect::get(&window, &BRIDGE.into())?;
        let reply =
            js_sys::Reflect::get(&bridge, &"reply".into())?.dyn_into::<js_sys::Function>()?;
        let payload = serde_json::to_string(&message)
            .map_err(|_| JsValue::from_str("Panel reply encoding failed"))?;
        reply.call1(&bridge, &payload.into())?;
        Ok(())
    }
    if send_to_shell(message).is_err() {
        leptos::logging::warn!("Could not post Panel reply to the desktop shell");
    }
}
