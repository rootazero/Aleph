use serde::{Serialize, de::DeserializeOwned};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
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

pub struct RpcClient {
    connector: Arc<Mutex<Box<dyn AlephConnector>>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, RpcError>>>>>,
    next_id: Arc<Mutex<u64>>,
}

impl RpcClient {
    pub fn new(connector: Box<dyn AlephConnector>) -> Self {
        Self {
            connector: Arc::new(Mutex::new(connector)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R, RpcError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let id = {
            let mut id_gen = self.next_id.lock().unwrap();
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
        self.pending.lock().unwrap().insert(id, tx);

        {
            let mut conn = self.connector.lock().unwrap();
            conn.send(request).await?;
        }

        let res = rx.await.map_err(|_| RpcError::ChannelClosed)??;
        
        Ok(serde_json::from_value(res)?)
    }

    pub fn handle_response(&self, response: Value) {
        if let Some(id) = response.get("id").and_then(|id| id.as_str()) {
            if let Some(tx) = self.pending.lock().unwrap().remove(id) {
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
