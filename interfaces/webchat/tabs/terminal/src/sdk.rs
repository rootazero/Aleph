//! Browser SDK adapter for the standalone terminal module.
//!
//! Only JSON values, DOM values, and explicit cleanup handles cross the module
//! boundary. The adapter keeps the connection projection and its polling loop
//! local to this WASM instance.

#[cfg(target_arch = "wasm32")]
mod browser {
    use aleph_terminal_ui::{
        TerminalConnection, TerminalEvent, TerminalEventStream, TerminalServices, TerminalText,
    };
    use futures::{future::LocalBoxFuture, FutureExt};
    use gloo_timers::future::TimeoutFuture;
    use js_sys::JSON;
    use leptos::prelude::{Get, RwSignal, Set, Signal};
    use leptos::task::spawn_local;
    use serde::Deserialize;
    use serde_json::Value;
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen::{prelude::*, JsCast};

    #[wasm_bindgen]
    extern "C" {
        #[derive(Clone)]
        type BrowserPanelSdk;

        #[wasm_bindgen(method, catch, js_name = invoke)]
        async fn sdk_invoke(
            this: &BrowserPanelSdk,
            method: String,
            params: JsValue,
        ) -> Result<JsValue, JsValue>;

        #[wasm_bindgen(method, catch, js_name = connection)]
        fn sdk_connection(this: &BrowserPanelSdk) -> Result<JsValue, JsValue>;

        #[wasm_bindgen(method, catch, js_name = subscribe)]
        async fn sdk_subscribe(
            this: &BrowserPanelSdk,
            topic: String,
        ) -> Result<BrowserPanelSubscription, JsValue>;

        #[derive(Clone)]
        type BrowserPanelSubscription;

        #[wasm_bindgen(method, catch, js_name = drain)]
        fn subscription_drain(this: &BrowserPanelSubscription) -> Result<JsValue, JsValue>;

        #[wasm_bindgen(method, js_name = dispose)]
        fn subscription_dispose(this: &BrowserPanelSubscription);

        #[wasm_bindgen(method, catch, js_name = locale)]
        fn sdk_locale(this: &BrowserPanelSdk) -> Result<String, JsValue>;
    }

    #[derive(Deserialize)]
    struct ConnectionWire {
        connected: bool,
        epoch: u64,
    }

    #[derive(Deserialize)]
    struct EventWire {
        topic: String,
        data: Value,
    }

    pub(crate) struct Adapter {
        pub(crate) services: TerminalServices,
        active: Rc<Cell<bool>>,
    }

    impl Adapter {
        pub(crate) fn dispose(&self) {
            self.active.set(false);
        }
    }

    pub(crate) fn make(sdk: JsValue) -> Result<Adapter, JsValue> {
        let sdk: BrowserPanelSdk = sdk.unchecked_into();
        let active = Rc::new(Cell::new(true));
        let connection = RwSignal::new(read_connection(&sdk)?);
        let signal = Signal::derive(move || connection.get());

        let loop_active = Rc::clone(&active);
        let loop_sdk = sdk.clone();
        spawn_local(async move {
            while loop_active.get() {
                TimeoutFuture::new(100).await;
                if !loop_active.get() {
                    break;
                }
                if let Ok(snapshot) = read_connection(&loop_sdk) {
                    connection.try_set(snapshot);
                }
            }
        });

        let invoke_active = Rc::clone(&active);
        let invoke_sdk = sdk.clone();
        let invoke = Rc::new(
            move |method: String,
                  params: Value|
                  -> LocalBoxFuture<'static, Result<Value, String>> {
                let active = Rc::clone(&invoke_active);
                let sdk = invoke_sdk.clone();
                async move {
                    if !active.get() {
                        return Err("terminal module disposed".to_string());
                    }
                    let value = sdk
                        .sdk_invoke(method, to_js(params)?)
                        .await
                        .map_err(js_error)?;
                    if !active.get() {
                        return Err("terminal module disposed".to_string());
                    }
                    from_js(value)
                }
                .boxed_local()
            },
        );

        let subscribe_active = Rc::clone(&active);
        let subscribe_sdk = sdk.clone();
        let subscribe = Rc::new(
            move |topic: String| -> LocalBoxFuture<'static, Result<TerminalEventStream, String>> {
                let active = Rc::clone(&subscribe_active);
                let sdk = subscribe_sdk.clone();
                async move {
                    if !active.get() {
                        return Err("terminal module disposed".to_string());
                    }
                    let epoch = read_connection(&sdk).map_err(js_error)?.epoch;
                    let subscription = sdk.sdk_subscribe(topic).await.map_err(js_error)?;
                    if !active.get() {
                        subscription.subscription_dispose();
                        return Err("terminal module disposed".to_string());
                    }
                    let disposed = Rc::new(Cell::new(false));
                    let drain_subscription = subscription.clone();
                    let drain_disposed = Rc::clone(&disposed);
                    let drain = Rc::new(move || {
                        if drain_disposed.get() {
                            return Ok(Vec::new());
                        }
                        let value = drain_subscription.subscription_drain().map_err(js_error)?;
                        let events: Vec<EventWire> = from_js(value)?;
                        Ok(events
                            .into_iter()
                            .map(|event| TerminalEvent {
                                topic: event.topic,
                                data: event.data,
                            })
                            .collect())
                    });
                    let dispose_subscription = subscription.clone();
                    let dispose_once = Rc::clone(&disposed);
                    let dispose = Rc::new(move || {
                        if !dispose_once.replace(true) {
                            dispose_subscription.subscription_dispose();
                        }
                    });
                    Ok(TerminalEventStream::new(epoch, drain, dispose))
                }
                .boxed_local()
            },
        );

        let locale = sdk.sdk_locale().unwrap_or_else(|_| "en".to_string());
        Ok(Adapter {
            services: TerminalServices {
                invoke,
                subscribe,
                connection: signal,
                text: terminal_text(&locale),
            },
            active,
        })
    }

    fn read_connection(sdk: &BrowserPanelSdk) -> Result<TerminalConnection, JsValue> {
        let wire: ConnectionWire =
            from_js(sdk.sdk_connection()?).map_err(|error: String| JsValue::from_str(&error))?;
        Ok(TerminalConnection {
            connected: wire.connected,
            epoch: wire.epoch,
        })
    }

    fn terminal_text(locale: &str) -> TerminalText {
        if locale == "zh" {
            TerminalText {
                new_tab: "新建终端".to_string(),
                close_tab: "关闭终端".to_string(),
                read_refusal: "该设置页需要 operator 权限,当前连接的角色无法读取服务器全局配置。"
                    .to_string(),
                write_refusal: "该改动需要 operator 权限,当前连接的角色无法修改服务器全局设置。"
                    .to_string(),
            }
        } else {
            TerminalText {
                new_tab: "New terminal".to_string(),
                close_tab: "Close terminal".to_string(),
                read_refusal: "This settings page needs operator privileges — the role this connection holds cannot read server-wide configuration.".to_string(),
                write_refusal: "This change needs operator privileges — the role this connection holds cannot modify server-wide settings.".to_string(),
            }
        }
    }

    fn from_js<T: for<'de> Deserialize<'de>>(value: JsValue) -> Result<T, String> {
        let json = JSON::stringify(&value)
            .map_err(js_error)?
            .as_string()
            .ok_or_else(|| "invalid JSON value".to_string())?;
        serde_json::from_str(&json).map_err(|error| error.to_string())
    }

    fn to_js(value: Value) -> Result<JsValue, String> {
        JSON::parse(&value.to_string()).map_err(js_error)
    }

    fn js_error(error: JsValue) -> String {
        error.as_string().unwrap_or_else(|| format!("{error:?}"))
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{make, Adapter};
