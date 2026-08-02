//! MCP Resource Content
//!
//! Content returned when reading resources from MCP servers - files, data,
//! and other content that can be referenced and read by the AI.
//!
//! MCP resources are similar to files or data sources that servers expose
//! for reading. They can be text files, images, database records, or any
//! other content type.

/// Content returned when reading a resource
#[derive(Debug, Clone)]
pub enum ResourceContent {
    /// Text content (most common)
    Text(String),
    /// Binary content with MIME type
    Binary {
        /// Raw binary data
        data: Vec<u8>,
        /// MIME type (e.g., "application/octet-stream")
        mime_type: String,
    },
}
