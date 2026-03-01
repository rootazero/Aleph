//! Tool display formatting with emoji and smart parameter summarization

use serde_json::Value;
use std::collections::HashMap;

/// Tool display metadata
#[derive(Debug, Clone)]
pub struct ToolDisplay {
    pub emoji: &'static str,
    pub label: String,
}

/// Get display metadata for a tool
pub fn get_tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "exec" | "shell" | "bash" | "run_command" => ToolDisplay { emoji: "🔨", label: "Exec".to_string() },
        "read" | "read_file" | "cat" => ToolDisplay { emoji: "📄", label: "Read".to_string() },
        "write" | "write_file" => ToolDisplay { emoji: "✏️", label: "Write".to_string() },
        "edit" | "edit_file" | "patch" => ToolDisplay { emoji: "📝", label: "Edit".to_string() },
        "web_fetch" | "fetch" | "http" => ToolDisplay { emoji: "🌐", label: "Fetch".to_string() },
        "search" | "grep" | "find" | "ripgrep" => ToolDisplay { emoji: "🔍", label: "Search".to_string() },
        "list" | "ls" | "dir" => ToolDisplay { emoji: "📁", label: "List".to_string() },
        "think" | "reason" => ToolDisplay { emoji: "💭", label: "Think".to_string() },
        "memory" | "remember" => ToolDisplay { emoji: "🧠", label: "Memory".to_string() },
        _ => ToolDisplay { emoji: "⚙️", label: tool_name.to_string() },
    }
}

/// Format tool parameters for display
pub fn format_tool_meta(tool_name: &str, params: &Value) -> String {
    match tool_name {
        "read" | "read_file" | "cat" => format_path_params(params, "path"),
        "write" | "write_file" => format_path_params(params, "path"),
        "edit" | "edit_file" | "patch" => format_edit_params(params),
        "exec" | "shell" | "bash" | "run_command" => format_exec_params(params),
        "web_fetch" | "fetch" | "http" => format_url_params(params),
        "search" | "grep" | "find" | "ripgrep" => format_search_params(params),
        _ => format_generic_params(params),
    }
}

/// Format complete tool summary: "🔨 Exec: mkdir -p /tmp"
pub fn format_tool_summary(tool_name: &str, params: &Value) -> String {
    let display = get_tool_display(tool_name);
    let meta = format_tool_meta(tool_name, params);

    if meta.is_empty() {
        format!("{} {}", display.emoji, display.label)
    } else {
        format!("{} {}: {}", display.emoji, display.label, meta)
    }
}

// --- Helper functions ---

fn format_path_params(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(shorten_path)
        .unwrap_or_default()
}

fn format_edit_params(params: &Value) -> String {
    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let line = params.get("line").and_then(|v| v.as_u64());
    let end_line = params.get("end_line").and_then(|v| v.as_u64());

    let short_path = shorten_path(path);
    match (line, end_line) {
        (Some(l), Some(e)) if l != e => format!("{}:{}-{}", short_path, l, e),
        (Some(l), _) => format!("{}:{}", short_path, l),
        _ => short_path,
    }
}

fn format_exec_params(params: &Value) -> String {
    let mut parts = Vec::new();

    if params.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("sudo".to_string());
    }
    if params.get("pty").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("pty".to_string());
    }

    if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
        parts.push(truncate_str(cmd, 50));
    }

    parts.join(" · ")
}

fn format_url_params(params: &Value) -> String {
    params
        .get("url")
        .and_then(|v| v.as_str())
        .map(|url| truncate_str(url, 60))
        .unwrap_or_default()
}

fn format_search_params(params: &Value) -> String {
    let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    if pattern.is_empty() {
        shorten_path(path)
    } else {
        format!("\"{}\" in {}", truncate_str(pattern, 20), shorten_path(path))
    }
}

fn format_generic_params(params: &Value) -> String {
    if let Some(obj) = params.as_object() {
        for (_, value) in obj.iter().take(1) {
            if let Some(s) = value.as_str() {
                return truncate_str(s, 40);
            }
        }
    }
    String::new()
}

fn shorten_path(path: &str) -> String {
    if path.len() <= 40 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        return truncate_str(path, 40);
    }

    let last_two = &parts[parts.len() - 2..];
    format!(".../{}", last_two.join("/"))
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = max_len.saturating_sub(3);
        let mut boundary = end;
        while boundary > 0 && !s.is_char_boundary(boundary) {
            boundary -= 1;
        }
        format!("{}...", &s[..boundary])
    }
}

/// Group multiple paths by directory: /tmp/{file1.txt, file2.txt}
pub fn group_paths(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    if paths.len() == 1 {
        return shorten_path(paths[0]);
    }

    let mut groups: HashMap<&str, Vec<&str>> = HashMap::new();
    for path in paths {
        if let Some(idx) = path.rfind('/') {
            let (dir, file) = path.split_at(idx + 1);
            groups.entry(dir).or_default().push(file);
        } else {
            groups.entry(".").or_default().push(path);
        }
    }

    groups
        .iter()
        .map(|(dir, files)| {
            if files.len() == 1 {
                format!("{}{}", dir, files[0])
            } else {
                format!("{}{{{}}}", dir, files.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_tool_display() {
        let display = get_tool_display("exec");
        assert_eq!(display.emoji, "🔨");
        assert_eq!(display.label, "Exec");

        let display = get_tool_display("read_file");
        assert_eq!(display.emoji, "📄");
    }

    #[test]
    fn test_format_exec_params() {
        let params = json!({"command": "mkdir -p /tmp/test", "elevated": true});
        let result = format_exec_params(&params);
        assert!(result.contains("sudo"));
        assert!(result.contains("mkdir"));
    }

    #[test]
    fn test_format_edit_params() {
        let params = json!({"path": "src/main.rs", "line": 42, "end_line": 56});
        let result = format_edit_params(&params);
        assert_eq!(result, "src/main.rs:42-56");
    }

    #[test]
    fn test_shorten_path() {
        assert_eq!(shorten_path("short.txt"), "short.txt");
        assert!(shorten_path("/very/long/path/to/some/deeply/nested/file.txt").contains("..."));
    }

    #[test]
    fn test_group_paths() {
        let paths = vec!["/tmp/file1.txt", "/tmp/file2.txt", "/home/test.rs"];
        let result = group_paths(&paths);
        assert!(result.contains("{file1.txt, file2.txt}") || result.contains("{file2.txt, file1.txt}"));
    }

    #[test]
    fn test_format_tool_summary() {
        let params = json!({"path": "src/lib.rs"});
        let summary = format_tool_summary("read", &params);
        assert_eq!(summary, "📄 Read: src/lib.rs");
    }
}
