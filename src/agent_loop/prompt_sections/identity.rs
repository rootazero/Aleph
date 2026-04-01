//! Identity section — who the assistant is.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

const DEFAULT_IDENTITY: &str = "You are a helpful personal AI assistant.";

pub fn render(soul_identity: Option<&str>, persona_prefix: Option<&str>) -> PromptSection {
    let mut content = String::from("# Identity\n\n");
    if let Some(persona) = persona_prefix {
        content.push_str(persona);
        content.push_str("\n\n");
    }
    content.push_str(soul_identity.unwrap_or(DEFAULT_IDENTITY));

    PromptSection {
        name: "identity".into(),
        stability: Stability::Stable,
        priority: 50,
        protected: true,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_identity_when_none() {
        let section = render(None, None);
        assert_eq!(section.name, "identity");
        assert!(section.protected);
        assert_eq!(section.priority, 50);
        assert!(section.content.contains(DEFAULT_IDENTITY));
    }

    #[test]
    fn custom_identity() {
        let section = render(Some("I am Aleph."), None);
        assert!(section.content.contains("I am Aleph."));
        assert!(!section.content.contains(DEFAULT_IDENTITY));
    }

    #[test]
    fn persona_prefix_prepended() {
        let section = render(Some("I am Aleph."), Some("You speak like a pirate."));
        let persona_pos = section.content.find("You speak like a pirate.").unwrap();
        let identity_pos = section.content.find("I am Aleph.").unwrap();
        assert!(persona_pos < identity_pos);
    }
}
