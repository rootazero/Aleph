use serde::{Serialize, de::DeserializeOwned};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use futures::channel::oneshot;
use crate::connection::connector::{AlephConnector, ConnectionError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RpcError {
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("RPC error: {0}")]
    ServerError(String),
    #[error("Timeout")]
    Timeout,
    #[error("Response channel closed")]
    ChannelClosed,
}

/// Pending RPC response senders, keyed by request ID.
type PendingMap = HashMap<String, oneshot::Sender<Result<Value, RpcError>>>;

pub struct RpcClient {
    connector: Rc<RefCell<Box<dyn AlephConnector>>>,
    pending: Rc<RefCell<PendingMap>>,
    next_id: RefCell<u64>,
}

impl RpcClient {
    pub fn new(connector: Box<dyn AlephConnector>) -> Self {
        Self {
            connector: Rc::new(RefCell::new(connector)),
            pending: Rc::new(RefCell::new(HashMap::new())),
            next_id: RefCell::new(1),
        }
    }

    // WASM is single-threaded; holding RefCell borrow across await is safe.
    #[allow(clippy::await_holding_refcell_ref)]
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R, RpcError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let id = {
            let mut id_gen = self.next_id.borrow_mut();
            let id = *id_gen;
            *id_gen += 1;
            id.to_string()
        };

        let request = json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "method": method,
            "params": params
        });

        let (tx, rx) = oneshot::channel();
        self.pending.borrow_mut().insert(id, tx);

        self.connector.borrow_mut().send(request).await?;

        let res = rx.await.map_err(|_| RpcError::ChannelClosed)??;

        Ok(serde_json::from_value(res)?)
    }

    pub fn handle_response(&self, response: Value) {
        if let Some(id) = response.get("id").and_then(|id| id.as_str()) {
            if let Some(tx) = self.pending.borrow_mut().remove(id) {
                if let Some(error) = response.get("error") {
                    let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error");
                    let _ = tx.send(Err(RpcError::ServerError(msg.to_string())));
                } else if let Some(result) = response.get("result") {
                    let _ = tx.send(Ok(result.clone()));
                }
            }
        }
    }
}
