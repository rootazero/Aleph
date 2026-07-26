//! Self-contained session export.
//!
//! Renders a conversation plus the artifacts that passed through it into ONE
//! HTML file that opens offline in any browser, with every image and document
//! embedded as a `data:` URI.
//!
//! # Why there is no JavaScript in the output
//!
//! The generated document contains **no `<script>` element at all**. That is
//! not an aesthetic choice: it is what lets the document declare
//!
//! ```text
//! default-src 'none'; img-src data:; style-src 'unsafe-inline'
//! ```
//!
//! An export is served from the gateway's own origin, so a script inside it
//! would run beside the Panel's `localStorage` and its WebSocket credentials.
//! With no `script-src` in the policy and `default-src 'none'`, script cannot
//! execute even if the escaping below has a hole — the CSP is the backstop,
//! the escaping is the first line. Adding a single `<script>` would forfeit
//! that, so the renderer must stay declarative.
//!
//! # Byte budget
//!
//! A self-contained document has no lazy loading: everything inlined is parsed
//! before first paint, and base64 inflates by 4/3. The renderer therefore
//! enforces both a per-artifact and a whole-document ceiling and degrades to a
//! listed-but-not-embedded row rather than emitting an unbounded file.

mod markdown;
mod session_html;

pub use session_html::render_session_html;

/// Largest single artifact that may be inlined as a `data:` URI.
pub const MAX_INLINE_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024;

/// Largest total volume of inlined artifact bytes in one document.
pub const MAX_INLINE_TOTAL_BYTES: u64 = 24 * 1024 * 1024;

/// One conversation turn as it should appear in the export.
///
/// `text` is Markdown; it is rendered through the sanitizing pipeline in
/// [`markdown`], never emitted raw.
#[derive(Debug, Clone)]
pub struct ExportMessage {
    /// Speaker label (`user`, `assistant`, …). Escaped before display.
    pub role: String,
    /// Markdown body.
    pub text: String,
    /// Pre-formatted timestamp. Escaped before display.
    pub timestamp: String,
}

/// One artifact offered to the export.
///
/// `bytes` is `None` when the caller already decided not to ship the payload
/// (missing on disk, over budget, …). The renderer still lists the artifact so
/// the reader knows it existed — a silently dropped attachment is worse than a
/// visible placeholder. The renderer re-checks the budget itself, so a caller
/// that hands over an oversized payload cannot blow the ceiling.
#[derive(Debug, Clone)]
pub struct ExportArtifact {
    /// Display name. Escaped before it reaches an attribute.
    pub filename: String,
    /// MIME type as recorded by the artifact store.
    pub mime_type: String,
    /// Payload, or `None` to list without embedding.
    pub bytes: Option<Vec<u8>>,
    /// Size in bytes of the original artifact, independent of `bytes`.
    pub size: u64,
}
