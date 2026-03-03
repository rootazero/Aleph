use super::connector::{AlephConnector, ConnectionError};
use async_trait::async_trait;
use futures::Stream;
use futures::channel::{mpsc, oneshot};
use serde_json::Value;
use std::cell::RefCell;
use std::pin::Pin;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, MessageEvent, WebSocket};

#[derive(Default)]
pub struct WasmConnector {
    ws: Option<WebSocket>,
    receiver: Option<mpsc::UnboundedReceiver<Result<Value, ConnectionError>>>,
    is_connected: bool,
}

impl WasmConnector {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait(?Send)]
impl AlephConnector for WasmConnector {
    async fn connect(&mut self, url: &str) -> Result<(), ConnectionError> {
        let ws = WebSocket::new(url).map_err(|e| ConnectionError::ConnectFailed(format!("{:?}", e)))?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let (tx, rx) = mpsc::unbounded();

        // OnOpen — signal readiness via oneshot channel
        let (open_tx, open_rx) = oneshot::channel::<()>();
        let open_tx = RefCell::new(Some(open_tx));
        let onopen_callback = Closure::wrap(Box::new(move |_: JsValue| {
            if let Some(tx) = open_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
        onopen_callback.forget();

        // OnMessage
        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Some(txt) = e.data().as_string() {
                if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                    let _ = tx.unbounded_send(Ok(val));
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        // OnError
        let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
            web_sys::console::error_1(&e);
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
        onerror_callback.forget();

        self.ws = Some(ws);
        self.receiver = Some(rx);

        // Wait for WebSocket to reach OPEN state before returning
        open_rx.await.map_err(|_| ConnectionError::ConnectFailed(
            "WebSocket onopen signal dropped".to_string(),
        ))?;

        self.is_connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), ConnectionError> {
        if let Some(ws) = self.ws.take() {
            let _ = ws.close();
        }
        self.is_connected = false;
        Ok(())
    }

    async fn send(&mut self, message: Value) -> Result<(), ConnectionError> {
        if let Some(ws) = &self.ws {
            let txt = serde_json::to_string(&message).map_err(|e| ConnectionError::SendFailed(e.to_string()))?;
            ws.send_with_str(&txt).map_err(|e| ConnectionError::SendFailed(format!("{:?}", e)))?;
            Ok(())
        } else {
            Err(ConnectionError::SendFailed("Not connected".into()))
        }
    }

    fn receive(&mut self) -> Pin<Box<dyn Stream<Item = Result<Value, ConnectionError>>>> {
        if let Some(rx) = self.receiver.take() {
            Box::pin(rx)
        } else {
            Box::pin(futures::stream::empty())
        }
    }

    fn is_connected(&self) -> bool {
        self.is_connected
    }
}
