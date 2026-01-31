//! Output formatting utilities for CLI commands.

use serde::Serialize;
use std::io;

/// Output format for CLI commands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// Machine-readable JSON format
    Json,
}

impl OutputFormat {
    /// Determine format from --json flag
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            OutputFormat::Json
        } else {
            OutputFormat::Table
        }
    }
}

/// Print data as JSON
pub fn print_json<T: Serialize>(data: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    println!("{}", json);
    Ok(())
}

/// Print a simple key-value table
pub fn print_table(rows: &[(&str, &str)]) {
    let max_key_len = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);

    for (key, value) in rows {
        println!("{:width$}  {}", key, value, width = max_key_len);
    }
}

/// Print a list table with headers
pub fn print_list_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(empty)");
        return;
    }

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header
    for (i, header) in headers.iter().enumerate() {
        if i > 0 {
            print!("  ");
        }
        print!("{:width$}", header.to_uppercase(), width = widths[i]);
    }
    println!();

    // Print separator
    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            print!("  ");
        }
        print!("{}", "-".repeat(*width));
    }
    println!();

    // Print rows
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                print!("  ");
            }
            if i < widths.len() {
                print!("{:width$}", cell, width = widths[i]);
            }
        }
        println!();
    }
}

/// Print success message
pub fn print_success(message: &str) {
    println!("✓ {}", message);
}

/// Print error message
pub fn print_error(message: &str) {
    eprintln!("✗ {}", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_from_flag() {
        assert_eq!(OutputFormat::from_json_flag(true), OutputFormat::Json);
        assert_eq!(OutputFormat::from_json_flag(false), OutputFormat::Table);
    }
}
