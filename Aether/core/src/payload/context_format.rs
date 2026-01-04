/// ContextFormat enumeration - Context data injection format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextFormat {
    /// Markdown format (default, implemented in MVP)
    #[default]
    Markdown,

    /// XML format (reserved)
    Xml,

    /// JSON format (reserved)
    Json,
}

impl ContextFormat {
    /// Parse from string (for config files)
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "markdown" | "md" => Ok(ContextFormat::Markdown),
            "xml" => Ok(ContextFormat::Xml),
            "json" => Ok(ContextFormat::Json),
            _ => Err(format!("Unknown context format: {}", s)),
        }
    }

    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            ContextFormat::Markdown => "markdown",
            ContextFormat::Xml => "xml",
            ContextFormat::Json => "json",
        }
    }
}

impl std::fmt::Display for ContextFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_format_parse() {
        assert_eq!(
            ContextFormat::parse("markdown").unwrap(),
            ContextFormat::Markdown
        );
        assert_eq!(ContextFormat::parse("MD").unwrap(), ContextFormat::Markdown);
        assert_eq!(ContextFormat::parse("xml").unwrap(), ContextFormat::Xml);
        assert_eq!(ContextFormat::parse("JSON").unwrap(), ContextFormat::Json);
        assert!(ContextFormat::parse("invalid").is_err());
    }

    #[test]
    fn test_context_format_default() {
        assert_eq!(ContextFormat::default(), ContextFormat::Markdown);
    }

    #[test]
    fn test_context_format_display() {
        assert_eq!(ContextFormat::Markdown.to_string(), "markdown");
        assert_eq!(ContextFormat::Xml.to_string(), "xml");
        assert_eq!(ContextFormat::Json.to_string(), "json");
    }
}
