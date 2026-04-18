//! WhatsApp Account
//!
//! Single WhatsApp account instance with state management.

use crate::gateway::channel::ChannelHealth;
use crate::gateway::channel_policy::E164Number;
use crate::gateway::interfaces::whatsapp::pairing::PairingState;
use chrono::{DateTime, Utc};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Default)]
pub enum AccountState {
    #[default]
    Disconnected,
    Connecting,
    Connected {
        since: DateTime<Utc>,
    },
    Error {
        message: String,
        since: DateTime<Utc>,
    },
}

pub struct WhatsAppAccount {
    pub id: AccountId,
    pub phone_number: Option<E164Number>,
    pub device_name: String,
    pub state: Arc<RwLock<AccountState>>,
    pub pairing: Arc<RwLock<PairingState>>,
    pub health: Arc<RwLock<ChannelHealth>>,
    pub auth_key: String,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl WhatsAppAccount {
    pub fn new(id: AccountId) -> Self {
        let auth_key = id.as_str().to_string();
        Self {
            id,
            phone_number: None,
            device_name: String::new(),
            state: Arc::new(RwLock::new(AccountState::Disconnected)),
            pairing: Arc::new(RwLock::new(PairingState::Idle)),
            health: Arc::new(RwLock::new(ChannelHealth::new())),
            auth_key,
        }
    }

    pub async fn is_connected(&self) -> bool {
        matches!(*self.state.read().await, AccountState::Connected { .. })
    }
}
