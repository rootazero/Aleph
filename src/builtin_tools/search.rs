//! Web search tool over the provider registry.
//!
//! Implements `AlephTool` trait for AI agent integration.

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
    /// Search query
    pub query: String,
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
}

/// Output from search tool containing results and original query
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub query: String,
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
    let mut clamped_snippets = 0usize;
    let mut clamped_bodies = 0usize;
    let mapped = results
        .into_iter()
        .map(|r| {
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
                provider: attribute_each_result.then_some(r.provider).flatten(),
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
/// There is exactly one way in: whatever boot could construct, resolved by
/// [`SearchRegistry::for_tool`]. An empty registry is a legitimate state — the
/// tool still exists and says what is missing when called.
#[derive(Clone)]
pub struct SearchTool {
    registry: Arc<SearchRegistry>,
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
         One query per call; ask several questions with several calls. \
         `recency` (`day|week|month|year`) bounds how old a result may be. \
         `domains` and `exclude_domains` restrict results by site. Both are \
         preferences, not guarantees: a backend that cannot express one still \
         answers, and the reply's notes say which dimension was dropped. \
         `full_content` returns whole page bodies instead of snippets — expensive, \
         so use it only when a summary will not do. `providers` asks exactly the \
         backends you name and fails rather than answering from another; naming two \
         or more asks them all at once and merges the answers, which spends one \
         call's quota per backend — breadth for a research question, waste for a lookup.";

    /// Create with a `SearchRegistry`, the only way in.
    ///
    /// Build the argument with [`SearchRegistry::for_tool`] rather than
    /// deciding here what an install with nothing configured should get: that
    /// decision has two callers, and it used to be written out at both.
    pub fn with_registry(registry: Arc<SearchRegistry>) -> Self {
        info!("SearchTool initialized with the provider registry");
        Self { registry }
    }

    /// Execute a web search over the configured backends.
    async fn call_impl(&self, args: SearchArgs) -> std::result::Result<SearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let args_summary = format!("搜索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        // Start from the operator's `[search]` defaults (max_results /
        // timeout_seconds); whatever the model named still wins.
        let options = args.to_options(&self.registry.default_options());

        match self.registry.search(&args.query, &options).await {
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
                    query: args.query,
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
}
