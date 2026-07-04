use crate::session::events::SessionEventRecord;
use crate::session::service::SessionId;

/// Fires exactly once per *newly appended* event (never on actor replay).
/// Must be non-blocking: implementations enqueue and return immediately.
pub trait SessionEventObserver: Send + Sync {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord);
}
