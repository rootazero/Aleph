# Fix review — H6

| ID | Verdict | Comment |
|---|---|---|
| H6-01 | FIXED | `let mut sanitized_dto = dto.clone();` followed by `backend.api_key = Some("[REDACTED]".to_string());` removes plaintext API keys from the published `ConfigChangedEvent` value. |
| H6-02 | PARTIAL | `if let Err(e) = next_cfg.save_incremental(&["generation"])` now precedes `vault.delete_secret(...)`, but `*cfg = next_cfg;` and the Vault deletion are still separate operations, so a Vault failure leaves the provider removed from configuration while its credential remains and the handler only logs the failure. |
| H6-03 | FIXED | `if config.providers.contains_key(name) { return Some("legacy".to_string()); }` routes legacy providers to the legacy bucket instead of deriving a typed bucket from capabilities. |
| H6-04 | PARTIAL | `settings_match` compares only `type_str` and `base_url`; it does not compare the probed credential with the saved provider’s current Vault credential, so a probe using a different key can still set `p.verified = outcome.success`. |
| H6-05 | FIXED | `tokio::task::spawn_blocking(move || { git2::Repository::clone(&url, &dest) ... })` moves the synchronous clone and filesystem validation off the async worker. |
| H6-06 | FIXED | `Some(Value::Bool(flag)) => Some(*flag), Some(_) => return ... INVALID_PARAMS` rejects explicit non-boolean values before acquiring the service lock or mutating the job. |
| H6-07 | FIXED | `let service = cron.lock().await;` is acquired before the timezone branch reads `service.get_job(...)` and remains held through `service.update_job(...)`, preventing the stale read-modify-write interleaving. |
| H6-08 | REGRESSION | The diff replaces the atomic temporary-file rename with `std::fs::write(path, new_contents.as_bytes())`, but the shown security writer still has no effective lock acquisition; the added `_lock` file handle alone does not serialize writers or protect the read-modify-write operation. |
| H6-09 | REGRESSION | `std::fs::write(path, new_contents.as_bytes())` writes the live config directly and creates it with umask-derived permissions when absent; it does not use the required owner-only 0600 temporary file and atomic replacement. |
| H6-10 | FIXED | `let window_secs_i64 = i64::try_from(window.window_secs)` rejects values above `i64::MAX` before inserting `toml::Value::Integer(window_secs_i64)`. |

## New problems

- `src/gateway/handlers/generation_providers/handlers.rs:384-407` — the deletion path now saves the cloned configuration and replaces shared state before deleting the Vault secret, but `vault.delete_secret` failure is only logged, leaving a successful configuration deletion with an orphaned credential and no error response.
- `src/gateway/handlers/security_config/toml_io.rs:456-461` — direct `std::fs::write` removes atomic replacement and owner-only permission guarantees; this is the same permission/atomicity regression described in H6-09 and H6-08.

## Batch verdict

Needs rework: H6-04, H6-08, and H6-09 remain unsafe, and H6-02 still reports success despite a Vault cleanup failure.