//! Record a `recall_signals` row per note fed into a `reflect()` synthesis call.

use crate::error::AlephError;
use crate::memory::reflector::packet_adapter::NoteMeta;
use crate::memory::reflector::types::ReflectOpts;
use std::collections::HashMap;

pub const REFLECT_CHANNEL: &str = "reflect";

/// Shape of one recall-signal row as seen by the reflector. The server-side
/// wiring (Task 8) unpacks this into whatever `record_signals(...)` the
/// concrete sqlite store exposes.
#[derive(Debug, Clone)]
pub struct SignalRow {
    pub note_path: String,
    /// Recording agent — the recall-signal scope key (per-agent partition).
    pub agent_id: String,
    pub query_text: String,
    pub channel: String,
    pub score: f32,
    pub session_id: Option<String>,
    pub namespace: String,
}

/// Write one signal per note in `note_lookup`. The first arg is a
/// user-supplied async callable — no new trait needed.
pub async fn record_reflect_signals<F, Fut>(
    record: F,
    query: &str,
    note_lookup: &HashMap<String, NoteMeta>,
    opts: &ReflectOpts,
) -> Result<(), AlephError>
where
    F: Fn(SignalRow) -> Fut,
    Fut: std::future::Future<Output = Result<(), AlephError>>,
{
    for (path, meta) in note_lookup {
        let row = SignalRow {
            // rust-doctor-disable-next-line excessive-clone
            note_path: path.clone(),
            // rust-doctor-disable-next-line excessive-clone
            agent_id: opts.agent_id.clone(),
            query_text: query.to_string(),
            channel: REFLECT_CHANNEL.to_string(),
            score: meta.relevance,
            // rust-doctor-disable-next-line excessive-clone
            session_id: opts.session_id.clone(),
            namespace: opts.namespace.to_namespace_value(),
        };
        record(row).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflector::packet_adapter::NoteMeta;
    use crate::memory::reflector::types::ReflectOpts;

    #[tokio::test]
    async fn records_one_row_per_note() {
        let captured =
            crate::sync_primitives::Arc::new(crate::sync_primitives::Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = move |row: SignalRow| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(row);
                Ok::<(), AlephError>(())
            }
        };
        let mut lookup = HashMap::new();
        lookup.insert(
            "reference/a".to_string(),
            NoteMeta {
                title: "A".into(),
                relevance: 0.5,
            },
        );
        lookup.insert(
            "reference/b".to_string(),
            NoteMeta {
                title: "B".into(),
                relevance: 0.8,
            },
        );
        let opts = ReflectOpts::for_agent("x");

        record_reflect_signals(record, "q", &lookup, &opts)
            .await
            .unwrap();

        let rows = captured.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.channel == "reflect"));
        assert!(rows.iter().all(|r| r.query_text == "q"));
    }

    #[tokio::test]
    async fn empty_lookup_writes_no_rows() {
        let captured =
            crate::sync_primitives::Arc::new(crate::sync_primitives::Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = move |row: SignalRow| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(row);
                Ok::<(), AlephError>(())
            }
        };
        let opts = ReflectOpts::for_agent("x");
        record_reflect_signals(record, "q", &HashMap::new(), &opts)
            .await
            .unwrap();
        assert!(captured.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn namespace_owner_serializes_to_expected_token() {
        // Sanity: Owner scope should produce "owner" — matches the sqlite
        // schema's default namespace column value.
        let opts = ReflectOpts::for_agent("x");
        let captured =
            crate::sync_primitives::Arc::new(crate::sync_primitives::Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = move |row: SignalRow| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(row);
                Ok::<(), AlephError>(())
            }
        };
        let mut lookup = HashMap::new();
        lookup.insert(
            "p".to_string(),
            NoteMeta {
                title: "t".into(),
                relevance: 0.1,
            },
        );
        record_reflect_signals(record, "q", &lookup, &opts)
            .await
            .unwrap();
        let rows = captured.lock().unwrap();
        assert_eq!(rows[0].namespace, "owner");
    }
}
