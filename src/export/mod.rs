//! Self-contained HTML documents.
//!
//! Two of them, deliberately distinct:
//!
//! * [`render_document_html`] — the **deliverable**: the finished work product
//!   the agent published for the user. One title, one body, its figures.
//! * [`render_session_html`] — the **transcript**: the whole conversation that
//!   produced it, with every attachment that passed through.
//!
//! Both are ONE HTML file that opens offline in any browser, with every image
//! and document embedded as a `data:` URI. They share their shell, policy and
//! typography through `page`; only the middle differs.
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
//!
//! Both inline ceilings are counted in the bytes the document actually holds
//! — the base64-inflated length of an inlined artifact — because that is the
//! unit the artifact store measures the finished file in. Counting payload
//! bytes instead understates the document by a third, and the export then
//! fails at the store's door with a storage error rather than with a size
//! that was never going to fit. The transcript counts towards the same
//! total: it is text in the document like any other.

mod collect;
mod document;
mod markdown;
mod page;
mod session_html;

pub use collect::collect_artifacts;
pub use document::render_document_html;
pub use session_html::render_session_html;

/// Largest single artifact that may be inlined as a `data:` URI, counted in
/// its base64-inflated length.
///
/// The unit is the encoded length, not the payload length: base64 turns every
/// three payload bytes into four, and the encoded length is what the document
/// holds.
pub const MAX_INLINE_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024;

/// Largest total volume of inlined artifact bytes in one document, counted in
/// base64-inflated length.
pub const MAX_INLINE_TOTAL_BYTES: u64 = 24 * 1024 * 1024;

/// Largest exported document the artifact store accepts.
///
/// The store refuses anything past [`crate::artifacts::MAX_ARTIFACT_BYTES`],
/// so the exporter measures against the same number: a document that passes
/// this check is one the store will take.
///
/// Crate-internal: the ceiling is the store's, not a knob of the exporter's,
/// so it is not part of the public surface.
pub(crate) const MAX_EXPORTED_DOCUMENT_BYTES: u64 = crate::artifacts::MAX_ARTIFACT_BYTES;

/// Bytes an exported document holds, counted in what is actually written.
///
/// Two terms: the transcript text, and the inlined artifacts after base64
/// inflation. The second term is why payload sizes are not the number to
/// compare against the store's ceiling — base64 turns every three bytes into
/// four, so counting payload bytes understates the document by a third.
///
/// Artifacts that are listed rather than embedded hold no bytes in the
/// document, so they contribute nothing here.
pub(crate) fn exported_document_bytes(transcript_bytes: u64, artifacts: &[ExportArtifact]) -> u64 {
    let inlined: u64 = artifacts
        .iter()
        .filter_map(|artifact| artifact.bytes.as_ref())
        .map(|bytes| base64_encoded_len(bytes.len() as u64))
        .sum();
    transcript_bytes.saturating_add(inlined)
}

/// Length of `bytes` once base64-encoded.
///
/// Base64 turns every three bytes into four and pads the last group, so the
/// encoded length is `4 * ceil(n / 3)`.
fn base64_encoded_len(bytes: u64) -> u64 {
    bytes.div_ceil(3) * 4
}

/// A published deliverable, ready to render.
///
/// The agent decides all three fields. Nothing here is inferred from the
/// conversation — a heuristic that guessed which part of a run was "the
/// result" would be exactly the kind of reasoning that belongs to the model,
/// not to the code around it.
#[derive(Debug, Clone)]
pub struct ExportDocument {
    /// Document title. Escaped before display.
    pub title: String,
    /// One line of provenance under the title (who made it, when). Escaped
    /// before display.
    pub provenance: String,
    /// The deliverable itself, as Markdown. Rendered through the sanitizing
    /// pipeline in `markdown`, never emitted raw.
    pub markdown: String,
}

/// One conversation turn as it should appear in the export.
///
/// `text` is Markdown; it is rendered through the sanitizing pipeline in
/// `markdown`, never emitted raw.
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

#[cfg(test)]
mod tests {
    use super::{base64_encoded_len, exported_document_bytes, ExportArtifact};

    /// The exported size has to be counted in the bytes the document holds:
    /// base64 turns every three payload bytes into four, and the transcript
    /// counts too. A budget counted in payload bytes passes an export the
    /// store will refuse.
    #[test]
    fn the_exported_size_counts_the_inflated_artifact_and_the_transcript() {
        let artifact = ExportArtifact {
            filename: "a.txt".to_string(),
            mime_type: "text/plain".to_string(),
            bytes: Some(vec![0u8; 300]),
            size: 300,
        };
        assert_eq!(base64_encoded_len(300), 400, "300 bytes encode to 400");
        assert_eq!(
            exported_document_bytes(1_000, &[artifact]),
            1_400,
            "the transcript and the inflated artifact both count"
        );
    }

    /// An artifact that is listed rather than embedded holds no bytes in the
    /// document, so it must not push the document towards the ceiling.
    #[test]
    fn a_listed_only_artifact_adds_nothing_to_the_exported_size() {
        let artifact = ExportArtifact {
            filename: "a.txt".to_string(),
            mime_type: "text/plain".to_string(),
            bytes: None,
            size: 10_000_000,
        };
        assert_eq!(exported_document_bytes(10, &[artifact]), 10);
    }
}
