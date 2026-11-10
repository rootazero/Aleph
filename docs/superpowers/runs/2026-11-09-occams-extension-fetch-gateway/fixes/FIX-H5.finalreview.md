# Fix review — H5

| ID | Verdict | Comment |
|---|---|---|
| H5-01 | PARTIAL | The diff rejects all existing targets with `Ok(Some(_)) => { return visibility::not_found_response(request.id); }`, but this is only a preflight check and still does not enforce create-if-absent atomically under the backend’s target metadata lock, so simultaneous creation can still overwrite an existing target. |
| H5-02 | FIXED | The deletion now persists the config first with `if let Err(e) = save_config(&cfg, &sections).await { ... return ... }` and deletes the vault secret only afterward, preventing a failed config save from destroying the sole credential. |
| H5-03 | FIXED | The runtime snapshot now hydrates the retained key via `registration_config.api_key = resolve_api_key(&params.name, &vault);` before hot reload. |
| H5-04 | PARTIAL | The update path now uses `let mut cfg_candidate = (*cfg).clone();` and publishes only after `save_config(&cfg_candidate, &["providers"]).await`, but create/delete/default mutation paths still mutate `cfg` directly before saving, so the shared-config failure mode remains for those operations. |
| H5-05 | PARTIAL | The update path constructs the replacement before saving and returns an error on construction failure, but create and set-default paths still save unvalidated configurations and can report success without installing a usable provider. |
| H5-06 | FIXED | `save_config` now clones the owned config and runs `cfg_clone.save_incremental(&section_refs)` inside `tokio::task::spawn_blocking`, moving synchronous locking and filesystem work off the Tokio worker. |
| H5-07 | FIXED | Both catalog cache paths now await `cached_models_async(...)`, and the handler snapshots providers then releases the config read guard before the awaited I/O. |
| H5-08 | FIXED | Template discovery is now executed in `tokio::task::spawn_blocking(|| { ... TemplateRegistry::discover(...) })`, so its directory and file reads no longer block a Tokio worker. |
| H5-09 | FIXED | The generated key now includes a random component via `let random_component: u64 = rand::random();`, removing the millisecond-only collision condition. |
| H5-10 | FIXED | The handler now applies `filter(\|t\| t.owner.as_deref() == Some(owner))` when an owner parameter is supplied. |
| H5-11 | FIXED | Non-object owned-task metadata is now rejected with `INVALID_PARAMS` in the `else` branch instead of creating a task without the dispatcher marker. |
| H5-12 | FIXED | Persistence failure now returns `JsonRpcResponse::error(...)` immediately rather than continuing to the observed/success response. |
| H5-13 | PARTIAL | The handler grows the raw window and stops after finding 200 visible bubbles, but it still stops at `MAX_RAW_FETCH_LIMIT = 64_000` and warns that older bubbles may be missing, so the fixed raw-window failure mode can still occur beyond that cap. |
| H5-14 | PARTIAL | Search now over-fetches with `let fetch_limit = limit * 10;`, but foreign hits still consume that finite window before ownership filtering, so more than ten foreign matches can still exhaust it and hide authorized results. |
| H5-15 | FIXED | Distilled rows are removed with `rows.retain(\|(r, watermark)\| r.created_at > *watermark);` before the global `rows.truncate(limit)`. |
| H5-16 | FIXED | The update handler now accepts only `pending`, `compressed`, and `archived`, returning `INVALID_PARAMS` for every other label. |
| H5-17 | FIXED | The unrestricted path counts all matching rows under the same predicates via repeated `list_sessions(...)` calls and returns that count rather than `sessions.len()`. |
| H5-18 | PARTIAL | The scoped path no longer uses `i64::MAX`, but it repeatedly fetches and filters every foreign storage batch and retains all visible rows in `sessions` to compute the count, so it still materializes the complete visible corpus and performs corpus-sized storage work for a bounded page. |

## New problems

- `src/gateway/handlers/memory/sessions.rs:168` — The new `store.list_sessions_by_ids(...)` call is not accompanied by an implementation in this diff, and the later paging code still uses `list_sessions`; unless that API already exists with exactly this signature, the change will not compile or cannot provide the claimed query-level filtering.
- `src/gateway/handlers/memory/sessions.rs:190` — The unrestricted count loop calls `list_sessions` with `fetch_offset` as the storage limit and `fetch_limit` as the offset (`list_sessions(..., fetch_offset, fetch_limit)`), contrary to the earlier call where `fetch_limit` is the limit and `params.offset` is the offset; this reverses the pagination arguments and can produce incorrect totals.
- `src/gateway/handlers/memory/sessions.rs:190` — The unrestricted count starts from `fetch_offset`, which was initialized from the earlier code but is no longer initialized in the rewritten branch, so this diff does not provide a coherent complete control-flow path and may fail to compile due to an undefined variable.
- `src/gateway/handlers/providers/handlers.rs:224` — `resolve_api_key` is called as a free function, while the helper diff defines it as `pub(super) fn resolve_api_key(...)`; the handler must import that helper explicitly, and the diff does not show such an import or a local definition.
- `src/gateway/handlers/session/db_handlers/create.rs:27` — `rand::random()` is used without any shown `rand` dependency/import; the fix does not establish that the crate is available or that the random generator is cryptographically/appropriately sourced for uniqueness.
- `src/gateway/handlers/providers/handlers.rs:398` — The create path still mutates `cfg.providers.insert(...)` before `save_config(&cfg, &["providers"]).await`, so a failed save leaves the shared configuration changed, contradicting the broader publish-after-save invariant introduced for updates.
- `src/gateway/handlers/providers/handlers.rs:1128` — The default-provider path still mutates the shared `cfg` before its awaited save, so a failed save can advertise a default that was never durably installed.

## Batch verdict

Needs rework: several individual fixes are correct, but H5-01, H5-04, H5-05, H5-13, H5-14, and H5-18 remain incomplete, and the session-listing/provider diff contains likely compile errors and reversed pagination arguments.