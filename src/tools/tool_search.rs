//! `tool_search` — ranked discovery meta-tool for the "deferred" exposure
//! tier. Tools deferred out of the model's initial list (MCP tools when
//! `[tools] defer_mcp_tools` is on) stay searchable here: the model queries by
//! capability and gets the top matches WITH their full input schema, so it can
//! call them directly. Registered per-request alongside `get_tool_schema`
//! (see `gateway/execution_engine/run_loop/inner.rs`), closing over a snapshot
//! of every tool's name + description + schema.
//!
//! Ranking is a self-contained BM25 over an identifier-aware tokenization of
//! (name + description) — no new dependency, no coupling to the memory FTS5
//! index. Mechanical lexical rank that the MODEL initiates → R7-clean (not
//! intent classification), R10 presentation layer (zero harness growth).
//!
//! The corpus is the raw registry snapshot, unfiltered by allow/deny/health
//! gates; a tool surfaced here but blocked by policy degrades gracefully to an
//! execute-time rejection rather than being silently absent from search.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, ToolResult};

/// One searchable tool: display name, description, full input schema.
#[derive(Clone)]
pub struct ToolDoc {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Identifier-aware tokenizer: lowercases, splits on every non-alphanumeric
/// boundary AND camelCase humps, so `browser_navigate`, `mcp:slack:post`, and
/// `getUserById` all yield their component words. Tokens under 2 bytes drop.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    let mut prev_cjk: Option<char> = None;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.len() >= 2 {
            out.push(cur.to_lowercase());
        }
        cur.clear();
    };
    for ch in s.chars() {
        // CJK has no spaces, and every CJK char is `is_alphanumeric()`, so the
        // latin path below would swallow a whole phrase into ONE token: the
        // description "发送一条消息" indexed as a single term that the query
        // "发送消息" can never equal. BM25 scored 0.0 for every Chinese query —
        // `tool_search` was simply dead in the user's own language.
        //
        // Split CJK runs into characters and also emit adjacent-character
        // bigrams (the standard CJK IR trick: unigrams alone match too loosely,
        // bigrams restore precision). This is a mechanical Unicode-script split
        // — no dictionary, no segmentation model, no new dependency.
        if is_cjk(ch) {
            flush(&mut cur, &mut out);
            prev_lower_or_digit = false;
            out.push(ch.to_string());
            if let Some(prev) = prev_cjk {
                out.push(format!("{prev}{ch}"));
            }
            prev_cjk = Some(ch);
            continue;
        }
        prev_cjk = None;
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower_or_digit {
                flush(&mut cur, &mut out); // camelCase boundary
            }
            cur.push(ch);
            prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        } else {
            flush(&mut cur, &mut out);
            prev_lower_or_digit = false;
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Chars that carry meaning one-per-glyph and are written without spaces, so
/// they must be segmented rather than accumulated into a latin-style word.
/// Covers Han (+ Extension A), Kana, Hangul, and the compatibility ideographs.
fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch),
        0x3040..=0x30FF   // Hiragana + Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

/// In-memory BM25 index over the tool corpus.
struct Bm25 {
    docs: Vec<ToolDoc>,
    doc_tokens: Vec<Vec<String>>,
    df: HashMap<String, usize>,
    avgdl: f64,
    n: usize,
}

impl Bm25 {
    fn build(docs: Vec<ToolDoc>) -> Self {
        let doc_tokens: Vec<Vec<String>> = docs
            .iter()
            .map(|d| {
                let mut t = tokenize(&d.name);
                t.extend(tokenize(&d.description));
                t
            })
            .collect();
        let n = docs.len();
        let mut df: HashMap<String, usize> = HashMap::new();
        for toks in &doc_tokens {
            let uniq: std::collections::HashSet<&String> = toks.iter().collect();
            for t in uniq {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
        }
        let total: usize = doc_tokens.iter().map(Vec::len).sum();
        let avgdl = if n == 0 { 0.0 } else { total as f64 / n as f64 };
        Self {
            docs,
            doc_tokens,
            df,
            avgdl,
            n,
        }
    }

    fn score(&self, q_tokens: &[String], doc_idx: usize) -> f64 {
        const K1: f64 = 1.5;
        const B: f64 = 0.75;
        let toks = &self.doc_tokens[doc_idx];
        let dl = toks.len() as f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in toks {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0;
        for q in q_tokens {
            let f = *tf.get(q.as_str()).unwrap_or(&0) as f64;
            if f == 0.0 {
                continue;
            }
            let nq = *self.df.get(q).unwrap_or(&0) as f64;
            let idf = (1.0 + (self.n as f64 - nq + 0.5) / (nq + 0.5)).ln();
            let denom = f + K1 * (1.0 - B + B * dl / self.avgdl.max(1.0));
            score += idf * (f * (K1 + 1.0)) / denom;
        }
        score
    }

    fn top_k(&self, query: &str, k: usize) -> Vec<(usize, f64)> {
        let q = tokenize(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, f64)> = (0..self.n)
            .map(|i| (i, self.score(&q, i)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        // Highest score first; deterministic name tiebreak.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| self.docs[a.0].name.cmp(&self.docs[b.0].name))
        });
        scored.truncate(k);
        scored
    }
}

/// Ranked discovery tool over a per-request corpus snapshot.
pub struct ToolSearchTool {
    index: Arc<Bm25>,
    /// The deferred tier, SHARED with the `ScopedToolService` that filters on
    /// it. Every hit this tool returns is promoted out of it, so the tool the
    /// model just read a schema for is actually in the native tool array on the
    /// next turn. Without this the tool was discoverable but uncallable — see
    /// `crate::tools::scoped::DeferredTools`.
    deferred: Arc<crate::tools::scoped::DeferredTools>,
}

impl ToolSearchTool {
    pub const NAME: &'static str = "tool_search";

    #[must_use]
    pub fn new(docs: Vec<ToolDoc>, deferred: Arc<crate::tools::scoped::DeferredTools>) -> Self {
        Self {
            index: Arc::new(Bm25::build(docs)),
            deferred,
        }
    }
}

#[async_trait]
impl LoopTool for ToolSearchTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    // Scheduling-safe despite technically mutating: promotion only flips
    // harness-internal presentation state (the deferred tier) behind a
    // generation counter — no session/file/store side effects another call in
    // the batch could observe torn. Models routinely batch several searches;
    // keeping them parallel is the whole point of the meta-tool.
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }

    fn description(&self) -> &str {
        "Search the full tool catalog by capability and get the best-matching tools \
         WITH their input schemas, ready to call. Use this to find tools not shown in \
         your initial tool list (e.g. connected MCP server tools). Query with plain \
         keywords describing what you want to do, e.g. \"send a slack message\"."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords describing the capability you need."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 5).",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.trim().is_empty() {
            return ToolResult::Error {
                error: "tool_search requires a non-empty `query`.".to_string(),
                retryable: false,
            };
        }
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 20) as usize;
        let hits = self.index.top_k(query, limit);
        let results: Vec<Value> = hits
            .iter()
            .map(|(i, score)| {
                let d = &self.index.docs[*i];
                json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.schema,
                    "score": (score * 1000.0).round() / 1000.0,
                })
            })
            .collect();

        // Promote every hit out of the deferred tier. This is the step that
        // makes the whole mechanism real: the tools array handed to the provider
        // is rebuilt from `metadata_schema()`, which filters on this exact set,
        // so until a name leaves it the model has no `tool_use` channel to call
        // the tool through — and this tool's own description promised it was
        // "ready to call". Model-initiated (the model chose to search), so R10's
        // progressive-disclosure exception holds.
        let found: Vec<String> = hits
            .iter()
            .map(|(i, _)| self.index.docs[*i].name.clone())
            .collect();
        self.deferred.undefer(&found);

        ToolResult::Success {
            output: json!({ "query": query, "count": results.len(), "results": results }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<ToolDoc> {
        vec![
            ToolDoc {
                name: "slack_post_message".into(),
                description: "Send a message to a Slack channel".into(),
                schema: json!({"type":"object","properties":{"channel":{"type":"string"}}}),
            },
            ToolDoc {
                name: "github_create_issue".into(),
                description: "Open a new GitHub issue in a repository".into(),
                schema: json!({"type":"object"}),
            },
            ToolDoc {
                name: "browser_navigate".into(),
                description: "Navigate the browser to a URL".into(),
                schema: json!({"type":"object"}),
            },
        ]
    }

    #[test]
    fn tokenize_splits_identifiers_and_camel() {
        assert!(tokenize("browser_navigate").contains(&"navigate".to_string()));
        assert!(tokenize("mcp:slack:post").contains(&"slack".to_string()));
        assert!(tokenize("getUserById").contains(&"user".to_string()));
    }

    /// Every CJK char is `is_alphanumeric()` and CJK is written without spaces,
    /// so the latin path used to accumulate a whole phrase into ONE token: a
    /// description indexed as "发送一条消息" could never be matched by the query
    /// "发送消息". BM25 scored 0.0 for every Chinese query — `tool_search` was
    /// dead in the user's own language.
    #[test]
    fn tokenizer_segments_cjk_into_characters_and_bigrams() {
        let toks = tokenize("发送消息");
        assert!(toks.contains(&"发".to_string()), "unigram; got {toks:?}");
        assert!(toks.contains(&"发送".to_string()), "bigram; got {toks:?}");
        assert!(
            !toks.contains(&"发送消息".to_string()),
            "the whole phrase must NOT be one term; got {toks:?}"
        );
        // Mixed scripts still split on the boundary.
        let mixed = tokenize("slack_发送");
        assert!(mixed.contains(&"slack".to_string()));
        assert!(mixed.contains(&"发".to_string()));
    }

    /// The end of the CJK story: a Chinese query now actually retrieves.
    #[tokio::test]
    async fn a_chinese_query_finds_the_tool() {
        let docs = vec![
            ToolDoc {
                name: "slack_post_message".into(),
                description: "发送一条消息到 Slack 频道".into(),
                schema: json!({"type":"object"}),
            },
            ToolDoc {
                name: "file_read".into(),
                description: "读取磁盘上的文件内容".into(),
                schema: json!({"type":"object"}),
            },
        ];
        let t = ToolSearchTool::new(docs, crate::tools::scoped::DeferredTools::empty());
        let out = t
            .execute(json!({"query":"发送消息"}), CancellationToken::new())
            .await;
        match out {
            ToolResult::Success { output } => {
                let results = output["results"].as_array().unwrap();
                assert!(!results.is_empty(), "a Chinese query must retrieve at all");
                assert_eq!(results[0]["name"], "slack_post_message");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    /// The hit must become CALLABLE, not merely visible: `tool_search` promotes
    /// what it returns out of the deferred tier, so the tool re-enters the native
    /// tool array. Without this the tool's own "ready to call" description was a
    /// lie — the model had no `tool_use` channel to reach it through.
    #[tokio::test]
    async fn search_promotes_its_hits_out_of_the_deferred_tier() {
        let deferred = crate::tools::scoped::DeferredTools::new(
            ["slack_post_message".to_string(), "file_read".to_string()].into(),
        );
        let t = ToolSearchTool::new(corpus(), deferred.clone());
        assert!(deferred.is_deferred("slack_post_message"));

        let _ = t
            .execute(
                json!({"query":"send slack message", "limit": 1}),
                CancellationToken::new(),
            )
            .await;

        assert!(
            !deferred.is_deferred("slack_post_message"),
            "the discovered tool must be promoted back into the tool array"
        );
        assert!(
            deferred.is_deferred("file_read"),
            "tools the model did not find stay deferred — the array does not blow open"
        );
    }

    #[tokio::test]
    async fn ranks_relevant_tool_first_with_schema() {
        let t = ToolSearchTool::new(corpus(), crate::tools::scoped::DeferredTools::empty());
        let out = t
            .execute(
                json!({"query":"send slack message"}),
                CancellationToken::new(),
            )
            .await;
        match out {
            ToolResult::Success { output } => {
                let results = output["results"].as_array().unwrap();
                assert!(!results.is_empty());
                assert_eq!(results[0]["name"], "slack_post_message");
                // top hit carries its full schema so the model can call directly
                assert!(results[0]["parameters"]["properties"]["channel"].is_object());
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn limit_is_respected() {
        let t = ToolSearchTool::new(corpus(), crate::tools::scoped::DeferredTools::empty());
        let out = t
            .execute(
                json!({"query":"a e i o u the to in","limit":2}),
                CancellationToken::new(),
            )
            .await;
        if let ToolResult::Success { output } = out {
            assert_eq!(output["results"].as_array().unwrap().len(), 2);
        } else {
            panic!("expected success");
        }
    }

    #[tokio::test]
    async fn empty_query_errors() {
        let t = ToolSearchTool::new(corpus(), crate::tools::scoped::DeferredTools::empty());
        let out = t
            .execute(json!({"query":"   "}), CancellationToken::new())
            .await;
        assert!(matches!(out, ToolResult::Error { .. }));
    }

    #[tokio::test]
    async fn no_match_returns_empty_not_error() {
        let t = ToolSearchTool::new(corpus(), crate::tools::scoped::DeferredTools::empty());
        let out = t
            .execute(
                json!({"query":"zzzqqq_nonexistent_capability"}),
                CancellationToken::new(),
            )
            .await;
        if let ToolResult::Success { output } = out {
            assert_eq!(output["count"], 0);
        } else {
            panic!("expected success with empty results");
        }
    }
}
