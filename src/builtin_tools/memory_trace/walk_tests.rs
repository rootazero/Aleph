use super::*;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::{RawMemory, RawMemorySource, SqliteMemoryBackend};
use aleph_protocol::memory_trace::NoteSourceStatus;
use std::path::PathBuf;

struct Corpus {
    dir: tempfile::TempDir,
    db: MemoryBackend,
    raw: Arc<MarkdownSessionStore>,
}

impl Corpus {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("notes.db")).unwrap());
        let raw = Arc::new(
            MarkdownSessionStore::new(&dir.path().join("sessions"))
                .await
                .unwrap(),
        );
        Self { dir, db, raw }
    }

    async fn write(&self, partition: &str, category: &str, note: &KnowledgeNote) -> PathBuf {
        NoteIndexer::new(self.dir.path().join("notes"), self.db.clone())
            .write_note(partition, category, note)
            .await
            .unwrap()
    }

    async fn add_raw(&self, partition: &str, id: &str, content: &str) {
        let mut raw = RawMemory::new(content.into(), RawMemorySource::Transcript);
        raw.id = id.into();
        raw.agent_id = partition.into();
        self.raw.insert_raw_memory(&raw).await.unwrap();
    }

    async fn trace(
        &self,
        partition: &str,
        kind: TraceKind,
        target: &str,
        max_results: Option<usize>,
    ) -> TraceResult {
        MemoryTraceTool::new(
            self.db.clone(),
            self.raw.clone(),
            partition,
            self.dir.path().join("notes"),
        )
        .call_impl(MemoryTraceArgs {
            target: target.into(),
            kind,
            max_results,
        })
        .await
        .unwrap()
    }
}

fn note(title: &str, sources: &[&str], facts: &[&str]) -> KnowledgeNote {
    KnowledgeNote {
        title: title.into(),
        source_notes: sources.iter().map(|source| (*source).into()).collect(),
        facts: facts.iter().map(|fact| (*fact).into()).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn trace_follows_note_and_fact_citations() {
    let corpus = Corpus::new().await;
    corpus.add_raw("main", "raw-1", "First transcript").await;
    corpus.add_raw("main", "raw-2", "Second transcript").await;
    corpus
        .add_raw("main", "raw-not-cited", "Not evidence")
        .await;
    corpus
        .write("main", "reference", &note("l1", &["raw-1"], &[]))
        .await;
    corpus
        .write("main", "reference", &note("fact-source", &[], &[]))
        .await;
    corpus.write("main", "synthesis", &note("test", &["reference/l1"], &[
        "First fact <!-- src: raw-2, origin: raw_source, inferred: false -->",
        "Second fact <!-- src: reference/fact-source, origin: prior_note, inferred: false -->",
        "Inference <!-- src: raw-not-cited, origin: inferred, inferred: true -->",
        "Structure <!-- src: raw-not-cited, origin: system, inferred: false -->",
        "Manual addition <!-- src: raw-not-cited, origin: legacy, inferred: false -->",
    ])).await;

    let result = corpus
        .trace("main", TraceKind::Note, "synthesis/test", None)
        .await;
    assert_eq!(result.notes, ["synthesis/test"]);
    let edges: Vec<_> = result
        .note_sources
        .iter()
        .map(|edge| {
            (
                edge.note_path.as_str(),
                edge.source_path.as_str(),
                edge.status,
            )
        })
        .collect();
    assert_eq!(
        edges,
        [
            (
                "synthesis/test",
                "reference/fact-source",
                NoteSourceStatus::Available
            ),
            (
                "synthesis/test",
                "reference/l1",
                NoteSourceStatus::Available
            ),
        ]
    );
    let evidence: Vec<_> = result
        .evidence
        .iter()
        .map(|row| {
            (
                row.raw_id.as_str(),
                row.via_note.as_deref(),
                row.content.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        evidence,
        [
            ("raw-2", Some("synthesis/test"), Some("Second transcript")),
            ("raw-1", Some("reference/l1"), Some("First transcript")),
        ]
    );
    assert_eq!(result.facts.len(), 5);
    assert_eq!(result.facts[0].text, "First fact");
    assert_eq!(result.facts[0].source_id.as_deref(), Some("raw-2"));
    assert!(!result.incomplete && !result.truncated);
}

#[tokio::test]
async fn trace_uses_current_markdown_not_stale_projection() {
    let corpus = Corpus::new().await;
    corpus
        .add_raw("main", "old", "Stale indexed transcript")
        .await;
    corpus.add_raw("main", "new", "Current transcript").await;
    let path = corpus
        .write("main", "reference", &note("edit", &["old"], &[]))
        .await;
    tokio::fs::write(
        &path,
        note("edit", &["new"], &["Current fact"]).to_markdown(),
    )
    .await
    .unwrap();

    let current = corpus
        .trace("main", TraceKind::Note, "reference/edit", None)
        .await;
    assert_eq!(current.evidence.len(), 1);
    assert_eq!(current.evidence[0].raw_id, "new");
    assert_eq!(current.facts[0].text, "Current fact");
    assert!(!current.incomplete);

    tokio::fs::remove_file(path).await.unwrap();
    let fallback = corpus
        .trace("main", TraceKind::Note, "reference/edit", None)
        .await;
    assert_eq!(fallback.evidence.len(), 1);
    assert_eq!(fallback.evidence[0].raw_id, "old");
    assert!(fallback.facts.is_empty() && fallback.incomplete);
}

#[tokio::test]
async fn trace_legacy_raw_match_beats_path_but_typed_prior_note_does_not() {
    let corpus = Corpus::new().await;
    corpus
        .add_raw("main", "reference/peer", "Matching raw ID")
        .await;
    corpus
        .write("main", "reference", &note("peer", &[], &[]))
        .await;
    corpus
        .write(
            "main",
            "reference",
            &note(
                "root",
                &["reference/peer"],
                &["Peer fact <!-- src: reference/peer, origin: prior_note, inferred: false -->"],
            ),
        )
        .await;

    let result = corpus
        .trace("main", TraceKind::Note, "reference/root", None)
        .await;
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(
        result.evidence[0].content.as_deref(),
        Some("Matching raw ID")
    );
    assert_eq!(result.note_sources.len(), 1);
    assert_eq!(result.note_sources[0].source_path, "reference/peer");
    assert_eq!(result.note_sources[0].status, NoteSourceStatus::Available);
}

#[tokio::test]
async fn trace_keeps_both_diamond_edges_and_terminates_cycles() {
    let corpus = Corpus::new().await;
    corpus.add_raw("main", "raw-1", "Only one transcript").await;
    corpus
        .write(
            "main",
            "reference",
            &note("root", &["reference/b", "reference/a"], &["Fact"]),
        )
        .await;
    corpus
        .write("main", "reference", &note("a", &["reference/c"], &[]))
        .await;
    corpus
        .write("main", "reference", &note("b", &["reference/c"], &[]))
        .await;
    corpus
        .write(
            "main",
            "reference",
            &note("c", &["reference/root", "raw-1"], &[]),
        )
        .await;

    let result = corpus
        .trace("main", TraceKind::Note, "reference/root", Some(1))
        .await;
    let edges: Vec<_> = result
        .note_sources
        .iter()
        .map(|edge| (edge.note_path.as_str(), edge.source_path.as_str()))
        .collect();
    assert_eq!(
        edges,
        [
            ("reference/root", "reference/a"),
            ("reference/root", "reference/b"),
            ("reference/a", "reference/c"),
            ("reference/b", "reference/c"),
            ("reference/c", "reference/root"),
        ]
    );
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].via_note.as_deref(), Some("reference/c"));
    assert!(!result.truncated && !result.incomplete);

    let zero = corpus
        .trace("main", TraceKind::Note, "reference/root", Some(0))
        .await;
    assert!(zero.evidence.is_empty() && zero.truncated);
    assert_eq!(zero.facts[0].text, "Fact");
    assert_eq!(zero.note_sources, result.note_sources);
}

#[tokio::test]
async fn trace_reports_budget_omissions_separately_from_missing_sources() {
    let corpus = Corpus::new().await;
    let paths: Vec<_> = (0..65)
        .map(|index| format!("reference/child-{index:02}"))
        .collect();
    for path in &paths {
        let name = path.strip_prefix("reference/").unwrap();
        corpus
            .write("main", "reference", &note(name, &[], &[]))
            .await;
    }
    let mut root = note("root", &[], &[]);
    root.source_notes = paths;
    root.facts = (0..101).map(|index| format!("Fact {index}")).collect();
    corpus.write("main", "reference", &root).await;
    let result = corpus
        .trace("main", TraceKind::Note, "reference/root", None)
        .await;
    assert!(result.truncated && !result.incomplete);
    assert_eq!(result.facts.len(), 100);
    assert_eq!(
        result
            .note_sources
            .iter()
            .filter(|edge| edge.status == NoteSourceStatus::Available)
            .count(),
        63
    );
    assert_eq!(result.note_sources[63].status, NoteSourceStatus::NotVisited);
    assert_eq!(result.note_sources.len(), 65);

    corpus
        .write(
            "main",
            "reference",
            &note("missing", &["reference/no-child", "raw-absent"], &[]),
        )
        .await;
    let missing = corpus
        .trace("main", TraceKind::Note, "reference/missing", None)
        .await;
    assert!(!missing.truncated && missing.incomplete);
    assert_eq!(
        missing.note_sources[0].status,
        NoteSourceStatus::Unavailable
    );
    assert!(missing.evidence[0].pruned);
}

#[tokio::test]
async fn trace_prioritizes_the_raw_target_over_bounded_reverse_matches() {
    let corpus = Corpus::new().await;
    corpus
        .add_raw("main", "raw-popular", "The requested transcript")
        .await;
    for index in 0..501 {
        let row = note(&format!("citation-{index:03}"), &["raw-popular"], &[]);
        corpus
            .db
            .index_note(&row, "main", "reference")
            .await
            .unwrap();
    }
    let result = corpus
        .trace("main", TraceKind::Raw, "raw-popular", Some(1))
        .await;
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(
        result.evidence[0].content.as_deref(),
        Some("The requested transcript")
    );
    assert_eq!(result.evidence[0].via_note, None);
    assert!(result.truncated);
    assert_eq!(result.notes.len(), 500);
    assert_eq!(result.notes[0], "reference/citation-000");
    assert_eq!(result.notes[499], "reference/citation-499");
}

#[tokio::test]
async fn trace_uses_same_partition_and_rejects_lossy_note_addresses() {
    let corpus = Corpus::new().await;
    corpus.add_raw("main", "org", "Org transcript").await;
    corpus
        .add_raw("main__u-alice", "alice", "Personal transcript")
        .await;
    corpus
        .write("main", "reference", &note("entry", &["org"], &["Org fact"]))
        .await;
    corpus
        .write(
            "main__u-alice",
            "reference",
            &note("entry", &["alice", "org"], &["Personal fact"]),
        )
        .await;

    let personal = corpus
        .trace("main__u-alice", TraceKind::Note, "reference/entry", None)
        .await;
    assert_eq!(personal.facts[0].text, "Personal fact");
    assert_eq!(
        personal.evidence[0].content.as_deref(),
        Some("Personal transcript")
    );
    assert_eq!(personal.evidence[1].raw_id, "org");
    assert!(personal.evidence[1].pruned && personal.incomplete);

    for (agent, target) in [
        ("MAIN", "reference/entry"),
        ("main", "reference/missing"),
        ("main", "reference//entry"),
        ("main", "reference/../entry"),
        ("main", "reference\\entry"),
        ("main", "reference/en*try"),
        ("../main", "reference/entry"),
    ] {
        assert_eq!(
            corpus.trace(agent, TraceKind::Note, target, None).await,
            TraceResult {
                target: target.into(),
                ..TraceResult::default()
            }
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn trace_does_not_read_through_cross_partition_symlinks() {
    use std::os::unix::fs::symlink;

    let corpus = Corpus::new().await;
    let secret = corpus
        .write(
            "main__u-bob",
            "reference",
            &note("entry", &[], &["Private fact"]),
        )
        .await;
    let root = corpus.dir.path().join("notes");
    symlink(root.join("main__u-bob"), root.join("alias")).unwrap();
    let alias = corpus
        .trace("alias", TraceKind::Note, "reference/entry", None)
        .await;
    assert!(alias.notes.is_empty() && alias.facts.is_empty());

    let public = corpus
        .write("main", "reference", &note("entry", &[], &["Public fact"]))
        .await;
    tokio::fs::remove_file(&public).await.unwrap();
    symlink(secret, &public).unwrap();
    let escaped = corpus
        .trace("main", TraceKind::Note, "reference/entry", None)
        .await;
    assert_eq!(escaped.notes, ["reference/entry"]);
    assert!(escaped.facts.is_empty() && escaped.incomplete);

    let bob_profile = root.join("main__u-bob/USER.md");
    tokio::fs::write(&bob_profile, "## Sources\n")
        .await
        .unwrap();
    symlink(bob_profile, root.join("main/USER.md")).unwrap();
    let profile = corpus
        .trace("main", TraceKind::ProfileSection, "Identity", None)
        .await;
    assert!(profile.notes.is_empty() && profile.incomplete);
}

#[tokio::test]
async fn trace_bounds_note_and_profile_file_reads() {
    let corpus = Corpus::new().await;
    let note_path = corpus
        .write("main", "reference", &note("entry", &[], &["Fact"]))
        .await;
    let too_large = vec![b'a'; 4 * 1024 * 1024 + 1];
    tokio::fs::write(&note_path, &too_large).await.unwrap();
    tokio::fs::write(corpus.dir.path().join("notes/main/USER.md"), &too_large)
        .await
        .unwrap();
    let note = corpus
        .trace("main", TraceKind::Note, "reference/entry", None)
        .await;
    assert_eq!(note.notes, ["reference/entry"]);
    assert!(note.facts.is_empty() && note.incomplete && !note.truncated);
    let profile = corpus
        .trace("main", TraceKind::ProfileSection, "Identity", None)
        .await;
    assert!(profile.notes.is_empty() && profile.incomplete && !profile.truncated);
}
