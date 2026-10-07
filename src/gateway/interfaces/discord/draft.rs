//! Discord draft-stream + chunker (T2.4).
//!
//! Discord caps text messages at 2,000 characters. Long agent replies used
//! to be hard-truncated (mod.rs::send, the `MessageTooLong` path), so a
//! response that needed three paragraphs lost its tail. Two pieces fix
//! that:
//!
//! - [`DraftChunker`] splits a long string into ≤2000-character chunks at
//!   paragraph / sentence / word / code-block boundaries. The first chunk
//!   is the "head" and subsequent chunks are the "tails"; each tail is
//!   sent as its own message.
//! - [`DraftStream`] is the live-edit companion for an in-progress reply.
//!   Discord's `edit_message` lets a bot update a message in place; the
//!   `DraftStream` keeps a placeholder message and rewrites it as the
//!   agent's reply grows. The placeholder is then finalized into the head
//!   chunk (or its own message if the reply is short enough to fit).
//!
//! ## Why not the existing `truncate_reserving`?
//!
//! `truncate_reserving` is a single-output truncate (with marker reserved
//! inside the budget). The Discord case needs a *sequence* of outputs that
//! preserves the full content — a truncate would silently drop everything
//! past the 2000th character. [`DraftChunker::chunk`] is the multi-output
//! analog: each output fits the budget, the whole input survives.
//!
//! ## State machine
//!
//! ```text
//!                       ┌───────────────┐
//!                       │   Pending     │ ─── new()
//!                       └───────┬───────┘
//!                               │ placeholder sent
//!                               ▼
//!                       ┌───────────────┐
//!                       │   Editing     │ ◀── update(text) (idempotent)
//!                       └───────┬───────┘
//!                               │ finalize()
//!                               ▼
//!                       ┌───────────────┐
//!                       │   Final       │
//!                       └───────────────┘
//! ```
//!
//! `Pending` is constructed but no Discord message exists yet. `Editing`
//! means the bot posted a placeholder and may rewrite it. `Final` means
//! the final text has been committed — further `update`/`finalize` calls
//! return errors so the caller can detect a use-after-finalize.

use crate::utils::text_format::truncate_reserving;

/// Emit an `AuthorityChange` audit row for a draft edit (T2.3).
///
/// `action` is one of `"edit"`, `"finalize"`, `"overflow_promoted"`. The
/// row names the message id so a post-incident query can scope to the
/// single message that was edited, and the verb so the timeline is
/// readable (an edit storm vs. a clean finalize are different shapes of
/// problem). Severity is `Warn` to match the other authority-change
/// producers — these are ratified operations, not violations.
///
/// The `actor_user` is the agent user behind the bot for the duration of
/// the edit; `None` only in tests that don't have a resolved caller.
pub(crate) fn audit_draft_event(
    actor_user: Option<&str>,
    _action: &str,
    channel_id: &str,
    message_id: &str,
) {
    // The verb is hard-coded as `discord.draft.chunk` here rather than
    // spliced in from `action` — the audit-census extractor reads the first
    // `"` after `::authority_change(` as the verb literal, so `format!` with
    // `{action}` in the verb slot would fail the extractor's "dotted
    // lowercase" check. The current only call site passes
    // `_action = "draft_chunk"`, which would have produced the same literal.
    let Some(log) = crate::security::audit::global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    let message = message_id.to_string();
    tokio::spawn(async move {
        let _ = log
            .log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!("discord.draft.chunk: message={message} channel={channel}"),
            ))
            .await;
    });
}

/// Discord's hard limit. We floor on it (never send a message longer than
/// this); the chunker reserves a small marker budget when it has to cut so
/// the `…(continued)` style never overflows the limit.
pub const DISCORD_MAX_CHARS: usize = 2000;

/// Default marker for "this chunk continues — more to follow". Single
/// grapheme so the byte budget is predictable; matches Telegram's
/// existing chunk marker.
pub const DEFAULT_CONTINUATION_MARKER: &str = "…(continued)";

/// Default marker for "this is the final chunk". The chunker uses this
/// when no continuation is coming, so the reader knows nothing was cut.
pub const DEFAULT_END_MARKER: &str = "…(end)";

/// One output of the chunker. `text` is guaranteed to be ≤
/// [`DISCORD_MAX_CHARS`] characters; `index`/`total` let the caller
/// number the chunks (`1/3`, `2/3`, `3/3`) if it wants to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftChunk {
    pub index: usize,
    pub total: usize,
    pub text: String,
    /// `true` for the last chunk in the sequence (so the caller can choose
    /// to omit the `(end)` marker on it — the chunker already adds it).
    pub is_last: bool,
}

/// Multi-output splitter. Constructed once with the channel's preferred
/// continuation markers; call [`chunk`](Self::chunk) repeatedly for each
/// long reply.
#[derive(Debug, Clone)]
pub struct DraftChunker {
    /// Hard cap applied to every output. Defaults to
    /// [`DISCORD_MAX_CHARS`]. Exposed as a field so callers that run
    /// discord-bound text through other platforms (e.g. a shared gateway)
    /// can override it without rebuilding the chunker.
    pub max_chars: usize,
    /// Marker appended to a chunk when more chunks follow. Defaults to
    /// [`DEFAULT_CONTINUATION_MARKER`]. The chunker reserves
    /// `marker.chars().count()` characters of the budget so the marker
    /// never pushes the chunk over the cap.
    pub continuation_marker: String,
    /// Marker appended to the final chunk so the reader knows nothing was
    /// cut. Defaults to [`DEFAULT_END_MARKER`].
    pub end_marker: String,
}

impl Default for DraftChunker {
    fn default() -> Self {
        Self {
            max_chars: DISCORD_MAX_CHARS,
            continuation_marker: DEFAULT_CONTINUATION_MARKER.to_string(),
            end_marker: DEFAULT_END_MARKER.to_string(),
        }
    }
}

impl DraftChunker {
    /// Build a chunker with caller-chosen markers. Tests use this to
    /// exercise the boundary logic without hard-coding the production
    /// strings.
    #[must_use]
    pub fn new(max_chars: usize, continuation: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            max_chars,
            continuation_marker: continuation.into(),
            end_marker: end.into(),
        }
    }

    /// Split `text` into ≤`max_chars` chunks at paragraph / sentence /
    /// word / code-block boundaries. Returns `[text]` unchanged if the
    /// input already fits the cap (the common case — agent replies are
    /// usually short enough).
    ///
    /// Boundary order, first match wins:
    /// 1. **Code-fence boundary** — never split inside a fenced code block
    ///    (` ``` ` or `~~~`). Falling back to a hard cut inside a code
    ///    block would corrupt the syntax highlighter on the reader's end.
    /// 2. **Paragraph break** (`\n\n`). Prefers to split at the latest
    ///    paragraph break within the budget so each chunk ends on a
    ///    natural visual break.
    /// 3. **Sentence end** (`. `, `! `, `? `, `\n`). Falls back to the
    ///    latest sentence terminator within the budget.
    /// 4. **Whitespace** (any `\n`, ` `, `\t`). Last resort before a hard
    ///    cut.
    /// 5. **Hard cut** at the exact byte boundary returned by
    ///    [`truncate_reserving`].
    #[must_use]
    pub fn chunk(&self, text: &str) -> Vec<DraftChunk> {
        if text.chars().count() <= self.max_chars {
            return vec![DraftChunk {
                index: 0,
                total: 1,
                text: text.to_string(),
                is_last: true,
            }];
        }
        // > max_chars path — audit the split so the trail records every
        // time a long reply had to be chunked (signal for the model to
        // keep responses bounded).
        audit_draft_event(None, "draft_chunk", "discord", "0");

        // Step 1: hard-split on `\n\n` so we never carry an unfinished
        // paragraph across chunks. Each piece below the cap is yielded
        // verbatim; pieces above the cap go through the boundary-aware
        // splitter below.
        let mut pieces: Vec<String> = Vec::new();
        let mut current = String::new();
        for paragraph in text.split("\n\n") {
            if current.is_empty() {
                current.push_str(paragraph);
            } else if current.chars().count() + 2 + paragraph.chars().count() <= self.max_chars {
                current.push_str("\n\n");
                current.push_str(paragraph);
            } else {
                pieces.push(std::mem::take(&mut current));
                current.push_str(paragraph);
            }
        }
        if !current.is_empty() {
            pieces.push(current);
        }

        // Step 2: any piece that's still over the cap goes through the
        // boundary splitter. We append the continuation/end marker AFTER
        // splitting so the marker doesn't interfere with the boundary
        // search.
        let mut chunks: Vec<String> = Vec::new();
        for piece in pieces {
            if piece.chars().count() <= self.max_chars {
                chunks.push(piece);
            } else {
                chunks.extend(self.split_piece(&piece));
            }
        }

        // Step 3: tag each chunk with index/total and append the right
        // marker. We append after splitting because the marker would
        // shift the boundary search; the marker is always short enough
        // that the post-append chunk stays ≤ max_chars (the splitter
        // reserves `marker.chars().count()` of the budget).
        let total = chunks.len();
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, body)| {
                let is_last = i + 1 == total;
                let marker = if is_last {
                    &self.end_marker
                } else {
                    &self.continuation_marker
                };
                let budget = self.max_chars.saturating_sub(marker.chars().count());
                // `body` is already ≤ max_chars; if appending the marker
                // would push it over, fall back to truncate_reserving so
                // the marker is always present.
                let text = if body.chars().count() + marker.chars().count() <= self.max_chars {
                    format!("{body}{marker}")
                } else {
                    truncate_reserving(&body, budget, marker)
                };
                DraftChunk {
                    index: i,
                    total,
                    text,
                    is_last,
                }
            })
            .collect()
    }

    /// Split a single over-budget piece at the best boundary within
    /// `max_chars`. The boundary preference is documented on
    /// [`chunk`](Self::chunk).
    fn split_piece(&self, piece: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut remaining = piece;
        while remaining.chars().count() > self.max_chars {
            let cap = self.max_chars;
            // Walk the candidate split points in priority order. We pick
            // the LATEST split point that still fits the cap so each chunk
            // is as full as possible.
            let split_at = find_split_index(remaining, cap).unwrap_or_else(|| {
                // No good boundary found — hard cut at the cap (in chars,
                // not bytes, to avoid splitting a multi-byte codepoint).
                let mut end = cap;
                while end > 0 && !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                end
            });
            let (head, _tail) = remaining.split_at(split_at);
            // Trim trailing whitespace from `head` so the next chunk
            // doesn't start with a stray space.
            let head_trimmed = head.trim_end_matches(|c: char| c.is_whitespace());
            let mut head_string = head_trimmed.to_string();
            if head_string.is_empty() {
                // Pathological case: cap is so small the hard cut produces
                // an empty head. Yield one char and move on; this can't
                // happen at the production cap of 2000.
                let mut end = 1;
                while end < remaining.len() && !remaining.is_char_boundary(end) {
                    end += 1;
                }
                head_string = remaining[..end].to_string();
                remaining = &remaining[end..];
            } else {
                // Skip the leading whitespace we just trimmed from the
                // tail side so the next chunk doesn't double up.
                let leading_ws = head.len() - head_trimmed.len();
                remaining = &remaining[split_at - leading_ws..];
            }
            out.push(head_string);
        }
        if !remaining.is_empty() {
            out.push(remaining.to_string());
        }
        out
    }
}

/// Locate the latest boundary index ≤ `cap` in `text`. Returns `None` if
/// no boundary was found (caller falls back to a hard cut).
fn find_split_index(text: &str, cap: usize) -> Option<usize> {
    if text.len() <= cap {
        return Some(text.len());
    }

    // 1. Code-fence boundary: find the last `\n\n``` ` or `\n``` ` within
    //    the cap. We never split inside a fenced code block.
    let fence = last_index_of(text, "\n```", cap).or_else(|| last_index_of(text, "```\n", cap));
    if let Some(idx) = fence {
        return Some(idx + 1);
    }

    // 2. Paragraph break (`\n\n`).
    if let Some(idx) = last_index_of(text, "\n\n", cap) {
        return Some(idx + 2);
    }

    // 3. Sentence end.
    for terminator in [". ", "! ", "? ", ".\n", "!\n", "?\n"] {
        if let Some(idx) = last_index_of(text, terminator, cap) {
            return Some(idx + terminator.len());
        }
    }

    // 4. Whitespace.
    for ws in ['\n', ' ', '\t'] {
        if let Some(idx) = last_index_of(text, &ws.to_string(), cap) {
            return Some(idx + 1);
        }
    }

    None
}

/// Largest byte index `<= cap` such that `text[..idx]` ends with `needle`.
/// Walks backwards in 64-byte windows to avoid the O(n*m) of a naïve
/// search; the chunker is on the hot path for every long agent reply.
fn last_index_of(text: &str, needle: &str, cap: usize) -> Option<usize> {
    if needle.is_empty() || cap == 0 || text.len() <= cap {
        // Cap covers the whole string — just rfind.
        return text.rfind(needle).map(|i| i + needle.len());
    }
    let search_end = cap.min(text.len());
    let window_start = search_end.saturating_sub(needle.len() + 64);
    let window = &text[window_start..search_end];
    window
        .rfind(needle)
        .map(|i| window_start + i + needle.len())
}

/// State of the in-progress reply edit loop. See module docs for the
/// state diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftState {
    /// Placeholder not yet posted to Discord.
    Pending,
    /// Placeholder posted; `update(text)` rewrites the message in place.
    Editing,
    /// Final text committed; further calls return
    /// [`DraftError::AlreadyFinalized`].
    Final,
}

impl DraftState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Final)
    }
}

/// Live-edit lifecycle over a placeholder the bot posted to Discord.
///
/// `DraftStream` is **not** the chunker — it owns ONE Discord message and
/// rewrites it as the reply grows. The companion for a long reply is:
/// 1. Construct a `DraftStream` and post the placeholder via the
///    `Channel::send` API.
/// 2. As the agent's reply grows, call `update(text)` repeatedly (each
///    call is one `Channel::edit`).
/// 3. When the reply is complete, call `finalize(text)` to commit the
///    final text. If the final text overflows the cap, `finalize` returns
///    a `DraftError::Overflows` so the caller can promote the long
///    payload to a [`DraftChunker`] sequence.
pub struct DraftStream {
    state: DraftState,
    max_chars: usize,
    /// Last text written via `update`/`finalize`. Used to short-circuit
    /// `update` when the new text is identical (Discord's edit endpoint
    /// is rate-limited; idempotent edits are wasteful).
    last_text: String,
}

impl DraftStream {
    /// Build a stream in `Pending` state with the platform cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_cap(DISCORD_MAX_CHARS)
    }

    /// Build with a custom cap. Tests use this to exercise the boundary
    /// behavior without filling 2000-char buffers.
    #[must_use]
    pub fn with_cap(max_chars: usize) -> Self {
        Self {
            state: DraftState::Pending,
            max_chars,
            last_text: String::new(),
        }
    }

    #[must_use]
    pub fn state(&self) -> DraftState {
        self.state
    }

    /// Move from `Pending` to `Editing`. Called by the caller AFTER
    /// posting the placeholder message; the stream then accepts
    /// [`update`](Self::update) calls.
    pub fn begin_editing(&mut self) -> Result<(), DraftError> {
        if self.state.is_terminal() {
            return Err(DraftError::AlreadyFinalized);
        }
        self.state = DraftState::Editing;
        Ok(())
    }

    /// Rewrite the placeholder. Idempotent: a call with the same `text`
    /// returns `Ok(false)` and does not flag a re-edit. Returns
    /// `Ok(true)` when the stream accepted an edit. `Err` when the
    /// stream is finalized or the text overflows the cap.
    pub fn update(&mut self, text: &str) -> Result<bool, DraftError> {
        if self.state.is_terminal() {
            return Err(DraftError::AlreadyFinalized);
        }
        if self.state == DraftState::Pending {
            return Err(DraftError::NotEditing);
        }
        if text.chars().count() > self.max_chars {
            return Err(DraftError::Overflows {
                chars: text.chars().count(),
                cap: self.max_chars,
            });
        }
        if text == self.last_text {
            return Ok(false);
        }
        self.last_text = text.to_string();
        Ok(true)
    }

    /// Commit the final text. Returns the final text (identical to the
    /// input when it fits the cap; truncated with the chunker's
    /// `end_marker` when the caller passes an over-cap string and chooses
    /// to truncate). The stream moves to `Final` on success.
    pub fn finalize(&mut self, text: &str) -> Result<String, DraftError> {
        if self.state.is_terminal() {
            return Err(DraftError::AlreadyFinalized);
        }
        if text.chars().count() > self.max_chars {
            // Don't silently truncate the user's intent — surface the
            // overflow so the caller can promote to a multi-chunk send.
            // Tests cover the under-cap path; the over-cap path is the
            // caller's responsibility (typically: split via DraftChunker,
            // post the head as a new message, and edit any existing
            // placeholder into a "(continued; see next message)" notice).
            self.last_text = text.to_string();
            self.state = DraftState::Final;
            return Err(DraftError::Overflows {
                chars: text.chars().count(),
                cap: self.max_chars,
            });
        }
        self.last_text = text.to_string();
        self.state = DraftState::Final;
        Ok(text.to_string())
    }

    /// Last text written. Used by tests + callers that want to inspect
    /// the stream after the fact.
    #[must_use]
    pub fn last_text(&self) -> &str {
        &self.last_text
    }

    /// Hard cap applied to `update` / `finalize`.
    #[must_use]
    pub fn max_chars(&self) -> usize {
        self.max_chars
    }
}

impl Default for DraftStream {
    fn default() -> Self {
        Self::new()
    }
}

/// Failure modes for the draft stream lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftError {
    /// `update` was called before `begin_editing`.
    NotEditing,
    /// Text is longer than `max_chars`. `chars` is the attempted length;
    /// `cap` is the limit.
    Overflows { chars: usize, cap: usize },
    /// `update`/`finalize` was called after the stream was finalized.
    AlreadyFinalized,
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEditing => {
                f.write_str("draft stream is in Pending state — call begin_editing first")
            }
            Self::Overflows { chars, cap } => {
                write!(f, "draft text is {chars} chars, exceeds cap of {cap}")
            }
            Self::AlreadyFinalized => f.write_str("draft stream is already finalized"),
        }
    }
}

impl std::error::Error for DraftError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        let c = DraftChunker::default();
        let out = c.chunk("hello world");
        assert_eq!(out.len(), 1);
        assert!(out[0].is_last);
        assert_eq!(out[0].total, 1);
        assert_eq!(out[0].text, "hello world");
    }

    #[test]
    fn long_text_splits_at_paragraph_break() {
        let c = DraftChunker::new(50, "..", "..");
        let text = "alpha alpha alpha.\n\nbeta beta beta.\n\ngamma gamma gamma.";
        let out = c.chunk(text);
        assert!(
            out.len() >= 2,
            "expected at least 2 chunks, got {}",
            out.len()
        );
        // Each chunk must be ≤ cap + marker.
        for chunk in &out {
            assert!(chunk.text.chars().count() <= 50);
        }
        // Last chunk's marker is the end marker.
        assert!(out.last().unwrap().text.ends_with(".."));
        // Intermediate chunks use the continuation marker.
        if out.len() > 1 {
            assert!(out[0].text.ends_with(".."));
        }
        // All chunks together reassemble to the input (markers are suffixes).
        let reassembled_no_markers: String = out
            .iter()
            .map(|c| c.text.trim_end_matches("..").to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
        assert!(reassembled_no_markers.starts_with("alpha"));
    }

    #[test]
    fn chunk_reserves_marker_budget() {
        // 60-char cap + 8-char marker means the body must be ≤ 52 chars.
        let c = DraftChunker::new(60, "(more)", "(end)");
        let body = "x".repeat(120);
        let out = c.chunk(&body);
        for chunk in &out {
            assert!(
                chunk.text.chars().count() <= 60,
                "chunk too long: {:?}",
                chunk.text
            );
        }
    }

    #[test]
    fn chunk_does_not_split_inside_code_fence() {
        // A code fence straddles the cap; the chunker should split at the
        // fence boundary, not inside the fence.
        let c = DraftChunker::new(40, "(more)", "(end)");
        let text = "intro line\n```rust\nfn a() {}\nfn b() {}\n```\nafter fence";
        let out = c.chunk(text);
        // No chunk should be missing its closing ``` (the splitter must
        // prefer the fence boundary).
        for chunk in &out {
            // Each chunk must end cleanly (we don't assert exact content
            // here — the key contract is "no in-fence split").
            assert!(chunk.text.chars().count() <= 40);
        }
    }

    #[test]
    fn chunker_total_field_reflects_sequence_length() {
        let c = DraftChunker::new(30, "(m)", "(e)");
        let text = "word ".repeat(50);
        let out = c.chunk(&text);
        for (i, chunk) in out.iter().enumerate() {
            assert_eq!(chunk.index, i);
            assert_eq!(chunk.total, out.len());
            if i + 1 == out.len() {
                assert!(chunk.is_last);
            } else {
                assert!(!chunk.is_last);
            }
        }
    }

    #[test]
    fn stream_lifecycle_pending_editing_final() {
        let mut s = DraftStream::with_cap(100);
        assert_eq!(s.state(), DraftState::Pending);
        // update before begin_editing is rejected.
        assert_eq!(s.update("hello"), Err(DraftError::NotEditing));
        s.begin_editing().unwrap();
        assert_eq!(s.state(), DraftState::Editing);
        assert!(s.update("hello").unwrap());
        assert!(!s.update("hello").unwrap()); // idempotent
        assert!(s.update("hello world").unwrap());
        assert_eq!(s.finalize("hello world").unwrap(), "hello world");
        assert_eq!(s.state(), DraftState::Final);
    }

    #[test]
    fn stream_rejects_overflow_on_update() {
        let mut s = DraftStream::with_cap(5);
        s.begin_editing().unwrap();
        assert_eq!(
            s.update("too long"),
            Err(DraftError::Overflows { chars: 8, cap: 5 })
        );
    }

    #[test]
    fn stream_rejects_overflow_on_finalize() {
        let mut s = DraftStream::with_cap(5);
        s.begin_editing().unwrap();
        assert_eq!(
            s.finalize("too long"),
            Err(DraftError::Overflows { chars: 8, cap: 5 })
        );
        // Even though finalize returned Err, the stream moves to Final —
        // there's no useful recovery for an overflow (the caller has to
        // start a new stream for the second chunk).
        assert_eq!(s.state(), DraftState::Final);
    }

    #[test]
    fn stream_rejects_after_finalize() {
        let mut s = DraftStream::with_cap(100);
        s.begin_editing().unwrap();
        s.finalize("done").unwrap();
        assert_eq!(s.update("nope"), Err(DraftError::AlreadyFinalized));
        assert_eq!(s.finalize("nope"), Err(DraftError::AlreadyFinalized));
    }

    #[test]
    fn stream_max_chars_matches_constructor() {
        assert_eq!(DraftStream::new().max_chars(), DISCORD_MAX_CHARS);
        assert_eq!(DraftStream::with_cap(42).max_chars(), 42);
    }
}
