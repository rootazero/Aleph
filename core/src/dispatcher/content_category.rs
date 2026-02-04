//! Content-based Tool Category Detection
//!
//! Analyzes user requests and skill instructions to infer which tool categories
//! might be needed. This is used by SmartToolFilter to enhance intent-based filtering.
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::dispatcher::{ContentCategory, infer_required_tools};
//!
//! let categories = infer_required_tools(
//!     "Download the YouTube video and summarize",
//!     "process this content"
//! );
//! // Returns [ContentCategory::YouTube, ContentCategory::WebFetch]
//! ```

use serde::{Deserialize, Serialize};

/// Content-based tool category for smart filtering
///
/// Unlike `ToolCategory` (which classifies by source: Builtin, Mcp, etc.),
/// this enum classifies tools by their **functional purpose** for content analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCategory {
    /// File system operations (read, write, list, move)
    FileOps,
    /// Web search capabilities
    Search,
    /// URL fetching and web scraping
    WebFetch,
    /// YouTube video information and transcripts
    YouTube,
    /// Shell/bash command execution
    Bash,
    /// Code execution (Python, JavaScript, etc.)
    CodeExec,
    /// Image generation (DALL-E, Stable Diffusion)
    ImageGen,
    /// Video generation
    VideoGen,
    /// Audio generation
    AudioGen,
    /// Speech synthesis (TTS)
    SpeechGen,
}

impl ContentCategory {
    /// Get all categories
    pub fn all() -> &'static [ContentCategory] {
        &[
            ContentCategory::FileOps,
            ContentCategory::Search,
            ContentCategory::WebFetch,
            ContentCategory::YouTube,
            ContentCategory::Bash,
            ContentCategory::CodeExec,
            ContentCategory::ImageGen,
            ContentCategory::VideoGen,
            ContentCategory::AudioGen,
            ContentCategory::SpeechGen,
        ]
    }

    /// Get keywords that indicate this category
    pub fn keywords(&self) -> &'static [&'static str] {
        match self {
            ContentCategory::FileOps => &[
                "file", "folder", "directory", "read", "write", "save", "delete",
                "move", "copy", "rename", "list", "organize", "path",
            ],
            ContentCategory::Search => &[
                "search", "find", "look up", "google", "query", "browse",
            ],
            ContentCategory::WebFetch => &[
                "fetch", "url", "http", "website", "web page", "download", "scrape",
            ],
            ContentCategory::YouTube => &[
                "youtube", "video", "transcript", "subtitles", "yt",
            ],
            ContentCategory::Bash => &[
                "bash", "shell", "terminal", "command", "execute", "run", "script",
            ],
            ContentCategory::CodeExec => &[
                "code", "python", "javascript", "execute code", "run code", "eval",
            ],
            ContentCategory::ImageGen => &[
                "image", "picture", "photo", "generate image", "create image",
                "dall-e", "stable diffusion", "draw", "illustration",
            ],
            ContentCategory::VideoGen => &[
                "generate video", "create video", "video generation", "animate",
            ],
            ContentCategory::AudioGen => &[
                "generate audio", "create audio", "sound", "music", "audio generation",
            ],
            ContentCategory::SpeechGen => &[
                "speech", "tts", "text to speech", "speak", "voice", "read aloud",
            ],
        }
    }
}

/// Infer required tool categories from skill instructions and user request
///
/// Analyzes both the skill instructions (workflow definition) and the user's
/// actual request to determine which tool categories might be needed.
///
/// # Arguments
///
/// * `skill_instructions` - The skill's instruction text (may be empty)
/// * `user_request` - The user's actual input request
///
/// # Returns
///
/// A vector of detected `ContentCategory` values
pub fn infer_required_tools(skill_instructions: &str, user_request: &str) -> Vec<ContentCategory> {
    let combined = format!("{} {}", skill_instructions, user_request).to_lowercase();

    ContentCategory::all()
        .iter()
        .filter(|category| {
            category.keywords().iter().any(|keyword| combined.contains(keyword))
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_file_ops() {
        let categories = infer_required_tools("", "organize my downloads folder");
        assert!(categories.contains(&ContentCategory::FileOps));
    }

    #[test]
    fn test_infer_youtube() {
        let categories = infer_required_tools(
            "Download YouTube video transcript and summarize",
            "process this content"
        );
        assert!(categories.contains(&ContentCategory::YouTube));
    }

    #[test]
    fn test_infer_image_gen() {
        let categories = infer_required_tools("", "create an image of a sunset");
        assert!(categories.contains(&ContentCategory::ImageGen));
    }

    #[test]
    fn test_infer_multiple() {
        let categories = infer_required_tools(
            "",
            "search for rust tutorials and save to a file"
        );
        assert!(categories.contains(&ContentCategory::Search));
        assert!(categories.contains(&ContentCategory::FileOps));
    }

    #[test]
    fn test_empty_input() {
        let categories = infer_required_tools("", "");
        assert!(categories.is_empty());
    }

    #[test]
    fn test_all_categories() {
        assert_eq!(ContentCategory::all().len(), 10);
    }
}
