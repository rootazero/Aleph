//! Disposable terminal-shaped Panel Tab. The host owns all transport and sessions.

use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
};

pub fn describe() -> PanelTabDescriptor {
    PanelTabDescriptor {
        key: PanelTabKey {
            plugin_id: "aleph".into(),
            tab_id: "terminal-pilot".into(),
        },
        label: "Terminal pilot".into(),
        icon: "terminal".into(),
        order: 0,
        entry: option_env!("ALEPH_PANEL_PILOT_ENTRY")
            .unwrap_or("tabs/terminal-pilot/terminal-pilot.js")
            .into(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: vec!["sessions.list".into()],
        status: PanelTabStatus::Enabled,
        status_detail: None,
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn session_labels(json: &str) -> Result<Vec<String>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct SessionList {
        sessions: Vec<aleph_protocol::SessionListRow>,
    }

    Ok(serde_json::from_str::<SessionList>(json)?
        .sessions
        .into_iter()
        .map(|row| row.topic.or(row.label).unwrap_or(row.key))
        .collect())
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct MountedTab {
    cleanup: Option<Box<dyn FnOnce()>>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
impl MountedTab {
    pub fn unmount(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{describe_js, mount};

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{session_labels, MountedTab};
    use leptos::{mount::mount_to, prelude::*};
    use wasm_bindgen::{prelude::*, JsCast};
    use web_sys::{Element, HtmlElement};

    // Only JSON values cross the SDK boundary. No host objects or DOM live here.
    #[wasm_bindgen(inline_js = "
export function descriptorValue(json) { return JSON.parse(json); }
export function sessionReader(sdk, callback) {
    let busy = false;
    return {
        read() {
            if (!sdk || busy) return;
            busy = true;
            const run = async () => {
                try {
                    const value = await sdk.invoke('sessions.list', { limit: 20 });
                    if (callback) callback(true, JSON.stringify(value));
                } catch (error) {
                    if (callback) callback(false, String(error));
                } finally {
                    busy = false;
                }
            };
            // A disposed/panicking WASM callback must not leak a rejected promise.
            void run().catch(() => {});
        },
        dispose() { callback = null; sdk = null; }
    };
}
")]
    extern "C" {
        #[wasm_bindgen(catch, js_name = descriptorValue)]
        fn descriptor_value(json: &str) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_name = sessionReader)]
        fn session_reader(sdk: JsValue, callback: &JsValue) -> SessionReader;
        #[derive(Clone)]
        type SessionReader;
        #[wasm_bindgen(method)]
        fn read(this: &SessionReader);
        #[wasm_bindgen(method)]
        fn dispose(this: &SessionReader);
    }

    #[wasm_bindgen(js_name = describe)]
    pub fn describe_js() -> Result<JsValue, JsValue> {
        descriptor_value(&serde_json::to_string(&super::describe()).expect("descriptor JSON"))
    }

    #[wasm_bindgen]
    pub fn mount(
        sidebar_root: Element,
        content_root: Element,
        sdk: JsValue,
    ) -> Result<MountedTab, JsValue> {
        // Validate both surfaces before mounting either; never query global DOM.
        let sidebar = sidebar_root.dyn_into::<HtmlElement>()?;
        let content = content_root.dyn_into::<HtmlElement>()?;
        if sidebar == content
            || sidebar.contains(Some(&content))
            || content.contains(Some(&sidebar))
        {
            return Err(JsValue::from_str("Pilot roots must be separate surfaces"));
        }
        let owner = Owner::new();
        let (sidebar, content) = owner.with(move || {
            let sessions = RwSignal::new(Vec::<String>::new());
            let log = RwSignal::new("Read-only pilot: list sessions through the host SDK.".to_string());
            let busy = RwSignal::new(false);
            let callback = Closure::<dyn FnMut(bool, String)>::new(move |ok: bool, value: String| {
                busy.try_set(false);
                if !ok {
                    log.try_set(format!("sessions.list failed: {value}"));
                    return;
                }
                match session_labels(&value) {
                    Ok(rows) => {
                        sessions.try_set(rows);
                        log.try_set(format!("$ sessions.list\n{value}"));
                    }
                    Err(error) => {
                        log.try_set(format!("Invalid sessions.list response: {error}"));
                    }
                }
            });
            let reader = session_reader(sdk, callback.as_ref());
            let action = reader.clone();
            let resources = StoredValue::new_local(Some((reader, callback)));
            on_cleanup(move || {
                let _ = resources.try_update_value(|resources| {
                    if let Some((reader, callback)) = resources.take() {
                        // Fence late results before dropping the WASM callback.
                        reader.dispose();
                        drop(callback);
                    }
                });
            });
            let sidebar = mount_to(sidebar, move || view! {
                <section aria-label="Terminal pilot sessions" style="padding:1rem;font-family:monospace">
                    <h2>"Sessions"</h2>
                    <button disabled=move || busy.get() on:click=move |_| {
                        busy.set(true);
                        action.read();
                    }>"List sessions"</button>
                    <p>"Read-only terminal pilot"</p>
                    <ul>{move || sessions.get().into_iter().map(|label| {
                        let selected = label.clone();
                        view! { <li><button on:click=move |_| {
                            log.set(format!("Selected: {selected}\nPilot only — no PTY attached."));
                        }>{label}</button></li> }
                    }).collect_view()}</ul>
                </section>
            });
            let content = mount_to(content, move || view! {
                <section aria-label="Terminal pilot output" style="padding:1rem;min-width:0;font-family:monospace">
                    <h2>"Terminal pilot"</h2>
                    <p>"No provider or session is stopped when this tab closes."</p>
                    <pre role="log" aria-live="polite" style="white-space:pre-wrap;overflow-wrap:anywhere">{move || log.get()}</pre>
                </section>
            });
            (sidebar, content)
        });
        Ok(MountedTab {
            cleanup: Some(Box::new(move || {
                owner.cleanup();
                drop(sidebar);
                drop(content);
            })),
        })
    }
}

impl Drop for MountedTab {
    fn drop(&mut self) {
        self.unmount();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::panel::{PanelTabStatus, PanelUiTrust};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn descriptor_matches_the_host_pilot_entry_and_read_permission() {
        let descriptor = describe();
        assert_eq!(descriptor.key.qualified(), "aleph:terminal-pilot");
        assert_eq!(descriptor.api_major, 1);
        assert_eq!(
            descriptor.entry,
            option_env!("ALEPH_PANEL_PILOT_ENTRY")
                .unwrap_or("tabs/terminal-pilot/terminal-pilot.js")
        );
        assert_eq!(descriptor.permissions, ["sessions.list"]);
        assert_eq!(descriptor.trust, PanelUiTrust::Verified);
        assert_eq!(descriptor.status, PanelTabStatus::Enabled);
        assert_eq!(descriptor.status_detail, None);
        let wire = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(wire["key"]["tab_id"], "terminal-pilot");
        assert_eq!(wire["api_major"], 1);
        assert_eq!(
            serde_json::from_value::<PanelTabDescriptor>(wire).unwrap(),
            descriptor
        );
    }

    #[test]
    fn unmount_releases_resources_once_even_when_handle_is_later_dropped() {
        let calls = Rc::new(Cell::new(0));
        let count = calls.clone();
        let mut mounted = MountedTab {
            cleanup: Some(Box::new(move || count.set(count.get() + 1))),
        };
        mounted.unmount();
        assert_eq!(calls.get(), 1);
        assert!(mounted.cleanup.is_none());
        mounted.unmount();
        drop(mounted);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn session_read_uses_protocol_titles_and_rejects_invalid_payloads() {
        assert_eq!(
            session_labels(r#"{"sessions":[{"key":"a","topic":"Build"},{"key":"b","label":"Review"},{"key":"c"}]}"#).unwrap(),
            ["Build", "Review", "c"]
        );
        assert!(session_labels(r#"{"sessions":"invalid"}"#).is_err());
        assert!(session_labels(r#"{}"#).is_err());
    }

    #[test]
    fn dropping_a_mounted_handle_releases_its_resources() {
        let released = Rc::new(Cell::new(false));
        let flag = released.clone();
        drop(MountedTab {
            cleanup: Some(Box::new(move || flag.set(true))),
        });
        assert!(released.get());
    }
}
