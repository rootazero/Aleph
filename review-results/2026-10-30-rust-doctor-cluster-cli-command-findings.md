# Findings index — rust-doctor review of `src/cluster`, `src/cli`, `src/command`

Generated 2026-10-30 from six reviewer subagents' reports (no diff — whole-module read) and the
`rust-doctor 0.9.0` machine scan of the workspace.

`jev` is the Jev `noul` score used for triage; `disposition` is the routing decision
(`fix-now` ≥ 0.80, `verify-then-fix` 0.60–0.80, `defer` < 0.60).

| # | severity | disposition | jev | location | title |
|---|---|---|---|---|---|
| 1 | P1 | fix-now | 0.88 | `src/cluster/registry.rs:578` | The node registry accepts unbounded peer-declared metadata (`device_name`, `tags`, `commands[].schema`) and re-clones it on every `list_environments()` call, which feeds the model-callable `node_list` |
| 2 | P1 | fix-now | 0.80 | `src/cluster/enrollment.rs:559` | `deregister_node` resolves its target through the registry's FUZZY name-substring tier before the precise store lookup, so deregistering an offline `worker` evicts and sticky-revokes an online `worker-2` |
| 3 | P1 | defer | 0.50 | `src/cluster/registry.rs:234` + `src/cluster/enrollment.rs:404` | Node identity is admitted with no proof of possession: a credential-less peer naming an enrolled node's `device_name` is adopted into that row, and `register` then hands the identity over, so later dispatches deliver command bodies and file payloads to the impostor |
| 4 | P1 | verify-then-fix | 0.63 | `src/cluster/node_file_cmd.rs:86` | The node's file-command jail root is never created on the file path (only the bash sandbox creates it), so every `file.read`/`file.write` fails on a node that has not yet run bash — while the node still advertises both commands |
| 5 | P1 | defer | 0.55 | `src/command/parser.rs:181` | A custom slash command's configured `system_prompt` is parsed, serialized into the mode JSON and then dropped: no consumer reads it, so the documented feature has no effect on any surface |
| 6 | P2 | fix-now | 0.81 | `src/cluster/enrollment.rs:304` | `admit_node`'s unknown-id path backfills a row with the peer-supplied name without consulting the by-name match, recreating the self-sealing duplicate the module forbids |
| 7 | P2 | fix-now | 0.85 | `src/cluster/enrollment.rs:280` | `admit_node` adopts any active device row regardless of `role`; only `device_type = "panel"` is refused |
| 8 | P2 | fix-now | 0.82 | `src/cluster/enrollment.rs:85` | An empty / punctuation-only node name creates a row that can never be addressed by name, and every repeat enroll mints another one |
| 9 | P2 | verify-then-fix | 0.74 | `src/cluster/enrollment.rs:211` | Name→row enrollment is SELECT-then-INSERT with no lock and no unique index on `device_name`, so concurrent enrolls mint duplicate rows |
| 10 | P2 | fix-now | 0.85 | `src/cluster/node_runtime.rs:42` | The `CommandTable` allowlist key and the command's self-reported descriptor name are two independent namespaces; a mismatch makes the node advertise a command it refuses |
| 11 | P2 | verify-then-fix | 0.76 | `src/cluster/node_runtime.rs:169` | The file-command jail root and the bash session directory are two independent inputs that are equal only by convention at the one call site |
| 12 | P2 | fix-now | 0.83 | `src/cluster/reverse_rpc.rs:315` | The reverse-RPC response deadline is taken at call start, so the outbound-enqueue time is subtracted from the response window — the code's own comment claims independent budgets |
| 13 | P2 | fix-now | 0.83 | `src/builtin_tools/node_file.rs:107` | The center's push leg caps only a stat snapshot and then reads the file unbounded; the node side is already cap-safe, the center is not |
| 14 | P2 | fix-now | 0.80 | `src/cluster/registry.rs:596` | A malformed peer `commands` array collapses to an EMPTY catalog, which the invoke path reads as "the node is authoritative" — a peer parse error silently disables the center's fail-fast layer |
| 15 | P2 | verify-then-fix | 0.76 | `src/gateway/server/connection/mod.rs:165` | `conn_id` is the peer socket address, so an OS-reissued ephemeral port makes the dead connection's cleanup evict the LIVE session |
| 16 | P2 | defer | 0.48 | `src/cluster/node_file_cmd.rs:99` | The containment check canonicalizes the path and the kernel re-resolves it at open time: an intermediate directory swapped for a symlink escapes the jail (`O_NOFOLLOW` guards only the leaf) |
| 17 | P2 | fix-now | 0.83 | `src/cli/ipc_client.rs:113` | The admin bearer token and the secret payload are sent to whatever host `.ipc-endpoint.json` names; the loopback gate covers only the HTTPS branch |
| 18 | P3 | fix-now | 0.85 | `src/cluster/enrollment.rs:597` | `DeregisterOutcome.device_removed` conflates "already revoked" with "the revoke write failed", so the operator warning can be factually wrong |
| 19 | P3 | fix-now | 0.87 | `src/cluster/node_file_cmd.rs:139` | A non-regular file (FIFO) inside the workspace blocks the node's dispatch forever; the byte cap cannot help because the block precedes any read |
| 20 | P3 | fix-now | 0.85 | `src/gateway/server/connection/mod.rs:290` | A JSON-RPC response whose result is `null` is not response-shaped (serde maps `null` → `None`), so the pending call waits out its full timeout although the peer answered |
| 21 | P3 | fix-now | 0.89 | `src/cluster/reverse_rpc.rs:344` | `OutboundWedged` reports the caller's `timeout_ms` instead of the enqueue budget actually spent (up to 240× overstated) |
| 22 | P3 | fix-now | 0.77 | `src/cluster/node_runtime.rs:92` | `descriptors()` sorts by a self-reported name with `sort_unstable_by`; equal names make the order follow HashMap iteration order, which is randomized per process |
| 23 | P3 | fix-now | 0.79 | `src/cli/ipc_client.rs:186` | The response body is read in full and only truncated afterwards, so a huge body is fully allocated (the 10 s timeout bounds time, not memory) |
| 24 | P3 | fix-now | 0.78 | `src/cli/ipc_client.rs:34` | `Url::join` with an absolute route REPLACES the base path, so an endpoint URL with a prefix silently loses it — the comment claims the opposite |
| 25 | P3 | fix-now | 0.86 | `src/cli/policy.rs:181` | The `LockOrIpc` arm discards the `LockHeldError` that names the running server's PID, so a broken channel reads as "no server is running" |
| 26 | P3 | fix-now | 0.79 | `src/cli/endpoint.rs:132` | `read_endpoint` collapses missing / empty / unsupported-version into `None`, and the caller then claims the file does not exist |
| 27 | P3 | fix-now | 0.73 | `src/command/parser.rs:144` | `tool_registry_arc()` is a public method with zero callers whose doc steers a future caller to clone a second `Arc` to the shared catalog |
| 28 | P3 | fix-now | 0.73 | `src/command/parser.rs:198` | An unknown `ToolSource` variant (`#[non_exhaustive]`) parses to `None`, which is indistinguishable from "not a command" — the slash command silently degrades to plain chat, with no log line |
| 29 | P3 | defer | 0.36 | `src/cli/policy.rs:87` | `CommandPolicy::LockOrIpc.route` is a `&'static str`, so the server's per-key admin routes are unreachable from the CLI (a pinned test pins the current refusal) |
| 30 | P3 | defer | 0.40 | `src/command/parser.rs:190` | `pattern` falls back to the command NAME when the catalog row has no regex, manufacturing a value that is not a regex |
| 31 | P3 | defer | 0.29 | `src/bin/aleph-server/commands/node.rs:297` | The node's jail root is derived from `SessionKey::ephemeral` (a fresh random UUID per process), so it changes on every restart and orphans pushed files |

## Machine scan (rust-doctor 0.9.0, whole workspace)

19 diagnostics fall inside the three modules; all are structural (duplicate function bodies, a
1336-line file, `eprintln!`/`process::exit` in the CLI policy layer) and were **not** actioned —
the project's Occam's-razor rule forbids refactoring for shape alone. The one non-structural
diagnostic — "indexing into a string may panic if the index is within a UTF-8 character"
(`src/cli/ipc_client.rs:86`) — is a **false positive**: the index comes from
`str::floor_char_boundary`, which is by definition a char boundary. The gauge lies; no fix.
