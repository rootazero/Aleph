# FIX-H2 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- H2-01: FIXED
- H2-03: FIXED
- H2-02: FIXED
- H2-06: FIXED
- H2-07: FIXED
- H2-08: FIXED
- H2-04: FIXED
- H2-05: FIXED

## H2-NEW-01 — FIXED — 撤回越界的 Fetch 超时调用改动 / Reverted only the out-of-scope Fetch timeout call to `FirecrawlFetchProvider::new(base_url, api_key)` from `79dacfa33`; added `firecrawl_connection_test_keeps_default_timeout_constructor` beside the handler tests. Existing embedding/provider changes and other findings are untouched.

验证 / Validation: equivalent source assertions failed before the rollback and passed afterward; the production `handle_test` body matches `79dacfa33` exactly; `git diff --check` and standalone `rustfmt --check` passed. Per instructions, no Cargo commands or Rust tests were run. The regression test pins constructor selection, not network timeout behavior; earlier FIXED statuses above are historical and were not re-audited in this wave.
