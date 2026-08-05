#[cfg(test)]
mod spec1_tests {
    use crate::memory::store::raw_memory::{
        RawMemory, RawMemorySource, RawMemoryStore, SessionEndReason,
    };
    use crate::sync_primitives::{Arc, Mutex};

    use super::super::emit_session_end_raw;

    #[derive(Default)]
    struct FakeWriter(Mutex<Vec<RawMemory>>);

    #[async_trait::async_trait]
    impl RawMemoryStore for FakeWriter {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), crate::error::AlephError> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(raw.clone());
            Ok(())
        }

        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, crate::error::AlephError> {
            Ok(vec![])
        }

        async fn mark_raw_as_processed(
            &self,
            _ids: &[String],
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }

        async fn count_unprocessed(
            &self,
            _agent_id: &str,
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }

        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, crate::error::AlephError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn emit_session_end_writes_disconnect_row() {
        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();
        emit_session_end_raw(
            writer,
            "agent-x".into(),
            "sess-y".into(),
            "user: hi\nassistant: yo".into(),
            SessionEndReason::Disconnect,
            None,
            None,
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let captured = fake.0.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].agent_id, "agent-x");
        assert_eq!(captured[0].session_id.as_deref(), Some("sess-y"));
        assert!(matches!(
            captured[0].source,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::Disconnect
            }
        ));
    }

    #[tokio::test]
    async fn empty_tail_does_not_emit() {
        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();
        emit_session_end_raw(
            writer,
            "agent-x".into(),
            "sess-y".into(),
            String::new(),
            SessionEndReason::Disconnect,
            None,
            None,
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(fake.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
    }
}
