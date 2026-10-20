//! Session Management UI Components
//!
//! Components for viewing session history, transcripts, tool outputs,
//! and attachments from the markdown-based session storage.

pub mod attachments;
pub mod detail;
pub mod filters;
pub mod list;
pub mod search;
pub mod tools;
pub mod transcript;
pub mod view;

pub use crate::api::session_management::{CompressionStatus, SessionMetadata};
pub use attachments::AttachmentGallery;
pub use detail::SessionDetail;
pub use search::SearchBar;
pub use tools::ToolOutputsTab;
pub use transcript::{MessageRole, TranscriptMessage, TranscriptViewer};
pub use view::{SessionFilterParams, SessionsView};
