//! Guide file deployment — embeds config guides at compile time
//! and deploys them to ~/.aleph/guides/ on server start.

use std::path::Path;

/// `(filename, embedded content)` for every deployed guide.
///
/// Public so the read side (`builtin_tools::config_guide::GuideTopic`) can
/// assert that every topic it advertises has a file behind it — the two lists
/// are the same fact and must not drift.
pub const GUIDE_FILES: &[(&str, &str)] = &[
    ("overview.md", include_str!("../../docs/guides/overview.md")),
    (
        "providers.md",
        include_str!("../../docs/guides/providers.md"),
    ),
    ("mcp.md", include_str!("../../docs/guides/mcp.md")),
    ("skills.md", include_str!("../../docs/guides/skills.md")),
    ("agents.md", include_str!("../../docs/guides/agents.md")),
    ("general.md", include_str!("../../docs/guides/general.md")),
    (
        "generation.md",
        include_str!("../../docs/guides/generation.md"),
    ),
    ("channels.md", include_str!("../../docs/guides/channels.md")),
    ("cron.md", include_str!("../../docs/guides/cron.md")),
    (
        "multi_channel.md",
        include_str!("../../docs/guides/multi_channel.md"),
    ),
    ("cluster.md", include_str!("../../docs/guides/cluster.md")),
];

/// Deploy guide files to `{aleph_dir}/guides/`.
///
/// Called during server startup. Overwrites existing files
/// (guides are not user-editable; source of truth is in the repo).
pub fn deploy_guides(aleph_dir: &Path) -> std::io::Result<()> {
    let guides_dir = aleph_dir.join("guides");
    std::fs::create_dir_all(&guides_dir)?;
    for (name, content) in GUIDE_FILES {
        std::fs::write(guides_dir.join(name), content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::GUIDE_FILES;

    #[test]
    fn embeds_new_architecture_guides() {
        let names: Vec<&str> = GUIDE_FILES.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"multi_channel.md"));
        assert!(names.contains(&"cluster.md"));
        for (name, content) in GUIDE_FILES {
            if *name == "multi_channel.md" || *name == "cluster.md" {
                assert!(!content.trim().is_empty(), "{name} is empty");
            }
        }
    }
}
