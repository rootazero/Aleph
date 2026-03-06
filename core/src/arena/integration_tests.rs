//! End-to-end integration tests for SharedArena collaboration scenarios.
//!
//! These tests simulate the two primary collaboration patterns from the design doc:
//! 1. Peer collaboration — "Analyze a technical report"
//! 2. Pipeline collaboration — "Translate and polish article"

use super::*;
use chrono::Utc;
use std::collections::HashMap;

#[test]
fn peer_collaboration_full_lifecycle() {
    // 1. Create Arena with Peer strategy, coordinator=main, workers=researcher+coder
    let mut manager = ArenaManager::new();

    let manifest = ArenaManifest {
        goal: "Analyze a technical report".to_string(),
        strategy: CoordinationStrategy::Peer {
            coordinator: "main".to_string(),
        },
        participants: vec![
            Participant {
                agent_id: "main".to_string(),
                role: ParticipantRole::Coordinator,
                permissions: ArenaPermissions::from_role(ParticipantRole::Coordinator),
            },
            Participant {
                agent_id: "researcher".to_string(),
                role: ParticipantRole::Worker,
                permissions: ArenaPermissions::from_role(ParticipantRole::Worker),
            },
            Participant {
                agent_id: "coder".to_string(),
                role: ParticipantRole::Worker,
                permissions: ArenaPermissions::from_role(ParticipantRole::Worker),
            },
        ],
        created_by: "main".to_string(),
        created_at: Utc::now(),
    };

    let (arena_id, handles) = manager.create_arena(manifest).unwrap();
    assert_eq!(handles.len(), 3);

    let main_handle = handles.get("main").unwrap();
    let researcher_handle = handles.get("researcher").unwrap();
    let coder_handle = handles.get("coder").unwrap();

    // 2. Researcher puts Text artifact ("Key findings: ..."), reports progress
    let researcher_artifact = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::Text,
        content: ArtifactContent::Inline("Key findings: the system has 3 critical risks".to_string()),
        metadata: HashMap::new(),
        created_at: Utc::now(),
    };
    researcher_handle.put_artifact(researcher_artifact).unwrap();
    researcher_handle
        .report_progress(Some("Analyzed report sections 1-3".to_string()), Some(1))
        .unwrap();

    // 3. Coder puts Code artifact ("fn verify() { ... }"), reports progress
    let coder_artifact = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::Code,
        content: ArtifactContent::Inline("fn verify() { /* risk validation logic */ }".to_string()),
        metadata: HashMap::new(),
        created_at: Utc::now(),
    };
    coder_handle.put_artifact(coder_artifact).unwrap();
    coder_handle
        .report_progress(Some("Wrote verification function".to_string()), Some(1))
        .unwrap();

    // 4. Researcher adds SharedFact ("Report identifies 3 critical risks")
    let fact = SharedFact {
        content: "Report identifies 3 critical risks".to_string(),
        source_agent: "researcher".to_string(),
        confidence: 0.95,
        tags: vec!["risk-analysis".to_string()],
        created_at: Utc::now(),
    };
    researcher_handle.add_shared_fact(fact).unwrap();

    // 5. Coordinator reads researcher and coder slots, verifies 1 artifact each
    let researcher_artifacts = main_handle
        .list_artifacts(&"researcher".to_string())
        .unwrap();
    assert_eq!(researcher_artifacts.len(), 1);
    assert_eq!(researcher_artifacts[0].kind, ArtifactKind::Text);

    let coder_artifacts = main_handle
        .list_artifacts(&"coder".to_string())
        .unwrap();
    assert_eq!(coder_artifacts.len(), 1);
    assert_eq!(coder_artifacts[0].kind, ArtifactKind::Code);

    // 6. Check progress: completed_steps == 2
    let progress = main_handle.get_progress();
    assert_eq!(progress.completed_steps, 2);

    // 7. Manager settle_with_facts() handles Active → Settling → Archived internally
    let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();
    assert_eq!(report.facts_persisted, 1);
    assert_eq!(report.artifacts_archived, 2);

    // 9. Verify facts[0].content matches
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "Report identifies 3 critical risks");
}

#[test]
fn pipeline_collaboration_full_lifecycle() {
    // 1. Create Arena with Pipeline strategy, stages=[translator→polisher]
    let mut manager = ArenaManager::new();

    let manifest = ArenaManifest {
        goal: "Translate and polish article".to_string(),
        strategy: CoordinationStrategy::Pipeline {
            stages: vec![
                StageSpec {
                    agent_id: "translator".to_string(),
                    description: "Translate English to Chinese".to_string(),
                    depends_on: vec![],
                },
                StageSpec {
                    agent_id: "polisher".to_string(),
                    description: "Polish Chinese translation".to_string(),
                    depends_on: vec!["translator".to_string()],
                },
            ],
        },
        participants: vec![
            Participant {
                agent_id: "translator".to_string(),
                role: ParticipantRole::Coordinator,
                permissions: ArenaPermissions::from_role(ParticipantRole::Coordinator),
            },
            Participant {
                agent_id: "polisher".to_string(),
                role: ParticipantRole::Worker,
                permissions: ArenaPermissions::from_role(ParticipantRole::Worker),
            },
        ],
        created_by: "translator".to_string(),
        created_at: Utc::now(),
    };

    let (arena_id, handles) = manager.create_arena(manifest).unwrap();
    assert_eq!(handles.len(), 2);

    let translator_handle = handles.get("translator").unwrap();
    let polisher_handle = handles.get("polisher").unwrap();

    // 2. Translator puts Text artifact ("中文翻译初稿...")
    let translation_artifact = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::Text,
        content: ArtifactContent::Inline("中文翻译初稿...".to_string()),
        metadata: HashMap::new(),
        created_at: Utc::now(),
    };
    translator_handle.put_artifact(translation_artifact).unwrap();

    // 3. Translator adds SharedFact ("Term mapping: quantum → 量子")
    let fact = SharedFact {
        content: "Term mapping: quantum → 量子".to_string(),
        source_agent: "translator".to_string(),
        confidence: 1.0,
        tags: vec!["terminology".to_string()],
        created_at: Utc::now(),
    };
    translator_handle.add_shared_fact(fact).unwrap();

    // 4. Polisher reads translator's slot, verifies 1 artifact
    let translator_artifacts = polisher_handle
        .list_artifacts(&"translator".to_string())
        .unwrap();
    assert_eq!(translator_artifacts.len(), 1);
    assert_eq!(translator_artifacts[0].kind, ArtifactKind::Text);

    // 5. Polisher puts Text artifact ("中文润色终稿...")
    let polished_artifact = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::Text,
        content: ArtifactContent::Inline("中文润色终稿...".to_string()),
        metadata: HashMap::new(),
        created_at: Utc::now(),
    };
    polisher_handle.put_artifact(polished_artifact).unwrap();

    // 6. Manager settle_with_facts() handles Active → Settling → Archived internally
    let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();
    assert_eq!(report.facts_persisted, 1);
    assert_eq!(report.artifacts_archived, 2);

    // 8. Verify facts[0].content matches
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "Term mapping: quantum → 量子");
}
