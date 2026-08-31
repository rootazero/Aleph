# qa/web_search — the web search face on a real wire

Five phases, each one claim:

| phase | claim | why an in-process test cannot make it |
|---|---|---|
| `reach` | a parameter the model named (`recency: "week"`) arrives in the backend's query string as `time_range=week` | the unit test asserts what `SearchOptions::searxng_time_range` returns. That value and an HTTP request are two objects on two paths, and this repo has shipped a feature whose only defect was the second one — fifteen per-provider decoders translating a freshness value no caller could set. |
| `order` | when a request needs a dimension only one backend can carry, that backend is asked first | `ordered_candidates` is unit-tested with mock providers. Whether the sorted list is the one a booted server's registry actually walks is a different question, and it is settled by which backend received a request. |
| `degrade` | a dimension nobody configured can express is reported to the model, not dropped | the note is generated in the registry; whether it survives the tool face, the result mapping and the tool-output pipeline into the model's next request is a claim about four layers. |
| `empty` | a backend answering "zero results" does not end the chain | the shape that shipped for four rounds: a default provider returning `[]` stopped eight others and the SERP fallback from ever being asked. |
| `fanout` | naming two backends asks both, concurrently, and merges their answers into one set | the merge is unit-tested over hand-built vectors. Whether two *configured* backends are both dispatched, and whether the same page found by both collapses to one row on the way to the model, is a claim about the registry, the url normaliser, the tool face and the tool-output pipeline together. Its `fallback_providers` is deliberately empty, so a chain run could never reach the second backend — that is what makes it a claim about fan-out rather than about failover. |

Every phase anchors before it negates. `"answered after" not in output` and
`no request arrived` are both satisfied by a turn that never ran, so each phase
first proves the search returned something.

## The one backend this can point at

SearXNG. Seven of the nine providers hardcode their endpoint, and the ninth
(firecrawl) needs a credential nobody has here. So these phases prove the
**wiring** — options reach a provider's request builder, the registry orders
and fails over, the notes reach the model — and say nothing about the other
eight backends' own request builders.

That gap is covered at the source level instead, by
`search::providers::capability_census`: it reads each provider's source and
asserts that a declared capability bit corresponds to a request builder that
actually calls the matching mapper. The two are complementary and neither
replaces the other — the census cannot see a wire, and this fixture cannot see
eight of the nine backends.

`fanout` runs two mock SearXNG instances configured under names that are *not*
the provider type (`alpha` / `bravo`), and hands each of them one url the other
also returns — spelled differently (`?utm_source=…` on one side, `www.` and a
trailing slash on the other). Both of those choices are load-bearing:

* the differing spellings mean a merge that collapses them went through url
  normalisation, and one that keeps both did not — string equality passes
  neither way, so the count is the only place the difference shows;
* the names being different from the provider type is what caught the round's
  last defect on this fixture's **first run**. Each provider stamps
  `SearchResult::provider` with its own `NAME` — the provider *type* — while
  `provider_used` and the tool face's `providers` parameter use the
  *configuration key*. On the ordinary config where key equals type the two
  coincide, so the only place the two vocabularies can be told apart is a
  fan-out over two instances of one provider type. Every row said `searxng`
  while the summary said `alpha+bravo`, which made per-row attribution useless
  in precisely the situation it was added for.

`order` is the one phase that reaches the network: Exa's endpoint is fixed at
`api.exa.ai` and its key here is deliberately invalid. The phase does not
depend on what happens out there — an auth failure and an unreachable host are
the same event to it — only on the fact that Exa was asked *before* SearXNG,
which the answer's notes report.

## Mutation — what proves these phases are not vacuous

A real-machine assertion "reached the code it claims to test" is a proposition
to be proved, not assumed. Break the fix and the phase must go red:

* **run 2026-08-31** — `SearchOptions::searxng_time_range` forced to `None`:
  `reach` failed on exactly the `time_range=week` assertion, on a binary built
  24 s earlier, and the other three assertions stayed green. The anchor
  ("a search result reached the model") passing is the half that matters: it
  says the phase reached the classifier rather than dying before it.
* **run 2026-08-31** — the zero-result branch in `SearchRegistry::search`
  reverted to returning on an empty vector: `empty` failed, and the backend
  logs said why — the zero-result mock received four requests and the second
  backend received none.
* **run 2026-09-01** — `merge_by_rank`'s dedup disabled (`seen.insert(...)`
  evaluated for its side effect, the branch forced true): `fanout` failed on
  exactly its two merge assertions — 4 rows where 3 are expected, and the
  "merged" note absent — while the anchor, both dispatch assertions, the
  attribution assertion and the whole control arm stayed green.
* **not run** — `ordered_candidates` returning its input unsorted should fail
  the `order` phase's domains arm while its control arm stays green. Written
  down rather than performed, so it is a claim about this fixture that nobody
  has tested yet.

### An instrument that lied on this fixture's first run

The per-row attribution assertion was first written as `'"provider"' in text`.
A tool_result's content is the JSON document *encoded as a JSON string*, so the
field is present in the payload as `provider\":\"alpha` and the substring test
reported a missing field that was right there — a FAIL sitting next to a
genuine one, in the same run, indistinguishable by eye. The structural
assertions now parse (`payload()`); the phases that assert on prose still use
substrings, where the encoding does not matter.

## Running

```
./qa/web_search/run.sh reach
./qa/web_search/run.sh order
./qa/web_search/run.sh degrade
./qa/web_search/run.sh empty
./qa/web_search/run.sh fanout

KEEP=1 ./qa/web_search/run.sh reach    # keep the scratch dir
SKIP_BUILD=1 ./qa/web_search/run.sh …  # reuse the binary already built
```

Ports default to 18821 (gateway), 18822 (mock provider), 18823/18824 (mock
SearXNG); override with `GATEWAY_PORT` / `MOCK_PORT` / `SEARX_A_PORT` /
`SEARX_B_PORT`.
