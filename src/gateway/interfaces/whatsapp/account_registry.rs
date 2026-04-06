//! WhatsApp Account Registry
//!
//! Multi-account support for WhatsApp channels.

use crate::gateway::interfaces::whatsapp::account::{AccountId, WhatsAppAccount};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct WhatsAppAccountRegistry {
    accounts: Arc<RwLock<HashMap<AccountId, Arc<WhatsAppAccount>>>>,
    default_id: Arc<RwLock<Option<AccountId>>>,
}

impl Default for WhatsAppAccountRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WhatsAppAccountRegistry {
    pub fn new() -> Self {
        Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            default_id: Arc::new(RwLock::new(None)),
        }
    }
    
    pub async fn add_account(&self, id: AccountId, account: Arc<WhatsAppAccount>) -> Result<(), String> {
        let mut accounts = self.accounts.write().await;
        let mut default = self.default_id.write().await;
        
        accounts.insert(id.clone(), account);
        
        if default.is_none() {
            *default = Some(id);
        }
        
        Ok(())
    }
    
    pub async fn get_account(&self, id: &AccountId) -> Option<Arc<WhatsAppAccount>> {
        self.accounts.read().await.get(id).cloned()
    }
    
    pub async fn default_account(&self) -> Option<Arc<WhatsAppAccount>> {
        let default_id = self.default_id.read().await.clone();
        if let Some(id) = default_id {
            self.get_account(&id).await
        } else {
            None
        }
    }
    
    pub async fn list_accounts(&self) -> Vec<Arc<WhatsAppAccount>> {
        self.accounts.read().await.values().cloned().collect()
    }
    
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), String> {
        let mut accounts = self.accounts.write().await;
        let mut default = self.default_id.write().await;
        
        accounts.remove(id);
        
        if default.as_ref() == Some(id) {
            *default = accounts.keys().next().cloned();
        }
        
        Ok(())
    }
}
