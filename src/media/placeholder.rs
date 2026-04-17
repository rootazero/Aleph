//! Structured media placeholders for non-native LLM injection.

use std::collections::HashMap;

/// Type of media represented by a placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaPlaceholderType {
    Image,
    Audio,
    File,
}

impl MediaPlaceholderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaPlaceholderType::Image => "image",
            MediaPlaceholderType::Audio => "audio",
            MediaPlaceholderType::File => "file",
        }
    }
}

/// A structured placeholder like `{{media:image:a1b2c3d4}}`.
#[derive(Debug, Clone)]
pub struct MediaPlaceholder {
    pub ty: MediaPlaceholderType,
    pub id: String,
}

impl MediaPlaceholder {
    pub fn new(ty: MediaPlaceholderType, id: impl Into<String>) -> Self {
        Self { ty, id: id.into() }
    }

    pub fn to_text(&self) -> String {
        format!("{{{{media:{}:{}}}}}", self.ty.as_str(), self.id)
    }
}

/// Registry mapping placeholder IDs back to human-readable descriptions.
#[derive(Debug, Clone, Default)]
pub struct MediaRegistry {
    entries: HashMap<String, MediaRecord>,
}

#[derive(Debug, Clone)]
pub struct MediaRecord {
    pub original_name: String,
    pub mime_type: String,
    pub description: Option<String>,
}

impl MediaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: impl Into<String>, record: MediaRecord) -> MediaPlaceholder {
        let id = id.into();
        self.entries.insert(id.clone(), record);
        MediaPlaceholder::new(MediaPlaceholderType::File, id)
    }

    pub fn resolve(&self, id: &str) -> Option<&MediaRecord> {
        self.entries.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_text_format() {
        let ph = MediaPlaceholder::new(MediaPlaceholderType::Image, "abc123");
        assert_eq!(ph.to_text(), "{{media:image:abc123}}");
    }

    #[test]
    fn test_registry_roundtrip() {
        let mut reg = MediaRegistry::new();
        let ph = reg.register(
            "doc1",
            MediaRecord {
                original_name: "report.pdf".to_string(),
                mime_type: "application/pdf".to_string(),
                description: None,
            },
        );
        assert_eq!(ph.to_text(), "{{media:file:doc1}}");
        let rec = reg.resolve("doc1").unwrap();
        assert_eq!(rec.original_name, "report.pdf");
    }
}
