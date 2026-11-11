# Fix review — E5

| ID | Verdict | Comment |
|---|---|---|
| E5-03 | PARTIAL | The diff restricts the placeholder to the path and rechecks the final URL (`kernel.check_http_request(&method, &egress_url)`), but it still replaces every occurrence in the path (`let path = parsed.path().replace(encoded_placeholder, &encoded);`) rather than a single parsed path segment, so a placeholder occurring in multiple segments can be injected in multiple locations; the diff does not prove the required final-URL check is performed immediately before sending or that the existing header/query/rate-limit behavior is preserved. |
| E5-05 | PARTIAL | The kernel now resets both counters (`self.http_call_count.store(0, ...); self.log_line_count.store(0, ...);`), but the diff adds no production call site invoking `reset_per_execution_counters()` before guest execution; the new test manually exercises the kernel and does not demonstrate that `call_bound` resets it. |
| E5-06 | NOT_FIXED | `register_agent` still has the stale-short-key behavior: the diff only changes `register_skill` (`let short_entry = self.skills.entry(short_key)...`), while the newly added `agent_refresh_updates_own_short_key_but_not_another_plugins` test expects agent refresh semantics but no production `register_agent` implementation change is shown. |
| E5-NEW-01 | NOT_FIXED | The diff's new tests call `read_workspace_file(&state, ...)`, but no definition or export of `read_workspace_file` appears in the diff; the tests therefore cannot compile if that helper is absent from the unchanged source. |
| E5-NEW-02 | FIXED | `reset_per_execution_counters` now resets both counters with `self.http_call_count.store(0, Ordering::SeqCst);` and `self.log_line_count.store(0, Ordering::SeqCst);`, matching the stated requirement. |

## New problems

- `src/extension/runtime/wasm/mod.rs:665` — The new test asserts reset behavior only by manually calling kernel methods; it never verifies that `call_bound` invokes `reset_per_execution_counters`, so the production failure mode remains untested and unfixed.
- `src/extension/runtime/wasm/host_functions.rs:383` — The tests reference `read_workspace_file`, but the diff provides no such helper; unless it already exists unchanged, this introduces a compile failure.
- `src/extension/registry/plugin_registry/mod.rs:389` — The added agent test has no corresponding production change to `register_agent`, so the test is expected to fail or the agent refresh requirement remains unmet.

## Batch verdict

Needs rework: E5-05 still lacks the required pre-guest-call reset invocation, E5-06 still lacks the `register_agent` fix, and the new host-function tests may not compile because `read_workspace_file` is not shown in the diff.