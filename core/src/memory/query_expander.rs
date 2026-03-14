//! Chinese-aware query expansion for BM25 recall improvement.
//!
//! When a query contains CJK characters, known synonyms are appended
//! to the BM25 query string so that full-text search can match
//! semantically equivalent terms that vector search would catch but
//! keyword search would miss.

use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Result of query expansion.
pub struct ExpandedQuery {
    /// The original query text, used for vector search.
    pub original: String,
    /// The query text for FTS — may have synonyms appended.
    pub bm25_query: String,
}

/// Built-in Chinese synonym table.
static SYNONYMS: Lazy<HashMap<&'static str, &'static [&'static str]>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("喜欢", ["偏好", "倾向", "爱好"].as_slice());
    m.insert("偏好", ["喜欢", "倾向"].as_slice());
    m.insert("设置", ["配置", "调整", "修改"].as_slice());
    m.insert("配置", ["设置", "调整"].as_slice());
    m.insert("问题", ["bug", "错误", "故障", "缺陷"].as_slice());
    m.insert("错误", ["问题", "bug", "故障"].as_slice());
    m.insert("使用", ["用", "利用", "采用"].as_slice());
    m.insert("代码", ["程序", "源码"].as_slice());
    m.insert("记忆", ["记得", "记住", "回忆"].as_slice());
    m.insert("之前", ["以前", "上次", "过去"].as_slice());
    m.insert("工作", ["项目", "任务"].as_slice());
    m.insert("学习", ["了解", "掌握"].as_slice());
    m
});

/// Returns true if the character is in the CJK Unified Ideographs block.
fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// Expand a query for hybrid (vector + BM25) retrieval.
///
/// - The `original` field is always the verbatim input (for vector search).
/// - The `bm25_query` field may have synonym terms appended when CJK
///   characters are detected and known keywords are found.
pub fn expand(query: &str) -> ExpandedQuery {
    let has_cjk = query.chars().any(is_cjk);

    if !has_cjk {
        return ExpandedQuery {
            original: query.to_string(),
            bm25_query: query.to_string(),
        };
    }

    // Collect all synonym expansions for keywords found in the query.
    let mut extras: Vec<&str> = Vec::new();
    for (keyword, synonyms) in SYNONYMS.iter() {
        if query.contains(keyword) {
            for syn in *synonyms {
                if !query.contains(syn) && !extras.contains(syn) {
                    extras.push(syn);
                }
            }
        }
    }

    let bm25_query = if extras.is_empty() {
        query.to_string()
    } else {
        format!("{} {}", query, extras.join(" "))
    };

    ExpandedQuery {
        original: query.to_string(),
        bm25_query,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_query_unchanged() {
        let result = expand("What programming language do you prefer?");
        assert_eq!(result.original, result.bm25_query);
    }

    #[test]
    fn chinese_query_expands_synonyms() {
        let result = expand("用户喜欢什么编程语言");
        assert_ne!(result.original, result.bm25_query);
        assert!(result.bm25_query.contains("偏好"), "bm25_query should contain synonym '偏好'");
        assert!(result.bm25_query.contains("用户喜欢什么编程语言"), "original text preserved in bm25_query");
    }

    #[test]
    fn chinese_without_known_keywords_unchanged() {
        let result = expand("天气很好");
        assert_eq!(result.original, result.bm25_query);
    }

    #[test]
    fn original_always_preserved() {
        let input = "我喜欢使用Rust";
        let result = expand(input);
        assert_eq!(result.original, input, "original field must be the verbatim input");
        // bm25_query should be expanded (contains synonyms for 喜欢 and 使用)
        assert!(result.bm25_query.starts_with(input));
    }
}
