//! Composite Observer - Dispatches session events to multiple observers.

use crate::session::events::SessionEventRecord;
use crate::session::observer::SessionEventObserver;
use crate::session::service::SessionId;
use std::sync::Arc;

/// Composite observer that dispatches events to multiple child observers.
///
/// This allows registering multiple projectors (e.g., MessageProjector + MarkdownProjector)
/// in the session event pipeline without modifying the core service architecture.
pub struct CompositeObserver {
    observers: Vec<Arc<dyn SessionEventObserver>>,
}

impl CompositeObserver {
    /// Create a new composite observer with the given child observers.
    pub fn new(observers: Vec<Arc<dyn SessionEventObserver>>) -> Self {
        Self { observers }
    }
}

impl SessionEventObserver for CompositeObserver {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        for observer in &self.observers {
            observer.on_appended(id, record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct CountingObserver(AtomicU64);

    impl SessionEventObserver for CountingObserver {
        fn on_appended(&self, _id: &SessionId, _record: &SessionEventRecord) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_composite_dispatches_to_all_observers() {
        let counter1 = Arc::new(CountingObserver(AtomicU64::new(0)));
        let counter2 = Arc::new(CountingObserver(AtomicU64::new(0)));

        let composite = CompositeObserver::new(vec![
            counter1.clone() as Arc<dyn SessionEventObserver>,
            counter2.clone() as Arc<dyn SessionEventObserver>,
        ]);

        let session_id = crate::routing::session_key::SessionKey::ephemeral("test");
        let record = SessionEventRecord {
            seq: 1,
            event: crate::session::events::SessionEvent::RunStarted {
                run_id: "test-run".to_string(),
                at: 0,
                project_root: None,
                envelope: None,
                telecom_provenance: Default::default(),
            },
            created_at_ms: 0,
        };

        composite.on_appended(&session_id, &record);

        assert_eq!(counter1.0.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.0.load(Ordering::SeqCst), 1);
    }
}
