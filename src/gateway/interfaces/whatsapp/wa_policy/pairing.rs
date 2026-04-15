use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct PairingRequest {
    pub sender_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct PairingTracker {
    requests: Arc<Mutex<HashMap<String, PairingRequest>>>,
    max_pending: usize,
    ttl_secs: u64,
}

impl Default for PairingTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingTracker {
    pub fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashMap::new())),
            max_pending: 3,
            ttl_secs: 3600,
        }
    }

    pub fn add(&self, sender_id: String) -> Result<(), String> {
        let mut req = self.requests.lock().unwrap();
        if req.len() >= self.max_pending {
            return Err("Max pending pairing requests reached".into());
        }
        req.insert(
            sender_id.clone(),
            PairingRequest {
                sender_id,
                created_at: chrono::Utc::now(),
            },
        );
        Ok(())
    }

    pub fn approve(&self, sender_id: &str) -> bool {
        self.requests.lock().unwrap().remove(sender_id).is_some()
    }

    pub fn is_approved_or_pending(&self, sender_id: &str) -> bool {
        let req = self.requests.lock().unwrap();
        req.contains_key(sender_id)
    }

    pub fn prune_expired(&self) {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(self.ttl_secs as i64);
        let mut req = self.requests.lock().unwrap();
        req.retain(|_, v| v.created_at > cutoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_approve() {
        let tracker = PairingTracker::new();
        tracker.add("+1".into()).unwrap();
        assert!(tracker.is_approved_or_pending("+1"));
        assert!(tracker.approve("+1"));
        assert!(!tracker.is_approved_or_pending("+1"));
    }
}
