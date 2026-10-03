use super::connector::{AlephConnector, ConnectionError};
use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use futures::Stream;
use serde_json::Value;
use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

#[derive(Default)]
pub struct WasmConnector {
    ws: Option<WebSocket>,
    receiver: Option<mpsc::UnboundedReceiver<Result<Value, ConnectionError>>>,
    is_connected: bool,
    // Owned handles for the four event-listener closures. Each connect()
    // installs a fresh set of closures that capture the per-connection
    // mpsc / oneshot state (tx / fail_slot / open_tx). The closures are
    // passed to the WebSocket via `as_ref().unchecked_ref()`, which only
    // borrows them — so the Rust handle must live as long as the socket
    // references the JS function, otherwise wasm-bindgen drops the inner
    // closure and a later event segfaults.
    //
    // The previous code used `.forget()` on every closure, which leaked
    // the captured state for the lifetime of the process: every reconnect
    // added another mpsc sender + oneshot slot the JS GC could never
    // reach. Now the closures live on this struct and drop with it.
    // `disconnect()` and `Drop` clear the WebSocket's handler refs first
    // so the JS engine is free to release the closures when their last
    // borrow (this struct) goes away.
    open_closure: Option<Closure<dyn FnMut(JsValue)>>,
    msg_closure: Option<Closure<dyn FnMut(MessageEvent)>>,
    err_closure: Option<Closure<dyn FnMut(ErrorEvent)>>,
    close_closure: Option<Closure<dyn FnMut(CloseEvent)>>,
}

impl Drop for WasmConnector {
    fn drop(&mut self) {
        // Clear the socket's JS handlers before the closures drop so the
        // engine can finalize the listener functions in the same tick.
        if let Some(ws) = &self.ws {
            ws.set_onopen(None);
            ws.set_onmessage(None);
            ws.set_onerror(None);
            ws.set_onclose(None);
        }
    }
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

        // Failure side of the open race. `onerror` / `onclose` also publish to
        // the receive stream (below), but nothing polls that stream until
        // *after* connect() returns Ok — so before OPEN it is a dead letter
        // box. Without this second signal a refused upgrade (the gateway
        // answering `403 origin not allowed` / `426` / `503`) is invisible
        // here: connect() would park on `open_rx` forever and the only escape
        // is the caller's open timeout, which reports a live-but-refusing
        // server as "timed out". Whichever handler fires first claims the
        // slot; after OPEN the send lands on a dropped receiver and is a no-op.
        let (fail_tx, fail_rx) = oneshot::channel::<String>();
        let fail_slot = Rc::new(RefCell::new(Some(fail_tx)));
        let onopen_callback = Closure::wrap(Box::new(move |_: JsValue| {
            if let Some(tx) = open_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));

        // OnMessage
        let msg_tx = tx.clone();
        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Some(txt) = e.data().as_string() {
                match serde_json::from_str::<Value>(&txt) {
                    Ok(val) => {
                        let _ = msg_tx.unbounded_send(Ok(val));
                    }
                    Err(e) => {
                        let _ = msg_tx.unbounded_send(Err(ConnectionError::ReceiveFailed(
                            format!("malformed frame: {e}"),
                        )));
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));

        // OnError — surface the error to the receive stream so the message
        // loop can observe it. onerror and onclose are independent events per
        // the WebSocket spec, so an error without a close (e.g. policy
        // violation) would otherwise never reach the receiver.
        let error_tx = tx.clone();
        let error_fail_slot = Rc::clone(&fail_slot);
        let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
            web_sys::console::error_1(&e);
            if let Some(fail) = error_fail_slot.borrow_mut().take() {
                let _ = fail.send("WebSocket error before open".to_string());
            }
            let _ = error_tx.unbounded_send(Err(ConnectionError::ConnectionLost(
                "WebSocket error".into(),
            )));
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));

        // OnClose — surface the close to the receive stream so the message
        // loop's `Err` branch fires, drains pending RPCs, flips is_connected,
        // and triggers auto-reconnect. Without this, a silent socket close is
        // never observed: the leaked onmessage sender keeps the stream alive
        // forever, the loop blocks, is_connected stays `true`, and the only
        // recovery is a full panel restart.
        let close_tx = tx.clone();
        let close_fail_slot = Rc::clone(&fail_slot);
        let onclose_callback = Closure::wrap(Box::new(move |e: CloseEvent| {
            if let Some(fail) = close_fail_slot.borrow_mut().take() {
                // Browsers report a refused upgrade as a plain 1006 with no
                // reason (the HTTP status is deliberately withheld from
                // script), so the code carries no diagnosis — the diagnosis is
                // that it arrived *before OPEN* at all.
                let _ = fail.send(format!(
                    "WebSocket closed before open: code={} reason={}",
                    e.code(),
                    e.reason()
                ));
            }
            let _ = close_tx.unbounded_send(Err(ConnectionError::ConnectionLost(format!(
                "WebSocket closed: code={} reason={}",
                e.code(),
                e.reason()
            ))));
        }) as Box<dyn FnMut(CloseEvent)>);
        ws.set_onclose(Some(onclose_callback.as_ref().unchecked_ref()));

        // Hand the closures to the struct so they live as long as the
        // connection does (and drop with it). Storing them here — instead
        // of `.forget()` — is what stops every reconnect from leaking the
        // captured mpsc / oneshot state forever.
        self.open_closure = Some(onopen_callback);
        self.msg_closure = Some(onmessage_callback);
        self.err_closure = Some(onerror_callback);
        self.close_closure = Some(onclose_callback);

        self.ws = Some(ws);
        self.receiver = Some(rx);

        // Race OPEN against refusal. Awaiting `open_rx` alone cannot fail:
        // its sender lives inside a `forget()`-leaked closure and is therefore
        // never dropped, so the "signal dropped" arm was unreachable and a
        // rejected handshake hung here indefinitely.
        match futures::future::select(open_rx, fail_rx).await {
            futures::future::Either::Left((Ok(()), _)) => {}
            futures::future::Either::Left((Err(_), _)) => {
                return Err(ConnectionError::ConnectFailed(
                    "WebSocket onopen signal dropped".to_string(),
                ))
            }
            futures::future::Either::Right((Ok(reason), _)) => {
                return Err(ConnectionError::FailedBeforeOpen(reason))
            }
            futures::future::Either::Right((Err(_), _)) => {
                return Err(ConnectionError::ConnectFailed(
                    "WebSocket failure signal dropped".to_string(),
                ))
            }
        }

        self.is_connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), ConnectionError> {
        // Clear JS handlers BEFORE dropping the socket so the engine can
        // finalize the listener closures in step with the Rust drop — and
        // so the closures' captured state (mpsc senders / oneshot slot)
        // doesn't outlive the connection it served.
        if let Some(ws) = &self.ws {
            ws.set_onopen(None);
            ws.set_onmessage(None);
            ws.set_onerror(None);
            ws.set_onclose(None);
        }
        if let Some(ws) = self.ws.take() {
            let _ = ws.close();
        }
        self.open_closure = None;
        self.msg_closure = None;
        self.err_closure = None;
        self.close_closure = None;
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
