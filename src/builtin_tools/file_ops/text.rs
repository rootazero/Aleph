//! Shared text-handling primitives for the `file_ops` tools.
//!
//! Centralizes binary-file detection and the line-rendering limits used by
//! `file_read`, so the (subtle) heuristics live in one place instead of being
//! duplicated or — worse — omitted. The Unicode-folding logic used for
//! whitespace-tolerant `file_edit` matching lives in [`super::edit_match`].

/// Maximum characters rendered for a single line before it is truncated.
///
/// Guards the model context against minified single-line files (e.g. bundled
/// JavaScript) that would otherwise return one multi-megabyte "line".
pub(super) const MAX_LINE_CHARS: usize = 2000;

/// Default number of lines `file_read` returns when no explicit `limit` is given.
///
/// Mirrors the windowed-read convention used by mainstream agent toolchains:
/// the model pages through large files via `offset` rather than receiving the
/// whole file (and flooding its context) in one call.
pub(super) const DEFAULT_READ_LINE_LIMIT: u64 = 2000;

/// Token ceiling for one `file_read` window — the second limit, applied across
/// lines, whichever binds first (pi's `truncate.ts` uses the same rule with a
/// line count and a byte count).
///
/// `file_read` deliberately has no result budget of its own — a read is the only
/// way the model can pull a persisted blob back, so persisting one would loop —
/// which means an oversized window is cut by the *generic* head+tail truncator,
/// silently removing the middle of content whose `message` / `returned_lines` /
/// `truncated` fields were already minted for the whole window. Staying under the
/// backstop is what keeps the window and its continuation offset truthful.
///
/// This constant is only the upper bound; [`read_window_tokens`] clamps it to the
/// backstop actually in force.
const READ_WINDOW_TOKENS_MAX: usize = 6000;

/// Fraction (in twentieths) of the backstop a window may occupy, leaving room for
/// JSON escaping (`\n` -> `\\n`, `\"` -> `\\\"`) and the sibling envelope fields
/// that ride alongside `content` in the flattened result.
const READ_WINDOW_HEADROOM_X20: usize = 17;

/// Token budget for one `file_read` window, clamped to the backstop in force.
///
/// The clamp matters: on a small-window model the boot-installed ceiling drops
/// the backstop to as little as 2 000 tokens, and a fixed 6 000-token window would
/// then overrun it on *every read*.
pub(super) fn read_window_tokens() -> usize {
    let backstop = crate::tools::result_processing::read_backstop_tokens();
    READ_WINDOW_TOKENS_MAX.min(backstop * READ_WINDOW_HEADROOM_X20 / 20)
}

/// Number of leading bytes inspected when classifying a file as binary.
const BINARY_SNIFF_BYTES: usize = 8192;

/// Heuristically decide whether `bytes` look like binary (non-text) data.
///
/// Uses the same rule as `git`: a NUL byte within the leading sniff window
/// marks the file as binary. Cheap, allocation-free, and reliable for the
/// source/text files an agent actually reads and edits — it lets the tools
/// degrade gracefully ("binary file, not displayable") instead of erroring
/// out of `read_to_string` with an opaque message.
pub(super) fn is_binary(bytes: &[u8]) -> bool {
    let window = bytes
        .get(..bytes.len().min(BINARY_SNIFF_BYTES))
        .expect("invariant: window end is clamped to bytes length");
    window.contains(&0)
}

/// Clamp a single line to [`MAX_LINE_CHARS`] characters (char-boundary safe),
/// appending a truncation marker when it overflows.
///
/// Shared by `file_read`'s window renderer and `file_edit`'s result snippet so
/// the (identical) guard against minified single-line files — and the marker
/// text — live in exactly one place.
pub(super) fn clamp_line(line: &str) -> String {
    let char_count = line.chars().count();
    if char_count <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{head}… [line truncated — {char_count} chars total]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_not_binary() {
        assert!(!is_binary(b"hello world\nsecond line\n"));
        assert!(!is_binary("UTF-8 文本 — with em-dash".as_bytes()));
        assert!(!is_binary(b""));
    }

    #[test]
    fn nul_byte_marks_binary() {
        assert!(is_binary(b"PK\x03\x04\x00\x00binary"));
        assert!(is_binary(&[0u8]));
    }

    #[test]
    fn nul_byte_past_sniff_window_is_ignored() {
        // A NUL only beyond the sniff window does not flip the verdict —
        // matches git's "peek the head" behavior.
        let mut data = vec![b'a'; BINARY_SNIFF_BYTES + 16];
        data[BINARY_SNIFF_BYTES + 8] = 0;
        assert!(!is_binary(&data));
    }
}
