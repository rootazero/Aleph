//! 工具卡片富渲染 —— 把一次工具调用（args/result）按工具类型渲染成
//! diff / shell / 全文 / patch 等富视图。左侧聊天与右侧工作区面板共用。
//!
//! 纯逻辑（ToolKind 分流、diff、截断、汇总）与视图组件分离：逻辑可在
//! 宿主机 `cargo test -p aleph-panel --lib` 下测试。

/// 工具大类 —— 决定卡片体如何渲染。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    FileEdit,
    FileWrite,
    ApplyPatch,
    FileRead,
    Bash,
    Search,
    Default,
}

impl ToolKind {
    /// 由工具名（大小写不敏感）映射到大类。未知名 → `Default`。
    pub fn from_name(name: &str) -> ToolKind {
        let n = name.to_lowercase();
        match n.as_str() {
            "file_edit" => ToolKind::FileEdit,
            "file_write" => ToolKind::FileWrite,
            "apply_patch" => ToolKind::ApplyPatch,
            "file_read" => ToolKind::FileRead,
            _ => {
                if n.starts_with("bash")
                    || n.starts_with("shell")
                    || n.starts_with("code_exec")
                    || n.contains("_exec")
                {
                    ToolKind::Bash
                } else if n == "search"
                    || n == "web_search"
                    || n == "grep"
                    || n == "find"
                    || n.starts_with("search")
                    || n.ends_with("_search")
                {
                    ToolKind::Search
                } else {
                    ToolKind::Default
                }
            }
        }
    }

    /// 卡片默认是否展开内容：文件改动类默认展开，其余默认折叠。
    pub fn default_open(self) -> bool {
        matches!(
            self,
            ToolKind::FileEdit | ToolKind::FileWrite | ToolKind::ApplyPatch
        )
    }
}

use serde_json::Value;
use similar::{ChangeTag, TextDiff};

/// 一行 diff：`sign` 为 `'+'`(新增)/`'-'`(删除)/`' '`(上下文)。
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub sign: char,
    pub text: String,
}

/// 从 `{"Success":{"output":..}}` 取出 output。
pub fn success_output(result: &Value) -> Option<&Value> {
    result.get("Success").and_then(|s| s.get("output"))
}

/// 从 `{"Error":{"error":..}}` 取出错误文案。
pub fn error_message(result: &Value) -> Option<String> {
    result
        .get("Error")
        .and_then(|e| e.get("error"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 行级 diff（带相等的上下文行），返回 (行, 新增数, 删除数)。
pub fn diff_lines(old: &str, new: &str) -> (Vec<DiffLine>, usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    let (mut added, mut removed) = (0usize, 0usize);
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => {
                removed += 1;
                '-'
            }
            ChangeTag::Insert => {
                added += 1;
                '+'
            }
            ChangeTag::Equal => ' ',
        };
        let text = change.value().trim_end_matches('\n').to_string();
        lines.push(DiffLine { sign, text });
    }
    (lines, added, removed)
}

/// 取前 `max_lines` 行；返回 (展示文本, 被隐藏行数)。隐藏数为 0 表示未截断。
pub fn split_preview(text: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return (text.to_string(), 0);
    }
    let shown = lines[..max_lines].join("\n");
    (shown, lines.len() - max_lines)
}

/// 按工具大类汇总计数，用于「无叙述」时合成占位标题。
/// 顺序固定（首次出现的大类先出），便于稳定渲染与测试。
pub fn summarize_tools(tools: &[(String, String)]) -> Vec<(ToolKind, usize)> {
    let mut order: Vec<ToolKind> = Vec::new();
    let mut counts: std::collections::HashMap<ToolKind, usize> = std::collections::HashMap::new();
    for (_id, name) in tools {
        let kind = ToolKind::from_name(name);
        if !counts.contains_key(&kind) {
            order.push(kind);
        }
        *counts.entry(kind).or_insert(0) += 1;
    }
    order.into_iter().map(|k| (k, counts[&k])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_output_and_error_extract() {
        let ok = serde_json::json!({"Success": {"output": {"stdout": "hi"}}});
        assert_eq!(
            success_output(&ok).and_then(|o| o.get("stdout")).and_then(|v| v.as_str()),
            Some("hi")
        );
        assert_eq!(error_message(&ok), None);

        let err = serde_json::json!({"Error": {"error": "boom", "retryable": false}});
        assert_eq!(success_output(&err), None);
        assert_eq!(error_message(&err).as_deref(), Some("boom"));
    }

    #[test]
    fn diff_lines_counts_add_remove_equal() {
        let (lines, added, removed) = diff_lines("let x = 1;\nlet y = 2;\n", "let x = 2;\nlet y = 2;\n");
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
        // 至少包含一条 '-'、一条 '+'、一条 ' '(相等的 y 行)
        assert!(lines.iter().any(|l| l.sign == '-'));
        assert!(lines.iter().any(|l| l.sign == '+'));
        assert!(lines.iter().any(|l| l.sign == ' '));
    }

    #[test]
    fn diff_lines_identical_is_zero() {
        let (_lines, added, removed) = diff_lines("same\n", "same\n");
        assert_eq!((added, removed), (0, 0));
    }

    #[test]
    fn split_preview_truncates_beyond_max() {
        let text = "a\nb\nc\nd\ne";
        let (shown, hidden) = split_preview(text, 3);
        assert_eq!(shown, "a\nb\nc");
        assert_eq!(hidden, 2);

        let (shown2, hidden2) = split_preview("a\nb", 5);
        assert_eq!(shown2, "a\nb");
        assert_eq!(hidden2, 0);
    }

    #[test]
    fn summarize_tools_counts_by_kind_in_order() {
        let tools = vec![
            ("t1".to_string(), "file_read".to_string()),
            ("t2".to_string(), "bash".to_string()),
            ("t3".to_string(), "file_read".to_string()),
            ("t4".to_string(), "search".to_string()),
        ];
        let got = summarize_tools(&tools);
        assert_eq!(
            got,
            vec![(ToolKind::FileRead, 2), (ToolKind::Bash, 1), (ToolKind::Search, 1)]
        );
    }

    #[test]
    fn summarize_tools_empty_is_empty() {
        assert!(summarize_tools(&[]).is_empty());
    }

    #[test]
    fn from_name_maps_known_and_unknown() {
        assert_eq!(ToolKind::from_name("file_edit"), ToolKind::FileEdit);
        assert_eq!(ToolKind::from_name("FILE_WRITE"), ToolKind::FileWrite);
        assert_eq!(ToolKind::from_name("apply_patch"), ToolKind::ApplyPatch);
        assert_eq!(ToolKind::from_name("file_read"), ToolKind::FileRead);
        assert_eq!(ToolKind::from_name("bash"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("code_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("python_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("web_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("hybrid_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("memory_recall"), ToolKind::Default);
    }

    #[test]
    fn default_open_only_for_file_mutations() {
        assert!(ToolKind::FileEdit.default_open());
        assert!(ToolKind::FileWrite.default_open());
        assert!(ToolKind::ApplyPatch.default_open());
        assert!(!ToolKind::Bash.default_open());
        assert!(!ToolKind::Search.default_open());
        assert!(!ToolKind::FileRead.default_open());
        assert!(!ToolKind::Default.default_open());
    }
}
