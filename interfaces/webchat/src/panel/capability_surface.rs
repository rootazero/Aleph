use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{t_string, use_i18n};
use aleph_protocol::capability::{UiAttachment, UiDisplayMessage, UiRenderAck};
use leptos::{ev, html, prelude::*, task::spawn_local};
use serde_json::json;
use std::sync::{Arc, Mutex};
use wasm_bindgen::{closure::Closure, JsCast};

const DISPLAY_TOPIC: &str = "capabilities.ui.display";

/// Outlives the reactive owner only while an RPC is settling. Late responses
/// must not touch disposed signals; attachment remains the single local handle.
struct SurfaceLifecycle {
    mounted: bool,
    epoch: u64,
    attaching: bool,
    attachment: Option<UiAttachment>,
}

#[component]
pub fn CapabilitySurface(state: DashboardState) -> impl IntoView {
    let i18n = use_i18n();
    let session_key = RwSignal::new(String::new());
    let rendered_text = RwSignal::new(String::new());
    let pending = RwSignal::new(None::<UiDisplayMessage>);
    let status = RwSignal::new(String::new());
    let focused = RwSignal::new(false);
    // These two signals are presentation projections, not attachment ownership.
    let attached = RwSignal::new(false);
    let attaching = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    let lifecycle = Arc::new(Mutex::new(SurfaceLifecycle {
        mounted: true, epoch: 0, attaching: false, attachment: None,
    }));
    let frame = StoredValue::new_local(None::<(i32, Closure<dyn FnMut(f64)>)>);

    let event_subscription = state.subscribe_events({
        let lifecycle = lifecycle.clone();
        move |event: GatewayEvent| {
            if event.topic != DISPLAY_TOPIC { return; }
            let life = lifecycle.lock().unwrap();
            if !life.mounted { return; }
            let Some(current) = life.attachment.as_ref() else { return; };
            // Event transport adds connection_id. Consume the Task3 wire shape
            // explicitly rather than defining another surface/event wire type.
            let (Some(surface_id), Some(generation), Some(request_id), Some(text)) = (
                event.data.get("surface_id").and_then(|v| v.as_str()),
                event.data.get("mount_generation").and_then(|v| v.as_u64()),
                event.data.get("request_id").and_then(|v| v.as_str()),
                event.data.get("text").and_then(|v| v.as_str()),
            ) else { return; };
            if current.surface_id == surface_id && current.mount_generation == generation {
                pending.set(Some(UiDisplayMessage { surface_id: surface_id.into(), mount_generation: generation,
                    request_id: request_id.into(), text: text.into() }));
            }
        }
    });

    spawn_local({
        let lifecycle = lifecycle.clone();
        async move {
            if let Err(error) = state.subscribe_topic_ephemeral(DISPLAY_TOPIC).await {
                if lifecycle.lock().unwrap().mounted { status.set(error); }
            }
        }
    });

    Effect::new({
        let lifecycle = lifecycle.clone();
        move |_| {
            let Some(message) = pending.get() else { return; };
            let epoch = {
                let life = lifecycle.lock().unwrap();
                if !life.mounted || !life.attachment.as_ref().is_some_and(|current|
                    current.surface_id == message.surface_id && current.mount_generation == message.mount_generation) { return; }
                life.epoch
            };
            rendered_text.set(message.text.clone());
            pending.set(None);
            frame.update_value(|previous| {
                if let Some((id, _callback)) = previous.take() {
                    if let Some(window) = web_sys::window() { let _ = window.cancel_animation_frame(id); }
                }
            });
            let ack = serde_json::to_value(UiRenderAck { surface_id: message.surface_id,
                mount_generation: message.mount_generation, request_id: message.request_id, rendered: true }).unwrap();
            let callback_lifecycle = lifecycle.clone();
            let callback = Closure::wrap(Box::new(move |_timestamp: f64| {
                {
                    let life = callback_lifecycle.lock().unwrap();
                    if !life.mounted || life.epoch != epoch { return; }
                }
                let lifecycle = callback_lifecycle.clone();
                let ack = ack.clone();
                spawn_local(async move {
                    if let Err(error) = state.rpc_call("capabilities.ui.ack", ack).await {
                        let life = lifecycle.lock().unwrap();
                        if life.mounted && life.epoch == epoch { status.set(error); }
                    }
                });
            }) as Box<dyn FnMut(f64)>);
            if let Some(window) = web_sys::window() {
                match window.request_animation_frame(callback.as_ref().unchecked_ref()) {
                    Ok(id) => frame.set_value(Some((id, callback))),
                    Err(error) => status.set(format!("requestAnimationFrame failed: {error:?}")),
                }
            }
        }
    });

    on_cleanup({
        let lifecycle = lifecycle.clone();
        move || {
            let current = {
                let mut life = lifecycle.lock().unwrap();
                life.mounted = false;
                life.epoch += 1;
                life.attaching = false;
                life.attachment.take()
            };
            frame.update_value(|previous| {
                if let Some((id, _callback)) = previous.take() {
                    if let Some(window) = web_sys::window() { let _ = window.cancel_animation_frame(id); }
                }
            });
            state.unsubscribe_events(event_subscription);
            spawn_local(async move {
                let _ = state.unsubscribe_topic(DISPLAY_TOPIC).await;
            });
            if let Some(current) = current {
                spawn_local(async move {
                    // Disconnect owns server-side close even if transport is gone.
                    let _ = state.rpc_call("capabilities.ui.detach", serde_json::to_value(current).unwrap()).await;
                });
            }
        }
    });

    Effect::new({
        let lifecycle = lifecycle.clone();
        move |_| {
            if !state.is_connected.get() {
                {
                    let mut life = lifecycle.lock().unwrap();
                    if !life.mounted { return; }
                    life.epoch += 1;
                    life.attaching = false;
                    life.attachment.take();
                }
                attaching.set(false);
                attached.set(false);
                pending.set(None);
                focused.set(false);
                status.set(t_string!(i18n, capability_surface.disconnected).to_string());
            }
        }
    });

    let attach = {
        let lifecycle = lifecycle.clone();
        move |_| {
            let epoch = {
                let mut life = lifecycle.lock().unwrap();
                // The handler guard also covers synthetic events and a click
                // before the disabled presentation has rendered.
                if !life.mounted || life.attaching || life.attachment.is_some() { return; }
                if !state.is_connected.get_untracked() { return; }
                life.epoch += 1;
                life.attaching = true;
                life.epoch
            };
            attaching.set(true);
            let key = session_key.get_untracked();
            let lifecycle = lifecycle.clone();
            spawn_local(async move {
                let result = state.rpc_call("capabilities.ui.attach", json!({ "session_key": key })).await
                    .and_then(|value| serde_json::from_value::<UiAttachment>(value).map_err(|error| error.to_string()));
                let current = {
                    let mut life = lifecycle.lock().unwrap();
                    let current = life.mounted && life.epoch == epoch;
                    if current {
                        life.attaching = false;
                        if let Ok(value) = &result { life.attachment = Some(value.clone()); }
                    }
                    current
                };
                if !current {
                    if let Ok(value) = result {
                        // No arena/signal access here. Unmount/disconnect may have
                        // disposed this component before attach returned its handle.
                        let _ = state.rpc_call("capabilities.ui.detach", serde_json::to_value(value).unwrap()).await;
                    }
                    return;
                }
                attaching.set(false);
                match result {
                    Ok(_) => {
                        attached.set(true);
                        status.set(t_string!(i18n, capability_surface.attached).to_string());
                    }
                    Err(error) => status.set(error),
                }
            });
        }
    };

    let key_down = {
        let lifecycle = lifecycle.clone();
        move |event: ev::KeyboardEvent| {
            let current = {
                let life = lifecycle.lock().unwrap();
                if !life.mounted || !focused.get_untracked() { return; }
                let Some(current) = life.attachment.clone() else { return; }; current
            };
            spawn_local(async move {
                let _ = state.rpc_call("capabilities.ui.submit_input", json!({
                    "surface_id": current.surface_id, "mount_generation": current.mount_generation,
                    "event": { "event": "key_down", "key": event.key() }
                })).await;
            });
        }
    };
    let mouse_down = {
        let lifecycle = lifecycle.clone();
        move |event: ev::MouseEvent| {
            let current = {
                let life = lifecycle.lock().unwrap();
                if !life.mounted || !focused.get_untracked() { return; }
                let Some(current) = life.attachment.clone() else { return; }; current
            };
            let button = u8::try_from(event.button()).unwrap_or(u8::MAX);
            spawn_local(async move {
                let _ = state.rpc_call("capabilities.ui.submit_input", json!({
                    "surface_id": current.surface_id, "mount_generation": current.mount_generation,
                    "event": { "event": "mouse_down", "x": event.offset_x(), "y": event.offset_y(), "button": button }
                })).await;
            });
        }
    };

    view! {
        <section class="capability-surface" aria-label=move || t_string!(i18n, capability_surface.title)>
            <div class="flex gap-2 items-end">
                <label class="flex flex-col flex-1">
                    <span>{move || t_string!(i18n, capability_surface.session_key)}</span>
                    <input type="text" prop:value=session_key on:input=move |event| {
                        session_key.set(event_target_value(&event));
                    } />
                </label>
                <button type="button" on:click=attach disabled=move || attaching.get() || attached.get() || !state.is_connected.get()>
                    {move || t_string!(i18n, capability_surface.attach)}
                </button>
            </div>
            <div node_ref=root tabindex="0" role="region" class="min-h-24 p-3 border rounded"
                on:focus=move |_| focused.set(true) on:blur=move |_| focused.set(false)
                on:keydown=key_down on:mousedown=mouse_down>
                {move || rendered_text.get()}
            </div>
            <div role="status" aria-live="polite">{move || status.get()}</div>
        </section>
    }
}
