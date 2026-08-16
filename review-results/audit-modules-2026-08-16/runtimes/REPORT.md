# Runtimes Module — Static Code Review Report

**Module:** `src/runtimes/`
**Date:** 2026-08-16
**Reviewer:** Aleph static code auditor (general-purpose subagent)
**Mode:** Read-only, no fixes applied
**Files reviewed:** 10 files / 3,755 lines
**Scope:** Three lenses — Seam (severed-wire), Logic, Architecture

---

## Module summary

The `runtimes` module implements a lightweight capability tracker: it probes
PATH for known runtimes, bootstraps missing ones via OS-specific installers,
and persists state to `~/.aleph/runtimes/ledger.json`. Three phases — probe,
bootstrap, ledger — orchestrated by `ensure_capability` (the only externally
significant function).

External consumers of the public API:

| Consumer | Uses |
|----------|------|
| `bin/aleph-server/commands/start/runtime_warmup.rs` | `probe::probe`, `SPECS`, `migrate_from_legacy`, `CapabilityLedger`, `CapabilityEntry`, `CapabilityStatus`, `get_runtimes_dir` |
| `bin/aleph-server/commands/bootstrap_runtime/mod.rs` | `ensure_capability`, `find_spec`, `supported_on_current_os`, `CapabilityLedger`, `CapabilityStatus`, `get_runtimes_dir` |
| `gateway/handlers/runtimes.rs` (list/refresh/install) | `ensure_capability`, `find_spec`, `supported_on_current_os`, `SPECS`, `probe::probe`, `CapabilityLedger`, `CapabilityEntry`, `CapabilityStatus` |
| `gateway/handlers/mod.rs::make_runtime_ledger` | `get_runtimes_dir`, `CapabilityLedger::load_or_create` |
| `browser/playwright_cli.rs::provision_binary` | `ensure_capability("playwright-cli", …)`, `get_runtimes_dir`, `CapabilityLedger` |
| `tools/probes/browser.rs` | `get_runtimes_dir`, `CapabilityLedger` |
| `builtin_tools/code_exec.rs` / `code_check.rs` | `ledger::build_enhanced_path` |
| `orchestrator/harness_bridge/prompt_build.rs` | `get_runtimes_dir`, `CapabilityLedger`, `format_entries_for_prompt` |

Every re-exported type and function has at least one live consumer; no
form-1 ("no consumer") wires exist.

Recent fixes reviewed (already merged; NOT re-reported):
- `dadef3f07` — npm globals install at platform-standard user prefix
- `fd06300cc` — 7 dead variants/arms/fields cut, `migrate_from_legacy` wired into warmup
- `901678a50` — unused imports / dead-code warnings from severed-wire cleanup
- `a25745e26` — capability ledger reset off `Bootstrapping` on dispatcher error

---

## Findings by severity

### [Medium] src/runtimes/ensure.rs:288-303 — `runtime_error` format makes the stderr tail uncatchable at the gateway boundary

**Category:** seam
**Confidence:** High

`runtime_error` formats the failure as:

```
Runtime '{capability}' is not available: {reason}\n
Stderr tail: {tail}\n\n
Fix options:\n  1. Run: ...\n  2. Open Panel ...\n  3. Install manually — {hint}
```

The `runtime` module owns the format. The gateway handler at
`src/gateway/handlers/runtimes.rs:175` recovers the stderr tail via:

```rust
let stderr = err_str
    .split_once("Stderr tail: ")
    .map(|(_, tail)| tail.to_string())
    .or_else(|| Some(err_str.clone()));
```

`split_once` returns everything **after** the marker, which includes the
`\n\nFix options: …` block. So the `stderr` field of the
`RuntimeInstallProgressEvent` carries noise that downstream Panel parsing
would mistake for stderr content.

The boundary is fragile in two directions: any change to the format
literal in `runtime_error` silently breaks the handler's recovery, and the
handler's parsing cannot be made precise against the current format.

**Suggested fix:** stop encoding stderr in the human-readable error
message; expose it as a structured field on `AlephError::runtime` (or a
new variant), or have `runtime_error` return a struct that the gateway
can serialize field-by-field.

---

### [Medium] src/runtimes/ledger.rs:66, 68 — `CapabilityEntry::source` and `last_probed` are dead data

**Category:** architecture
**Confidence:** High

`CapabilityEntry` has two fields that are written by every code path but
read by none:

- `source: CapabilitySource` — set in 9 sites (`ensure.rs`, `runtime_warmup.rs`,
  `gateway/handlers/runtimes.rs::handle_refresh`, `migrate_from_legacy`,
  tests), never read. The grep `\.source\b` across `src/` returns matches
  only in the *test* assertions and in the construction sites. The variant
  `CapabilitySource::AlephManaged` vs `System` has no observable effect.
- `last_probed: u64` — set in the same sites plus the `new_ready`
  constructor; not read anywhere. The doc-comment at `ledger.rs:239`
  explains "this is on the startup path; the recorded version stays as-of
  `last_probed`, which is why that field exists" — but no code consults
  it for re-probe scheduling or staleness decisions. The `Stale` variant
  is never written by any code path either (verified by grep: only the
  variant declaration and one migrate-from-legacy test reference it).

These two fields (and the `Stale` variant) form a small inert-config
group: persisted to JSON on every `persist()`, consumed on reload by
`status()` and `executable()`, but the values they carry are dead.

**Suggested fix:** either delete the fields (and the `Stale` variant) if
nothing is planned to use them, or wire them into the re-probe scheduler
so the existing infrastructure pays for itself.

---

### [Low] src/runtimes/probe.rs:107-148 — `find_on_path` / `get_version` have no timeout

**Category:** logic
**Confidence:** High

The probe runs `which`/`where` and `<bin> --version` with no timeout:

```rust
fn get_version(bin_path, version_flag, version_regex, search_path) -> Option<String> {
    let mut cmd = Command::new(bin_path);
    cmd.arg(version_flag).env("PATH", search_path);
    let output = cmd.no_window().output().ok()?;
    ...
}
```

The bootstrap path (`bootstrap.rs::run_cmd`) wraps its child in
`timeout(Duration::from_secs(600), …)`. The probe does not. A binary on
`PATH` that ignores `--version` (a script with no signal handling, a GUI
binary that prompts, an `fnm` install under a corrupted network mount
that blocks on stat) will hang the probe forever, blocking the daemon's
startup warmup and every subsequent `ensure_capability` that hits the
same name.

`probe::probe` is `fn` (synchronous) but `ensure_capability_recursive`
already holds a write lock when invoking it; a stuck probe blocks every
caller.

**Suggested fix:** wrap the `which`/`where`/`--version` invocations in
`tokio::time::timeout` (and convert `probe::probe` to `async fn` to match
the rest of the orchestrator), with a fallback `None` on timeout so the
warmup continues.

---

### [Low] src/runtimes/ensure.rs:51-126 — Nested lock acquisition order in `ensure_capability_recursive` has no global invariant

**Category:** logic
**Confidence:** Medium

The function acquires the outer capability's lock *before* recursing into
each dependency:

```rust
let cap_lock = capability_lock(capability);   // outer
let _install_guard = cap_lock.lock().await;    // held across recurse

for dep in bootstrap::dependencies(capability) {
    Box::pin(ensure_capability_recursive(dep, ledger, depth + 1)).await?;  // inner
}
```

`capability_lock(name)` returns `Arc<Mutex<()>>` keyed by name, so there
is no per-spec ordering rule enforced anywhere. With the current `SPECS`
table this is safe (only one of `node → fnm`, `playwright-cli → node`
exists, and the recursion is strictly deeper-then-shallower). But the
data-driven shape of `SPECS` means any future addition of e.g. a
`symmetric_dep` entry that lists both directions would deadlock without
the `MAX_BOOTSTRAP_DEPTH` guard catching it — `MAX_BOOTSTRAP_DEPTH = 10`
catches infinite recursion, not deadlock.

**Suggested fix:** either (a) enforce a global lock order (sort deps by
name before acquiring), or (b) document explicitly in `RuntimeSpec.deps`
that the lock ordering is by name.

---

### [Low] src/runtimes/post_install.rs:109-138 — `fnm list` parsing is fragile against future fnm output formats

**Category:** logic
**Confidence:** Medium

```rust
let version = text
    .lines()
    .filter_map(|l| {
        if l.trim().starts_with('*') {
            l.split_whitespace()
                .find(|t| t.starts_with('v'))
                .map(String::from)
        } else { None }
    })
    .next()
    .ok_or(PostInstallError::NoNodeVersion)?;
```

The parser hard-codes two assumptions about `fnm list`:
1. The default version line begins with `*`.
2. A version token in that line begins with `v`.

The post-install test in `specs.rs` does not exercise this code path with
a real `fnm list` output (it uses the `RunSubcommand` action for
`playwright-cli`). Recent fnm releases (1.x → 2.x transitions) have
changed column delimiters at least once. If the default line no longer
starts with `*`, every `node` install produces `NoNodeVersion` and the
`lts` alias is silently never created — the post-install surfaces no
indication of partial completion.

**Suggested fix:** prefer `fnm current` (which returns just the current
default version, no parsing) over scanning `fnm list`. `fnm alias
<version> <alias>` only needs the version string.

---

### [Low] src/runtimes/ensure.rs:173 — `CapabilityStatus::Bootstrapping` has no transition guard

**Category:** architecture
**Confidence:** Medium

The state machine records `Bootstrapping` in the ledger at line 173, then
immediately transitions out on success or failure. No code path reads the
`Bootstrapping` value to gate behavior. The variant exists solely as a
transient signal for any external reader (Panel `runtimes.list`) that
queries during the install.

In `gateway/handlers/runtimes.rs::handle_refresh` there is no check for
`Bootstrapping`; a refresh arriving while `ensure_capability` holds its
install guard can write `Missing` (probe-failed) over a ledger entry
that is mid-install. The bootstrap's eventual `Ready` write repairs
this — but if the refresh's `Missing` write reaches disk first via
`guard.mark_missing` (which does not persist by itself, but its caller
does not either — `handle_refresh` does not `persist()` after the loop),
the on-disk ledger flips briefly to `Missing` only when the ledger is
re-read between the refresh write and the bootstrap write.

The variant + the field are essentially documentation-in-the-type-system;
no code reacts to it.

**Suggested fix:** either drop the variant (it's not used as a state),
or wire it into `handle_refresh` to skip entries that are currently
`Bootstrapping` (with the writer's cap_lock held to detect in-flight
installs).

---

### [Low] src/runtimes/bootstrap.rs:204-273 — `enrich_path_for_reprobe` mutates process-wide PATH

**Category:** architecture
**Confidence:** High

```rust
prepend_existing_dirs(candidates);
// ...
std::env::set_var("PATH", joined);
```

The function permanently mutates the daemon's `PATH` for the lifetime
of the process. The code comment acknowledges this ("we accept it") and
the `PATH_LOCK` mutex makes the read-modify-write atomic against other
callers of `prepend_existing_dirs` from within the runtime module.

But: nothing prevents the rest of the daemon (or any other module that
imports `std::env`) from reading `PATH` *concurrently* with the locked
section. Tokio's `set_var` is `unsafe` in Rust 2024 (editions beyond
2021); under edition 2021 the documentation warning is "not thread-safe".
The risk surface is: a `Command::new` elsewhere in the daemon spawns
with an inconsistent `PATH` view mid-`set_var`. In practice the window
is microseconds and the daemon has few sibling threads reading `PATH`,
so this is a latent risk rather than a current defect.

**Suggested fix:** the cleaner fix is to switch the daemon to a single
`Arc<Mutex<OsString>>` `PATH`-equivalent and pass it explicitly to
`Command::env("PATH", …)` (mirroring what `probe.rs::enriched_search_path`
already does correctly). Keep `set_var` only as a legacy fallback.

---

## Findings explicitly NOT re-reported

These were already addressed in the recent fix series and are
**deliberately omitted** from this report:

- npm global prefix location and probe parity (dadef3f07)
- `CapabilityStatus::Stale` written-but-never-read (partially — `Stale`
  as a *write* target via `migrate_from_legacy` was wired into warmup;
  the dead-field aspect of `source`/`last_probed` is a residual of the
  same class, listed above as a separate finding)
- 7 dead variants/arms/fields removed (fd06300cc)
- `migrate_from_legacy` into warmup (fd06300cc)
- Lock guard under ABI for `PATH_LOCK` (901678a50 — unused imports)
- `Bootstrapping`-stuck-on-dispatcher-error (a25745e26)

---

## Notes on code health

**Strong points:**

- Process-wide serialization on `PATH` mutation is well-designed
  (two-layer mutex: test-level `PATH_ENV`, runtime-level `PATH_LOCK`).
- `revalidate_ready` correctly handles the dangling-symlink case with a
  documented test (`revalidate_treats_a_dangling_symlink_as_gone`).
- `migrate_from_legacy` is robust to either/both file presence.
- Atomic temp-file persist (`ledger.rs:300-318`) with `pid+seq` suffix
  handles concurrent persisters across the gateway handlers, warmup,
  CLI, and Playwright paths.
- `CapabilityLedger` re-exports a focused API surface; no dead
  re-exports.
- `install.rs:118` test enforces "no Via argv is a global npm" — a
  compile-time-ish guard against regressing the prefix-less shape.

**Fragile areas (worth monitoring):**

- The `runtime_error` ↔ `gateway/handlers/runtimes.rs` text-format
  contract (see [Medium] finding).
- Lock ordering with nested `ensure_capability_recursive` (see [Low]).
- External process discovery (`fnm list`, `fnm exec --using lts`) is
  based on shell command text rather than structured IO (see [Low]).
- The `CapabilitySource`/`last_probed`/`Stale` dead-data cluster (see
  [Low]).

---

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0     |
| High     | 0     |
| Medium   | 2     |
| Low      | 5     |
| **Total**| **7** |

| Category | Count |
|----------|-------|
| Seam (severed wire) | 1 |
| Logic               | 3 |
| Architecture        | 3 |

Files with findings:

| File | Findings |
|------|----------|
| `src/runtimes/ensure.rs` | M-1, L-4, L-6 |
| `src/runtimes/ledger.rs` | M-2 |
| `src/runtimes/probe.rs` | L-3 |
| `src/runtimes/post_install.rs` | L-5 |
| `src/runtimes/bootstrap.rs` | L-7 |