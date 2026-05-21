//! Stage E integration tests for the filesystem agent loader.

use alephcore::agents::loader::load_agents;
use alephcore::agents::AgentSource;
use std::io::Write;
use std::path::Path;

fn write_md(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
}

#[test]
fn priority_project_over_user_over_builtin() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let user_dir = home.path().join("data/agents");
    let project_dir = project.path().join(".aleph/agents");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    write_md(
        &user_dir,
        "explore.md",
        "---\nid: explore\ndescription: User explore override\nwhen_to_use: user-tier test\n---\n",
    );
    write_md(&project_dir, "explore.md", "---\nid: explore\ndescription: Project explore override\nwhen_to_use: project-tier test\n---\n");

    let (agents, shadows) = load_agents(home.path(), Some(project.path())).expect("load_agents");
    let by_id: std::collections::HashMap<_, _> = agents.iter().map(|a| (a.id.clone(), a)).collect();
    let explore = by_id.get("explore").expect("explore must be present");
    assert_eq!(explore.source, AgentSource::Project);
    assert_eq!(explore.description, "Project explore override");
    assert_eq!(shadows.len(), 2);
    assert!(shadows.iter().any(|s| s.id == "explore"
        && s.winner_source == AgentSource::User
        && s.shadowed_source == AgentSource::Builtin));
    assert!(shadows.iter().any(|s| s.id == "explore"
        && s.winner_source == AgentSource::Project
        && s.shadowed_source == AgentSource::User));
}

#[test]
fn skip_malformed_file_continues_loading() {
    let home = tempfile::tempdir().unwrap();
    let user_dir = home.path().join("data/agents");
    std::fs::create_dir_all(&user_dir).unwrap();
    write_md(
        &user_dir,
        "good-agent.md",
        "---\nid: good-agent\ndescription: Loads OK\nwhen_to_use: testing skip\n---\n",
    );
    write_md(&user_dir, "broken.md", "this file has no frontmatter\n");
    let (agents, _shadows) = load_agents(home.path(), None).expect("load_agents");
    let by_id: std::collections::HashMap<_, _> = agents.iter().map(|a| (a.id.clone(), a)).collect();
    assert!(by_id.contains_key("good-agent"));
    assert!(!by_id.contains_key("broken"));
}

#[test]
fn loader_error_on_id_mismatch_is_skipped_per_file() {
    let home = tempfile::tempdir().unwrap();
    let user_dir = home.path().join("data/agents");
    std::fs::create_dir_all(&user_dir).unwrap();
    write_md(
        &user_dir,
        "foo.md",
        "---\nid: bar\ndescription: x\nwhen_to_use: x\n---\n",
    );
    write_md(
        &user_dir,
        "valid.md",
        "---\nid: valid\ndescription: x\nwhen_to_use: x\n---\n",
    );
    let (agents, _) = load_agents(home.path(), None).expect("load_agents");
    let ids: Vec<_> = agents.iter().map(|a| a.id.clone()).collect();
    assert!(ids.contains(&"valid".to_string()));
    assert!(!ids.contains(&"bar".to_string()));
}

#[test]
fn loader_returns_ok_when_no_user_agent_dir() {
    let home = tempfile::tempdir().unwrap();
    let result = load_agents(home.path(), None);
    assert!(result.is_ok());
    let (agents, shadows) = result.unwrap();
    assert!(!agents.is_empty());
    assert!(shadows.is_empty());
}
