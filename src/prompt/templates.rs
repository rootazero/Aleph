//! Prompt template system with variable substitution.
//!
//! Provides a simple template system for dynamic prompt generation.

use std::collections::HashMap;

/// A variable that can be substituted in a template
#[derive(Debug, Clone)]
pub enum TemplateVar {
    /// Simple string value
    String(String),
    /// List of items (will be joined with newlines)
    List(Vec<String>),
    /// Optional value (empty string if None)
    Optional(Option<String>),
}

impl From<String> for TemplateVar {
    fn from(s: String) -> Self {
        TemplateVar::String(s)
    }
}

impl From<&str> for TemplateVar {
    fn from(s: &str) -> Self {
        TemplateVar::String(s.to_string())
    }
}

impl From<Vec<String>> for TemplateVar {
    fn from(v: Vec<String>) -> Self {
        TemplateVar::List(v)
    }
}

impl<T: Into<String>> From<Option<T>> for TemplateVar {
    fn from(opt: Option<T>) -> Self {
        TemplateVar::Optional(opt.map(Into::into))
    }
}

impl TemplateVar {
    /// Render the variable to a string
    pub fn render(&self) -> String {
        match self {
            TemplateVar::String(s) => s.clone(),
            TemplateVar::List(items) => items.join("\n"),
            TemplateVar::Optional(opt) => opt.clone().unwrap_or_default(),
        }
    }
}

/// A prompt template with variable placeholders
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// Raw template string with {variable} placeholders
    template: String,
}

impl PromptTemplate {
    /// Create a new template
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
        }
    }

    /// Render the template with variables
    pub fn render(&self, vars: &HashMap<String, TemplateVar>) -> String {
        let mut result = self.template.clone();

        for (key, value) in vars {
            let placeholder = format!("{{{}}}", key);
            result = result.replace(&placeholder, &value.render());
        }

        // Clean up any remaining empty placeholders
        result = Self::clean_empty_sections(&result);
        result
    }

    /// Remove empty sections (sections with only whitespace after variable substitution)
    fn clean_empty_sections(text: &str) -> String {
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get the raw template
    pub fn raw(&self) -> &str {
        &self.template
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_basic_substitution() {
        let template = PromptTemplate::new("Hello, {name}!");
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), TemplateVar::String("World".to_string()));

        let result = template.render(&vars);
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_template_list_substitution() {
        let template = PromptTemplate::new("Tools:\n{tools}");
        let mut vars = HashMap::new();
        vars.insert(
            "tools".to_string(),
            TemplateVar::List(vec!["- tool1".to_string(), "- tool2".to_string()]),
        );

        let result = template.render(&vars);
        assert!(result.contains("- tool1"));
        assert!(result.contains("- tool2"));
    }

    #[test]
    fn test_template_optional_empty() {
        let template = PromptTemplate::new("Value: {maybe}");
        let mut vars = HashMap::new();
        vars.insert("maybe".to_string(), TemplateVar::Optional(None));

        let result = template.render(&vars);
        // Empty value leaves trailing space (trimming is not automatic)
        assert_eq!(result, "Value: ");
    }

    #[test]
    fn test_template_optional_present() {
        let template = PromptTemplate::new("Value: {maybe}");
        let mut vars = HashMap::new();
        vars.insert(
            "maybe".to_string(),
            TemplateVar::Optional(Some("present".to_string())),
        );

        let result = template.render(&vars);
        assert_eq!(result, "Value: present");
    }

    #[test]
    fn test_template_from_conversions() {
        let _s: TemplateVar = "test".into();
        let _v: TemplateVar = vec!["a".to_string(), "b".to_string()].into();
        let _o: TemplateVar = Some("value".to_string()).into();
        let _n: TemplateVar = Option::<String>::None.into();
    }
}
