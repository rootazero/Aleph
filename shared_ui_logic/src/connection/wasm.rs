//! WASM WebSocket connector implementation
//!
//! This module provides a WebSocket connector for WASM environments (browsers).
//! It uses the browser's native WebSocket API through web-sys bindings.

use super::{AlephConnector, ConnectionError};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

/// WASM-based WebSocket connector using browser's native WebSocket API
///
/// # Example
///
/// ```ignore
/// use aleph_ui_logic::connection::{WasmConnector, AlephConnector};
///
/// #[wasm_bindgen]
/// pub async fn connect_to_gateway() {
///     let mut connector = WasmConnector::new();
///     connector.connect("ws://127.0.0.1:18789").await.unwrap();
/// }
/// ```
#[derive(Clone)]
pub struct WasmConnector {
    #[cfg(target_arch = "wasm32")]
    ws: Arc<Mutex<Option<WebSocket>>>,
    send_tx: Option<mpsc::UnboundedSender<Value>>,
    recv_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Result<Value, ConnectionError>>>>>,
    is_connected: bool,
}

impl WasmConnector {
    /// Create a new WASM connector
    pub fn new() -> Self {
        Self {
            #[cfg(target_arch = "wasm32")]
            ws: Arc::new(Mutex::new(None)),
            send_tx: None,
            recv_rx: Arc::new(Mutex::new(None)),
            is_connected: false,
        }
    }
}

impl Default for WasmConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl AlephConnector for WasmConnector {
    async fn connect(&mut self, url: &str) -> Result<(), ConnectionError> {
        #[cfg(target_arch = "wasm32")]
        {
            let ws = WebSocket::new(url)
                .map_err(|e| ConnectionError::ConnectionFailed(format!("{:?}", e)))?;

            // Set binary type to arraybuffer for efficient data transfer
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

            let (send_tx, mut send_rx) = mpsc::unbounded_channel::<Value>();
            let (recv_tx, recv_rx) = mpsc::unbounded_channel::<Result<Value, ConnectionError>>();

            // Clone for closures
            let ws_clone = ws.clone();
            let recv_tx_clone = recv_tx.clone();

            // onopen handler
            let onopen_callback = Closure::wrap(Box::new(move |_| {
                web_sys::console::log_1(&"WebSocket connected".into());
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
            onopen_callback.forget();

            // onmessage handler
            let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                    let text: String = txt.into();
                    match serde_json::from_str::<Value>(&text) {
                        Ok(value) => {
                            let _ = recv_tx_clone.send(Ok(value));
                        }
                        Err(err) => {
                            let _ = recv_tx_clone.send(Err(ConnectionError::InvalidMessage(
                                format!("Failed to parse JSON: {}", err),
                            )));
                        }
                    }
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
            onmessage_callback.forget();

            // onerror handler
            let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
                web_sys::console::error_1(&format!("WebSocket error: {:?}", e.message()).into());
            }) as Box<dyn FnMut(ErrorEvent)>);
            ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
            onerror_callback.forget();

            // onclose handler
            let recv_tx_close = recv_tx.clone();
            let onclose_callback = Closure::wrap(Box::new(move |e: CloseEvent| {
                web_sys::console::log_1(
                    &format!("WebSocket closed: code={}, reason={}", e.code(), e.reason()).into(),
                );
                let _ = recv_tx_close.send(Err(ConnectionError::Disconnected));
            }) as Box<dyn FnMut(CloseEvent)>);
            ws.set_onclose(Some(onclose_callback.as_ref().unchecked_ref()));
            onclose_callback.forget();

            // Spawn task to handle outgoing messages
            let ws_send = ws_clone.clone();
            wasm_bindgen_futures::spawn_local(async move {
                while let Some(msg) = send_rx.recv().await {
                    if let Ok(text) = serde_json::to_string(&msg) {
                        if let Err(e) = ws_send.send_with_str(&text) {
                            web_sys::console::error_1(
                                &format!("Failed to send message: {:?}", e).into(),
                            );
                            break;
                        }
                    }
                }
            });

            *self.ws.lock().await = Some(ws);
            self.send_tx = Some(send_tx);
            *self.recv_rx.lock().await = Some(recv_rx);
            self.is_connected = true;

            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(ConnectionError::ConnectionFailed(
                "WASM connector only works in WASM environment".to_string(),
            ))
        }
    }

    async fn disconnect(&mut self) -> Result<(), ConnectionError> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(ws) = self.ws.lock().await.take() {
                ws.close().map_err(|e| {
                    ConnectionError::ConnectionFailed(format!("Failed to close: {:?}", e))
                })?;
            }
            self.send_tx = None;
            *self.recv_rx.lock().await = None;
            self.is_connected = false;
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(ConnectionError::ConnectionFailed(
                "WASM connector only works in WASM environment".to_string(),
            ))
        }
    }

    async fn send(&mut self, message: Value) -> Result<(), ConnectionError> {
        if let Some(tx) = &self.send_tx {
            tx.send(message)
                .map_err(|_| ConnectionError::SendFailed("Channel closed".to_string()))?;
            Ok(())
        } else {
            Err(ConnectionError::NotConnected)
        }
    }

    fn receive(
        &mut self,
    ) -> Pin<Box<dyn Stream<Item = Result<Value, ConnectionError>> + Send + 'static>> {
        let recv_rx = self.recv_rx.clone();
        Box::pin(async_stream::stream! {
            let mut rx = recv_rx.lock().await;
            if let Some(rx) = rx.as_mut() {
                while let Some(msg) = rx.recv().await {
                    yield msg;
                }
            }
        })
    }

    fn is_connected(&self) -> bool {
        self.is_connected
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_new_connector() {
        let connector = WasmConnector::new();
        assert!(!connector.is_connected());
    }

    #[wasm_bindgen_test]
    fn test_default() {
        let connector = WasmConnector::default();
        assert!(!connector.is_connected());
    }
}
