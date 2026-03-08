//! Inline directive extraction from user input.
//!
//! Pre-processes user input to extract directives like `/think high`, `/model claude`,
//! `/verbose` before intent classification. Unregistered `/xxx` tokens (e.g. `/etc/hosts`,
//! `/search`) are preserved in the cleaned text.

use std::collections::HashMap;

/// A registered directive definition.
#[derive(Debug, Clone)]
pub struct DirectiveDefinition {
    /// The directive name (lowercase).
    pub name: String,
    /// Whether this directive accepts a value argument.
    pub accepts_value: bool,
}

/// An extracted directive from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// The directive name (lowercase).
    pub name: String,
    /// The optional value (preserves original case).
    pub value: Option<String>,
}

/// Result of parsing user input for directives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInput {
    /// The input text with directives removed and whitespace collapsed.
    pub cleaned_text: String,
    /// Extracted directives in order of appearance.
    pub directives: Vec<Directive>,
}

impl ParsedInput {
    /// Check if a directive was present (case-insensitive name).
    pub fn has_directive(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.directives.iter().any(|d| d.name == lower)
    }

    /// Get the value of a directive by name (case-insensitive).
    pub fn directive_value(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.directives
            .iter()
            .find(|d| d.name == lower)
            .and_then(|d| d.value.as_deref())
    }
}

/// Parser that extracts registered directives from user input.
#[derive(Debug, Clone)]
pub struct DirectiveParser {
    registry: HashMap<String, DirectiveDefinition>,
}

impl DirectiveParser {
    /// Create a parser with an empty registry.
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    /// Create a parser with built-in directives registered.
    pub fn with_builtins() -> Self {
        let mut parser = Self::new();
        parser.register("think", true);
        parser.register("model", true);
        parser.register("verbose", false);
        parser.register("brief", false);
        parser.register("notools", false);
        parser
    }

    /// Register a directive name. `accepts_value` controls whether the next
    /// token after the directive is consumed as its value.
    pub fn register(&mut self, name: &str, accepts_value: bool) {
        let lower = name.to_lowercase();
        self.registry.insert(
            lower.clone(),
            DirectiveDefinition {
                name: lower,
                accepts_value,
            },
        );
    }

    /// Parse input, extracting registered directives and producing cleaned text.
    pub fn parse(&self, input: &str) -> ParsedInput {
        let tokens = self.tokenize(input);
        let mut directives = Vec::new();
        let mut kept_tokens: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < tokens.len() {
            let token = tokens[i];
            if let Some(name) = self.match_directive(token) {
                if let Some(def) = self.registry.get(&name) {
                    if def.accepts_value {
                        // Consume next token as value if available
                        let value = if i + 1 < tokens.len() {
                            i += 1;
                            Some(tokens[i].to_string())
                        } else {
                            None
                        };
                        directives.push(Directive { name, value });
                    } else {
                        directives.push(Directive {
                            name,
                            value: None,
                        });
                    }
                } else {
                    // Not registered — keep in output
                    kept_tokens.push(token);
                }
            } else {
                kept_tokens.push(token);
            }
            i += 1;
        }

        ParsedInput {
            cleaned_text: kept_tokens.join(" "),
            directives,
        }
    }

    /// Split input into whitespace-delimited tokens.
    fn tokenize<'a>(&self, input: &'a str) -> Vec<&'a str> {
        input.split_whitespace().collect()
    }

    /// If `token` starts with `/` and the rest is a registered ASCII-alphanumeric
    /// directive name, return the lowercased name. Otherwise return None.
    fn match_directive(&self, token: &str) -> Option<String> {
        // Must start with '/'
        if !token.starts_with('/') {
            return None;
        }

        // Extract the name part after '/' — must be all ASCII alphanumeric
        let name_part = &token[1..]; // safe: '/' is single-byte ASCII
        if name_part.is_empty() {
            return None;
        }
        if !name_part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }

        let lower = name_part.to_lowercase();
        if self.registry.contains_key(&lower) {
            Some(lower)
        } else {
            None
        }
    }
}

impl Default for DirectiveParser {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> DirectiveParser {
        DirectiveParser::with_builtins()
    }

    #[test]
    fn extract_single_directive_with_value() {
        let result = parser().parse("/think high help me code");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("high"));
        assert_eq!(result.cleaned_text, "help me code");
    }

    #[test]
    fn extract_multiple_directives() {
        let result = parser().parse("translate this /model claude /verbose");
        assert_eq!(result.directives.len(), 2);
        assert_eq!(result.directives[0].name, "model");
        assert_eq!(result.directives[0].value.as_deref(), Some("claude"));
        assert_eq!(result.directives[1].name, "verbose");
        assert_eq!(result.directives[1].value, None);
        assert_eq!(result.cleaned_text, "translate this");
    }

    #[test]
    fn unregistered_directive_preserved() {
        let result = parser().parse("read /etc/hosts");
        assert!(result.directives.is_empty());
        assert_eq!(result.cleaned_text, "read /etc/hosts");
    }

    #[test]
    fn slash_command_preserved() {
        let result = parser().parse("/search rust async");
        assert!(result.directives.is_empty());
        assert_eq!(result.cleaned_text, "/search rust async");
    }

    #[test]
    fn boolean_directive() {
        let result = parser().parse("/verbose what is the weather");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "verbose");
        assert_eq!(result.directives[0].value, None);
        assert_eq!(result.cleaned_text, "what is the weather");
    }

    #[test]
    fn directive_only_no_text() {
        let result = parser().parse("/think high");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("high"));
        assert_eq!(result.cleaned_text, "");
    }

    #[test]
    fn directive_at_start_with_slash_command() {
        let result = parser().parse("/think high /search query");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("high"));
        assert_eq!(result.cleaned_text, "/search query");
    }

    #[test]
    fn case_insensitive_directive() {
        let result = parser().parse("/Think High some text");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("High"));
        assert_eq!(result.cleaned_text, "some text");
    }

    #[test]
    fn empty_input() {
        let result = parser().parse("");
        assert!(result.directives.is_empty());
        assert_eq!(result.cleaned_text, "");
    }

    #[test]
    fn no_directives_in_plain_text() {
        let result = parser().parse("hello world");
        assert!(result.directives.is_empty());
        assert_eq!(result.cleaned_text, "hello world");
    }

    #[test]
    fn directive_between_text() {
        let result = parser().parse("help me /think high with coding");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("high"));
        assert_eq!(result.cleaned_text, "help me with coding");
    }

    #[test]
    fn multiple_spaces_collapsed() {
        let result = parser().parse("hello  /verbose  world");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.cleaned_text, "hello world");
    }
}
