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
    std::fs::create_dir_all(sk.join("references")).unwrap();
    std::fs::write(
        sk.join("SKILL.md"),
        "---\nname: withref\ndescription: d\n---\nx",
    )
    .unwrap();
    std::fs::write(sk.join("references").join("guide.md"), "REF-CONTENT").unwrap();

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
    std::fs::create_dir_all(&sk).unwrap();
    std::fs::write(
        sk.join("SKILL.md"),
        "---\nname: trav\ndescription: d\n---\nx",
    )
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
async fn duplicate_skill_across_dirs_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("a");
    let dir_b = tmp.path().join("b");
    for d in [&dir_a, &dir_b] {
        let sk = d.join("dup");
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(
            sk.join("SKILL.md"),
            "---\nname: dup\ndescription: d\n---\nx",
        )
        .unwrap();
    }
    let tool = ReadSkillTool::with_directories(vec![dir_a, dir_b]);
    let err = AlephTool::call(
        &tool,
        ReadSkillArgs {
            skill_id: "dup".to_string(),
            file_name: None,
        },
    )
    .await
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ambiguous") || msg.contains("multiple"),
        "got: {msg}"
    );
}
