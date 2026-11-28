use super::*;
use tempfile::TempDir;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (name, content) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

fn resources(output: &AdapterOutput) -> Vec<(&str, SkillType)> {
    output
        .capabilities
        .iter()
        .filter_map(|entry| match entry {
            CapabilityDeclaration::Skill(skill) => Some((skill.name.as_str(), skill.skill_type)),
            _ => None,
        })
        .collect()
}

#[test]
fn pi_filters_only_the_selected_resource_kind() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        &[
            (
                "skills/visible.md",
                "---\nname: visible\ndescription: A flat skill\n---\nwork",
            ),
            (
                "skills/empty/SKILL.md",
                "---\nname: empty\ndescription: A nested skill\n---\nwork",
            ),
            (
                "prompts/inner/aa.md",
                "---\nname: renamed\ndescription: A Pi prompt\n---\n$1",
            ),
            ("prompts/inner/ab.md", "drop"),
            ("prompts/inner/ac.md", "remove even after re-add"),
        ],
    );
    let conventional = PiAdapter.parse(dir.path()).unwrap();
    assert_eq!(
        resources(&conventional),
        [
            ("empty", SkillType::Skill),
            ("visible", SkillType::Skill),
            ("aa", SkillType::Command),
            ("ab", SkillType::Command),
            ("ac", SkillType::Command),
        ]
    );
    write(
        dir.path(),
        &[(
            "package.json",
            r#"{
        "name": "pi-package", "pi": {"skills": [], "prompts": [
            "prompts/**/a?.md", "!a*.md", "+prompts/inner/aa.md",
            "+prompts/inner/ac.md", "-prompts/inner/ac.md"
        ]}
    }"#,
        )],
    );
    let explicit = PiAdapter.parse(dir.path()).unwrap();
    assert_eq!(resources(&explicit), [("aa", SkillType::Command)]);
}

#[test]
fn unsupported_pi_construct_omits_only_affected_command() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        &[
            (
                "package.json",
                r#"{"name":"pi-package","pi":{"prompts":["prompts"],"extensions":["should-not-run.js"]}}"#,
            ),
            ("prompts/bad.md", "unsupported ${@:1:2:3}"),
            ("prompts/good.md", "selected ${@:1:1}"),
        ],
    );
    let output = PiAdapter.parse(dir.path()).unwrap();
    assert_eq!(resources(&output), [("good", SkillType::Command)]);
    assert!(
        output
            .diagnostics
            .iter()
            .any(|item| item.path.ends_with("bad.md"))
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|item| item.path.ends_with("package.json") && item.message.contains("extensions"))
    );
}

#[test]
fn pi_escape_rejects_candidate_even_with_a_valid_skill() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        &[
            (
                "package.json",
                r#"{"name":"pi-package","pi":{"skills":["skills"],"prompts":["../external.md"]}}"#,
            ),
            (
                "skills/ok/SKILL.md",
                "---\nname: ok\ndescription: Skill\n---\nwork",
            ),
        ],
    );
    let err = PiAdapter.parse(dir.path()).unwrap_err();
    assert!(err.to_string().contains("within the package"));
}

#[test]
fn ordinary_npm_package_is_not_a_pi_plugin() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        &[("package.json", r#"{"name":"an-npm-package"}"#)],
    );
    assert!(PiAdapter.parse_if_detected(dir.path()).unwrap().is_none());
    let registry = crate::extension::manifest::adapter::AdapterRegistry::defaults();
    let error = registry.parse_dir_fresh(dir.path()).unwrap_err();
    assert!(error.is::<crate::extension::manifest::adapter::NoManifestAdapter>());
    write(
        dir.path(),
        &[
            (
                "aleph.plugin.toml",
                "[plugin]\nid = 'native-plugin'\nname = 'Native'\nversion = '1.0.0'\n",
            ),
            (
                "package.json",
                r#"{"name":"pi-package","pi":{"prompts":[]}}"#,
            ),
        ],
    );
    assert_eq!(
        registry.parse_dir_fresh(dir.path()).unwrap().source.format,
        SourceFormat::AlephToml
    );
}
