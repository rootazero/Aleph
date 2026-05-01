//! Property-based tests for budget invariants.

use proptest::prelude::*;
use tempfile::tempdir;
use tokio::runtime::Runtime;

use super::store::{CuratedError, CuratedMemoryStore};

fn entry_str() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,-]{1,40}".prop_map(String::from)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn add_never_exceeds_limit(entries in prop::collection::vec(entry_str(), 0..30)) {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let d = tempdir().unwrap();
            let s = CuratedMemoryStore::load(d.path().join("MEMORY.md"), 200, "p").await.unwrap();
            for e in &entries {
                let _ = s.add(e).await;
            }
            let used = super::budget::used_chars(&s.current_entries());
            prop_assert!(used <= 200);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn remove_decrements_or_errors(initial in prop::collection::vec(entry_str(), 1..10)) {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let d = tempdir().unwrap();
            let s = CuratedMemoryStore::load(d.path().join("MEMORY.md"), 4_000, "p").await.unwrap();
            for e in &initial { let _ = s.add(e).await; }
            let before = s.current_entries().len();
            if let Some(target) = s.current_entries().first().cloned() {
                let r = s.remove(&target).await;
                match r {
                    Ok(_) => prop_assert_eq!(s.current_entries().len(), before - 1),
                    Err(CuratedError::Ambiguous(_)) => prop_assert_eq!(s.current_entries().len(), before),
                    Err(e) => prop_assert!(false, "unexpected: {e}"),
                }
            }
            Ok(())
        }).unwrap();
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::super::store::{CuratedError, CuratedMemoryStore, WriteOutcome};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::task::JoinSet;

    /// Two tokio tasks adding distinct entries concurrently. The fs2 lock
    /// + in-process Mutex must serialize them so both entries land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_adds_do_not_lose_entries() {
        let d = tempdir().unwrap();
        let store: Arc<CuratedMemoryStore> = Arc::new(
            CuratedMemoryStore::load(d.path().join("MEMORY.md"), 1_000, "p")
                .await
                .unwrap(),
        );
        let mut set: JoinSet<Result<WriteOutcome, CuratedError>> = JoinSet::new();
        for i in 0..10 {
            let s = store.clone();
            set.spawn(async move { s.add(&format!("entry {i}")).await });
        }
        let mut ok = 0;
        while let Some(r) = set.join_next().await {
            if r.unwrap().is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, 10, "all 10 distinct adds should succeed under serialization");
        assert_eq!(store.current_entries().len(), 10);
    }
}
