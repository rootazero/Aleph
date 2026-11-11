# Fix review — F1

| ID | Verdict | Comment |
|---|---|---|
| F1-02 | PARTIAL | The diff adds `new_with_timeout(...)` and applies `request = request.timeout(timeout)`, but the production construction path was not changed to call it; `pub fn new(...) { Self::new_with_timeout(base_url, api_key, None) }` still defaults every existing caller to no configured timeout. |
| F1-03 | FIXED | The diff replaces string concatenation with `url::Url::parse(&self.base_url)` and `.extend(&["v2", "scrape"])`, and the test verifies that a base path is preserved while query and fragment data remain separate. |
| F1-05 | FIXED | The diff now matches `ToolError::Network(_)` and returns `AlephError::network(message)`, while non-network failures remain `AlephError::provider(message)` and retain the `crawl4ai:` context. |

## New problems

- `src/fetch/providers/firecrawl.rs:134-140` — URL parsing and `path_segments_mut()` still occur inside `fetch` rather than during construction, so malformed or non-base URLs can fail on every request after the client has already been built; the `scrape_url` field is never populated or used.
- `src/fetch/providers/firecrawl.rs:113-140` — The added `scrape_url` field is dead state, and the request still uses a freshly parsed endpoint on each call, contradicting the test and the stated intent to construct the endpoint once; this also means the new timeout test mutates `scrape_url` without affecting the request actually sent.
- `src/fetch/providers/firecrawl.rs:134-140` — Parsing the URL in `fetch` introduces a new runtime failure mode for values accepted by `new`; endpoint validation and construction should occur before returning the provider, or the request should use a precomputed URL.

## Batch verdict

Needs rework: F1-05 is fixed and F1-03 is substantially addressed, but F1-02 remains unconnected to the configured production path, and Firecrawl now has an unused `scrape_url` field plus per-request URL parsing that introduces unnecessary failure behavior.