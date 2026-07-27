use super::connector::{AlephConnector, ConnectionError};
use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use futures::Stream;
use serde_json::Value;
use std::cell::RefCell;
use std::pin::Pin;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

#[derive(Default)]
pub struct WasmConnector {
    ws: Option<WebSocket>,
    receiver: Option<mpsc::UnboundedReceiver<Result<Value, ConnectionError>>>,
    is_connected: bool,
}

impl WasmConnector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait(?Send)]
impl AlephConnector for WasmConnector {
    async fn connect(&mut self, url: &str) -> Result<(), ConnectionError> {
        let ws =
            WebSocket::new(url).map_err(|e| ConnectionError::ConnectFailed(format!("{e:?}")))?;
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
        let msg_tx = tx.clone();
        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Some(txt) = e.data().as_string() {
                match serde_json::from_str::<Value>(&txt) {
                    Ok(val) => {
                        let _ = msg_tx.unbounded_send(Ok(val));
                    }
                    Err(e) => {
                        let _ = msg_tx.unbounded_send(Err(
                            ConnectionError::ReceiveFailed(format!(
                                "malformed frame: {e}"
                            )),
                        ));
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        // OnError — surface the error to the receive stream so the message
        // loop can observe it. onerror and onclose are independent events per
        // the WebSocket spec, so an error without a close (e.g. policy
        // violation) would otherwise never reach the receiver.
        let error_tx = tx.clone();
        let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
            web_sys::console::error_1(&e);
            let _ = error_tx.unbounded_send(Err(ConnectionError::ConnectionLost(
                "WebSocket error".into(),
            )));
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
        onerror_callback.forget();

        // OnClose — surface the close to the receive stream so the message
        // loop's `Err` branch fires, drains pending RPCs, flips is_connected,
        // and triggers auto-reconnect. Without this, a silent socket close is
        // never observed: the leaked onmessage sender keeps the stream alive
        // forever, the loop blocks, is_connected stays `true`, and the only
        // recovery is a full panel restart.
        let close_tx = tx.clone();
        let onclose_callback = Closure::wrap(Box::new(move |e: CloseEvent| {
            let _ = close_tx.unbounded_send(Err(ConnectionError::ConnectionLost(format!(
                "WebSocket closed: code={} reason={}",
                e.code(),
                e.reason()
            ))));
        }) as Box<dyn FnMut(CloseEvent)>);
        ws.set_onclose(Some(onclose_callback.as_ref().unchecked_ref()));
        onclose_callback.forget();

        self.ws = Some(ws);
        self.receiver = Some(rx);

        // Wait for WebSocket to reach OPEN state before returning
        open_rx.await.map_err(|_| {
            ConnectionError::ConnectFailed("WebSocket onopen signal dropped".to_string())
        })?;

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
            let txt = serde_json::to_string(&message)
                .map_err(|e| ConnectionError::SendFailed(e.to_string()))?;
            ws.send_with_str(&txt)
                .map_err(|e| ConnectionError::SendFailed(format!("{e:?}")))?;
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
