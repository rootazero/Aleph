//! L1 Regex pattern matching for intent classification (<5ms).

use once_cell::sync::Lazy;
use regex::Regex;

use super::keywords::EXCLUSION_VERBS;
use super::types::ExecutableTask;
use crate::intent::types::TaskCategory;

/// Regex patterns for L1 classification (Chinese + English)
pub static EXECUTABLE_PATTERNS: Lazy<Vec<(Regex, TaskCategory)>> = Lazy::new(|| {
    vec![
        // FileOrganize: organize/sort/classify + file
        (
            Regex::new(r"(?i)(整理|归类|分类|organize|sort|classify).*(文件|files?|folder|文件夹)")
                .unwrap(),
            TaskCategory::FileOrganize,
        ),
        // FileTransfer: move/copy/transfer + to
        (
            Regex::new(r"(?i)(移动|复制|拷贝|转移|move|copy|transfer).*(到|to)").unwrap(),
            TaskCategory::FileTransfer,
        ),
        // FileCleanup: delete/remove/clean
        (
            Regex::new(r"(?i)(删除|清理|清空|清除|delete|remove|clean)").unwrap(),
            TaskCategory::FileCleanup,
        ),
        // CodeExecution: run/execute + script/code
        (
            Regex::new(r"(?i)(运行|执行|跑一下|run|execute).*(脚本|代码|script|code)").unwrap(),
            TaskCategory::CodeExecution,
        ),
        // DocumentGenerate: generate/create/export + document/report
        (
            Regex::new(
                r"(?i)(生成|创建|导出|写|generate|create|export).*(文档|报告|document|report)",
            )
            .unwrap(),
            TaskCategory::DocumentGenerate,
        ),
    ]
});

/// Path extraction pattern
/// Matches Unix paths (/path or ~/path) and Windows paths (C:\path)
/// Stops at whitespace, quotes, or CJK characters (U+4E00-U+9FFF)
pub static PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"['"]?([/~][A-Za-z0-9_./-]+|[A-Za-z]:\\[A-Za-z0-9_.\\/]+)['"]?"#).unwrap()
});

/// L1: Regex pattern matching (<5ms)
pub fn match_regex(input: &str) -> Option<ExecutableTask> {
    let input_lower = input.to_lowercase();

    // Check exclusion patterns first - analysis/understanding verbs override regex matches
    if contains_exclusion_verb(&input_lower) {
        return None;
    }

    for (pattern, category) in EXECUTABLE_PATTERNS.iter() {
        if pattern.is_match(input) {
            let target = extract_path(input);
            return Some(ExecutableTask {
                category: *category,
                action: input.to_string(),
                target,
                confidence: 1.0, // Regex match = high confidence
            });
        }
    }
    None
}

/// Extract file path from input
pub fn extract_path(input: &str) -> Option<String> {
    PATH_PATTERN.captures(input).map(|c| c[1].to_string())
}

/// Check if input contains exclusion verbs (analysis/understanding actions)
pub fn contains_exclusion_verb(input: &str) -> bool {
    EXCLUSION_VERBS.iter().any(|v| input.contains(v))
}
