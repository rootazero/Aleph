//! Keyword definitions for L2 classification.

use crate::intent::types::TaskCategory;

/// Keyword sets for L2 classification
pub struct KeywordSet {
    pub verbs: &'static [&'static str],
    pub nouns: &'static [&'static str],
    pub category: TaskCategory,
}

/// Exclusion patterns - inputs containing these should NOT trigger agent mode
/// These are non-executable verbs that indicate analysis/understanding rather than action
pub static EXCLUSION_VERBS: &[&str] = &[
    // Chinese analysis/understanding verbs
    "分析",
    "理解",
    "解释",
    "描述",
    "识别",
    "检测",
    "看看",
    "看一下",
    "看下",
    "告诉我",
    "说说",
    "讲讲",
    "什么是",
    "是什么",
    "怎么样",
    // Chinese summarization verbs
    "总结",
    "摘要",
    "概括",
    "归纳",
    "提炼",
    "概述",
    "梳理",
    "提取要点",
    // English analysis verbs
    "analyze",
    "analyse",
    "understand",
    "explain",
    "describe",
    "identify",
    "detect",
    "recognize",
    "what is",
    "tell me",
    "look at",
    // English summarization verbs
    "summarize",
    "summarise",
    "summary",
    "abstract",
    "recap",
    "outline",
    "extract",
    "highlight",
    "key points",
];

/// Static keyword sets for L2 matching
pub static KEYWORD_SETS: &[KeywordSet] = &[
    KeywordSet {
        // Removed "分" - too short, causes false matches (e.g., "分析" contains "分")
        verbs: &["整理", "归类", "分类", "organize", "sort", "classify"],
        nouns: &[
            "文件",
            "文件夹",
            "目录",
            "下载",
            "照片",
            "图片",
            "files",
            "folder",
            "directory",
            "downloads",
            "photos",
            "pictures",
        ],
        category: TaskCategory::FileOrganize,
    },
    KeywordSet {
        verbs: &["移动", "复制", "拷贝", "转移", "move", "copy", "transfer"],
        nouns: &["文件", "文件夹", "到", "files", "folder", "to"],
        category: TaskCategory::FileTransfer,
    },
    KeywordSet {
        verbs: &[
            "删除", "清理", "清空", "移除", "delete", "remove", "clean", "clear",
        ],
        nouns: &["文件", "缓存", "垃圾", "files", "cache", "trash"],
        category: TaskCategory::FileCleanup,
    },
    KeywordSet {
        verbs: &["运行", "执行", "跑", "run", "execute"],
        nouns: &["脚本", "代码", "程序", "script", "code", "program"],
        category: TaskCategory::CodeExecution,
    },
    KeywordSet {
        verbs: &[
            "生成", "创建", "导出", "写", "generate", "create", "export", "write",
        ],
        nouns: &["文档", "报告", "document", "report", "pdf"],
        category: TaskCategory::DocumentGenerate,
    },
];
