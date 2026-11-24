//! WASM-only JS bridge. Never exports DashboardState or transport handles.

use super::{map_error_code, LoadedModule, MountedTab, PanelLoadError};
use crate::appearance::ThemeMode;
use crate::panel::sdk::{PanelSdk, PanelSubscription, PanelTerminalMutations};
use shared_ui_logic::terminal_mutation::TerminalMutationScope;
use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey};
use futures::{future::LocalBoxFuture, FutureExt};
use js_sys::{Reflect, JSON};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;
use web_sys::Element;

// index.html publishes this lazy bridge using Trunk's explicit public root.
// A module import here would create a second loader under wasm-bindgen snippets.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = __alephLoadPanelModule)]
    async fn load_module(
        entry: &str,
        integrity: Option<String>,
        credential: JsValue,
    ) -> Result<JsValue, JsValue>;

    type Module;
    #[wasm_bindgen(method, getter, js_name = descriptorJson)]
    fn descriptor_json(this: &Module) -> String;
    #[wasm_bindgen(method, catch)]
    fn mount(
        this: &Module,
        sidebar: &Element,
        content: &Element,
        sdk: BrowserPanelSdk,
    ) -> Result<Handle, JsValue>;

    type Handle;
    #[wasm_bindgen(method, catch)]
    fn unmount(this: &Handle) -> Result<(), JsValue>;
}

pub(super) fn load(
    descriptor: PanelTabDescriptor,
    sidebar: Element,
    content: Element,
    integrity: Option<String>,
    sdk: PanelSdk,
) -> LocalBoxFuture<'static, Result<LoadedModule, PanelLoadError>> {
    async move {
        // The compile-time deterministic spike remains disconnected and has no
        // production credential claim. Normal community delivery requires one.
        let fixture =
            cfg!(feature = "panel-tab-spike") && descriptor.key.plugin_id == "community.fixture";
        let mut fence = None;
        let mut binding = None;
        let credential = if integrity.is_some() && !fixture {
            let epoch = sdk
                .connection()
                .map_err(|_| PanelLoadError::AssetFetchFailed)?
                .epoch;
            let context = sdk.asset_project_context().ok_or(PanelLoadError::AssetFetchFailed)?;
            binding = Some((epoch, context));
            let (gateway_url, token) = sdk
                .asset_credential()
                .await
                .map_err(|_| PanelLoadError::AssetFetchFailed)?;
            let watched = descriptor.clone();
            let watching = sdk.clone();
            let current =
                Closure::<dyn FnMut() -> bool>::new(move || watching.asset_context_is_current(&watched, epoch, context));
            let credential = to_js(json!({ "gateway_url": gateway_url, "token": token, "project_generation": context.1.to_string() }))
                .map_err(|_| PanelLoadError::AssetFetchFailed)?;
            Reflect::set(&credential, &"current".into(), current.as_ref())
                .map_err(|_| PanelLoadError::AssetFetchFailed)?;
            fence = Some(current);
            credential
        } else {
            JsValue::NULL
        };
        let result = load_module(&descriptor.entry, integrity, credential)
            .await
            .map_err(|error| map_error(error, PanelLoadError::ImportFailed));
        if matches!(&result, Err(PanelLoadError::AssetFetchFailed)) {
            if let Some((epoch, context)) = binding {
                sdk.asset_refused(&descriptor, epoch, context);
            }
        }
        let module = Module::from(result?);
        drop(fence);
        let descriptor = serde_json::from_str(&module.descriptor_json())
            .map_err(|_| PanelLoadError::InvalidDescriptor)?;
        Ok(LoadedModule {
            descriptor,
            mount: Box::new(move |sdk| {
                let handle = module
                    .mount(&sidebar, &content, BrowserPanelSdk { sdk })
                    .map_err(|error| map_error(error, PanelLoadError::MountFailed))?;
                Ok(MountedTab::new(move || {
                    handle.unmount().map_err(|_| PanelLoadError::UnmountFailed)
                }))
            }),
        })
    }
    .boxed_local()
}

fn map_error(error: JsValue, fallback: PanelLoadError) -> PanelLoadError {
    map_error_code(
        Reflect::get(&error, &"code".into())
            .ok()
            .and_then(|code| code.as_string())
            .as_deref(),
        fallback,
    )
}

fn from_js(value: &JsValue) -> Result<Value, JsValue> {
    serde_json::from_str(
        &JSON::stringify(value)?
            .as_string()
            .ok_or_else(|| JsValue::from_str("Invalid JSON"))?,
    )
    .map_err(|_| JsValue::from_str("Invalid JSON"))
}

fn to_js(value: Value) -> Result<JsValue, JsValue> {
    JSON::parse(&value.to_string())
}

#[wasm_bindgen]
pub struct BrowserPanelSdk {
    sdk: PanelSdk,
}

#[wasm_bindgen]
impl BrowserPanelSdk {
    /// Payload-free coordinator for the exact authorized builtin Terminal only.
    #[wasm_bindgen(js_name = terminalMutations)]
    pub fn terminal_mutations(&self) -> Result<BrowserTerminalMutations, JsValue> {
        self.sdk.terminal_mutations()
            .map(|scope| BrowserTerminalMutations { scope })
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub async fn invoke(&self, method: String, params: JsValue) -> Result<JsValue, JsValue> {
        let value = self
            .sdk
            .invoke(&method, from_js(&params)?)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        to_js(value)
    }

    pub async fn subscribe(&self, topic: String) -> Result<BrowserPanelSubscription, JsValue> {
        Ok(BrowserPanelSubscription {
            subscription: self
                .sdk
                .subscribe(&topic)
                .await
                .map_err(|error| JsValue::from_str(&error))?,
        })
    }

    #[wasm_bindgen(js_name = subscribeTerminal)]
    pub async fn subscribe_terminal(
        &self,
        topic: String,
    ) -> Result<BrowserPanelSubscription, JsValue> {
        Ok(BrowserPanelSubscription {
            subscription: self
                .sdk
                .subscribe_terminal(&topic)
                .await
                .map_err(|error| JsValue::from_str(&error))?,
        })
    }

    pub fn connection(&self) -> Result<JsValue, JsValue> {
        let snapshot = self
            .sdk
            .connection()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(json!({
            "connected": snapshot.connected,
            "epoch": snapshot.epoch,
        }))
    }

    pub fn navigate(&self, key: JsValue) -> Result<(), JsValue> {
        let key: PanelTabKey = serde_json::from_value(from_js(&key)?)
            .map_err(|_| JsValue::from_str("InvalidNavigation"))?;
        self.sdk
            .navigate(key)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn theme(&self) -> Result<JsValue, JsValue> {
        let theme = self.sdk.theme();
        to_js(
            json!({ "mode": match theme.mode { ThemeMode::System => "system", ThemeMode::Light => "light", ThemeMode::Dark => "dark" }, "accent": theme.accent.id() }),
        )
    }

    pub fn locale(&self) -> String {
        self.sdk.locale()
    }
}

#[wasm_bindgen]
pub struct BrowserTerminalMutations {
    scope: PanelTerminalMutations,
}

#[wasm_bindgen]
impl BrowserTerminalMutations {
    pub fn revision(&self) -> String {
        self.scope.revision().to_string()
    }
    pub fn pending(&self) -> bool {
        self.scope.pending()
    }
    pub fn reserve(&self) -> Option<String> {
        self.scope.reserve().map(|ticket| ticket.to_string())
    }
    pub fn cancel(&self, ticket: String) {
        if let Ok(ticket) = ticket.parse() {
            self.scope.cancel(ticket);
        }
    }
    pub fn settle(&self, ticket: String) {
        if let Ok(ticket) = ticket.parse() {
            self.scope.settle(ticket);
        }
    }
}

#[wasm_bindgen]
pub struct BrowserPanelSubscription {
    subscription: PanelSubscription,
}

#[wasm_bindgen]
impl BrowserPanelSubscription {
    /// Decimal text preserves the confirmed u64 epoch across the JS boundary.
    #[wasm_bindgen(js_name = coverageEpoch)]
    pub fn coverage_epoch(&self) -> Result<String, JsValue> {
        self.subscription
            .coverage_epoch()
            .map(|epoch| epoch.to_string())
            .ok_or_else(|| JsValue::from_str("Not a terminal subscription"))
    }

    pub fn drain(&self) -> Result<JsValue, JsValue> {
        to_js(Value::Array(
            self.subscription
                .try_drain()
                .map_err(|error| JsValue::from_str(&error))?
                .into_iter()
                .map(|event| json!({ "topic": event.topic, "data": event.data }))
                .collect(),
        ))
    }

    pub fn dispose(&self) {
        self.subscription.clone().dispose();
    }
}
