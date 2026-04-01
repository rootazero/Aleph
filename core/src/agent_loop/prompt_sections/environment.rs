//! Environment section — runtime environment information.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};
use crate::context::EnvironmentInfo;

pub fn render(env: &EnvironmentInfo) -> PromptSection {
    let mut lines = Vec::new();
    lines.push(format!("- Working directory: {}", env.cwd));
    lines.push(format!("- Is git repository: {}", env.is_git));
    if let Some(branch) = &env.git_branch {
        lines.push(format!("- Git branch: {}", branch));
    }
    lines.push(format!("- Platform: {}", env.os));
    lines.push(format!("- OS Version: {}", env.os_version));
    lines.push(format!("- Shell: {}", env.shell));
    lines.push(format!("- Date: {}", env.date));
    if let Some(model) = &env.model_name {
        lines.push(format!("- Model: {}", model));
    }
    if let Some(cutoff) = &env.knowledge_cutoff {
        lines.push(format!("- Knowledge cutoff: {}", cutoff));
    }

    PromptSection {
        name: "environment".into(),
        stability: Stability::Dynamic,
        priority: 1600,
        protected: true,
        content: format!("# Environment\n\n{}", lines.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_fields_from_test_env() {
        let env = EnvironmentInfo::for_test();
        let section = render(&env);

        assert_eq!(section.name, "environment");
        assert_eq!(section.stability, Stability::Dynamic);
        assert_eq!(section.priority, 1600);
        assert!(section.protected);

        assert!(section.content.contains("Working directory: /test/workspace"));
        assert!(section.content.contains("Is git repository: true"));
        assert!(section.content.contains("Git branch: main"));
        assert!(section.content.contains("Platform: macos"));
        assert!(section.content.contains("OS Version: Darwin 25.4.0"));
        assert!(section.content.contains("Shell: zsh"));
        assert!(section.content.contains("Date: 2026-04-01"));
        assert!(section.content.contains("Model: claude-sonnet-4-6"));
        assert!(section.content.contains("Knowledge cutoff: May 2025"));
    }

    #[test]
    fn omits_optional_fields_when_none() {
        let env = EnvironmentInfo {
            cwd: "/tmp".into(),
            is_git: false,
            git_branch: None,
            os: "linux".into(),
            os_version: "6.1.0".into(),
            shell: "bash".into(),
            date: "2026-01-01".into(),
            model_name: None,
            knowledge_cutoff: None,
        };
        let section = render(&env);
        assert!(!section.content.contains("Git branch"));
        assert!(!section.content.contains("Model:"));
        assert!(!section.content.contains("Knowledge cutoff"));
    }
}
