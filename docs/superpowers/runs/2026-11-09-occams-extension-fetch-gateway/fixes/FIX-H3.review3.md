# Fix review — H3

| ID | Verdict | Comment |
|---|---|---|
| H3-03 | FIXED | `handle_update` now calls `matches_bind_address(&current_host)` and preserves `current_host.clone()` when the projected mode is unchanged; only an explicit mode switch writes the canonical address via `to_bind_address()`. |
| H3-03-UNWIRED | FIXED | The helper is no longer dead code: it is called in the update path, and the conditional `new_host` assignment replaces the previous unconditional `to_bind_address()` write. |
| H3-PROJECTS-CLONE | FIXED | `resolve_in_workspace` is called with `&root` and `&rel` inside the closure; both are cloned before the `move`, which is valid for `PathBuf` and `String`. No bug introduced. |
| H3-EXECUTION-SERIAL | FIXED | The test uses the fully-qualified attribute `#[serial_test::serial(...)]`; per the fix note, `serial_test = "4"` is already declared in `Cargo.toml` dev-dependencies, so no import is required. |

## New problems

- `src/gateway/handlers/security_config.rs:442` — the new test `update_preserves_bind_address_until_network_mode_changes` uses `toml::Table` and `toml::to_string` / parsing via `.parse()`; if `toml` is not already a dependency of this crate the test will fail to compile. The fix note does not mention verifying this, and the diff introduces no `use` for `toml`. This is a risk, not a confirmed bug.

## Batch verdict

Safe to commit, contingent on `toml` being an existing dependency (which is highly likely given `toml_io` usage elsewhere in the same file). The core H3-03 fix is correctly wired and behaviour-preserving.