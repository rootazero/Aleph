use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::tools::AlephTool;

fn create_test_skill(dir: &Path, id: &str, name: &str, description: &str) {
    let skill_dir = dir.join(id);
    fs::create_dir_all(&skill_dir).unwrap();

    let content = format!(
        r#"---
name: {}
description: {}
triggers:
  - test
---

# {} Skill

These are the skill instructions.
Follow them carefully.
"#,
        name, description, name
    );

    fs::write(skill_dir.join("SKILL.md"), content).unwrap();

    // Add an extra resource file
    fs::write(
        skill_dir.join("REFERENCE.md"),
        "# Reference\n\nAdditional reference material.",
    )
    .unwrap();
}

#[tokio::test]
async fn test_read_skill_success() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

    let tool = ReadSkillTool::new(skills_dir);
    let args = ReadSkillArgs {
        skill_id: "test-skill".to_string(),
        file_name: None,
    };

    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.skill_id, "test-skill");
    assert_eq!(result.file_name, "SKILL.md");
    assert!(result.content.contains("Test Skill"));
    assert!(result.content.contains("skill instructions"));
    // SKILL.md is excluded from available_files (it's the primary file, not a reference)
    assert!(!result.available_files.contains(&"SKILL.md".to_string()));
    assert!(result.available_files.contains(&"REFERENCE.md".to_string()));
}

#[tokio::test]
async fn test_read_skill_resource() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

    let tool = ReadSkillTool::new(skills_dir);
    let args = ReadSkillArgs {
        skill_id: "test-skill".to_string(),
        file_name: Some("REFERENCE.md".to_string()),
    };

    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.file_name, "REFERENCE.md");
    assert!(result.content.contains("Additional reference material"));
}

#[tokio::test]
async fn test_read_skill_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    let tool = ReadSkillTool::new(skills_dir);
    let args = ReadSkillArgs {
        skill_id: "nonexistent".to_string(),
        file_name: None,
    };

    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found") || err_msg.contains("NotFound"),
        "Error should indicate not found: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_read_skill_path_traversal() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    let tool = ReadSkillTool::new(skills_dir);

    // Test skill_id path traversal
    let args = ReadSkillArgs {
        skill_id: "../etc/passwd".to_string(),
        file_name: None,
    };
    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await;
    assert!(result.is_err());

    // Test file_name path traversal
    let args = ReadSkillArgs {
        skill_id: "test".to_string(),
        file_name: Some("../../../etc/passwd".to_string()),
    };
    let result = AlephTool::call(&tool, args).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_skills() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    create_test_skill(&skills_dir, "skill-a", "Skill A", "First skill");
    create_test_skill(&skills_dir, "skill-b", "Skill B", "Second skill");

    let tool = ListSkillsTool::new(skills_dir);
    let args = ListSkillsArgs { filter: None };

    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.count, 2);
    assert_eq!(result.skills[0].id, "skill-a");
    assert_eq!(result.skills[1].id, "skill-b");
}

#[tokio::test]
async fn test_list_skills_filter() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    create_test_skill(&skills_dir, "refine-text", "Refine Text", "Improve writing");
    create_test_skill(&skills_dir, "translate", "Translate", "Translate text");

    let tool = ListSkillsTool::new(skills_dir);
    let args = ListSkillsArgs {
        filter: Some("writing".to_string()),
    };

    // Use fully qualified syntax
    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.count, 1);
    assert_eq!(result.skills[0].id, "refine-text");
}

#[tokio::test]
async fn test_multi_directory_discovery() {
    // Create two separate skills directories
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    let skills_dir1 = temp_dir1.path().to_path_buf();
    let skills_dir2 = temp_dir2.path().to_path_buf();

    // Create skills in different directories
    create_test_skill(&skills_dir1, "skill-a", "Skill A", "From directory 1");
    create_test_skill(&skills_dir2, "skill-b", "Skill B", "From directory 2");

    // Test ListSkillsTool with multiple directories
    let tool = ListSkillsTool::with_directories(vec![skills_dir1.clone(), skills_dir2.clone()]);
    let args = ListSkillsArgs { filter: None };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.count, 2);

    // Both skills should be found
    let ids: Vec<&str> = result.skills.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"skill-a"));
    assert!(ids.contains(&"skill-b"));
}

#[tokio::test]
async fn test_multi_directory_deduplication() {
    // Create two directories with the same skill
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    let skills_dir1 = temp_dir1.path().to_path_buf();
    let skills_dir2 = temp_dir2.path().to_path_buf();

    // Create same skill ID in both directories
    create_test_skill(
        &skills_dir1,
        "same-skill",
        "Skill From Dir1",
        "First directory",
    );
    create_test_skill(
        &skills_dir2,
        "same-skill",
        "Skill From Dir2",
        "Second directory",
    );

    // Test that first occurrence wins
    let tool = ListSkillsTool::with_directories(vec![skills_dir1.clone(), skills_dir2.clone()]);
    let args = ListSkillsArgs { filter: None };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.count, 1);
    assert_eq!(result.skills[0].id, "same-skill");
    // Should get the one from dir1 (first in list)
    assert!(result.skills[0].description.contains("First directory"));
}

#[tokio::test]
async fn test_read_skill_multi_directory() {
    // Create two directories
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    let skills_dir1 = temp_dir1.path().to_path_buf();
    let skills_dir2 = temp_dir2.path().to_path_buf();

    // Only create skill in the second directory
    create_test_skill(&skills_dir2, "unique-skill", "Unique Skill", "Only in dir2");

    // ReadSkillTool should find it even though it's in the second directory
    let tool = ReadSkillTool::with_directories(vec![skills_dir1, skills_dir2]);
    let args = ReadSkillArgs {
        skill_id: "unique-skill".to_string(),
        file_name: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert!(result.success);
    assert_eq!(result.skill_id, "unique-skill");
    assert!(result.content.contains("Unique Skill"));
}

#[tokio::test]
async fn skill_read_can_reach_references_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let sk = tmp.path().join("withref");
    tokio::fs::create_dir_all(sk.join("references"))
        .await
        .unwrap();
    tokio::fs::write(
        sk.join("SKILL.md"),
        "---\nname: withref\ndescription: d\n---\nx",
    )
    .await
    .unwrap();
    tokio::fs::write(sk.join("references").join("guide.md"), "REF-CONTENT")
        .await
        .unwrap();

    let tool = ReadSkillTool::with_directories(vec![tmp.path().to_path_buf()]);
    let out = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "withref".to_string(),
            file_name: Some("references/guide.md".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(out.content.contains("REF-CONTENT"));
}

#[tokio::test]
async fn skill_read_rejects_traversal_in_file_name() {
    let tmp = tempfile::tempdir().unwrap();
    let sk = tmp.path().join("trav");
    tokio::fs::create_dir_all(&sk).await.unwrap();
    tokio::fs::write(
        sk.join("SKILL.md"),
        "---\nname: trav\ndescription: d\n---\nx",
    )
    .await
    .unwrap();
    let tool = ReadSkillTool::with_directories(vec![tmp.path().to_path_buf()]);
    let err = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "trav".to_string(),
            file_name: Some("../../etc/passwd".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("invalid")
            || err.to_string().to_lowercase().contains("traversal"),
        "got: {err}"
    );
}

#[tokio::test]
async fn duplicate_skill_across_dirs_resolves_by_precedence() {
    // Two DISTINCT physical dirs share a skill id — e.g. a project skill
    // shadowing a same-named global one. `skill_read` must resolve to the
    // highest-precedence root (first in the list), matching `skill_list`'s
    // first-occurrence dedup and Claude Code's "closer skill wins", rather
    // than stranding the caller with an "ambiguous" refusal.
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("a");
    let dir_b = tmp.path().join("b");
    create_test_skill(&dir_a, "dup", "Dup A", "from dir a (higher precedence)");
    create_test_skill(&dir_b, "dup", "Dup B", "from dir b (shadowed)");

    let tool = ReadSkillTool::with_directories(vec![dir_a, dir_b]);
    let result = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "dup".to_string(),
            file_name: None,
        },
    )
    .await
    .expect("genuine same-name collisions resolve by precedence, not refusal");
    assert!(result.success);
    // First directory wins; the shadowed one is not read.
    assert!(result.content.contains("Dup A"), "got: {}", result.content);
    assert!(!result.content.contains("Dup B"), "got: {}", result.content);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_twins_across_roots_resolve_to_single_skill() {
    // Regression for the real-world `agentkey` case: a skill installed once,
    // then surfaced through several roots via symlinks (`~/.aleph/skills/<id>`
    // and `~/.claude/skills/<id>` both symlinking `~/.agents/skills/<id>`).
    // Both roots canonicalize to the same physical dir, so `skill_read` must
    // treat it as ONE hit — not a false "ambiguous" collision that strands the
    // model into a raw `cat` loop.
    let tmp = tempfile::tempdir().unwrap();
    let real_root = tmp.path().join("real");
    let alias_root = tmp.path().join("alias");
    std::fs::create_dir_all(&alias_root).unwrap();
    create_test_skill(&real_root, "twin", "Twin Skill", "installed once");

    // alias_root/twin -> real_root/twin : same physical directory.
    std::os::unix::fs::symlink(real_root.join("twin"), alias_root.join("twin")).unwrap();

    let tool = ReadSkillTool::with_directories(vec![real_root, alias_root]);
    let result = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "twin".to_string(),
            file_name: None,
        },
    )
    .await
    .expect("symlink twins across roots must not be a false ambiguity");
    assert!(result.success);
    assert!(result.content.contains("Twin Skill"));
}

#[tokio::test]
async fn test_read_skill_usage_hint_present_with_support_files() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    // create_test_skill ships a REFERENCE.md alongside SKILL.md.
    create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

    let tool = ReadSkillTool::new(skills_dir);
    let result = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "test-skill".to_string(),
            file_name: None,
        },
    )
    .await
    .unwrap();

    // A skill with supporting files gets a hint steering reads to skill_read
    // (not `cat`) — the anti-cat affordance.
    assert!(
        !result.usage_hint.is_empty(),
        "hint expected when support files exist"
    );
    assert!(result.usage_hint.contains("skill_read"));
    assert!(result.usage_hint.contains("Do not cat"));
}

#[tokio::test]
async fn test_read_skill_usage_hint_empty_without_support_files() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    // A skill shipping ONLY SKILL.md (available_files ends up empty).
    let skill_dir = skills_dir.join("solo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: solo\ndescription: only\n---\nbody",
    )
    .unwrap();

    let tool = ReadSkillTool::new(skills_dir);
    let result = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "solo".to_string(),
            file_name: None,
        },
    )
    .await
    .unwrap();

    assert!(
        result.available_files.is_empty(),
        "no support files expected"
    );
    assert!(
        result.usage_hint.is_empty(),
        "hint must be empty without support files"
    );
}

#[tokio::test]
async fn test_list_skills_elides_absolute_location() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

    let tool = ListSkillsTool::new(skills_dir);
    let out = AlephTool::call(&tool, ListSkillsArgs { filter: None })
        .await
        .unwrap();

    assert_eq!(out.count, 1);
    let summary = &out.skills[0];
    assert_eq!(summary.id, "test-skill");
    // The listing must not hand the model an absolute path table (a `cat`
    // enabler); the model reads a skill by its `id`.
    assert!(
        summary.location.is_none(),
        "location must be elided in the listing"
    );
    let v = serde_json::to_value(summary).unwrap();
    assert!(
        v.get("location").is_none(),
        "location key must be skipped on serialize"
    );
}

/// A skill body is returned as instructions — unfenced, and rightly so, since
/// a skill *is* instructions. This asserts the other half arrives with it: the
/// reader can see where the words came from.
///
/// Asserting the field on the tool's *output* rather than that `guess_source`
/// was called: a test that only proves the derivation exists guards the origin,
/// not the wire.
#[tokio::test]
async fn a_read_skill_says_where_its_instructions_came_from() {
    let temp = TempDir::new().unwrap();
    create_test_skill(temp.path(), "demo", "Demo", "A demo skill");

    let tool = ReadSkillTool::new(temp.path().to_path_buf());
    let out = tool
        .call(ReadSkillArgs {
            skill_id: "demo".to_string(),
            file_name: None,
        })
        .await
        .expect("the fixture skill reads");

    assert!(out.success);
    assert!(
        !out.provenance.trim().is_empty(),
        "the body arrives as instructions; where it came from must arrive with it"
    );
    // A temp dir is neither the global skills root nor a `.aleph/skills`
    // checkout path, so `guess_source` classes it `Bundled` — the important
    // part is that the value is the shared derivation's, not a second reading
    // invented here.
    assert_eq!(
        out.provenance,
        crate::skill::guess_source(std::path::Path::new(&out.location)).provenance()
    );

    // And it survives serialisation — this output is read by a model, not by
    // Rust, so a field that never leaves the struct is not delivered.
    let wire = serde_json::to_value(&out).unwrap();
    assert_eq!(
        wire.get("provenance").and_then(|v| v.as_str()),
        Some(out.provenance.as_str())
    );
}
