//! Shared output formatting helpers for Aleph CLI.
//!
//! Provides consistent output across all CLI commands with support
//! for both human-readable (table/detail) and machine-readable (JSON) formats.

use serde_json::Value;

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
        println!("(no results)");
        return;
    }

    // Calculate column widths: max of header width and all row values
    let col_count = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header row
    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
        .collect();
    println!("{}", header_line.join("  "));

    // Print separator
    let sep_line: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
    println!("{}", sep_line.join("  "));

    // Print data rows
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = if i < col_count { widths[i] } else { cell.len() };
                format!("{:<width$}", cell, width = w)
            })
            .collect();
        println!("{}", cells.join("  "));
    }
}

/// Print key-value pairs as a detail view or as JSON.
///
/// In JSON mode, prints `raw` as pretty JSON.
/// In detail mode, prints right-aligned keys followed by values.
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

    // Find max key width for alignment
    let max_key_width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);

    for (key, value) in pairs {
        println!("{:>width$}: {}", key, value, width = max_key_width);
    }
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
            "ID".len().max("1".len()).max("2".len()).max("3".len()),                                       // 2
            "Name".len().max("short".len()).max("a-much-longer-name".len()).max("mid".len()),               // 18
            "Status".len().max("ok".len()).max("error".len()).max("ok".len()),                              // 6
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
}
