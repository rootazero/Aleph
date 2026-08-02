//! Frontmatter round-trip safety for typed relation edges (§2.5①).
//!
//! `Relation.to` and `Relation.rel_type` are unvalidated model input: the ingest
//! prompt tells the model `"to": "<entity path or [P<n>] token>"` and `type` is
//! an explicitly free-form LLM-chosen verb (R7 — no fixed taxonomy). Every other
//! frontmatter scalar is emitted through `yaml_scalar`; the `relations:` block
//! bypassed it, so a single YAML metacharacter in either field made the whole
//! note permanently unparseable — and every dream stage swallows that parse
//! error (`.ok()?` in `mention_weave`, `continue` in `note_decay`), so the note
//! silently drops out of the corpus forever with no log above `debug!`.
//!
//! These live as an integration test (rather than beside the unit tests in
//! `src/memory/notes/note/tests.rs`, where equivalents were also added) because
//! the `alephcore` lib-test binary does not currently build on main: earlier
//! dead-code-cut commits deleted production APIs and left their tests behind
//! (47 errors across a2a / browser / tool_metadata / config / context / swarm).
//! Integration tests only need the lib itself, which compiles clean.

use alephcore::memory::notes::{KnowledgeNote, Relation};

/// Every YAML metacharacter `needs_yaml_quote` guards against, in the two
/// fields that carry raw model output.
#[test]
fn relation_frontmatter_survives_yaml_metacharacters() {
    // `[[x]]` is the form the model sees everywhere else in the note API, so it
    // is the single most likely value to arrive here. Unquoted it parses as a
    // nested flow sequence and `serde_yaml` fails on the ENTIRE frontmatter.
    let hostile = [
        ("[[plan/old-roadmap]]", "supersedes:v2"),
        ("entity/bob", "reports_to: lead"),
        ("#tag/x", "relates|to"),
        ("entity/o'brien", "works_at"),
        ("{brace}", "a,b"),
        ("*star", "&anchor"),
        ("%pct", "@at"),
        ("!bang", ">fold"),
    ];

    for (to, rel_type) in hostile {
        let note = KnowledgeNote {
            title: "alice".to_string(),
            category: "entity".to_string(),
            relations: vec![Relation {
                to: to.to_string(),
                rel_type: rel_type.to_string(),
                confidence: 0.9,
            }],
            ..Default::default()
        };

        let md = note.to_markdown();
        let parsed = KnowledgeNote::from_markdown("alice", &md).unwrap_or_else(|e| {
            panic!("relation to={to:?} type={rel_type:?} bricked the note: {e}")
        });

        assert_eq!(
            parsed.relations, note.relations,
            "relation to={to:?} type={rel_type:?} did not survive the round trip"
        );
    }
}

/// A note carrying a hostile relation must stay parseable even when other
/// frontmatter scalars are also hostile — the quoting is per-field, so this
/// guards against a regression that fixes one field and not the other.
#[test]
fn hostile_relation_and_hostile_title_coexist() {
    let note = KnowledgeNote {
        title: "[wip] plans: q3".to_string(),
        category: "entity".to_string(),
        aliases: vec!["a: b".to_string()],
        tags: vec!["#x".to_string()],
        relations: vec![
            Relation {
                to: "[[plan/x]]".to_string(),
                rel_type: "supersedes".to_string(),
                confidence: 1.0,
            },
            Relation {
                to: "entity/y".to_string(),
                rel_type: "depends_on: hard".to_string(),
                confidence: 0.25,
            },
        ],
        ..Default::default()
    };

    let parsed = KnowledgeNote::from_markdown("alice", &note.to_markdown())
        .expect("hostile title + hostile relations must round-trip");

    // `title` is deliberately NOT read back from frontmatter — the filename is
    // the single source of truth (`parsing.rs`: "Not mapped into
    // KnowledgeNote.title"), so `parsed.title` is the argument, not the
    // frontmatter value. The hostile title still matters here: it must not
    // break the parse for the *other* fields.
    assert_eq!(parsed.title, "alice");
    assert_eq!(parsed.aliases, note.aliases);
    assert_eq!(parsed.tags, note.tags);
    assert_eq!(parsed.relations, note.relations);
    // confidence is emitted as `{:.4}`, so it survives exactly at this precision
    assert!((parsed.relations[1].confidence - 0.25).abs() < 1e-4);
}
