# Fix review — E5

| ID | Verdict | Comment |
|---|---|---|
| E5-01 | PARTIAL | Production code now re-checks the canonical relative path via `"state.kernel.check_workspace_read(&canonical_relative)"`, which closes the granted-prefix symlink escape, but the added test exercises the separate `"fn read_workspace_file"` helper rather than `host_workspace_read`, so it does not prove the production path is fixed. |
| E5-02 | FIXED | The production path now opens the file, rejects it with `"if !metadata.is_file()"`, and uses `"std::io::Read::take(file, MAX_WORKSPACE_READ_BYTES + 1)"`, preventing FIFO blocking and unbounded reads while preserving UTF-8 handling. |
| E5-03 | PARTIAL | The diff adds `"kernel.check_http_request(&method, &egress_url)"` before sending, but `UrlPath` injection still uses `"let path = parsed.path().replace(encoded_placeholder, &encoded);"`, which can replace multiple matching path segments and does not establish that the placeholder occupies exactly one parsed path segment; therefore the full suggested boundary is not proven. |
| E5-04 | FIXED | Both request and client-build errors now use `"e.without_url()"`, specifically `"format!(\"request failed: {}\", e.without_url())"`, preventing injected URL secrets from being returned through those reqwest diagnostics. |
| E5-05 | PARTIAL | The kernel resets both counters with `"self.http_call_count.store(0, Ordering::SeqCst)"` and `"self.log_line_count.store(0, Ordering::SeqCst)"`, but the production diff only adds `"self.reset_per_execution_counters();"` in `mod.rs` and retains the loaded instance’s kernel separately; this diff does not show the reset occurring immediately before each guest call on the same kernel used by that call. |
| E5-06 | FIXED | Both registration paths now refresh only same-owner short aliases: skills use `"if short_entry.plugin_id == skill.plugin_id"` and agents use `"Some(existing) if existing.plugin_id != agent.plugin_id => {}"`, while namespaced entries remain updated. |

## New problems

- `src/extension/runtime/wasm/host_functions.rs:381` — `"fn read_workspace_file(...)"` duplicates production path logic but is not used by `host_workspace_read`; consequently the new workspace tests can pass while the production implementation regresses, and the helper itself does not enforce the allowed-prefix check.
- `src/extension/runtime/wasm/mod.rs:67` — adding `"kernel: Arc<WasmCapabilityKernel>"` to `LoadedWasmPlugin` without showing where it is initialized or used by the guest execution means the reset may target a different kernel from the one bound into the guest.

## Batch verdict

needs rework: E5-02 and E5-04 are fixed, but E5-01, E5-03, and E5-05 are only partially demonstrated; specifically, the tests do not exercise the production workspace-read path, UrlPath segment replacement remains insufficiently constrained, and the per-execution reset is not clearly applied to the guest’s actual kernel before each execution.