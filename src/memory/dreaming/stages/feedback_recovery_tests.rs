use super::tests::{applied_rules, ctx_with_three_corrections, stage};
use crate::error::AlephError;
use crate::memory::dreaming::stages::feedback_state::FeedbackSnapshot;
use crate::memory::dreaming::{DreamContext, DreamReport, DreamStage};
use crate::memory::notes::{store::NoteStore, NoteIndexer};
use crate::memory::store::raw_memory::{
    RawMemory, RawMemorySource, RawMemoryStore, CORRECTION_PATH_PREFIX,
};
use crate::memory::store::{markdown::MarkdownSessionStore, SqliteMemoryBackend};
use crate::providers::mock::MockProvider;
use crate::routing::DEFAULT_AGENT_ID;
use std::path::Path;
use std::sync::Arc;

const FIRST_RULE: &str = "feedback/verify-actual-corpus";
const RESPONSE: &str = r#"{"actions":[
    {"type":"new","title":"verify-actual-corpus","rule":"Verify the active corpus before publishing notes.",
     "confidence":0.9,"severity":"high","source_facts":["c0"]},
    {"type":"skip","source_fact":"c1","reason":"transient"},
    {"type":"skip","source_fact":"c2","reason":"transient"}
]}"#;

async fn inject_state_write_failure(ctx: &DreamContext, condition: &str) {
    let sql = format!(
        "CREATE TRIGGER stop_feedback_once BEFORE UPDATE OF value ON compression_metadata
         WHEN NEW.key = 'dream_feedback_batch__{DEFAULT_AGENT_ID}' AND ({condition})
         BEGIN SELECT RAISE(ABORT, 'test: feedback state write interrupted'); END;"
    );
    ctx.database
        .with_conn(move |conn| {
            conn.execute_batch(&sql)
                .map_err(|err| AlephError::other(err.to_string()))
        })
        .await
        .unwrap();
}

async fn reopen(mut ctx: DreamContext, root: &Path) -> DreamContext {
    let database = Arc::new(SqliteMemoryBackend::new(&root.join("mem.db")).unwrap());
    ctx.indexer = NoteIndexer::new(root.join("note"), database.clone());
    ctx.raw_store = Some(Arc::new(MarkdownSessionStore::new(root).await.unwrap()));
    ctx.database = database;
    ctx.database
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER IF EXISTS stop_feedback_once")
                .map_err(|err| AlephError::other(err.to_string()))
        })
        .await
        .unwrap();
    // A re-plan with this provider would fail decision decoding. Prove resume
    // from the existing effect, not just that a helper can read saved JSON.
    ctx.provider = Arc::new(MockProvider::new("unexpected new provider call"));
    ctx.report = DreamReport::default();
    ctx.evolution_budget = Default::default();
    ctx
}

fn committed(ctx: &DreamContext) -> i64 {
    FeedbackSnapshot::load(&ctx.database, DEFAULT_AGENT_ID)
        .unwrap()
        .state
        .committed_seq
}

#[tokio::test]
async fn corrupt_persisted_feedback_batch_remains_blocked() {
    let (ctx, _, dir) = ctx_with_three_corrections(RESPONSE).await;
    let corrupted = r#"{"version":2,"committed_seq":0,"pending":null}"#;
    let saved = ctx
        .database
        .compare_exchange_feedback_batch(DEFAULT_AGENT_ID, None, corrupted)
        .unwrap();
    assert!(saved);

    let ctx = reopen(ctx, dir.path()).await;
    let ctx = stage().execute(ctx).await.unwrap();
    assert!(ctx
        .report
        .errors
        .as_deref()
        .is_some_and(|detail| detail.contains("state is unavailable or invalid")));
    assert!(ctx.report.distill_actions.is_empty());
    assert!(projected(&ctx).await.is_none());
    assert_eq!(
        ctx.database
            .feedback_batch_record(DEFAULT_AGENT_ID)
            .unwrap()
            .as_deref(),
        Some(corrupted),
        "unknown versions must remain intact instead of being reset or consumed",
    );
}

async fn projected(ctx: &DreamContext) -> Option<String> {
    ctx.database
        .get_note_index(FIRST_RULE, DEFAULT_AGENT_ID)
        .await
        .unwrap()
        .map(|note| note.content_hash)
}

#[tokio::test]
async fn feedback_recovers_the_same_plan_at_three_persistence_boundaries() {
    for (label, condition, effect_visible, counted_after_resume) in [
        (
            "before effect",
            "instr(NEW.value, '\"status\":\"prepared\"') > 0",
            false,
            1,
        ),
        (
            "after effect before receipt",
            "instr(NEW.value, '\"outcome\":\"applied\"') > 0",
            true,
            1,
        ),
        (
            "after receipt before cursor",
            "instr(NEW.value, '\"pending\":null') > 0",
            true,
            0,
        ),
    ] {
        let (ctx, _, dir) = ctx_with_three_corrections(RESPONSE).await;
        let file = dir
            .path()
            .join("note")
            .join(DEFAULT_AGENT_ID)
            .join("feedback/verify-actual-corpus.md");
        inject_state_write_failure(&ctx, condition).await;
        let ctx = stage().execute(ctx).await.unwrap();
        assert!(
            ctx.report.errors.is_some(),
            "{label}: interruption must be visible"
        );
        assert_eq!(
            committed(&ctx),
            0,
            "{label}: cursor advanced through failure"
        );
        let snapshot = FeedbackSnapshot::load(&ctx.database, DEFAULT_AGENT_ID).unwrap();
        assert!(snapshot.state.pending.is_some(), "{label}: plan was lost");
        assert_eq!(projected(&ctx).await.is_some(), effect_visible, "{label}");
        assert_eq!(file.is_file(), effect_visible, "{label}");
        let before = std::fs::read(&file).ok();

        let ctx = reopen(ctx, dir.path()).await;
        let ctx = stage().execute(ctx).await.unwrap();
        assert!(
            ctx.report.errors.is_none(),
            "{label}: {:?}",
            ctx.report.errors
        );
        assert!(committed(&ctx) > 0, "{label}: resume did not commit");
        assert!(FeedbackSnapshot::load(&ctx.database, DEFAULT_AGENT_ID)
            .unwrap()
            .state
            .pending
            .is_none());
        assert!(projected(&ctx).await.is_some());
        assert_eq!(applied_rules(&ctx.report), counted_after_resume, "{label}");
        let after = std::fs::read(&file).unwrap();
        if let Some(before) = before {
            assert_eq!(before, after, "{label}: replay regenerated the note");
        }
    }
}

#[tokio::test]
async fn feedback_never_settles_a_prepared_effect_after_unrelated_file_edit() {
    let (ctx, _, dir) = ctx_with_three_corrections(RESPONSE).await;
    let file = dir
        .path()
        .join("note")
        .join(DEFAULT_AGENT_ID)
        .join("feedback/verify-actual-corpus.md");
    inject_state_write_failure(&ctx, "instr(NEW.value, '\"outcome\":\"applied\"') > 0").await;
    let ctx = stage().execute(ctx).await.unwrap();
    assert!(projected(&ctx).await.is_some());
    let unrelated = b"An unrelated user edit must remain authoritative.\n";
    std::fs::write(&file, unrelated).unwrap();

    let ctx = reopen(ctx, dir.path()).await;
    let ctx = stage().execute(ctx).await.unwrap();
    assert!(ctx
        .report
        .errors
        .as_deref()
        .is_some_and(|detail| detail.contains("conflicting edits")));
    assert_eq!(applied_rules(&ctx.report), 0);
    assert_eq!(committed(&ctx), 0);
    assert!(FeedbackSnapshot::load(&ctx.database, DEFAULT_AGENT_ID)
        .unwrap()
        .state
        .pending
        .is_some());
    assert_eq!(std::fs::read(file).unwrap(), unrelated);
}

#[tokio::test]
async fn feedback_progresses_through_a_same_second_backlog_larger_than_a_page() {
    let (mut ctx, _, dir) = ctx_with_three_corrections("unused").await;
    let raw = Arc::new(
        MarkdownSessionStore::new(&dir.path().join("backlog"))
            .await
            .unwrap(),
    );
    for index in 0..5 {
        let mut record = RawMemory::new(
            format!("correction {index}"),
            RawMemorySource::Correction {
                severity: "critical".into(),
                suggested_rule: None,
            },
        )
        .with_agent(DEFAULT_AGENT_ID);
        record.id = format!("same-second-{index}");
        record.path = Some(format!("{CORRECTION_PATH_PREFIX}{}", record.id));
        record.created_at = 1_700_000_000;
        raw.insert_raw_memory(&record).await.unwrap();
    }
    ctx.raw_store = Some(raw.clone());
    let config = super::FeedbackDistillStage {
        max_per_cycle: 2,
        min_candidates: 2,
        lookback: 2,
    };
    let mut previous = 0;
    for sequence in [0..2, 2..4, 4..5] {
        let expected = sequence.len();
        let actions = sequence.map(|index| serde_json::json!({
            "type":"skip", "source_fact":format!("same-second-{index}"), "reason":"transient",
        })).collect::<Vec<_>>();
        ctx.provider = Arc::new(MockProvider::new(
            serde_json::json!({"actions": actions}).to_string(),
        ));
        ctx.report = DreamReport::default();
        assert!(
            super::has_undistilled_corrections(
                &ctx.database,
                Some(raw.as_ref()),
                DEFAULT_AGENT_ID,
                &config
            )
            .await
        );
        ctx = config.execute(ctx).await.unwrap();
        assert!(ctx.report.errors.is_none(), "{:?}", ctx.report.errors);
        assert_eq!(ctx.report.distill_actions.len(), expected);
        let next = committed(&ctx);
        assert!(next > previous);
        previous = next;
    }
    assert!(
        !super::has_undistilled_corrections(
            &ctx.database,
            Some(raw.as_ref()),
            DEFAULT_AGENT_ID,
            &config
        )
        .await
    );
}

#[tokio::test]
async fn correction_tool_reaches_admission_note_recall_and_fix_queue() {
    use crate::builtin_tools::flag_user_correction::{
        FlagUserCorrectionArgs, FlagUserCorrectionTool,
    };
    use crate::gateway::handlers::memory::handle_list_corrections;
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::memory::notes::{KnowledgeNote, Severity};
    use crate::tools::AlephTool;
    use serde_json::json;

    let (mut ctx, _, dir) = ctx_with_three_corrections("unused").await;
    let raw = Arc::new(
        MarkdownSessionStore::new(&dir.path().join("tool-raw"))
            .await
            .unwrap(),
    );
    ctx.raw_store = Some(raw.clone());
    ctx.indexer
        .write_note(
            DEFAULT_AGENT_ID,
            "feedback",
            &KnowledgeNote {
                title: "obsolete".into(),
                category: "feedback".into(),
                facts: vec!["Prefer the old obsidian convention.".into()],
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let tool = FlagUserCorrectionTool::new(raw.clone(), DEFAULT_AGENT_ID);
    let correction = tool
        .call(FlagUserCorrectionArgs {
            content: "The obsolete convention is wrong for this corpus".into(),
            severity: Severity::Critical,
            suggested_rule: Some("Prefer the zirconium naming convention.".into()),
        })
        .await
        .unwrap();
    assert!(correction.success);
    let request = || {
        JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({"agent_id":DEFAULT_AGENT_ID})),
            json!(1),
        )
    };
    let listed = handle_list_corrections(request(), raw.clone(), ctx.database.clone()).await;
    let listed = listed.result.unwrap();
    assert_eq!(listed["corrections"][0]["id"], correction.raw_memory_id);
    assert_eq!(listed["corrections"][0]["status"], "pending");
    let config = stage();
    assert!(
        super::has_undistilled_corrections(
            &ctx.database,
            Some(raw.as_ref()),
            DEFAULT_AGENT_ID,
            &config,
        )
        .await
    );
    ctx.provider = Arc::new(MockProvider::new(
        json!({"actions":[{
            "type":"supersede", "old_note_path":"feedback/obsolete", "title":"successor",
            "rule":"Prefer the zirconium naming convention.", "confidence":0.99,
            "severity":"high", "source_facts":[correction.raw_memory_id],
        }]})
        .to_string(),
    ));
    let ctx = config.execute(ctx).await.unwrap();
    assert!(ctx.report.errors.is_none(), "{:?}", ctx.report.errors);
    assert_eq!(applied_rules(&ctx.report), 1);
    assert_eq!(
        ctx.database
            .search_notes_fts("zirconium", DEFAULT_AGENT_ID, 5)
            .await
            .unwrap()[0]
            .path,
        "feedback/successor"
    );
    assert!(ctx
        .database
        .get_note_index("feedback/obsolete", DEFAULT_AGENT_ID)
        .await
        .unwrap()
        .is_none());
    assert!(!dir
        .path()
        .join("note")
        .join(DEFAULT_AGENT_ID)
        .join("feedback/obsolete.md")
        .exists());
    let listed = handle_list_corrections(request(), raw.clone(), ctx.database.clone()).await;
    assert_eq!(
        listed.result.unwrap()["corrections"][0]["status"],
        "distilled"
    );
    assert!(
        !super::has_undistilled_corrections(
            &ctx.database,
            Some(raw.as_ref()),
            DEFAULT_AGENT_ID,
            &config,
        )
        .await
    );
}

#[tokio::test]
async fn each_action_uses_the_prior_actions_verified_state_for_the_same_note() {
    // The second New is deliberately another candidate for the same path; the
    // existing common writer folds that collision into genuine strengthening.
    let response = r#"{"actions":[
        {"type":"new","title":"verify-actual-corpus","rule":"Verify the active corpus before publishing notes.",
         "confidence":0.9,"severity":"high","source_facts":["c0"]},
        {"type":"new","title":"verify-actual-corpus","rule":"Use the corpus from the active scope.",
         "confidence":0.9,"severity":"high","source_facts":["c1"]},
        {"type":"skip","source_fact":"c2","reason":"transient"}
    ]}"#;
    let (ctx, _, dir) = ctx_with_three_corrections(response).await;
    let ctx = stage().execute(ctx).await.unwrap();
    assert!(ctx.report.errors.is_none(), "{:?}", ctx.report.errors);
    assert_eq!(applied_rules(&ctx.report), 2);
    assert!(committed(&ctx) > 0);
    let bytes = std::fs::read_to_string(
        dir.path()
            .join("note")
            .join(DEFAULT_AGENT_ID)
            .join("feedback/verify-actual-corpus.md"),
    )
    .unwrap();
    let note =
        crate::memory::notes::KnowledgeNote::from_markdown("verify-actual-corpus", &bytes).unwrap();
    assert_eq!(note.source_notes, ["c0", "c1"]);
    assert_eq!(
        projected(&ctx).await.as_deref(),
        Some(note.content_hash.as_str())
    );
}
