//! Web search tool over the provider registry.
//!
//! Implements `AlephTool` trait for AI agent integration.

use arc_swap::ArcSwap;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::error::ToolError;
use crate::error::Result;
use crate::search::notes::snippets_clamped;
use crate::search::{Recency, SearchOptions, SearchRegistry};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::utils::text_format::truncate_chars;

/// Arguments for search tool
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SearchArgs {
    /// Search query. Optional when `queries` is given; the two are merged.
    #[serde(default)]
    pub query: String,
    /// More questions to ask alongside `query`, for research that needs
    /// several angles at once. Each is answered independently and the answers
    /// merge into one set, with each result tagged by the query that found
    /// it. At most 5 queries total per call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    /// Max results. Omit to use the operator's `[search].max_results`.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How fresh results have to be. Omit for no constraint.
    ///
    /// Backends with no freshness parameter are ranked behind those that have
    /// one; if none can express it the search still runs and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<Recency>,
    /// Only return results from these domains, e.g. `["docs.rs"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    /// Drop results from these domains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,
    /// Return page bodies, not just snippets. Costs far more context; use it
    /// when the answer is in the page rather than in the summary of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<bool>,
    /// Ask exactly these backends instead of the configured chain. One name
    /// asks that backend alone; two or more ask all of them at once and merge
    /// the answers, dropping pages more than one of them returned. Naming a
    /// backend that is not configured fails rather than answering from
    /// another.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
}

impl SearchArgs {
    /// Overlay the arguments the model actually gave onto the operator's
    /// `[search]` defaults.
    ///
    /// Only what was given: an omitted parameter has to mean "whatever the
    /// operator configured", never a default this file invented. That is why
    /// every field but `query` is an `Option` or a list — the two states
    /// ("unset" and "set to the same value as the default") are different
    /// requests once the operator changes the default.
    #[must_use]
    pub fn to_options(&self, base: &SearchOptions) -> SearchOptions {
        let mut options = base.clone();
        if let Some(limit) = self.limit {
            options.max_results = limit;
        }
        if let Some(recency) = self.recency {
            options.recency = Some(recency);
        }
        if !self.domains.is_empty() {
            options.include_domains.clone_from(&self.domains);
        }
        if !self.exclude_domains.is_empty() {
            options.exclude_domains.clone_from(&self.exclude_domains);
        }
        if let Some(full_content) = self.full_content {
            options.include_full_content = full_content;
        }
        if !self.providers.is_empty() {
            options.providers.clone_from(&self.providers);
        }
        // Tell the backends the bound this face is going to apply anyway.
        // Most cannot act on it and ignore it; Exa can honour it server-side,
        // where the difference is a whole page body per result that it
        // otherwise retrieves, bills for and hands us to throw away.
        //
        // Plus one, deliberately: told to return exactly the budget, a backend
        // makes "this page was 600 characters" and "this page was cut at 600"
        // the same observation, and `snippets_clamped` — the note that offers
        // the reader `web_fetch` for the rest — would stop firing for pages
        // that really do have more. One extra character keeps that signal.
        options.snippet_budget_chars = Some(SNIPPET_MAX_CHARS + 1);
        options
    }
}

/// Most queries a single call will ask.
///
/// Each query pays a full walk of the provider chain, so the bound is what
/// keeps "give me forty angles" from becoming forty rate-limit windows at
/// once. Five covers the research pattern this exists for — two to four
/// angles, asked together — with one to spare, and the schema description
/// says the number so the model learns it before the error has to teach it.
const MAX_QUERIES: usize = 5;

/// The distinct queries this call will ask: `query` first, then `queries`,
/// trimmed, empties dropped, repeats collapsed.
///
/// One rule for both fields because they name one concept — what to ask —
/// and a caller should not have to know which spelling won. The failure
/// names both fields too: an empty `queries` array is only a mistake when
/// `query` is empty as well.
fn resolve_queries(args: &SearchArgs) -> std::result::Result<Vec<String>, ToolError> {
    let mut seen = std::collections::HashSet::new();
    let queries: Vec<String> = std::iter::once(&args.query)
        .chain(args.queries.iter())
        .map(|q| q.trim())
        .filter(|q| !q.is_empty())
        .filter(|q| seen.insert(q.to_string()))
        .map(str::to_string)
        .collect();
    if queries.is_empty() {
        return Err(ToolError::InvalidArgs(
            "give `query` or `queries`: at least one non-empty query is required".to_string(),
        ));
    }
    if queries.len() > MAX_QUERIES {
        return Err(ToolError::InvalidArgs(format!(
            "at most {MAX_QUERIES} queries per call (got {}); narrow the question, or run the \
             rest as separate calls",
            queries.len()
        )));
    }
    Ok(queries)
}

/// Longest snippet handed to the model, in characters.
///
/// Deliberately not `grep`'s 240: a grep line is a *locator* for a file the
/// model can then read, so cutting it costs a little context and nothing else.
/// A snippet is the answer itself — cut it too short and the search has to be
/// run again, which costs a round trip and another provider call. 600 fits a
/// paragraph, which is what a search backend actually returns.
const SNIPPET_MAX_CHARS: usize = 600;

/// Longest page body handed to the model per result, in characters.
///
/// `full_content` is the one parameter that can make a single `search` carry
/// more than a `web_fetch`, because it returns N bodies rather than one. The
/// per-body bound keeps the comparison to "a few pages", and the overall
/// budget is capped again by [`SearchTool::max_result_tokens`].
const FULL_CONTENT_MAX_CHARS: usize = 20_000;

/// A single search result
///
/// The three optional fields are omitted when the backend did not report
/// them, which is not the same as reporting nothing: an absent
/// `published_date` means *this backend does not say*, and inventing one
/// would be worse than leaving the reader to ask another backend.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,
    /// Which backend returned this result.
    ///
    /// Present only when the call asked more than one backend. With a single
    /// one it would be the same name on every row — `provider_used` already
    /// says it once — and the previous round left the field off the tool face
    /// for exactly that reason, writing down that it would arrive when a
    /// merge gave it a first consumer. This is that consumer: in a merged set
    /// the rows come from different places and "who found this" is not
    /// recoverable from anything else in the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Which of the call's queries returned this result, as an index into
    /// the answer's `queries` list.
    ///
    /// Present only when the call asked several questions, for the same
    /// reason `provider` is present only in a multi-backend merge: with one
    /// query the index would be 0 on every row and `query` has already said
    /// it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_index: Option<usize>,
}

/// Output from search tool containing results and original query
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub query: String,
    /// Every distinct query the call asked, in asking order — the list each
    /// result's `query_index` points into. Present only when the call asked
    /// more than one question; a single-query answer is byte-identical to
    /// what it was before `queries` existed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    /// Which backend answered. The chain can fall through several, and "who
    /// said this" is not recoverable from the results.
    pub provider_used: String,
    /// What this answer is missing and which lever gets it — a dimension the
    /// backend could not express, failures it answered after, a clamp that
    /// fired. Empty in the ordinary case, and omitted from the wire then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Map registry results onto the tool face, bounding what a single call can
/// spend, and saying so whenever it spends the bound.
///
/// A clamp nobody announces reads exactly like a page that was short: the
/// model cannot tell "this is all there is" from "there is more, fetch the
/// url". Both bounds therefore return a note rather than quietly cutting.
fn render_results(
    results: Vec<crate::search::SearchResult>,
    attribute_each_result: bool,
) -> (Vec<SearchResult>, Vec<String>) {
    render_all(
        results.into_iter().map(|r| (0, r)).collect(),
        attribute_each_result,
        false,
    )
}

/// The same rendering for a merged multi-query set, with each row tagged by
/// the query that found it.
fn render_multi(
    results: Vec<(usize, crate::search::SearchResult)>,
    attribute_each_result: bool,
) -> (Vec<SearchResult>, Vec<String>) {
    render_all(results, attribute_each_result, true)
}

/// The one clamp loop both renderings share. `attribute_provider` /
/// `attribute_query` decide which provenance the rows carry; the clamping —
/// and the notes it owes — is identical either way, and written once because
/// two clamp loops is how one of them silently stops firing.
fn render_all(
    results: Vec<(usize, crate::search::SearchResult)>,
    attribute_provider: bool,
    attribute_query: bool,
) -> (Vec<SearchResult>, Vec<String>) {
    let mut clamped_snippets = 0usize;
    let mut clamped_bodies = 0usize;
    let mapped = results
        .into_iter()
        .map(|(query_index, r)| {
            // A snippet cut off a result that also carries its page body is
            // not a loss: the text is right there, one field down. Counting
            // it would emit `snippets_clamped`, whose lever is "fetch the url
            // with web_fetch" — the one move that buys the reader nothing
            // here. Backends that return only bodies (Exa) would otherwise
            // make that wrong note fire on every single result.
            let carries_body = r.full_content.is_some();
            let snippet = if r.snippet.chars().count() > SNIPPET_MAX_CHARS {
                if !carries_body {
                    clamped_snippets += 1;
                }
                truncate_chars(&r.snippet, SNIPPET_MAX_CHARS).to_string()
            } else {
                r.snippet
            };
            let full_content = r.full_content.map(|body| {
                if body.chars().count() > FULL_CONTENT_MAX_CHARS {
                    clamped_bodies += 1;
                    truncate_chars(&body, FULL_CONTENT_MAX_CHARS).to_string()
                } else {
                    body
                }
            });
            SearchResult {
                title: r.title,
                url: r.url,
                snippet,
                relevance_score: r.relevance_score,
                published_date: r.published_date,
                full_content,
                provider: attribute_provider.then_some(r.provider).flatten(),
                query_index: attribute_query.then_some(query_index),
            }
        })
        .collect();

    let mut notes = Vec::new();
    if clamped_snippets > 0 {
        notes.push(snippets_clamped(clamped_snippets, SNIPPET_MAX_CHARS));
    }
    if clamped_bodies > 0 {
        notes.push(crate::search::notes::full_content_truncated(
            clamped_bodies,
            FULL_CONTENT_MAX_CHARS,
        ));
    }
    (mapped, notes)
}

/// Web search tool over the provider registry.
///
/// The registry is read through an [`ArcSwap`] cell on every call rather
/// than captured at construction: `[search]` is a declared-live config
/// section, and the cell is how a hot-applied rebuild
/// ([`crate::search::SearchHandle`]) reaches the very next search without a
/// restart. A tool built from a bare registry wraps it in a cell nobody
/// swaps — the pre-live-apply behaviour, kept for construction sites with
/// no handle (tests, one-shot callers).
#[derive(Clone)]
pub struct SearchTool {
    registry: Arc<ArcSwap<SearchRegistry>>,
}

impl SearchTool {
    /// Tool identifier
    pub const NAME: &'static str = "search";

    /// Tool description for AI prompt.
    ///
    /// Longer than one line on purpose: five of these parameters landed with
    /// this round, and a knob the model cannot tell when to reach for is a
    /// knob nobody turns. The bytes are paid on every request that can see
    /// this tool, so each clause is here because it changes what the model
    /// sends: how to ask several questions, what freshness values exist, that
    /// domain filtering is a preference the answer reports on rather than a
    /// guarantee, that page bodies are expensive, and that naming a backend
    /// is an instruction rather than a hint.
    pub const DESCRIPTION: &'static str = "Search the web for current information. \
         `query` asks one question; add `queries` to ask several angles of a \
         research question in one call — at most 5 total. They run concurrently, \
         and the answer merges them: a page several queries found appears once, \
         tagged with the query that found it. \
         `recency` (`day|week|month|year`) bounds how old a result may be. \
         `domains` and `exclude_domains` restrict results by site. Both are \
         preferences, not guarantees: a backend that cannot express one still \
         answers, and the reply's notes say which dimension was dropped. \
         `full_content` returns whole page bodies instead of snippets — expensive, \
         so use it only when a summary will not do. `providers` asks exactly the \
         backends you name and fails rather than answering from another; naming two \
         or more asks them all at once and merges the answers, which spends one \
         call's quota per backend — breadth for a research question, waste for a lookup.";

    /// Create with a `SearchRegistry`, wrapped in a cell nobody swaps.
    ///
    /// Build the argument with [`SearchRegistry::for_tool`] rather than
    /// deciding here what an install with nothing configured should get: that
    /// decision has two callers, and it used to be written out at both.
    ///
    /// Construction sites that CAN have a live handle (the daemon's tool
    /// registry) must use [`Self::with_registry_cell`] instead — a tool built
    /// here never observes a `[search]` hot-apply.
    pub fn with_registry(registry: Arc<SearchRegistry>) -> Self {
        Self::with_registry_cell(Arc::new(ArcSwap::new(registry)))
    }

    /// Create over the live swap cell a [`crate::search::SearchHandle`]
    /// publishes to. Every call reads the current generation, so a
    /// `[search]` config write hot-applied by `config::live_apply` is what
    /// the very next search runs on.
    pub fn with_registry_cell(registry: Arc<ArcSwap<SearchRegistry>>) -> Self {
        info!("SearchTool initialized over the provider registry cell");
        Self { registry }
    }

    /// Execute a web search over the configured backends.
    async fn call_impl(&self, args: SearchArgs) -> std::result::Result<SearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // `query` and `queries` merge into one list; both empty is a usage
        // error, not an empty search.
        let queries = match resolve_queries(&args) {
            Ok(queries) => queries,
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                return Err(e);
            }
        };
        let args_summary = if queries.len() == 1 {
            format!("搜索: {}", queries[0])
        } else {
            format!("搜索: {} 等 {} 个查询", queries[0], queries.len())
        };
        notify_tool_start(Self::NAME, &args_summary);

        // One coherent registry generation for this whole call: a hot-apply
        // landing mid-call must not mix the old chain's defaults with the new
        // chain's providers.
        let registry = self.registry.load_full();

        // Start from the operator's `[search]` defaults (max_results /
        // timeout_seconds); whatever the model named still wins.
        let options = args.to_options(&registry.default_options());

        if queries.len() == 1 {
            // The single-query path, byte for byte what it was before
            // `queries` existed: one chain walk, one answer, no merge.
            let query = queries[0].clone();
            return match registry.search(&query, &options).await {
                Ok(answer) => {
                    // Attribute per result only for a merged answer: with one
                    // backend the name is the same on every row and
                    // `provider_used` has already said it.
                    let (results, clamp_notes) =
                        render_results(answer.results, options.providers.len() > 1);
                    // The registry's notes first: which backend answered and what
                    // it could not express frames everything below it.
                    let mut notes = answer.notes;
                    notes.extend(clamp_notes);

                    info!(count = results.len(), "Search completed via registry");
                    let result_summary = format!("找到 {} 条搜索结果", results.len());
                    notify_tool_result(Self::NAME, &result_summary, true);

                    Ok(SearchOutput {
                        results,
                        query,
                        queries: Vec::new(),
                        provider_used: answer.provider,
                        notes,
                    })
                }
                Err(e) => {
                    // Every backend failed, or none was configured. The registry's
                    // message already distinguishes the two and names the lever
                    // for each, so it goes to the model unedited.
                    let error_msg = e.to_string();
                    notify_tool_result(Self::NAME, &error_msg, false);
                    Err(ToolError::Execution(error_msg))
                }
            };
        }

        match registry.search_multi(&queries, &options).await {
            Ok(answer) => {
                // Per-row provider attribution when the queries were answered
                // by different backends; per-row query attribution always —
                // that is the whole shape of a multi-query answer.
                let (results, clamp_notes) =
                    render_multi(answer.results, answer.providers.len() > 1);
                let mut notes = answer.notes;
                notes.extend(clamp_notes);

                info!(
                    queries = answer.queries.len(),
                    count = results.len(),
                    "Multi-query search completed via registry"
                );
                let result_summary = format!("找到 {} 条搜索结果", results.len());
                notify_tool_result(Self::NAME, &result_summary, true);

                Ok(SearchOutput {
                    results,
                    query: answer.queries[0].clone(),
                    queries: answer.queries,
                    provider_used: answer.providers.join("+"),
                    notes,
                })
            }
            Err(e) => {
                // Every query failed. The registry's report already lists
                // each query and its per-backend failures.
                let error_msg = e.to_string();
                notify_tool_result(Self::NAME, &error_msg, false);
                Err(ToolError::Execution(error_msg))
            }
        }
    }
}

/// Implementation of `AlephTool` trait for `SearchTool`
///
/// This allows `SearchTool` to be used with Aleph's unified tool system.
#[async_trait]
impl AlephTool for SearchTool {
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = SearchArgs;
    type Output = SearchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Delegate to the internal implementation, converting ToolError to AlephError
        self.call_impl(args).await.map_err(Into::into)
    }

    /// Above the global default, below `web_fetch`'s page budget per result:
    /// with `full_content` set this call carries N page bodies where a fetch
    /// carries one, so the ceiling is on the call, not on the page.
    fn max_result_tokens(&self) -> Option<usize> {
        Some(8_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget travels, and it travels one character wide of the clamp.
    /// A backend told to return exactly `SNIPPET_MAX_CHARS` would make a
    /// full-length page indistinguishable from a truncated one, and the note
    /// pointing the reader at `web_fetch` would go quiet for the pages that
    /// most need it.
    #[test]
    fn the_face_declares_the_snippet_bound_it_is_going_to_apply() {
        let options = SearchArgs {
            query: "q".to_string(),
            ..Default::default()
        }
        .to_options(&SearchOptions::default());
        assert_eq!(options.snippet_budget_chars, Some(SNIPPET_MAX_CHARS + 1));
        assert!(
            options.snippet_budget_chars > Some(SNIPPET_MAX_CHARS),
            "asking for exactly the budget hides whether anything was cut"
        );
    }

    #[test]
    fn test_search_args_limit_omitted_defers_to_config() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(
            args.limit, None,
            "an omitted limit must stay None so `[search].max_results` applies"
        );
    }

    #[test]
    fn test_search_args_with_limit() {
        let args: SearchArgs =
            serde_json::from_str(r#"{"query": "rust programming", "limit": 10}"#).unwrap();
        assert_eq!(args.query, "rust programming");
        assert_eq!(args.limit, Some(10));
    }

    /// The prose and the schema must not disagree about what this tool
    /// accepts. A DESCRIPTION naming a parameter the schema does not carry
    /// teaches the model to send something that will be rejected; the reverse
    /// hides a parameter nobody will use.
    ///
    /// There is deliberately no exemption list. A backticked word that reads
    /// like a parameter and is not one gets un-backticked or reworded — an
    /// exemption list here would be a licence with no expiry, and the words it
    /// would hold are ours to change.
    #[test]
    fn every_parameter_named_in_the_search_description_exists_in_its_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(SearchArgs)).unwrap();
        let props: std::collections::BTreeSet<String> = schema["properties"]
            .as_object()
            .expect("SearchArgs must render an object schema")
            .keys()
            .cloned()
            .collect();
        let mut named = 0usize;
        for word in SearchTool::DESCRIPTION.split('`').skip(1).step_by(2) {
            if props.contains(word) {
                named += 1;
                continue;
            }
            // Not every backticked run is a parameter — a value list like
            // `day|week|month|year` is not a lowercase identifier, so it
            // cannot be read as one.
            assert!(
                !word.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "`{word}` reads as a parameter name but SearchArgs has no such field"
            );
        }
        assert!(
            named >= 5,
            "the description must actually name the parameters; it named {named}"
        );
    }

    fn body(len: usize) -> String {
        "x".repeat(len)
    }

    /// A snippet cut short is worth a note only when the text is actually
    /// gone. When the same result carries its page body, `snippets_clamped`
    /// would tell the reader to `web_fetch` the url for text they already
    /// have — the wrong lever, printed on every result of every Exa search.
    #[test]
    fn a_clamped_snippet_is_not_reported_when_the_body_came_with_it() {
        let with_body = crate::search::SearchResult {
            full_content: Some(body(100)),
            ..crate::search::SearchResult::new("t", "https://x.test", body(SNIPPET_MAX_CHARS + 1))
        };
        let (rendered, notes) = render_results(vec![with_body], false);
        assert_eq!(rendered[0].snippet.chars().count(), SNIPPET_MAX_CHARS);
        assert!(
            !notes.iter().any(|n| n.contains("snippet")),
            "the body is right there: {notes:?}"
        );

        // Without a body the clamp is a real loss and keeps its note.
        let bare =
            crate::search::SearchResult::new("t", "https://x.test", body(SNIPPET_MAX_CHARS + 1));
        let (_, notes) = render_results(vec![bare], false);
        assert!(notes.iter().any(|n| n.contains("snippet")), "{notes:?}");
    }

    /// Per-result attribution is information only in a merged answer. With
    /// one backend it is the same name on every row and `provider_used` has
    /// already said it once.
    #[test]
    fn results_are_attributed_only_when_several_backends_were_asked() {
        let r = crate::search::SearchResult {
            provider: Some("exa".into()),
            ..crate::search::SearchResult::new("t", "https://x.test", "s")
        };
        let (single, _) = render_results(vec![r.clone()], false);
        assert!(single[0].provider.is_none());
        let (merged, _) = render_results(vec![r], true);
        assert_eq!(merged[0].provider.as_deref(), Some("exa"));
    }

    /// An omitted `providers` must stay empty, which is what selects the
    /// operator's configured chain. A default of "the first configured
    /// backend" invented here would be a second answer to a question
    /// `[search].default_provider` already owns.
    #[test]
    fn an_omitted_providers_list_leaves_the_configured_chain_in_charge() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "q"}"#).unwrap();
        assert!(args.providers.is_empty());
        assert!(args
            .to_options(&SearchOptions::default())
            .providers
            .is_empty());

        let named: SearchArgs =
            serde_json::from_str(r#"{"query":"q","providers":["exa","tavily"]}"#).unwrap();
        assert_eq!(
            named.to_options(&SearchOptions::default()).providers,
            vec!["exa".to_string(), "tavily".to_string()]
        );
    }

    #[test]
    fn test_search_tool_creation() {
        assert_eq!(SearchTool::NAME, "search");
        assert!(!SearchTool::DESCRIPTION.is_empty());
    }

    /// The tool must exist even with nothing configured, and say so when
    /// called — a missing tool reads to the model as "this harness cannot
    /// search", which is a different and wrong statement.
    #[tokio::test]
    async fn a_registry_with_no_providers_fails_with_an_actionable_message() {
        let tool = SearchTool::with_registry(SearchRegistry::for_tool(None, None));
        let err = tool
            .call_impl(SearchArgs {
                query: "q".into(),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no search backend"), "{err}");
        assert!(
            err.contains("TAVILY_API_KEY") || err.contains("[search]"),
            "the message has to name what to set: {err}"
        );
    }

    // ─── Multi-query ────────────────────────────────────────────────

    /// `query` and `queries` name one concept: the list to ask. `query`
    /// leads, both are trimmed, empties are dropped, repeats collapse — and
    /// either spelling alone is enough.
    #[test]
    fn query_and_queries_merge_into_one_list() {
        let args = SearchArgs {
            query: " alpha ".into(),
            queries: vec!["beta".into(), "alpha".into(), "  ".into()],
            ..Default::default()
        };
        assert_eq!(resolve_queries(&args).unwrap(), vec!["alpha", "beta"]);

        let queries_only = SearchArgs {
            queries: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        assert_eq!(resolve_queries(&queries_only).unwrap(), vec!["a", "b"]);

        let query_only: SearchArgs = serde_json::from_str(r#"{"query": "x"}"#).unwrap();
        assert_eq!(resolve_queries(&query_only).unwrap(), vec!["x"]);
    }

    /// Both fields empty is a usage error naming both fields — an empty
    /// `queries` array is only a mistake when `query` is empty as well.
    #[test]
    fn neither_query_nor_queries_is_rejected() {
        let err = resolve_queries(&SearchArgs::default()).unwrap_err().to_string();
        assert!(err.contains("query"), "{err}");
        assert!(err.contains("queries"), "{err}");

        let empty_array: SearchArgs = serde_json::from_str(r#"{"queries": []}"#).unwrap();
        assert!(resolve_queries(&empty_array).is_err());
    }

    /// The cap the description promises is the cap the tool enforces, and
    /// the error says the number.
    #[test]
    fn more_queries_than_the_cap_is_rejected() {
        let args = SearchArgs {
            queries: (0..=MAX_QUERIES).map(|i| format!("q{i}")).collect(),
            ..Default::default()
        };
        let err = resolve_queries(&args).unwrap_err().to_string();
        assert!(err.contains(&MAX_QUERIES.to_string()), "{err}");

        let at_cap = SearchArgs {
            queries: (0..MAX_QUERIES).map(|i| format!("q{i}")).collect(),
            ..Default::default()
        };
        assert_eq!(resolve_queries(&at_cap).unwrap().len(), MAX_QUERIES);

        assert!(
            SearchTool::DESCRIPTION.contains(&MAX_QUERIES.to_string()),
            "the description must state the cap the tool enforces"
        );
        assert!(
            SearchTool::DESCRIPTION.contains("`queries`"),
            "the description must name the parameter"
        );
    }

    /// A backend whose answers key on the query text, so two queries return
    /// different pages, plus one page every query shares — and a query
    /// containing "boom" fails.
    struct StubProvider;

    #[async_trait::async_trait]
    impl crate::search::SearchProvider for StubProvider {
        async fn search(
            &self,
            query: &str,
            _options: &SearchOptions,
        ) -> Result<Vec<crate::search::SearchResult>> {
            if query.contains("boom") {
                return Err(crate::error::AlephError::network("boom"));
            }
            Ok(vec![
                crate::search::SearchResult::new("shared", "https://stub.test/shared", "s"),
                crate::search::SearchResult::new(
                    format!("{query} page"),
                    format!("https://stub.test/{}", query.replace(' ', "-")),
                    "s",
                ),
            ])
        }

        fn name(&self) -> &str {
            "stub"
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    fn stub_tool() -> SearchTool {
        let mut registry = SearchRegistry::new("stub");
        registry.add_provider("stub".to_string(), Arc::new(StubProvider));
        SearchTool::with_registry(Arc::new(registry))
    }

    /// A call that asks one question is byte-identical on the wire to what
    /// it was before `queries` existed: no `queries` list, no per-row
    /// `query_index`.
    #[tokio::test]
    async fn a_single_query_call_carries_no_multi_query_machinery() {
        let output = stub_tool()
            .call_impl(SearchArgs {
                query: "rust".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(output.query, "rust");
        assert!(output.queries.is_empty());
        assert!(output.results.iter().all(|r| r.query_index.is_none()));

        let wire = serde_json::to_value(&output).unwrap();
        assert!(wire.get("queries").is_none(), "{wire}");
        assert!(
            wire["results"].as_array().unwrap().iter().all(|r| r.get("query_index").is_none()),
            "{wire}"
        );
    }

    /// Several questions, one merged answer: the shared page appears once,
    /// every row names the query that found it, and the merge says how many
    /// repeats it dropped.
    #[tokio::test]
    async fn several_queries_merge_and_stay_attributed() {
        let output = stub_tool()
            .call_impl(SearchArgs {
                queries: vec!["alpha".into(), "beta".into()],
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(output.queries, vec!["alpha", "beta"]);
        assert_eq!(output.query, "alpha", "the first query leads");
        assert_eq!(output.provider_used, "stub");

        let shared: Vec<&SearchResult> = output
            .results
            .iter()
            .filter(|r| r.url == "https://stub.test/shared")
            .collect();
        assert_eq!(shared.len(), 1, "the page both queries found, once");
        assert_eq!(shared[0].query_index, Some(0), "kept by the first query");

        let by_url: std::collections::HashMap<&str, Option<usize>> = output
            .results
            .iter()
            .map(|r| (r.url.as_str(), r.query_index))
            .collect();
        assert_eq!(by_url["https://stub.test/alpha"], Some(0));
        assert_eq!(by_url["https://stub.test/beta"], Some(1));
        assert!(
            output.notes.iter().any(|n| n.contains("more than one query")),
            "{:?}",
            output.notes
        );
    }

    /// One question failing leaves the other's answer standing, and the
    /// notes name the query that failed.
    #[tokio::test]
    async fn a_failing_query_does_not_sink_the_others() {
        let output = stub_tool()
            .call_impl(SearchArgs {
                queries: vec!["fine".into(), "boom".into()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            output.results.iter().all(|r| r.query_index == Some(0)),
            "only the surviving query contributed"
        );
        assert!(
            output
                .notes
                .iter()
                .any(|n| n.contains("query `boom`") && n.contains("failed")),
            "{:?}",
            output.notes
        );
    }

    /// Every query failing is the only multi-query `Err`.
    #[tokio::test]
    async fn every_query_failing_is_the_only_multi_query_error() {
        let err = stub_tool()
            .call_impl(SearchArgs {
                queries: vec!["boom one".into(), "boom two".into()],
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("All 2 queries failed"), "{err}");
    }
}
