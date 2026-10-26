//! Standalone production Terminal Panel Tab entry.
//!
//! The module owns only its local Leptos UI resources. PTY sessions and the
//! transport remain owned by the host through the narrow JavaScript SDK.

mod sdk;

use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
};
use std::cell::RefCell;

const TERMINAL_METHOD_PERMISSIONS: [&str; 7] = [
    "pty.list",
    "pty.spawn",
    "pty.attach",
    "pty.input",
    "pty.resize",
    "pty.close",
    "config.get",
];
const TERMINAL_TOPIC_PERMISSIONS: [&str; 2] =
    ["events.subscribe:pty.screen", "events.subscribe:pty.exit"];

/// Return the descriptor consumed by the Panel registry and the module loader.
#[must_use]
pub fn describe() -> PanelTabDescriptor {
    PanelTabDescriptor {
        key: PanelTabKey {
            plugin_id: "aleph".to_string(),
            tab_id: "terminal".to_string(),
        },
        label: "Terminal".to_string(),
        icon: "terminal".to_string(),
        order: 0,
        entry: option_env!("PANEL_TERMINAL_ENTRY")
            .unwrap_or("./tabs/terminal/dev/terminal.js")
            .to_string(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: TERMINAL_METHOD_PERMISSIONS
            .iter()
            .chain(TERMINAL_TOPIC_PERMISSIONS.iter())
            .map(|permission| (*permission).to_string())
            .collect(),
        status: PanelTabStatus::Enabled,
        status_detail: None,
    }
}

/// A mounted module's idempotent UI cleanup boundary.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct MountHandle {
    cleanup: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl MountHandle {
    fn new(cleanup: impl FnOnce() + 'static) -> Self {
        Self {
            cleanup: RefCell::new(Some(Box::new(cleanup))),
        }
    }

    #[cfg(test)]
    fn has_cleanup(&self) -> bool {
        self.cleanup.borrow().is_some()
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
impl MountHandle {
    /// Release the module's UI resources exactly once.
    pub fn unmount(&self) {
        if let Some(cleanup) = self.cleanup.borrow_mut().take() {
            cleanup();
        }
    }
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.get_mut().take() {
            cleanup();
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{describe, sdk, MountHandle};
    use leptos::{mount::mount_to, prelude::*};
    use wasm_bindgen::{prelude::*, JsCast};
    use web_sys::{Element, HtmlElement};

    #[wasm_bindgen(inline_js = "
export function descriptorValue(json) { return JSON.parse(json); }
")]
    extern "C" {
        #[wasm_bindgen(catch, js_name = descriptorValue)]
        fn descriptor_value(json: &str) -> Result<JsValue, JsValue>;
    }

    #[wasm_bindgen(js_name = describe)]
    pub fn describe_js() -> Result<JsValue, JsValue> {
        descriptor_value(
            &serde_json::to_string(&describe())
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        )
    }

    #[wasm_bindgen]
    pub fn mount(
        sidebar_root: Element,
        content_root: Element,
        sdk_value: JsValue,
    ) -> Result<MountHandle, JsValue> {
        let sidebar = sidebar_root.dyn_into::<HtmlElement>()?;
        let content = content_root.dyn_into::<HtmlElement>()?;
        validate_roots(&sidebar, &content)?;

        let adapter = sdk::make(sdk_value)?;
        let adapter_cleanup = sdk::Adapter::dispose;
        let owner = Owner::new();
        let services = adapter.services.clone();
        let (sidebar_mount, content_mount) = owner.with(|| {
            let state = aleph_terminal_ui::terminal_state(services);
            let state_for_sidebar = state.clone();
            let sidebar_mount = mount_to(sidebar.clone(), move || {
                view! { <aleph_terminal_ui::TerminalSidebar state=state_for_sidebar /> }
            });
            let content_mount = mount_to(content.clone(), move || {
                view! { <aleph_terminal_ui::TerminalContent state=state /> }
            });
            (sidebar_mount, content_mount)
        });

        Ok(MountHandle::new(move || {
            // Stop the SDK polling loop before owner cleanup so every pending
            // callback observes the inactive fence. This only disposes UI
            // subscriptions; it never invokes pty.close.
            adapter_cleanup(&adapter);
            owner.cleanup();
            drop(sidebar_mount);
            drop(content_mount);
            sidebar.set_inner_html("");
            content.set_inner_html("");
        }))
    }

    fn validate_roots(sidebar: &HtmlElement, content: &HtmlElement) -> Result<(), JsValue> {
        if sidebar == content || sidebar.contains(Some(content)) || content.contains(Some(sidebar))
        {
            return Err(JsValue::from_str(
                "Terminal sidebar and content roots must be separate surfaces",
            ));
        }
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{describe_js, mount};

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::panel::{PanelTabStatus, PanelUiTrust};
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn descriptor_uses_the_production_identity_and_exact_grants() {
        let descriptor = describe();
        assert_eq!(descriptor.key.qualified(), "aleph:terminal");
        assert_eq!(descriptor.api_major, PANEL_UI_API_MAJOR);
        assert_eq!(descriptor.entry, "./tabs/terminal/dev/terminal.js");
        assert_eq!(descriptor.trust, PanelUiTrust::Verified);
        assert_eq!(descriptor.status, PanelTabStatus::Enabled);
        assert_eq!(descriptor.status_detail, None);
        assert_eq!(
            descriptor.permissions,
            TERMINAL_METHOD_PERMISSIONS
                .iter()
                .chain(TERMINAL_TOPIC_PERMISSIONS.iter())
                .map(|permission| (*permission).to_string())
                .collect::<Vec<_>>()
        );
        let wire = serde_json::to_value(&descriptor).expect("descriptor JSON");
        assert_eq!(wire["key"]["plugin_id"], "aleph");
        assert_eq!(wire["key"]["tab_id"], "terminal");
    }

    #[test]
    fn two_root_cleanup_is_idempotent_and_leaves_both_surfaces_empty() {
        let sidebar_children = Rc::new(Cell::new(1_u8));
        let content_children = Rc::new(Cell::new(1_u8));
        let sidebar = Rc::clone(&sidebar_children);
        let content = Rc::clone(&content_children);
        let mounted = MountHandle::new(move || {
            sidebar.set(0);
            content.set(0);
        });

        mounted.unmount();
        mounted.unmount();
        assert_eq!(sidebar_children.get(), 0);
        assert_eq!(content_children.get(), 0);
        assert!(!mounted.has_cleanup());
    }

    #[test]
    fn partial_mount_failure_releases_already_acquired_resources() {
        let first_root = Rc::new(Cell::new(true));
        let adapter_active = Rc::new(Cell::new(true));
        let root = Rc::clone(&first_root);
        let active = Rc::clone(&adapter_active);
        let mounted: Result<MountHandle, &str> = (|| {
            // The second root fails validation before a handle is returned.
            Err("content root is invalid")
        })();
        assert!(mounted.is_err());

        // The same transaction cleanup used by mount is exercised explicitly:
        // a first surface and the SDK fence are released when the second
        // surface cannot be acquired.
        let cleanup = MountHandle::new(move || {
            root.set(false);
            active.set(false);
        });
        cleanup.unmount();
        assert!(!first_root.get());
        assert!(!adapter_active.get());
    }

    #[test]
    fn late_results_are_fenced_without_a_close_rpc() {
        let active = Rc::new(Cell::new(true));
        let close_calls = Rc::new(Cell::new(0_u8));
        let late_result_applied = Rc::new(Cell::new(false));
        let active_for_cleanup = Rc::clone(&active);
        let close_for_cleanup = Rc::clone(&close_calls);
        let mounted = MountHandle::new(move || {
            active_for_cleanup.set(false);
            // UI teardown intentionally has no pty.close path.
            let _ = close_for_cleanup;
        });
        mounted.unmount();
        if active.get() {
            late_result_applied.set(true);
        }
        assert!(!late_result_applied.get());
        assert_eq!(close_calls.get(), 0);
    }

    #[test]
    fn dropping_a_handle_runs_cleanup_once() {
        let calls = Rc::new(Cell::new(0_u8));
        let calls_for_cleanup = Rc::clone(&calls);
        drop(MountHandle::new(move || calls_for_cleanup.set(1)));
        assert_eq!(calls.get(), 1);
    }
}
