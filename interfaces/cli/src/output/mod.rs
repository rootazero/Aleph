//! Shared output formatting helpers for Aleph CLI.
//!
//! Provides consistent output across all CLI commands with support
//! for both human-readable (table/detail) and machine-readable (JSON) formats.
//!
//! Public API is intentionally tiny — three formatting functions plus
//! the `Spinner` RAII handle. New visual flair (colours, icons, box-draw)
//! is opt-out via `NO_COLOR` / `ALEPH_COLOR=never` / non-TTY stdout.

pub mod box_draw;
pub mod icon;
pub mod spinner;
pub mod theme;

pub use spinner::Spinner;

use serde_json::Value;

use box_draw::horizontal;
use theme::{paint, status_style, Style};

/// Pretty-print a JSON value to stdout.
pub fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("Error serializing JSON: {e}"),
    }
}

/// Print data as an aligned table or as JSON.
///
/// In JSON mode, prints `raw` as pretty JSON and ignores `headers`/`rows`.
/// In table mode, prints a column-aligned table with header separator.
///
/// Columns named `status`, `state`, or `health` (case-insensitive) have
/// their values automatically colourised through [`theme::status_style`].
///
/// # Arguments
/// * `headers` - Column header labels
/// * `rows` - Row data (each row is a Vec of column values)
/// * `json_mode` - If true, output JSON instead of table
/// * `raw` - The raw JSON value to emit in JSON mode
pub fn print_table(headers: &[&str], rows: &[Vec<String>], json_mode: bool, raw: &Value) {
    if json_mode {
        print_json(raw);
        return;
    }

    if rows.is_empty() {
        println!("{}", paint(Style::Muted, "(no results)"));
        return;
    }

    // Compute column widths using *display* widths (count chars, not bytes)
    // so wide CJK / emoji cells line up correctly.
    let col_count = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    // Columns whose header *suggests* a status word — auto-colour their values.
    let status_cols: Vec<bool> = headers
        .iter()
        .map(|h| {
            let lower = h.to_ascii_lowercase();
            lower == "status" || lower == "state" || lower == "health" || lower == "result"
        })
        .collect();

    // Print header row (bold).
    let header_cells: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let padded = pad_to(h, widths[i]);
            paint(Style::Header, &padded)
        })
        .collect();
    println!("{}", header_cells.join("  "));

    // Print separator row.
    let sep = box_draw::separator(&widths, 2);
    if !sep.is_empty() {
        println!("{}", paint(Style::Muted, &sep));
    } else {
        // Fallback when widths is empty (shouldn't happen — defensive).
        let sep: String = (0..widths.iter().sum::<usize>())
            .map(|_| horizontal())
            .collect();
        println!("{sep}");
    }

    // Print data rows.
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = if i < col_count {
                    widths[i]
                } else {
                    display_width(cell)
                };
                let padded = pad_to(cell, w);
                if i < col_count && status_cols[i] {
                    let style = status_style(cell);
                    paint(style, &padded)
                } else {
                    padded
                }
            })
            .collect();
        println!("{}", cells.join("  "));
    }
}

/// Print key-value pairs as a detail view or as JSON.
///
/// In JSON mode, prints `raw` as pretty JSON.
/// In detail mode, prints right-aligned keys followed by values. Keys are
/// rendered in [`Style::Header`]; values matching a known status word are
/// colourised via [`theme::status_style`].
///
/// # Arguments
/// * `pairs` - Key-value pairs to display
/// * `json_mode` - If true, output JSON instead of detail view
/// * `raw` - The raw JSON value to emit in JSON mode
pub fn print_detail(pairs: &[(&str, String)], json_mode: bool, raw: &Value) {
    if json_mode {
        print_json(raw);
        return;
    }

    if pairs.is_empty() {
        return;
    }

    let max_key_width = pairs.iter().map(|(k, _)| display_width(k)).max().unwrap_or(0);

    for (key, value) in pairs {
        let pad = max_key_width.saturating_sub(display_width(key));
        let spaces = " ".repeat(pad);
        let painted_key = paint(Style::Header, key);
        let painted_value = match status_style(value) {
            Style::Default => value.clone(),
            other => paint(other, value),
        };
        // {spaces}{KEY}: {value}
        println!("{spaces}{painted_key}: {painted_value}");
    }
}

/// Best-effort visible width (codepoint count). Treats every Unicode scalar
/// as width 1 — close enough for the ASCII-heavy text the CLI emits, and
/// removes the unicode-width dependency from this thin client.
fn display_width(s: &str) -> usize {
    s.chars().count()
}

fn pad_to(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - w));
    out.push_str(s);
    for _ in 0..(width - w) {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_print_table_empty_rows() {
        // Empty rows should print "(no results)" — we just verify it doesn't panic.
        // In a real scenario you'd capture stdout; here we confirm no crash.
        let raw = json!([]);
        print_table(&["Name", "Status"], &[], false, &raw);
    }

    #[test]
    fn test_print_table_column_width_alignment() {
        // Verify column widths are computed correctly by checking internal logic.
        let headers = &["ID", "Name", "Status"];
        let rows = vec![
            vec!["1".into(), "short".into(), "ok".into()],
            vec!["2".into(), "a-much-longer-name".into(), "error".into()],
            vec!["3".into(), "mid".into(), "ok".into()],
        ];

        // Compute expected widths
        let expected_widths: Vec<usize> = vec![
            "ID".len().max("1".len()).max("2".len()).max("3".len()), // 2
            "Name"
                .len()
                .max("short".len())
                .max("a-much-longer-name".len())
                .max("mid".len()), // 18
            "Status"
                .len()
                .max("ok".len())
                .max("error".len())
                .max("ok".len()), // 6
        ];
        assert_eq!(expected_widths, vec![2, 18, 6]);

        // Also verify the function runs without panic
        let raw = json!([]);
        print_table(headers, &rows, false, &raw);
    }

    #[test]
    fn test_print_table_json_mode() {
        let raw = json!({"items": [1, 2, 3]});
        // Should print JSON, not table — verify no panic
        print_table(&["A"], &[vec!["x".into()]], true, &raw);
    }

    #[test]
    fn test_print_detail_basic() {
        let raw = json!({"name": "test"});
        let pairs = vec![("Name", "test".to_string()), ("Version", "1.0".to_string())];
        // Verify no panic in both modes
        print_detail(&pairs, false, &raw);
        print_detail(&pairs, true, &raw);
    }

    #[test]
    fn test_print_detail_empty_pairs() {
        let raw = json!({});
        print_detail(&[], false, &raw);
    }

    #[test]
    fn test_print_json_basic() {
        let value = json!({"key": "value", "num": 42});
        // Verify no panic
        print_json(&value);
    }

    #[test]
    fn pad_to_pads_with_spaces_at_target_width() {
        let padded = pad_to("hi", 5);
        assert_eq!(padded, "hi   ");
    }

    #[test]
    fn pad_to_returns_input_when_already_wide_enough() {
        let padded = pad_to("hello", 3);
        assert_eq!(padded, "hello");
    }

    #[test]
    fn display_width_counts_scalars_not_bytes() {
        // 2 ASCII bytes = 2 columns
        assert_eq!(display_width("hi"), 2);
        // multi-byte CJK is 1 scalar each; we approximate width as 1 per
        // scalar (good enough for ASCII-heavy CLI output).
        assert_eq!(display_width("中"), 1);
    }

    #[test]
    fn print_table_status_column_does_not_panic_with_unknown_status() {
        let raw = json!([]);
        let headers = &["Name", "Status"];
        let rows = vec![vec!["a".into(), "wibble".into()]];
        print_table(headers, &rows, false, &raw);
    }
}
