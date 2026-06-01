#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn extract_provenance_markers_handles_all_origins() {
        let body = "- a <!-- src: raw/abc, origin: raw_source, inferred: false -->\n- b <!-- origin: inferred, inferred: true -->\n- c <!-- src: note/x, origin: prior_note, inferred: false -->\n- legacy fact with no marker\n";
        let provs = parsing::extract_provenance_markers(body, &parsing::extract_facts(body));
        assert_eq!(provs.len(), 4);
        assert_eq!(provs[0].origin, types::ProvenanceOrigin::RawSource);
        assert_eq!(provs[0].source_id.as_deref(), Some("raw/abc"));
        assert_eq!(provs[1].origin, types::ProvenanceOrigin::Inferred);
        assert!(provs[1].inferred);
        assert_eq!(provs[2].origin, types::ProvenanceOrigin::PriorNote);
        assert_eq!(provs[3].origin, types::ProvenanceOrigin::Legacy);
    }

    #[test]
    fn fts_body_strips_provenance_comments() {
        let n = KnowledgeNote::from_markdown("t",
            "---\ncategory: preference\ntags: []\n---\n\n- a <!-- src: raw/x, origin: raw_source, inferred: false -->\n- b\n",
        ).unwrap();
        let fts = n.body_text_for_fts();
        assert!(!fts.contains("<!--"));
        assert!(fts.contains("a"));
        assert!(fts.contains("b"));
    }

    #[test]
    fn note_status_default_active_for_legacy() {
        let md = "---\ncategory: skill\ntags: []\ncreated: \"2026-04-29\"\nupdated: \"2026-04-29\"\n---\n\n- f\n";
        let n = KnowledgeNote::from_markdown("legacy", md).unwrap();
        assert_eq!(n.status, types::NoteStatus::Active);
        assert!(n.supersedes.is_empty());
        assert!(n.superseded_by.is_empty());
    }

    #[test]
    fn note_status_round_trip_contradicted() {
        let n = KnowledgeNote {
            title: "x".into(),
            category: "preference".into(),
            facts: vec!["body".into()],
            status: types::NoteStatus::Contradicted,
            supersedes: vec!["preference/old".into()],
            superseded_by: vec!["preference/new".into()],
            ..Default::default()
        };
        let md = n.to_markdown();
        assert!(md.contains("status: contradicted"));
        assert!(md.contains("supersedes: [preference/old]"));
        assert!(md.contains("superseded_by: [preference/new]"));

        let parsed = KnowledgeNote::from_markdown("x", &md).unwrap();
        assert_eq!(parsed.status, types::NoteStatus::Contradicted);
        assert_eq!(parsed.supersedes, vec!["preference/old".to_string()]);
        assert_eq!(parsed.superseded_by, vec!["preference/new".to_string()]);
    }

    #[test]
    fn severity_default_is_low_for_backward_compat() {
        let s: types::Severity = Default::default();
        assert_eq!(s, types::Severity::Low);
    }

    #[test]
    fn severity_serde_roundtrip_all_variants() {
        for s in [
            types::Severity::Low,
            types::Severity::Med,
            types::Severity::High,
            types::Severity::Critical,
        ] {
            let j = serde_json::to_string(&s).unwrap();
            let back: types::Severity = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn severity_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&types::Severity::Low).unwrap(),
            "\"low\""
        );
        assert_eq!(
            serde_json::to_string(&types::Severity::Med).unwrap(),
            "\"med\""
        );
        assert_eq!(
            serde_json::to_string(&types::Severity::High).unwrap(),
            "\"high\""
        );
        assert_eq!(
            serde_json::to_string(&types::Severity::Critical).unwrap(),
            "\"critical\""
        );
    }

    #[test]
    fn knowledge_note_old_markdown_loads_with_defaults() {
        let old_md = "---\ncategory: skill\ntags: [a]\ncreated: 2026-04-29\nupdated: 2026-04-29\n---\n\n- existing fact\n";
        let n = KnowledgeNote::from_markdown("legacy-note", old_md).expect("must parse");
        assert!(
            (n.confidence - 1.0).abs() < 1e-6,
            "old notes get confidence=1.0"
        );
        assert_eq!(
            n.severity,
            types::Severity::Low,
            "old notes get severity=Low"
        );
        assert!(
            n.source_notes.is_empty(),
            "old notes get empty source_notes"
        );
    }

    #[test]
    fn knowledge_note_new_markdown_roundtrips_new_fields() {
        // Legacy on-disk YAML still uses the old `source_facts:` key — verify
        // the serde alias deserializes it into the renamed `source_notes` field.
        let md = "---\ncategory: skill\ntags: [x]\ncreated: 2026-04-29\nupdated: 2026-04-29\nconfidence: 0.85\nseverity: high\nsource_facts: [synthesis/learning-syn]\n---\n\n- the rule\n";
        let n = KnowledgeNote::from_markdown("new-note", md).expect("must parse");
        assert!((n.confidence - 0.85).abs() < 1e-6);
        assert_eq!(n.severity, types::Severity::High);
        assert_eq!(n.source_notes, vec!["synthesis/learning-syn".to_string()]);
    }

    #[test]
    fn legacy_source_facts_yaml_round_trips_to_source_notes() {
        // Feeds legacy `source_facts:` markdown, verifies the serde alias maps
        // it to `source_notes`, and asserts re-serialization emits `source_notes:`
        // only (no legacy `source_facts:`).
        let legacy_md = "---\ncategory: skill\ntags: []\ncreated: 2026-04-29\nupdated: 2026-04-29\nconfidence: 0.9\nseverity: low\nsource_facts: [synthesis/legacy-a, synthesis/legacy-b]\n---\n\n- legacy rule\n";
        let n = KnowledgeNote::from_markdown("legacy-syn", legacy_md).expect("must parse");
        assert_eq!(
            n.source_notes,
            vec![
                "synthesis/legacy-a".to_string(),
                "synthesis/legacy-b".to_string()
            ]
        );

        let re = n.to_markdown();
        assert!(
            re.contains("source_notes:"),
            "re-serialized markdown must use new key:\n{re}"
        );
        assert!(
            !re.contains("source_facts:"),
            "re-serialized markdown must NOT contain legacy key:\n{re}"
        );
    }

    #[test]
    fn new_source_notes_field_round_trips() {
        let n = KnowledgeNote {
            title: "synthetic".into(),
            category: "skill".into(),
            facts: vec!["the rule".into()],
            created_at: 1714377600,
            updated_at: 1714377600,
            confidence: 0.9,
            severity: types::Severity::Med,
            source_notes: vec!["synthesis/y".into()],
            ..Default::default()
        };
        let md = n.to_markdown();
        assert!(md.contains("source_notes:"), "missing source_notes:\n{md}");
        assert!(!md.contains("source_facts:"));

        let parsed = KnowledgeNote::from_markdown("synthetic", &md).expect("roundtrip");
        assert_eq!(parsed.source_notes, vec!["synthesis/y".to_string()]);
    }

    #[test]
    fn knowledge_note_default_has_legacy_safe_values() {
        let n = KnowledgeNote::default();
        assert_eq!(
            n.confidence, 1.0,
            "Default confidence must be 1.0 (legacy-safe)"
        );
        assert_eq!(
            n.severity,
            types::Severity::Low,
            "Default severity must be Low (legacy-safe)"
        );
        assert!(n.source_notes.is_empty());
    }

    #[test]
    fn to_markdown_emits_new_frontmatter_fields_when_set() {
        let n = KnowledgeNote {
            title: "test".into(),
            category: "skill".into(),
            tags: vec!["distilled".into()],
            facts: vec!["the rule".into()],
            links: vec![],
            created_at: 1714377600,
            updated_at: 1714377600,
            content_hash: String::new(),
            confidence: 0.85,
            severity: types::Severity::High,
            source_notes: vec!["synthesis/syn-1".into()],
            ..Default::default()
        };
        let md = n.to_markdown();
        assert!(md.contains("confidence: 0.85"), "missing confidence:\n{md}");
        assert!(md.contains("severity: high"), "missing severity:\n{md}");
        assert!(md.contains("source_notes:"), "missing source_notes:\n{md}");
        assert!(md.contains("synthesis/syn-1"), "missing source ref:\n{md}");

        let parsed = KnowledgeNote::from_markdown("test", &md).expect("roundtrip");
        assert!((parsed.confidence - 0.85).abs() < 1e-6);
        assert_eq!(parsed.severity, types::Severity::High);
        assert_eq!(parsed.source_notes, vec!["synthesis/syn-1".to_string()]);
    }

    #[test]
    fn to_markdown_legacy_defaults_roundtrip() {
        let n = KnowledgeNote {
            title: "legacy".into(),
            category: "preference".into(),
            tags: vec![],
            facts: vec!["fact".into()],
            links: vec![],
            created_at: 1714377600,
            updated_at: 1714377600,
            content_hash: String::new(),
            confidence: 1.0,
            severity: types::Severity::Low,
            source_notes: vec![],
            ..Default::default()
        };
        let md = n.to_markdown();
        assert!(md.contains("confidence: 1"), "missing confidence:\n{md}");
        assert!(md.contains("severity: low"), "missing severity:\n{md}");
        assert!(md.contains("source_notes: []"));
        let parsed = KnowledgeNote::from_markdown("legacy", &md).unwrap();
        assert_eq!(parsed.confidence, 1.0);
        assert_eq!(parsed.severity, types::Severity::Low);
        assert!(parsed.source_notes.is_empty());
    }

    const SAMPLE_NOTE: &str = "\
---
category: preference
tags: [editor, vim]
created: 2026-04-01
updated: 2026-04-10
---

- The user prefers Vim for coding
- The user uses LazyVim configuration

Related: [[Rust Learning]] [[Dev Environment]]
";

    #[test]
    fn parses_note_from_markdown() {
        let note = KnowledgeNote::from_markdown("Editor Preferences", SAMPLE_NOTE).unwrap();

        assert_eq!(note.title, "Editor Preferences");
        assert_eq!(note.category, "preference");
        assert_eq!(note.tags, vec!["editor", "vim"]);
        assert_eq!(
            note.facts,
            vec![
                "The user prefers Vim for coding",
                "The user uses LazyVim configuration",
            ]
        );
        assert_eq!(note.links, vec!["Rust Learning", "Dev Environment"]);
        // 2026-04-01 00:00:00 UTC
        assert!(note.created_at > 0);
        assert!(note.updated_at > note.created_at);
        assert!(!note.content_hash.is_empty());
    }

    #[test]
    fn serializes_note_to_markdown() {
        let note = KnowledgeNote::from_markdown("Editor Preferences", SAMPLE_NOTE).unwrap();
        let output = note.to_markdown();

        assert!(output.contains("category: preference"));
        assert!(output.contains("tags: [editor, vim]"));
        assert!(output.contains("- The user prefers Vim for coding"));
        assert!(output.contains("- The user uses LazyVim configuration"));
        assert!(output.contains("[[Rust Learning]]"));
        assert!(output.contains("[[Dev Environment]]"));
    }

    #[test]
    fn body_text_joins_facts() {
        let note = KnowledgeNote::from_markdown("Test", SAMPLE_NOTE).unwrap();
        let text = note.body_text();
        assert!(text.contains("The user prefers Vim for coding"));
        assert!(text.contains('\n'));
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let result = KnowledgeNote::from_markdown("Bad", "No frontmatter here");
        assert!(result.is_err());
    }

    #[test]
    fn sanitize_title_strips_path_traversal() {
        assert_eq!(
            helpers::sanitize_title("../../etc/passwd").unwrap(),
            "etcpasswd"
        );
        assert_eq!(
            helpers::sanitize_title("normal title").unwrap(),
            "normal title"
        );
        assert_eq!(helpers::sanitize_title("has/slash").unwrap(), "hasslash");
        assert_eq!(helpers::sanitize_title("has\\back").unwrap(), "hasback");
        assert_eq!(helpers::sanitize_title("a]b*c?d").unwrap(), "a]bcd");
        assert_eq!(helpers::sanitize_title("  spaces  ").unwrap(), "spaces");
    }

    #[test]
    fn sanitize_title_rejects_empty_result() {
        for bad in ["", "..", "///", "   "] {
            let err = helpers::sanitize_title(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::AlephError::Validation(_)),
                "expected Validation variant for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn sanitize_title_returns_ok_for_normal_input() {
        assert_eq!(
            helpers::sanitize_title("rust learning").unwrap(),
            "rust learning"
        );
        assert_eq!(
            helpers::sanitize_title("../etc/passwd").unwrap(),
            "etcpasswd"
        );
    }

    #[test]
    fn sanitize_title_strips_trailing_md_extension() {
        // A filename leaking in as a title must not yield a "*.md.md" file.
        assert_eq!(helpers::sanitize_title("toolchain.md").unwrap(), "toolchain");
        assert_eq!(
            helpers::sanitize_title("rust-ownership").unwrap(),
            "rust-ownership"
        );
        // Only one extension stripped; a non-.md title is untouched.
        assert_eq!(helpers::sanitize_title("notes.txt").unwrap(), "notes.txt");
    }

    #[test]
    fn yaml_inline_array_quotes_special_chars() {
        let items = vec![
            "plain".to_string(),
            "has, comma".to_string(),
            "has: colon".to_string(),
            "has '' quote".to_string(),
        ];
        let s = helpers::yaml_inline_array(&items);
        assert_eq!(s, "[plain, 'has, comma', 'has: colon', 'has '''' quote']");
    }

    #[test]
    fn yaml_inline_array_empty() {
        assert_eq!(helpers::yaml_inline_array(&[]), "[]");
    }

    #[test]
    fn tags_with_special_chars_round_trip() {
        let n = KnowledgeNote {
            title: "t".into(),
            category: "preference".into(),
            tags: vec!["has, comma".into(), "has: colon".into()],
            facts: vec!["x".into()],
            content_hash: String::new(),
            ..Default::default()
        };
        let md = n.to_markdown();
        let parsed = KnowledgeNote::from_markdown("t", &md).expect("must round-trip");
        assert_eq!(
            parsed.tags,
            vec!["has, comma".to_string(), "has: colon".to_string()]
        );
    }

    #[test]
    fn yaml_inline_array_quotes_implicit_scalars() {
        let cases = [
            "null", "Null", "NULL", "~", "true", "false", "Yes", "NO", "on", "OFF", "123", "-1",
            "1.5", ".inf", ".nan", "0x1a", "0o7",
        ];
        for c in cases {
            let s = helpers::yaml_inline_array(&[c.to_string()]);
            assert!(
                s.starts_with("['") && s.ends_with("']"),
                "expected {c:?} to be quoted, got {s:?}"
            );
        }
    }

    #[test]
    fn yaml_inline_array_quotes_leading_and_trailing_space() {
        let s = helpers::yaml_inline_array(&[
            " lead".to_string(),
            "trail ".to_string(),
            "".to_string(),
        ]);
        assert_eq!(s, "[' lead', 'trail ', '']");
    }

    #[test]
    fn implicit_scalar_tags_round_trip() {
        let n = KnowledgeNote {
            title: "t".into(),
            category: "preference".into(),
            tags: vec!["null".into(), "true".into(), "123".into()],
            facts: vec!["x".into()],
            content_hash: String::new(),
            ..Default::default()
        };
        let md = n.to_markdown();
        let parsed = KnowledgeNote::from_markdown("t", &md).expect("must round-trip");
        assert_eq!(
            parsed.tags,
            vec!["null".to_string(), "true".to_string(), "123".to_string()]
        );
    }

    #[test]
    fn date_writer_quotes_iso_string() {
        let n = KnowledgeNote {
            title: "t".into(),
            category: "preference".into(),
            facts: vec!["x".into()],
            created_at: 1714377600,
            updated_at: 1714377600,
            ..Default::default()
        };
        let md = n.to_markdown();
        assert!(
            md.contains("created: \"2024-04-29\""),
            "expected quoted date, got:\n{md}"
        );
        // Writer formats only the date (`%Y-%m-%d`), not the time, so the
        // round-trip truncates to start-of-day UTC: the input
        // 1714377600 (2024-04-29 08:00:00 UTC) becomes 1714348800
        // (2024-04-29 00:00:00 UTC). Pin the observed value to catch future
        // regressions; if the truncation is ever fixed this assertion must
        // be updated.
        let parsed = KnowledgeNote::from_markdown("t", &md).expect("round-trip parse");
        assert_eq!(parsed.created_at, 1714348800);
        assert_eq!(parsed.updated_at, 1714348800);
    }

    #[test]
    fn date_reader_accepts_native_yaml_date() {
        let md = "---\ncategory: skill\ntags: []\ncreated: 2026-04-01\nupdated: 2026-04-01\n---\n\n- fact\n";
        let n = KnowledgeNote::from_markdown("t", md).expect("must parse native date");
        assert_eq!(
            n.created_at, 1775001600,
            "created should be 2026-04-01 00:00:00 UTC"
        );
        assert_eq!(
            n.updated_at, 1775001600,
            "updated should be 2026-04-01 00:00:00 UTC"
        );
    }

    #[test]
    fn date_reader_accepts_quoted_iso_date() {
        let md = "---\ncategory: skill\ntags: []\ncreated: \"2026-04-01\"\nupdated: \"2026-04-01\"\n---\n\n- fact\n";
        let n = KnowledgeNote::from_markdown("t", md).expect("must parse quoted date");
        assert_eq!(
            n.created_at, 1775001600,
            "created should be 2026-04-01 00:00:00 UTC"
        );
        assert_eq!(
            n.updated_at, 1775001600,
            "updated should be 2026-04-01 00:00:00 UTC"
        );
    }

    #[test]
    fn handles_empty_optional_fields() {
        let content = "\
---
category: misc
tags: []
---

- A simple fact
";
        let note = KnowledgeNote::from_markdown("Simple", content).unwrap();
        assert_eq!(note.category, "misc");
        assert!(note.tags.is_empty());
        assert_eq!(note.facts, vec!["A simple fact"]);
        assert!(note.links.is_empty());
        assert_eq!(note.created_at, 0);
        assert_eq!(note.updated_at, 0);
    }

    #[test]
    fn extract_facts_keeps_subbullets() {
        let body = "- top fact\n  - sub fact\n- second top\n";
        let facts = parsing::extract_facts(body);
        assert_eq!(facts.len(), 2);
        assert!(facts[0].contains("top fact"));
        assert!(
            facts[0].contains("sub fact"),
            "sub-bullet must attach to parent: {:?}",
            facts[0]
        );
        assert_eq!(facts[1].trim(), "second top");
    }

    #[test]
    fn extract_facts_keeps_continuation_lines() {
        let body =
            "- claim line one\n  continuation line two\n  continuation line three\n- next claim\n";
        let facts = parsing::extract_facts(body);
        assert_eq!(facts.len(), 2);
        assert!(facts[0].contains("continuation line two"));
        assert!(facts[0].contains("continuation line three"));
    }

    #[test]
    fn extract_facts_empty_line_ends_fact() {
        let body = "- one\n\n  this should NOT belong to one\n- two\n";
        let facts = parsing::extract_facts(body);
        assert_eq!(facts.len(), 2);
        assert!(!facts[0].contains("should NOT"));
    }
}
