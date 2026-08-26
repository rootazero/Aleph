# Capability Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Aleph's 46 process-global capability handles declare themselves, be verified by a rule-derived census, and report at runtime whether they were installed — so "never installed" stops being indistinguishable from "installed with this value".

**Architecture:** Four layers, dependency-ordered. A correct production-source extractor (Phase 1) replaces the repo's `split("#[cfg(test)]")` idiom, which is blind to 276 of 1734 files. A `CapabilitySlot<T>` newtype (Phase 2) makes "write the value" and "stamp the roster" a single act, and gives boot's conditional-install `else` arms a place to say *why* (`decline(because)` — the Rust shape of Cordis's unsatisfied `inject`). A rule-derived source census (Phase 3) keeps the class closed. One diagnostics check (Phase 4) reports it, reusing the existing doctor battery so all four client faces inherit it for free.

**Tech Stack:** Rust 1.96 (MSRV 1.95), tokio, `std::sync::OnceLock`, `arc_swap`, `async_trait`, existing `alephcore::diagnostics` engine. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-24-capability-wiring-design.md`


> ### ⚠️ Correction — the count is 46, but NOT the specification's 46 (2026-08-24, after Task 6 and its fix round)
>
> Every "46" below that describes the roster size is again **46** — and that number is
> a trap, so read this before relying on it. Task 6's rule-derived census found the
> specification's 46 was wrong in **three** members. The numbers agree; the rosters do
> not. **The decomposition is the tell: the spec's 46 was 46 *written* handles; this
> one is 45 written + 1 first-caller-wins.**
>
> - **OUT** — `extension/template.rs::FILE_REF_REGEX`, a compiled-regex cache in a
>   zero-parameter fn. It was on the spec's roster only because `get_or_try_init` sat
>   in the writer set, which is where a fallible initialiser had to go before install
>   form 2 existed.
> - **IN** — `metrics/mod.rs::METRICS_RUNTIME`, a real handle no setter search saw,
>   because rustfmt put its `.set(` on the next line. Roster membership was a function
>   of line length.
> - **SAME, FOR A DIFFERENT REASON** — `providers/route_handle.rs::GLOBAL`, on the
>   spec's roster by a **name collision**: seven container statics in `src/` are called
>   `GLOBAL` and six are `.set(` in their own files, so a corpus-wide word-boundary
>   search cannot tell the seventh from them. It is now selected by derivation, so a
>   rename can no longer drop it silently.
>
> Arithmetic: spec's 46, −1 `FILE_REF_REGEX`, +1 `METRICS_RUNTIME` = 46. `GLOBAL` does
> not move the count — it was already counted, wrongly. **This is the third cancelling
> coincidence around this number in one task**, which is exactly why the disambiguation
> lives in the census's own assertion message and module doc, not only here.
>
> Authoritative: `src/capability/census.rs` and
> `.superpowers/sdd/2026-08-24-capability-wiring/capability-inventory.txt` (46 lines).
> Task 6's own step block below is kept verbatim and marked SUPERSEDED.
>
> A known gap is recorded in the census module doc: **interior-mutable installs** —
> a container built lazily with no argument and then filled *through* a guard — are
> invisible to both rule arms while having the full failure semantics. At least four
> instances, including both gateway fan-out registries and the **security audit trail**
> (`security/audit.rs::GLOBAL_AUDIT`, which has no `OnceLock` at all). Unfixed this
> round; direction is under-see.

## Global Constraints

- **Branch isolation.** All implementation commits land on worktree branch `capability-wiring`. `main` receives no implementation commits.
- **No new dependencies.** `linkme` / `inventory` are explicitly rejected (spec §2 non-goal 7). Roster completeness is enforced by source census.
- **`src/harness/` is untouched.** Zero lines added or removed; `src/harness/tests/budget.rs::CEILING` unchanged (spec §2 non-goal 3).
- **No new client surface.** No new RPC method, no new tool, no Panel/TUI/CLI code (spec §2 non-goal 6). The doctor battery is shared; the four faces inherit.
- **No crate split, no boot reordering.** `alephcore` stays one crate; boot stays a hand-sequenced script (spec §2 non-goals 1–2).
- **Consumers are not rewritten.** The 9 `global_session_service()` consumers keep their `None` handling until Task 15 adjudicates each one individually (spec §2 non-goal 5).
- **Every new guard is manually falsified once** before its task is committed, using this classifier **in this order** (spec §6):
  `running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → anything else (no `test result:` line) ⇒ BUILD-ERROR.
  cargo prints `^error:` for test *failures* too, and prints `0 passed` for a genuine RED — classify by the line that only one outcome can print.
- **Minimum trusted verification set** (spec §6) — `cargo check -p alephcore` alone is NOT verification:
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
  cargo clippy --all-targets
  cargo test -p aleph-tui -p aleph-cli
  ```
- **Commit message format:** `<scope>: <description>`, English, e.g. `utils: replace the cfg(test) prefix cut with item-aware extraction`.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/utils/source_scan.rs` **(create)** | `production_prefix()` + `strip_comment_lines()` — the only implementation of "which half of this file is production code". |
| `src/utils/mod.rs` **(modify)** | Declare `pub mod source_scan;`. |
| `src/capability/mod.rs` **(create)** | `MissingSemantics`, `Outcome`, `SlotStatus`, `CapabilitySlot<T>`, `MutableCapabilitySlot<T>`, `ALL_SLOTS`. |
| `src/capability/census.rs` **(create)** | The membership rule + the two source-level guards. |
| `src/lib.rs` **(modify)** | Declare `pub mod capability;` (alphabetically after `canvas`). |
| `src/diagnostics/checks/capability_wiring.rs` **(create)** | The `core/capability-wiring` health check. |
| `src/diagnostics/checks/mod.rs` **(modify)** | Declare + re-export `CapabilityWiringCheck`. |
| `src/diagnostics/mod.rs` **(modify)** | Register the check in `default_registry()`. |
| `src/gateway/shutdown_forensics.rs` **(modify)** | Add `booted() -> bool`; migrate `BOOT_INSTANT` to a slot. |
| 46 handle-owning modules **(modify)** | Replace the bare `static` with a slot; keep `set_*` / `global_*` as `#[inline]` wrappers. |
| ~20 census-guard modules **(modify)** | Replace hand-rolled `split("#[cfg(test)]")` with `production_prefix()`. |
| `docs/superpowers/plans/2026-08-24-capability-wiring-triage.md` **(create)** | Triage ledger for REDs surfaced by Task 3. |

---

## Task 0: Create the isolated worktree

**Files:** none (git only)

**Interfaces:**
- Consumes: nothing
- Produces: a worktree at `../Aleph-capability-wiring` on branch `capability-wiring`; every later task runs there

- [ ] **Step 1: Create the worktree**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git worktree add ../Aleph-capability-wiring -b capability-wiring
cd ../Aleph-capability-wiring
```

- [ ] **Step 2: Verify the submodules are populated**

```bash
git submodule update --init --recursive
ls skills/ plugins/ | head
```

Expected: both directories non-empty. `include_dir!` is a compile-time macro — an empty `skills/` or `plugins/` fails the build in a way that looks unrelated. A fresh worktree does NOT populate submodules automatically.

- [ ] **Step 3: Establish the pre-existing baseline**

```bash
cargo test -p alephcore --lib --no-run 2>&1 | tail -5
cargo test -p alephcore --lib 2>&1 | tail -3 | tee /tmp/baseline-lib.txt
```

Record the pass/fail counts. **Any test already failing here is pre-existing** and must not be attributed to this round. Save the numbers — Task 3 compares against them.

- [ ] **Step 4: Commit nothing**

No commit. This task only establishes the workspace and baseline.

---

## Task 1: `production_prefix()` — item-aware production extraction

**Files:**
- Create: `src/utils/source_scan.rs`
- Modify: `src/utils/mod.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/utils/source_scan.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub fn production_prefix(src: &str) -> String`
  - `pub fn strip_comment_lines(src: &str) -> String`

- [ ] **Step 1: Write the failing tests**

Create `src/utils/source_scan.rs` with only the test module and stub signatures:

```rust
//! The production half of a Rust source file, for source-level census guards.

/// Placeholder so the test module compiles; replaced in step 3.
pub fn production_prefix(_src: &str) -> String {
    unimplemented!()
}

/// Placeholder so the test module compiles; replaced in step 3.
pub fn strip_comment_lines(_src: &str) -> String {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the old `split("#[cfg(test)]")` handles correctly: one
    /// trailing test module and nothing after it.
    #[test]
    fn trailing_test_module_is_removed() {
        let src = "pub fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn a()"));
        assert!(!out.contains("mod tests"));
        assert!(!out.contains("fn t()"));
    }

    /// The 203-file class: a mid-file test item, with production code AFTER
    /// it. The old prefix cut discarded everything from the attribute on.
    #[test]
    fn production_after_a_mid_file_test_item_survives() {
        let src = "pub fn before() {}\n\
                   #[cfg(test)]\n\
                   pub(crate) static GUARD: Mutex<()> = Mutex::new(());\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn before()"));
        assert!(
            out.contains("pub fn after()"),
            "production after a mid-file #[cfg(test)] item must survive; got:\n{out}"
        );
        assert!(!out.contains("GUARD"));
    }

    /// The 73-file class: `#[cfg(test)] mod tests;` at the top of the file.
    /// The old prefix cut discarded the ENTIRE file.
    #[test]
    fn top_of_file_test_module_declaration_does_not_eat_the_file() {
        let src = "#[cfg(test)]\nmod tests;\n\npub fn everything() {}\n";
        let out = production_prefix(src);
        assert!(
            out.contains("pub fn everything()"),
            "a `#[cfg(test)] mod tests;` declaration must not discard the file; got:\n{out}"
        );
        assert!(!out.contains("mod tests;"));
    }

    /// A brace inside a string literal must not be counted, or the item skip
    /// runs off the end and eats the rest of the file.
    #[test]
    fn braces_inside_string_literals_do_not_confuse_the_skip() {
        let src = "#[cfg(test)]\n\
                   mod tests {\n\
                       const S: &str = \"unbalanced { brace\";\n\
                   }\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("unbalanced"));
    }

    /// CRLF checkouts are real on Windows; a `\n`-anchored scan matches
    /// nothing there and the guard silently covers the test module too.
    #[test]
    fn crlf_input_is_handled() {
        let src = "pub fn a() {}\r\n#[cfg(test)]\r\nmod tests {\r\n    fn t() {}\r\n}\r\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn a()"));
        assert!(!out.contains("fn t()"));
    }

    #[test]
    fn strip_comment_lines_drops_line_and_block_comment_lines() {
        let src = "// a doc mention of foo()\npub fn real() {}\n/* block */\n * continued\n";
        let out = strip_comment_lines(src);
        assert!(out.contains("pub fn real()"));
        assert!(!out.contains("doc mention"));
        assert!(!out.contains("block"));
        assert!(!out.contains("continued"));
    }
}
```

Add to `src/utils/mod.rs`, alphabetically between `pub mod sqlite_open;` and `pub mod text_format;`:

```rust
pub mod source_scan;
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib utils::source_scan 2>&1 | tail -20
```

Expected: RED — `test result: FAILED`, every test panicking at `unimplemented!()`. If you see `running 0 tests`, the module was not declared (VACUOUS, not RED) — fix `src/utils/mod.rs` first.

- [ ] **Step 3: Write the implementation**

Replace the two stubs in `src/utils/source_scan.rs`:

```rust
//! The production half of a Rust source file, for source-level census guards.
//!
//! # Why this is not `src.split("#[cfg(test)]").next()`
//!
//! That idiom cuts at the *first place a test attribute appears*, which is not
//! a boundary. Measured on this repo (1734 files carrying the attribute):
//! 1458 have one trailing `mod tests {` and are cut correctly; **73** open with
//! `#[cfg(test)] mod tests;` and lose the ENTIRE file; **203** carry a mid-file
//! test item and are truncated arbitrarily. `src/utils/paths.rs` declares a
//! test-only mutex at 5% of the file, so 95% of it was invisible to every
//! guard using the prefix cut — and `src/spend/mod.rs` (the anchor of the
//! §5.22 round-7 capability-handle fix) was cut at byte 2,024 of 30,470.
//!
//! `\r` is dropped first: this repo is checked out CRLF on Windows, where a
//! `\n`-anchored separator matches nothing and the scan silently covers the
//! test module too.

/// The production half of a Rust source file.
///
/// Removes each `#[cfg(test)]`-attributed *item* (by brace matching, or to the
/// terminating `;` for one-line items) and each `#[cfg(test)] mod <name>;`
/// declaration, keeping everything else — including production code that
/// follows a mid-file test item.
///
/// Deliberately orthogonal to [`strip_comment_lines`]: a guard decides for
/// itself whether comments are in scope. Most want both.
#[must_use]
pub fn production_prefix(src: &str) -> String {
    let normalized = src.replace('\r', "");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("#[cfg(test)]") {
            out.push(lines[i]);
            i += 1;
            continue;
        }
        // The attribute applies to the next non-blank line's item.
        let mut item = i + 1;
        while item < lines.len() && lines[item].trim().is_empty() {
            item += 1;
        }
        if item >= lines.len() {
            break; // dangling attribute at EOF
        }
        i = end_of_item(&lines, item);
    }
    out.join("\n")
}

/// Index of the first line AFTER the item beginning at `start`.
fn end_of_item(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut k = start;
    while k < lines.len() {
        let code = code_only(lines[k]);
        depth += i32::try_from(code.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(code.matches('}').count()).unwrap_or(0);
        if code.contains('{') {
            opened = true;
        }
        if opened && depth <= 0 {
            return k + 1;
        }
        // One-line item (`mod tests;`, `static X: T = v;`) — no block opened.
        if !opened && code.trim_end().ends_with(';') {
            return k + 1;
        }
        k += 1;
    }
    lines.len()
}

/// A line with line-comments and string/char literal *contents* removed, so
/// braces inside them are not counted by [`end_of_item`].
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_str = false;
    let mut in_char = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_str || in_char => escaped = true,
            '"' if !in_char => {
                in_str = !in_str;
            }
            '\'' if !in_str => {
                in_char = !in_char;
            }
            '/' if !in_str && !in_char && chars.peek() == Some(&'/') => break,
            _ if in_str || in_char => {}
            _ => out.push(c),
        }
    }
    out
}

/// Drop whole-line comments (`//`, `/*`, and continuation `*` lines).
///
/// A scanner judges code; a comment is documentation. A doc comment naming a
/// symbol is not a call site, and an explanatory comment describing a bug is
/// not the bug — this repo has been bitten in both directions.
#[must_use]
pub fn strip_comment_lines(src: &str) -> String {
    src.replace('\r', "")
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib utils::source_scan 2>&1 | tail -12
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/utils/source_scan.rs src/utils/mod.rs
git commit -m "utils: add item-aware production-source extraction

The `split(\"#[cfg(test)]\").next()` idiom cuts at the first test attribute,
which is not a boundary: of 1734 files carrying the attribute, 73 lose the
entire file and 203 are truncated mid-way."
```

---

## Task 2: The three self-guards for `production_prefix()`

**Files:**
- Modify: `src/utils/source_scan.rs` (extend the test module)

**Interfaces:**
- Consumes: `production_prefix` from Task 1
- Produces: nothing consumed by later tasks (guards only)

- [ ] **Step 1: Write the three guards**

Append inside `src/utils/source_scan.rs`'s `mod tests`:

```rust
    /// Walk every `.rs` file under `src/`, returning `(repo-relative path, text)`.
    fn all_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");
        files
            .into_iter()
            .filter_map(|file| {
                let rel = file
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                std::fs::read_to_string(&file).ok().map(|t| (rel, t))
            })
            .collect()
    }

    fn old_prefix_cut(src: &str) -> String {
        let src = src.replace('\r', "");
        src.split("#[cfg(test)]").next().unwrap_or(&src).to_string()
    }

    /// Guard 1 — no regression. Where the old cut was right, we agree with it.
    ///
    /// "Agree" is checked on the retained *code*, not byte-for-byte text: the
    /// old cut keeps the blank lines and the attribute's leading whitespace
    /// that preceded the test module, which carry no meaning to any scanner.
    #[test]
    fn production_prefix_agrees_with_the_old_cut_where_the_old_cut_was_right() {
        let mut compared = 0usize;
        for (rel, text) in all_sources() {
            let old = old_prefix_cut(&text);
            // "old cut was right" == nothing but whitespace follows the test
            // module, i.e. the new extractor found no extra code.
            let new = production_prefix(&text);
            if new.split_whitespace().eq(old.split_whitespace()) {
                compared += 1;
                continue;
            }
            assert!(
                new.len() >= old.trim_end().len(),
                "{rel}: new extraction is SHORTER than the old prefix cut — the \
                 extractor is dropping production code"
            );
        }
        assert!(
            compared > 1_000,
            "expected >1000 files where old and new agree, saw {compared} — either the \
             extractor regressed or the corpus changed shape; investigate, do not relax"
        );
    }

    /// Guard 2 — real expansion. The 276-file class must actually recover code.
    ///
    /// The count is asserted because a shrinking census and a broken census
    /// look identical in a passing report.
    #[test]
    fn production_prefix_recovers_code_the_old_cut_discarded() {
        let mut recovered = 0usize;
        let mut worst = (0usize, String::new());
        for (rel, text) in all_sources() {
            let old = old_prefix_cut(&text).trim_end().len();
            let new = production_prefix(&text).trim_end().len();
            if new > old {
                recovered += 1;
                if new - old > worst.0 {
                    worst = (new - old, rel);
                }
            }
        }
        assert!(
            recovered >= 250,
            "expected >=250 files to recover production code (measured 276 on \
             2026-08-24); saw {recovered}. A drop means the extractor stopped \
             recognising a shape — investigate before lowering this floor."
        );
        assert!(worst.0 > 10_000, "worst-case recovery {worst:?} is implausibly small");
    }

    /// Guard 3 — no second author. The rule, not an exemption list.
    #[test]
    fn no_module_hand_rolls_the_cfg_test_prefix_cut() {
        let mut offenders = Vec::new();
        for (rel, text) in all_sources() {
            if rel == "src/utils/source_scan.rs" {
                continue; // defines the replacement and tests the old shape
            }
            for (n, line) in strip_comment_lines(&text).lines().enumerate() {
                if line.contains(r#"split("#[cfg(test)]")"#)
                    || line.contains(r#"find("#[cfg(test)]")"#)
                    || line.contains(r#"split_once("#[cfg(test)]")"#)
                {
                    offenders.push(format!("{rel}:{}", n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these hand-roll the production-prefix cut instead of calling \
             `utils::source_scan::production_prefix`:\n  {}",
            offenders.join("\n  ")
        );
    }
```

- [ ] **Step 2: Run the guards — expect guard 3 RED**

```bash
cargo test -p alephcore --lib utils::source_scan 2>&1 | tail -30
```

Expected: guards 1 and 2 PASS, guard 3 **RED** listing ~20 offenders. That RED is the work item for Task 3 — do not weaken the guard.

- [ ] **Step 3: Falsify guards 1 and 2 manually**

```bash
# Break guard 2: make the extractor behave like the old cut.
sed -i.bak 's/if !lines\[i\].trim_start().starts_with("#\[cfg(test)\]") {/if false {/' src/utils/source_scan.rs
cargo test -p alephcore --lib utils::source_scan::tests::production_prefix_recovers 2>&1 | tail -8
mv src/utils/source_scan.rs.bak src/utils/source_scan.rs
```

Expected classification: `test result: FAILED` ⇒ **RED**, and the message must name the shortfall. If you get `running 0 tests` that is VACUOUS (wrong test filter), not a passing guard.

- [ ] **Step 4: Re-run to confirm restoration**

```bash
cargo test -p alephcore --lib utils::source_scan 2>&1 | tail -8
```

Expected: same as step 2 (guards 1–2 green, guard 3 red).

- [ ] **Step 5: Commit**

```bash
git add src/utils/source_scan.rs
git commit -m "utils: guard the production-source extractor against regression and second authors

Guard 3 is RED by design until the ~20 hand-rolled prefix cuts are migrated."
```

---

## Task 3: Migrate the hand-rolled prefix cuts, then triage what they newly see

**Files:**
- Modify: every file listed by Task 2's guard 3 (~20), including
  `src/executor/builtin_registry/dispatchable.rs:59`,
  `src/tasks/cron/mod.rs:971`, `src/tools/usage/store.rs:476`,
  `src/memory/notes/ingest/apply.rs:1364`,
  `src/bin/aleph-server/commands/start/{helpers.rs:573,mod.rs:3525}`,
  `src/providers/metering.rs:650`, `src/agents/subagent_spawner/fork/tests.rs:124`,
  `src/agents/subagent_tool/loop_tool.rs:2074`, `src/browser/testkit.rs:397`,
  `src/sandbox/worktree.rs:655`, `src/teams/mod.rs:82`,
  `src/orchestrator/tests/loader.rs:{78,103}`, `src/builtin_tools/plugin_manage.rs:685`,
  `src/builtin_tools/memory_search.rs:831`, `src/builtin_tools/browser_tools/{mod.rs:1003,tabs.rs:382,wait_for.rs:330}`,
  `src/gateway/session_snapshot.rs:198`
- Create: `docs/superpowers/plans/2026-08-24-capability-wiring-triage.md`

**Interfaces:**
- Consumes: `utils::source_scan::{production_prefix, strip_comment_lines}`
- Produces: a triage ledger consumed by Task 13

- [ ] **Step 1: Get the authoritative offender list**

```bash
cargo test -p alephcore --lib utils::source_scan::tests::no_module_hand_rolls 2>&1 \
  | sed -n '/hand-roll/,/^$/p'
```

Use this output, not the list above — the file may have moved since the plan was written.

- [ ] **Step 2: Migrate each offender**

For each site, replace the local helper body with a call. Example — `src/executor/builtin_registry/dispatchable.rs`, whose local helper is `production_source`:

```rust
// BEFORE
fn production_source(src: &str) -> String {
    let src = src.replace('\r', "");
    let body = src.split("#[cfg(test)]").next().unwrap_or(&src).to_string();
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// AFTER
fn production_source(src: &str) -> String {
    crate::utils::source_scan::strip_comment_lines(&crate::utils::source_scan::production_prefix(src))
}
```

Keep each local wrapper name — the call sites do not change, and the wrapper's existing doc comment records *why* that guard wants comments stripped. Where a site inlined the cut without a helper, call `production_prefix` directly.

- [ ] **Step 3: Run guard 3 to verify it is now green**

```bash
cargo test -p alephcore --lib utils::source_scan 2>&1 | tail -8
```

Expected: `test result: ok. 9 passed`.

- [ ] **Step 4: Run the full lib suite and diff against the Task 0 baseline**

```bash
cargo test -p alephcore --lib 2>&1 | tail -40 | tee /tmp/after-task3.txt
diff <(grep -oE '^test [a-z_:]+' /tmp/baseline-lib.txt | sort) \
     <(grep -oE '^test [a-z_:]+' /tmp/after-task3.txt | sort) || true
```

Every newly-failing test is a guard that just saw code it was structurally blind to. **These predate this round.**

- [ ] **Step 5: Write the triage ledger — do NOT fix here**

Create `docs/superpowers/plans/2026-08-24-capability-wiring-triage.md`:

```markdown
# Task 3 triage — guards that newly see previously invisible code

Each row predates this round: the guard existed, the code existed, the guard
could not read it. Verdicts are CONNECT (wire it), CUT (delete the dead
abstraction), or REPORT (needs a human decision — do not guess).

| # | Failing test | File it newly reads | What it found | Verdict | Task |
|---|---|---|---|---|---|
| 1 | _(fill from step 4)_ | | | | |
```

Fill one row per newly-failing test. If step 4 produced no new failures, write
that explicitly with the command output — a silent empty ledger and an unrun
diff look identical.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "census: route every source-level guard through the shared production extractor

Guards previously blind to 276 of 1734 files now read them. Newly-failing
guards are triaged in docs/superpowers/plans/2026-08-24-capability-wiring-triage.md
and fixed individually in Task 13 — none are regressions from this change."
```

---

## Task 4: `CapabilitySlot<T>` core types

**Files:**
- Create: `src/capability/mod.rs`
- Modify: `src/lib.rs`
- Test: inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub enum MissingSemantics { IndistinguishableDefault { reads_as: &'static str }, ConsumerDecides, FailsClosed, FailsOpen }`
  - `pub enum Outcome { Installed, Declined { because: &'static str } }`
  - `pub trait SlotStatus: Sync { fn id(&self) -> &'static str; fn missing(&self) -> MissingSemantics; fn outcome(&self) -> Option<&Outcome>; }`
    (`MissingSemantics` is `Copy`, so this returns by value — no lifetime gymnastics, no `unsafe`.)
  - `pub struct CapabilitySlot<T: 'static>` with
    `const fn new(&'static str, MissingSemantics) -> Self`,
    `fn install(&'static self, v: T) -> bool`,
    `fn decline(&'static self, because: &'static str)`,
    `#[inline] fn get(&self) -> Option<&T>`

- [ ] **Step 1: Write the failing tests**

Create `src/capability/mod.rs`:

```rust
//! Process-global capability handles that can say whether they were installed.

#[cfg(test)]
mod tests {
    use super::*;

    static UNSET: CapabilitySlot<u32> =
        CapabilitySlot::new("test/unset", MissingSemantics::ConsumerDecides);
    static INSTALLED: CapabilitySlot<u32> =
        CapabilitySlot::new("test/installed", MissingSemantics::FailsOpen);
    static DECLINED: CapabilitySlot<u32> = CapabilitySlot::new(
        "test/declined",
        MissingSemantics::IndistinguishableDefault { reads_as: "0" },
    );

    #[test]
    fn an_untouched_slot_reports_no_outcome_at_all() {
        // The distinction this whole type exists for: "nobody reached it" is
        // NOT "installed", and it is NOT "declined".
        assert!(UNSET.get().is_none());
        assert!(UNSET.outcome().is_none());
    }

    #[test]
    fn install_writes_the_value_and_stamps_the_roster_in_one_act() {
        assert!(INSTALLED.install(7));
        assert_eq!(INSTALLED.get(), Some(&7));
        assert!(matches!(INSTALLED.outcome(), Some(Outcome::Installed)));
    }

    #[test]
    fn install_is_idempotent_like_the_setters_it_replaces() {
        static S: CapabilitySlot<u32> =
            CapabilitySlot::new("test/idem", MissingSemantics::FailsClosed);
        assert!(S.install(1));
        assert!(!S.install(2), "second install must be a no-op returning false");
        assert_eq!(S.get(), Some(&1));
    }

    #[test]
    fn decline_records_why_and_leaves_the_value_unset() {
        DECLINED.decline("state database absent: [gateway] state_db unset");
        assert!(DECLINED.get().is_none());
        match DECLINED.outcome() {
            Some(Outcome::Declined { because }) => {
                assert!(because.contains("state_db"));
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    #[test]
    fn slot_status_erases_the_type_for_the_roster() {
        let erased: &'static dyn SlotStatus = &UNSET;
        assert_eq!(erased.id(), "test/unset");
        assert!(matches!(erased.missing(), MissingSemantics::ConsumerDecides));
    }
}
```

Add to `src/lib.rs`, alphabetically between `pub mod canvas;` and `pub mod clarification;`:

```rust
pub mod capability;
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib capability:: 2>&1 | tail -20
```

Expected: BUILD-ERROR (`cannot find type CapabilitySlot`). That is the correct pre-implementation state for a type-introducing task; it is not VACUOUS.

- [ ] **Step 3: Write the implementation**

Prepend to `src/capability/mod.rs` (above the test module):

```rust
//! Process-global capability handles that can say whether they were installed.
//!
//! # The problem
//!
//! A bare `static X: OnceLock<Arc<T>>` plus `install_x()` plus `x()` cannot
//! distinguish "boot never installed this" from "boot installed exactly this
//! value" — §5.22 round-7 recorded the shape on `spend`: `spend.query` reports
//! `configured: false`, which is a true statement about a box with no ceiling
//! AND a true statement about a box that configured one whose handle was never
//! installed. That round fixed two handles by hand. There are 46.
//!
//! # The shape
//!
//! [`CapabilitySlot::install`] writes the value **and** stamps the outcome in
//! one act: a caller that cannot reach the inner `OnceLock` cannot forget the
//! stamp. This is the `MetaGuard` idiom (make the correct thing the only
//! constructible thing), not a "remember to also call `mark()`" discipline —
//! that discipline fails in exactly the shape this type prevents, and its
//! failure mode is a *confident lie* ("not installed" about an installed
//! handle), which is worse than today's silence.
//!
//! [`CapabilitySlot::decline`] is the other half and the reason this round
//! exists: boot's conditional-install `else` arms now have somewhere to say
//! **why**. That is deepseek-harness/Cordis's unsatisfied `static inject`
//! ("waiting for: sessionPersistence") in Rust's shape — no plugin tree, no
//! topological boot, just the sentence a reader needs.

use std::sync::OnceLock;

/// What a read observes when this capability was NEVER installed.
///
/// ⚠️ Membership in the roster is decided by THIS — the failure direction —
/// not by the handle's type or its name. A handle belongs iff losing it yields
/// a *wrong answer* rather than a crash. The 63 lazy caches in `src/` cannot
/// write an honest variant here ("not built yet" is not a wrong answer), which
/// is why the Task 6 rule excludes them by derivation rather than by a
/// hand-written exclusion list that would rot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSemantics {
    /// A read yields a legal-looking value and no caller can tell.
    /// (`spend` policy reads as "no ceiling" — the round-7 shape.)
    IndistinguishableDefault { reads_as: &'static str },
    /// A read yields `None` and every consumer decides for itself what that
    /// means. (`GLOBAL_SESSION_SERVICE`: 9 consumers, one silently returns.)
    ConsumerDecides,
    /// Fails closed — safe, but the feature is dead and says nothing.
    FailsClosed,
    /// Fails OPEN — a gate silently stops gating.
    FailsOpen,
}

/// What boot did about this slot, when boot reached it at all.
///
/// `None` (no outcome recorded) is a third state and is NOT `Declined`: it
/// means nothing ever reached this slot — either this process did not boot, or
/// boot died before getting here. Collapsing the two is the mistake this type
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    /// Boot reached this slot and could not install it. `because` is shown to
    /// operators verbatim, so name the missing input, not the symptom.
    Declined { because: &'static str },
}

/// Type-erased view of a slot, for the roster and the diagnostics check.
pub trait SlotStatus: Sync {
    fn id(&self) -> &'static str;
    /// By value: `MissingSemantics` is `Copy`, which keeps this trait free of
    /// lifetime gymnastics and of any `unsafe`.
    fn missing(&self) -> MissingSemantics;
    fn outcome(&self) -> Option<&Outcome>;
}

/// Install-once capability handle. Replaces a bare `static X: OnceLock<T>`.
pub struct CapabilitySlot<T: 'static> {
    id: &'static str,
    missing: MissingSemantics,
    value: OnceLock<T>,
    outcome: OnceLock<Outcome>,
}

impl<T: 'static> CapabilitySlot<T> {
    #[must_use]
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self {
        Self { id, missing, value: OnceLock::new(), outcome: OnceLock::new() }
    }

    /// Install the value and stamp the roster. Returns `false` when already
    /// installed — the same idempotence the `set_*` / `init_*` functions this
    /// replaces already promised in their doc comments.
    pub fn install(&'static self, v: T) -> bool {
        let fresh = self.value.set(v).is_ok();
        if fresh {
            let _ = self.outcome.set(Outcome::Installed);
        }
        fresh
    }

    /// Record that boot reached this slot and could not install it.
    ///
    /// First writer wins, mirroring `install`: a slot is decided once.
    pub fn decline(&'static self, because: &'static str) {
        let _ = self.outcome.set(Outcome::Declined { because });
    }

    /// Read the installed value.
    ///
    /// This is `OnceLock::get()` and nothing else — the stamp is written only
    /// on the `install`/`decline` side, so migrating a hot handle onto this
    /// type does not add a branch or an atomic to any read.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    #[must_use]
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}

impl<T: Send + Sync + 'static> SlotStatus for CapabilitySlot<T> {
    fn id(&self) -> &'static str {
        self.id
    }
    fn missing(&self) -> MissingSemantics {
        self.missing
    }
    fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}
```

⚠️ `missing()` returns `MissingSemantics` **by value**. The enum is `Copy`, so this needs no lifetime and no `unsafe`; `Task 5`, `Task 8`, and `Task 12` all assume this exact signature.

- [ ] **Step 4: Run tests and clippy**

```bash
cargo test -p alephcore --lib capability:: 2>&1 | tail -12
cargo clippy -p alephcore --all-targets 2>&1 | grep -A5 'capability' | head -30
```

Expected: `test result: ok. 5 passed`, zero clippy warnings in `src/capability/`.

- [ ] **Step 5: Commit**

```bash
git add src/capability/mod.rs src/lib.rs
git commit -m "capability: add CapabilitySlot, the install-once handle that records its outcome

install() writes the value and stamps the roster in one act; decline(because)
gives boot's conditional-install else arms somewhere to say what was missing."
```

---

## Task 5: `MutableCapabilitySlot<T>` for the one live-swap handle

**Files:**
- Modify: `src/capability/mod.rs`

**Interfaces:**
- Consumes: `MissingSemantics`, `Outcome`, `SlotStatus` from Task 4
- Produces: `pub struct MutableCapabilitySlot<T: 'static>` with
  `const fn new`, `fn install(&'static self, v: T) -> bool`,
  `fn update(&'static self, v: T) -> bool`, `fn decline(&'static self, &'static str)`,
  `#[inline] fn load(&self) -> Option<arc_swap::Guard<std::sync::Arc<T>>>`

- [ ] **Step 1: Write the failing tests**

Append to `src/capability/mod.rs`'s `mod tests`:

```rust
    #[test]
    fn update_before_install_returns_false_and_changes_nothing() {
        static M: MutableCapabilitySlot<u32> =
            MutableCapabilitySlot::new("test/mut-unset", MissingSemantics::FailsOpen);
        // This is spend::update_policy's EXISTING contract: the live-apply
        // verdict downgrades to Restart when no handle has been installed yet.
        assert!(!M.update(5), "update on an uninstalled slot must report false");
        assert!(M.load().is_none());
        assert!(M.outcome().is_none());
    }

    #[test]
    fn install_then_update_swaps_the_value_and_keeps_the_stamp() {
        static M: MutableCapabilitySlot<u32> =
            MutableCapabilitySlot::new("test/mut-live", MissingSemantics::FailsOpen);
        assert!(M.install(1));
        assert_eq!(**M.load().expect("installed"), 1);
        assert!(M.update(2));
        assert_eq!(**M.load().expect("installed"), 2);
        assert!(matches!(M.outcome(), Some(Outcome::Installed)));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib capability::tests::update 2>&1 | tail -10
cargo test -p alephcore --lib capability::tests::install_then_update 2>&1 | tail -10
```

Expected: BUILD-ERROR (`cannot find type MutableCapabilitySlot`).

- [ ] **Step 3: Write the implementation**

Append to `src/capability/mod.rs` (above the test module):

```rust
/// Install-once, then live-swap. Exactly one member today:
/// `spend::GLOBAL_POLICY` (`OnceLock<ArcSwap<SpendPolicy>>`, hot-applied by
/// the config live-reload path).
///
/// `update` returning `false` when nothing was installed is an EXISTING
/// contract, not a new one: `spend::update_policy` feeds the live-apply
/// verdict's honest downgrade to `Restart`. It is preserved exactly.
///
/// ⚠️ If migration finds this type has no second member and `spend` could use
/// `CapabilitySlot<ArcSwap<T>>` directly, delete it (R10 YAGNI withdrawal) —
/// it exists to carry an existing handle, not to reserve a shape.
pub struct MutableCapabilitySlot<T: 'static> {
    id: &'static str,
    missing: MissingSemantics,
    value: OnceLock<arc_swap::ArcSwap<T>>,
    outcome: OnceLock<Outcome>,
}

impl<T: 'static> MutableCapabilitySlot<T> {
    #[must_use]
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self {
        Self { id, missing, value: OnceLock::new(), outcome: OnceLock::new() }
    }

    pub fn install(&'static self, v: T) -> bool {
        let fresh = self.value.set(arc_swap::ArcSwap::from_pointee(v)).is_ok();
        if fresh {
            let _ = self.outcome.set(Outcome::Installed);
        }
        fresh
    }

    /// Hot-apply a new value. `false` means no handle has been installed yet.
    pub fn update(&'static self, v: T) -> bool {
        match self.value.get() {
            Some(cell) => {
                cell.store(std::sync::Arc::new(v));
                true
            }
            None => false,
        }
    }

    pub fn decline(&'static self, because: &'static str) {
        let _ = self.outcome.set(Outcome::Declined { because });
    }

    #[inline]
    pub fn load(&self) -> Option<arc_swap::Guard<std::sync::Arc<T>>> {
        self.value.get().map(arc_swap::ArcSwap::load)
    }

    #[must_use]
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}

impl<T: Send + Sync + 'static> SlotStatus for MutableCapabilitySlot<T> {
    fn id(&self) -> &'static str {
        self.id
    }
    fn missing(&self) -> MissingSemantics {
        self.missing
    }
    fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}
```

Both `SlotStatus` impls are identical in shape — `missing()` returns by value.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib capability:: 2>&1 | tail -10
```

Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/capability/mod.rs
git commit -m "capability: add MutableCapabilitySlot for the one install-then-swap handle"
```

---

## Task 6: The membership rule, as a listing test

**Files:**
- Create: `src/capability/census.rs`
- Modify: `src/capability/mod.rs` (declare `pub(crate) mod census;`)

**Interfaces:**
- Consumes: `utils::source_scan::{production_prefix, strip_comment_lines}`
- Produces:
  - `pub(crate) fn capability_handles() -> Vec<HandleSite>` where
    `pub(crate) struct HandleSite { pub file: String, pub name: String, pub container: String, pub is_slot: bool }`
  - The authoritative inventory that Tasks 7–10 migrate and Task 11 closes.

- [ ] **Step 1: Write the listing test**

Create `src/capability/census.rs`:

```rust
//! The membership rule for capability handles, and the guards that close it.
//!
//! # The rule (derived, never a hand-written list)
//!
//! A `static` of an install-once container (`OnceLock` / `OnceCell` /
//! `ArcSwap*`) is a **capability handle** iff something writes it
//! (`set` / `store` / `swap` / `get_or_try_init`). A container that is only
//! ever `get_or_init`-ed is a lazy cache: "not built yet" is the correct
//! answer there, so it cannot write an honest `MissingSemantics` and is
//! excluded by derivation, not by an exemption list.
//!
//! ⚠️ The type pattern MUST accept qualified paths (`std::sync::OnceLock`,
//! `once_cell::sync::OnceCell`, `arc_swap::ArcSwap`). A first pass that
//! matched only bare type names counted 29 boot handles where the true number
//! is 40 — and `spend::GLOBAL_LEDGER`, the anchor of the round-7 fix this
//! generalises, is written in the qualified form. A guard's green only covers
//! the shapes its recogniser knows.

#[cfg(test)]
pub(crate) struct HandleSite {
    pub file: String,
    pub name: String,
    pub container: String,
    pub is_slot: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};

    fn all_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");
        files
            .into_iter()
            .filter_map(|f| {
                let rel = f
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&f)
                    .to_string_lossy()
                    .replace('\\', "/");
                std::fs::read_to_string(&f).ok().map(|t| (rel, t))
            })
            .collect()
    }

    const CONTAINERS: &[&str] =
        &["OnceLock", "OnceCell", "ArcSwapOption", "ArcSwapAny", "ArcSwap"];

    /// Parse `static NAME : <maybe::qualified::>Container <`.
    fn parse_static_decl(line: &str) -> Option<(String, String)> {
        let t = line.trim_start();
        let rest = t.strip_prefix("static ")?;
        let (name, after) = rest.split_once(':')?;
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()) {
            return None;
        }
        // Drop any qualifying path segments before the type name.
        let ty = after.trim().split('<').next()?.trim();
        let last = ty.rsplit("::").next()?.trim();
        let container = CONTAINERS.iter().find(|c| **c == last)?;
        Some((name.to_string(), (*container).to_string()))
    }

    pub(crate) fn capability_handles() -> Vec<HandleSite> {
        let mut out = Vec::new();
        for (rel, text) in all_sources() {
            let prod = strip_comment_lines(&production_prefix(&text));
            for line in prod.lines() {
                let Some((name, container)) = parse_static_decl(line) else { continue };
                let written = ["set(", "store(", "swap(", "get_or_try_init("]
                    .iter()
                    .any(|m| prod.contains(&format!("{name}.{m}")));
                if !written {
                    continue; // lazy cache — excluded by the rule, not by a list
                }
                out.push(HandleSite {
                    file: rel.clone(),
                    name,
                    container,
                    is_slot: false,
                });
            }
            // Slots are declared with the newtype, not a raw container.
            for line in prod.lines() {
                let t = line.trim_start();
                if !t.starts_with("static ") {
                    continue;
                }
                if t.contains("CapabilitySlot<") || t.contains("MutableCapabilitySlot<") {
                    if let Some((name, _)) = t.strip_prefix("static ").and_then(|r| r.split_once(':')) {
                        out.push(HandleSite {
                            file: rel.clone(),
                            name: name.trim().to_string(),
                            container: "CapabilitySlot".into(),
                            is_slot: true,
                        });
                    }
                }
            }
        }
        out
    }

    /// The inventory this round migrates. Asserted, not printed: a census that
    /// silently shrinks and a census that stopped matching look identical.
    #[test]
    fn the_capability_handle_inventory_is_the_size_we_measured() {
        let sites = capability_handles();
        let raw = sites.iter().filter(|s| !s.is_slot).count();
        let slots = sites.iter().filter(|s| s.is_slot).count();
        eprintln!("--- capability handles: {raw} raw, {slots} slots ---");
        for s in sites.iter().filter(|s| !s.is_slot) {
            eprintln!("  RAW  {:14} {:32} {}", s.container, s.name, s.file);
        }
        assert_eq!(
            raw + slots,
            46,
            "the rule selected {} handles; 46 was measured on 2026-08-24 with this \
             exact algorithm. A different number means the corpus changed or the \
             recogniser did — investigate before editing this number.",
            raw + slots
        );
    }
}
```

Add to `src/capability/mod.rs`:

```rust
#[cfg(test)]
pub(crate) mod census;
```

- [ ] **Step 2: Run the listing test**

```bash
cargo test -p alephcore --lib capability::census -- --nocapture 2>&1 | tail -60
```

Expected: either PASS with 46 raw handles listed, or RED naming a different count. **If the count differs from 46, stop and investigate** — do not edit the constant. Capture the printed list; Tasks 7–10 migrate exactly these.

> **SUPERSEDED — Task 6 executed and the answer is 47, not 46.** This block is kept as written for the record. The rule found that the spec's 46 was *the right roster reached by the wrong reason* (`route_handle::GLOBAL` was in it only by a name collision with three unrelated `GLOBAL` statics) and that `metrics::METRICS_RUNTIME` was **excluded because rustfmt broke its writer across two lines** — roster membership was a function of line length. Authoritative count and inventory: `src/capability/census.rs` and `.superpowers/sdd/2026-08-24-capability-wiring/capability-inventory.txt`. See commit `66244e97b`.

- [ ] **Step 3: Save the inventory**

```bash
cargo test -p alephcore --lib capability::census -- --nocapture 2>&1 \
  | grep '^  RAW' > /tmp/capability-inventory.txt
wc -l /tmp/capability-inventory.txt
```

- [ ] **Step 4: Commit**

```bash
git add src/capability/census.rs src/capability/mod.rs
git commit -m "capability: derive the handle inventory from a rule, not a list

Accepts qualified type paths: a bare-name pattern counted 29 boot handles
where the true number is 40, and missed the round-7 anchor entirely."
```

---

## Task 7: Migrate the `spend` handles (the round-7 anchor)

**Files:**
- Modify: `src/spend/mod.rs:347,353,367,374` and the `update_policy` / `current_policy` / `global_ledger` readers
- Test: existing `spend` tests + a new outcome test

**Interfaces:**
- Consumes: `CapabilitySlot`, `MutableCapabilitySlot`, `MissingSemantics` (Tasks 4–5)
- Produces: the migration pattern every later batch copies

- [ ] **Step 1: Write the failing test**

Append to `src/spend/mod.rs`'s test module:

```rust
    #[test]
    fn the_policy_handle_reports_whether_it_was_installed() {
        // The §5.22 round-7 shape, now answerable: `configured: false` is a
        // true statement about an unconfigured box AND about a box whose
        // handle boot never installed. Only the outcome separates them.
        use crate::capability::SlotStatus;
        let erased: &dyn SlotStatus = &GLOBAL_POLICY;
        assert_eq!(erased.id(), "spend/policy");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p alephcore --lib spend::tests::the_policy_handle 2>&1 | tail -10
```

Expected: BUILD-ERROR — `GLOBAL_POLICY` is not a slot yet.

- [ ] **Step 3: Migrate both handles**

```rust
// BEFORE
static GLOBAL_LEDGER: std::sync::OnceLock<Arc<dyn SpendLedger>> = std::sync::OnceLock::new();
static GLOBAL_POLICY: std::sync::OnceLock<arc_swap::ArcSwap<SpendPolicy>> =
    std::sync::OnceLock::new();

// AFTER
use crate::capability::{CapabilitySlot, MissingSemantics, MutableCapabilitySlot};

static GLOBAL_LEDGER: CapabilitySlot<Arc<dyn SpendLedger>> = CapabilitySlot::new(
    "spend/ledger",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "an in-memory ledger that resets every restart",
    },
);

static GLOBAL_POLICY: MutableCapabilitySlot<SpendPolicy> = MutableCapabilitySlot::new(
    "spend/policy",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "SpendPolicy::default() — enabled() == false, i.e. no ceiling",
    },
);
```

Then the setters and readers become wrappers — the public signatures do not change:

```rust
pub fn install_ledger(ledger: Arc<dyn SpendLedger>) {
    let _ = GLOBAL_LEDGER.install(ledger);
}

pub fn install_policy(policy: crate::config::types::policies::SpendPolicy) {
    let _ = GLOBAL_POLICY.install(policy);
}

/// Unchanged contract: `false` means no handle has been installed yet, which
/// is what lets the live-apply verdict downgrade honestly to `Restart`.
pub fn update_policy(policy: crate::config::types::policies::SpendPolicy) -> bool {
    GLOBAL_POLICY.update(policy)
}
```

Adjust `global_ledger()` and `current_policy()` to read through `.get()` / `.load()`, preserving their existing lazy-fallback behaviour exactly. Do not change what they return.

- [ ] **Step 4: Run the spend suite**

```bash
cargo test -p alephcore --lib spend 2>&1 | tail -12
cargo test -p alephcore --lib capability::census 2>&1 | tail -6
```

Expected: spend tests green (same count as before plus 1); the census inventory now shows 44 raw + 2 slots.

- [ ] **Step 5: Commit**

```bash
git add src/spend/mod.rs
git commit -m "spend: migrate the policy and ledger handles onto CapabilitySlot

Generalises the round-7 fix: 'configured: false' can now be told apart from
'nobody ever installed the handle'."
```

---

## Task 8: Migrate the session / tools handles (batch B)

**Files:**
- Modify: `src/session/service.rs:60-78`, `src/session/store.rs` (`GLOBAL_EVENT_STORE`),
  `src/tools/result_store.rs:85` (`GLOBAL_STORE`), `src/tools/turn_budget.rs:85` (`GLOBAL_BUDGET`),
  `src/tools/in_flight.rs:56` (`GLOBAL_REGISTRY`), `src/tools/result_processing.rs:53` (`RESULT_BUDGET_CEILING`)

**Interfaces:**
- Consumes: `CapabilitySlot`, `MissingSemantics`
- Produces: unchanged public signatures — `set_global_session_service(Arc<dyn SessionService>)`,
  `global_session_service() -> Option<Arc<dyn SessionService>>`, and the five siblings

- [ ] **Step 1: Write the failing test**

Append to `src/session/service.rs` (replacing the existing empty test module, whose comment
already explains that `OnceLock` cannot be reset between tests — the outcome check does not
need resetting, so it is safe):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::SlotStatus;

    /// NOTE: OnceLock is process-global and cannot be reset between tests, so
    /// no round-trip install test lives here. Identity and semantics can be
    /// asserted without touching the value.
    #[test]
    fn the_session_service_handle_declares_that_consumers_decide() {
        let erased: &dyn SlotStatus = &GLOBAL_SESSION_SERVICE;
        assert_eq!(erased.id(), "session/service");
        assert!(matches!(
            erased.missing(),
            crate::capability::MissingSemantics::ConsumerDecides
        ));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p alephcore --lib session::service 2>&1 | tail -10
```

Expected: BUILD-ERROR.

- [ ] **Step 3: Migrate all six handles**

Pattern (shown for `session/service`; repeat for the other five):

```rust
// BEFORE
static GLOBAL_SESSION_SERVICE: OnceLock<Arc<dyn SessionService>> = OnceLock::new();

pub fn set_global_session_service(svc: Arc<dyn SessionService>) {
    let _ = GLOBAL_SESSION_SERVICE.set(svc);
}

pub fn global_session_service() -> Option<Arc<dyn SessionService>> {
    GLOBAL_SESSION_SERVICE.get().cloned()
}

// AFTER
use crate::capability::{CapabilitySlot, MissingSemantics};

/// `ConsumerDecides`: nine production call sites read this, and they do NOT
/// agree — `tools/scoped/dispatch.rs` silently returns on `None`, while
/// `builtin_tools/sessions/compact_tool.rs` turns it into an error. Whether
/// each of those is right is adjudicated in Task 15; what this variant records
/// is that a missing handle here produces *nine different wrong answers*, not
/// one.
static GLOBAL_SESSION_SERVICE: CapabilitySlot<Arc<dyn SessionService>> =
    CapabilitySlot::new("session/service", MissingSemantics::ConsumerDecides);

/// Idempotent: a second call is ignored. Unchanged.
#[inline]
pub fn set_global_session_service(svc: Arc<dyn SessionService>) {
    let _ = GLOBAL_SESSION_SERVICE.install(svc);
}

/// Fetch the process-wide `SessionService`, if one has been installed.
#[inline]
pub fn global_session_service() -> Option<Arc<dyn SessionService>> {
    GLOBAL_SESSION_SERVICE.get().cloned()
}
```

Assign each handle a `MissingSemantics` by asking *what a read observes when nobody installed it*:

| Handle | id | Variant |
|---|---|---|
| `session/service.rs::GLOBAL_SESSION_SERVICE` | `session/service` | `ConsumerDecides` |
| `session/store.rs::GLOBAL_EVENT_STORE` | `session/event-store` | `ConsumerDecides` |
| `tools/result_store.rs::GLOBAL_STORE` | `tools/result-store` | `FailsClosed` |
| `tools/turn_budget.rs::GLOBAL_BUDGET` | `tools/turn-budget` | `FailsOpen` |
| `tools/in_flight.rs::GLOBAL_REGISTRY` | `tools/in-flight` | `ConsumerDecides` |
| `tools/result_processing.rs::RESULT_BUDGET_CEILING` | `tools/result-budget-ceiling` | `IndistinguishableDefault { reads_as: "<compiled-in default ceiling>" }` ⚠️ **PLACEHOLDER, WRONG IN SUBSTANCE — Task 8 found it names a different constant from what `result_budget_ceiling()` actually falls back to. Derive `reads_as` from the accessor's real fallback, never from this cell.** |

⚠️ Read each handle's current fallback before choosing. If a reader does `unwrap_or(DEFAULT)`, it is `IndistinguishableDefault` and `reads_as` must quote the actual default. If a *gate* reads it and skips its check on `None`, it is `FailsOpen`. Do not copy the table without checking — the table is a starting hypothesis, and the whole point of this round is that these two are indistinguishable from outside.

- [ ] **Step 4: Run the affected suites**

```bash
cargo test -p alephcore --lib -- session:: tools::result_store tools::turn_budget tools::in_flight tools::result_processing 2>&1 | tail -14
cargo test -p alephcore --lib capability::census 2>&1 | tail -6
```

⚠️ **This command shipped without the `--` and could not run** — `cargo test`
accepts one positional TESTNAME, so the multi-filter form exits 1 with
`unexpected argument 'tools::result_store'` before compiling anything. Task 8 hit
it and worked around it; the `--` above is the fix. Left annotated rather than
silently corrected because later batches read this plan as a template, and the
shape that invites the error is worth seeing next to the shape that works. The
failure is loud, so the cost is a minute — the reason to record it is that a step
which has never been executed is worth knowing about.

Expected: all green; census shows 38 raw + 8 slots.

- [ ] **Step 5: Commit**

```bash
git add src/session src/tools
git commit -m "session,tools: migrate six capability handles onto CapabilitySlot"
```

---

## Task 9: Migrate the gateway handles (batch C)

**Files:**
- Modify: the ~19 gateway handles from `/tmp/capability-inventory.txt`, including
  `src/gateway/channel_policy.rs` (`CHANNEL_CONFIG_SNAPSHOT`),
  `src/gateway/i18n.rs` (`INSTALLED_LOCALE`),
  `src/gateway/runtime_footer.rs` (`GLOBAL_FOOTER_CONFIG`),
  `src/gateway/execution_engine/tool_service_builder.rs` (`CONFIG_APPROVAL_REQUESTER`, `CONFIRMATION_REQUESTER`, `MCP_TOOL_REGISTRY`),
  `src/gateway/execution_engine/concurrency_handle.rs` (`HANDLE`),
  `src/gateway/resume_coordinator.rs` (`GLOBAL_RESUME_COORDINATOR`),
  `src/gateway/security/shared_token.rs` (`GLOBAL_SHARED_TOKEN_MANAGER`),
  `src/gateway/handlers/channel.rs` (`TELEGRAM_TOOL_REGISTRY`),
  `src/gateway/codex_token_refresher.rs` (`GLOBAL`),
  `src/gateway/shutdown_forensics.rs` (`BOOT_INSTANT`)
- Test: one identity assertion per file, in that file's existing test module

**Interfaces:**
- Consumes: `CapabilitySlot`, `MissingSemantics`
- Produces: `pub fn booted() -> bool` in `src/gateway/shutdown_forensics.rs`, consumed by Task 12

- [ ] **Step 1: Write the failing test for the sentinel**

In `src/gateway/shutdown_forensics.rs`'s test module:

```rust
    /// `booted()` is how the wiring check tells "this process never ran boot"
    /// from "boot ran and left holes". Without it a cold `aleph-server doctor`
    /// would report every slot missing on a perfectly healthy machine.
    #[test]
    fn booted_is_false_before_mark_boot_and_true_after() {
        // Runs in a test binary that never calls `mark_boot`, so the negative
        // half is the meaningful assertion; `mark_boot` then flips it.
        assert!(!booted());
        mark_boot();
        assert!(booted());
    }
```

⚠️ This test mutates a process-global. Give it a unique name and accept that it must be the only test in the binary touching `BOOT_INSTANT` — grep to confirm before writing.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p alephcore --lib shutdown_forensics::tests::booted 2>&1 | tail -10
```

Expected: BUILD-ERROR — `booted` not found.

- [ ] **Step 3: Migrate the sentinel, then the rest of the batch**

```rust
// BEFORE
static BOOT_INSTANT: OnceLock<Instant> = OnceLock::new();

pub fn mark_boot() {
    let _ = BOOT_INSTANT.set(Instant::now());
}

// AFTER
use crate::capability::{CapabilitySlot, MissingSemantics};

/// `FailsClosed`: uptime is reported as unknown rather than wrong. This slot
/// is also the wiring check's process sentinel — see [`booted`].
static BOOT_INSTANT: CapabilitySlot<Instant> =
    CapabilitySlot::new("gateway/boot-instant", MissingSemantics::FailsClosed);

pub fn mark_boot() {
    let _ = BOOT_INSTANT.install(Instant::now());
}

/// True iff this process ran `aleph-server start` far enough to reach
/// `mark_boot` (the first statement after argv parsing).
///
/// The `core/capability-wiring` check keys its three-state verdict on this: a
/// cold `aleph-server doctor` process installs nothing, so reporting its empty
/// roster as a problem — or as a pass — would both be fiction.
#[must_use]
pub fn booted() -> bool {
    BOOT_INSTANT.get().is_some()
}
```

For each remaining gateway handle, apply the Task 8 pattern. Choose `MissingSemantics` by reading the reader, per the Task 8 step-3 warning. `CHANNEL_CONFIG_SNAPSHOT`, `INSTALLED_LOCALE` and `GLOBAL_FOOTER_CONFIG` have `unwrap_or_*` readers — they are `IndistinguishableDefault` and `reads_as` must quote the real default.

- [ ] **Step 4: Run the gateway suite**

```bash
cargo test -p alephcore --lib gateway:: 2>&1 | tail -14
cargo test -p alephcore --lib capability::census 2>&1 | tail -6
```

Expected: gateway green; census shows ~19 raw + ~27 slots.

- [ ] **Step 5: Commit**

```bash
git add src/gateway
git commit -m "gateway: migrate 19 capability handles onto CapabilitySlot; add booted()"
```

---

## Task 10: Migrate the remaining handles (batch D)

**Files:**
- Modify: everything still listed as `RAW` by the census — `src/pii/engine.rs` (`PII_ENGINE`),
  `src/identity/ledger.rs` (`LEDGER`, `WRITER`), `src/providers/route_observe.rs` (`GLOBAL`),
  `src/providers/session_model_handle.rs` (`PINNABLE_PROVIDERS`, `PIN_SINK`),
  `src/config/load.rs` (`EFFECTIVE_CONFIG_PATH`), `src/config/defaults_override.rs` (`DEFAULTS_OVERRIDE`),
  `src/metrics/mod.rs` (`METRICS_RUNTIME`), `src/extension/manager_global.rs` (`EXTENSION_MANAGER`),
  `src/loop_graph/mod.rs` (`GLOBAL`, `EVENT_BUS`), `src/looping/mod.rs` (`GLOBAL`),
  `src/goal/mod.rs` (`GLOBAL`), `src/strategy/mod.rs` (`GLOBAL`),
  `src/tasks/cron/mod.rs` (`GLOBAL_CRON`), `src/mcp/sampling_bridge.rs` (`SAMPLING_LLM`),
  `src/context/compact/manual.rs` (`MANUAL_WIRING`),
  `src/thinker/memory_context_provider/helpers.rs` (`OPEN_LOOP_INJECT`, `SESSION_END_*`, `SESSION_REFLECTOR`)

**Interfaces:**
- Consumes: `CapabilitySlot`, `MissingSemantics`
- Produces: zero `RAW` rows remaining in the census inventory

- [ ] **Step 1: Get the remaining list**

```bash
cargo test -p alephcore --lib capability::census -- --nocapture 2>&1 | grep '^  RAW'
```

- [ ] **Step 2: Migrate each, one commit per subsystem**

Apply the Task 8 pattern. Two handles need care:

```rust
// src/pii/engine.rs — a redaction engine that is NOT installed means output
// leaves the process unmasked. This is the FailsOpen case, and it is the
// highest-severity member of the whole roster.
static PII_ENGINE: CapabilitySlot<PiiEngine> =
    CapabilitySlot::new("pii/engine", MissingSemantics::FailsOpen);

// src/config/load.rs — `Config::effective_path()` falls back to the default
// path, so a doctor that parses "the config file" parses one nothing is using.
static EFFECTIVE_CONFIG_PATH: CapabilitySlot<PathBuf> = CapabilitySlot::new(
    "config/effective-path",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "the default ~/.aleph config path, even when --config named another",
    },
);
```

- [ ] **Step 3: Verify the inventory is fully migrated**

```bash
cargo test -p alephcore --lib capability::census -- --nocapture 2>&1 | grep -c '^  RAW'
```

Expected: `0`.

- [ ] **Step 4: Delete the wrappers that migration left without callers (spec §5 entropy)**

Keeping a wrapper is right when it has consumers — that is why the migration did not touch
call sites. A wrapper with **zero** consumers after migration is dead code and the spec
requires deleting it, not leaving it "in case".

```bash
cargo clippy --all-targets 2>&1 | grep -E 'never used|is never read' | grep -iE 'fn (set|init|install|global)_' 
```

⚠️ `cargo check` does not compile `#[cfg(test)]`, and neither compiles `tests/`. Before
deleting any `pub fn`, confirm it has no caller in the targets that `--lib` cannot see:

```bash
for f in $(cargo clippy --all-targets 2>&1 | grep -oE '\bfn [a-z_]+' | awk '{print $2}' | sort -u); do
  n=$(grep -rn --include='*.rs' "\b$f(" src/ tests/ interfaces/ shared/ | grep -vc "fn $f")
  echo "$n  $f"
done | sort -n | head -20
```

Delete only the zero-caller ones. Commit the deletions separately so a mistaken removal is
one `git revert` away.

```bash
git add -A
git commit -m "capability: drop the install wrappers migration left without callers"
```

- [ ] **Step 5: Run the minimum trusted verification set**

```bash
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli
```

All six must pass. `cargo check -p alephcore` alone is not verification.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "capability: migrate the remaining install-once handles onto CapabilitySlot

All 46 members of the rule-derived inventory now record their own outcome."
```

---

## Task 11: Close the class — the two census guards

**Files:**
- Modify: `src/capability/census.rs`
- Modify: `src/capability/mod.rs` (add `ALL_SLOTS`)

**Interfaces:**
- Consumes: `capability_handles()` (Task 6), every migrated slot (Tasks 7–10)
- Produces: `pub static ALL_SLOTS: &[&'static dyn SlotStatus]`, consumed by Task 12

- [ ] **Step 1: Write the roster and the two guards**

Add to `src/capability/mod.rs`:

```rust
/// Every capability slot in the process, for the `core/capability-wiring`
/// diagnostic.
///
/// Hand-written on purpose: `linkme`/`inventory` would add a dependency and
/// link-section magic for one feature (R3), and this list's completeness is
/// enforced by `census::every_declared_slot_is_in_the_roster`, which fails BY
/// ID when a new slot is not listed. The list is a data structure; the rule
/// is the guard.
pub static ALL_SLOTS: &[&'static dyn SlotStatus] = &[
    // Fill from `cargo test -p alephcore --lib capability::census -- --nocapture`.
    // One line per slot, e.g.:
    // &crate::spend::GLOBAL_LEDGER_FOR_ROSTER,
];
```

⚠️ Slots are private to their modules. Expose each via a `pub(crate) fn` returning
`&'static dyn SlotStatus` in its owning module (e.g. `pub(crate) fn policy_slot() -> &'static dyn SlotStatus { &GLOBAL_POLICY }`)
and list the accessors, rather than making 46 statics `pub`. Least-knowledge (P5): the roster
needs status, not the handle.

Replace the Task 6 count test with the two closing guards in `src/capability/census.rs`:

```rust
    /// Guard A — the class is closed. A new bare install-once static is a new
    /// handle nobody can observe.
    #[test]
    fn every_installed_global_is_a_capability_slot() {
        let offenders: Vec<String> = capability_handles()
            .into_iter()
            .filter(|s| !s.is_slot)
            .map(|s| format!("{}::{} ({})", s.file, s.name, s.container))
            .collect();
        assert!(
            offenders.is_empty(),
            "these are written at runtime but are not CapabilitySlots, so nothing \
             can tell 'never installed' from 'installed with this value':\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Guard B — the roster is complete. A slot missing from ALL_SLOTS is
    /// invisible to the doctor face, which is the same silence as before.
    #[test]
    fn every_declared_slot_is_in_the_roster() {
        let declared: std::collections::BTreeSet<String> = capability_handles()
            .into_iter()
            .filter(|s| s.is_slot)
            .map(|s| s.name)
            .collect();
        let rostered: std::collections::BTreeSet<String> = crate::capability::ALL_SLOTS
            .iter()
            .map(|s| s.id().to_string())
            .collect();
        assert_eq!(
            declared.len(),
            rostered.len(),
            "declared slots: {declared:?}\nroster ids: {rostered:?}\n\
             every CapabilitySlot::new() must be reachable from ALL_SLOTS"
        );
        assert!(
            rostered.len() >= 40,
            "roster has {} entries; 46 were measured (NOT the spec's 46 -- see the correction block at the top of this plan). A shrinking roster and a \
             broken scan look identical in a green report.",
            rostered.len()
        );
    }
```

⚠️ Guard B compares *counts*, not names, because a slot's `id` (`"spend/policy"`) is
deliberately not its static's name (`GLOBAL_POLICY`). If you want name-level matching, have
the census also parse the `CapabilitySlot::new("<id>"` literal — do that only if the count
comparison proves too weak in practice.

- [ ] **Step 2: Run both guards**

```bash
cargo test -p alephcore --lib capability::census 2>&1 | tail -12
```

Expected: both PASS after Tasks 7–10.

- [ ] **Step 3: Falsify guard A**

```bash
cp src/tools/in_flight.rs /tmp/in_flight.rs.bak
# Revert one migrated handle to a bare container.
python3 - <<'EOF'
import re
p='src/tools/in_flight.rs'; s=open(p).read()
s=re.sub(r'static GLOBAL_REGISTRY: CapabilitySlot<[^>]+> = CapabilitySlot::new\([^;]+;',
         'static GLOBAL_REGISTRY: std::sync::OnceLock<InFlightToolCalls> = std::sync::OnceLock::new();',
         s, count=1)
open(p,'w').write(s)
EOF
cargo test -p alephcore --lib capability::census::tests::every_installed_global 2>&1 | tail -12
cp /tmp/in_flight.rs.bak src/tools/in_flight.rs
```

Expected: `test result: FAILED` ⇒ **RED**, and the message must contain
`src/tools/in_flight.rs::GLOBAL_REGISTRY`. A RED that does not name the file is a guard you
cannot act on. (The migration will not compile after this mutation if the wrappers still call
`.install()`; a BUILD-ERROR here is also acceptable evidence — record which one you saw.)

- [ ] **Step 4: Falsify guard B**

```bash
cp src/capability/mod.rs /tmp/capability_mod.rs.bak
# Drop the first roster entry.
python3 - <<'EOF'
p='src/capability/mod.rs'; s=open(p).read()
i=s.index('pub static ALL_SLOTS'); j=s.index('&', i); k=s.index('\n', j)
s=s[:j]+'// '+s[j:k]+s[k:]
open(p,'w').write(s)
EOF
cargo test -p alephcore --lib capability::census::tests::every_declared_slot 2>&1 | tail -12
cp /tmp/capability_mod.rs.bak src/capability/mod.rs
```

Expected: `test result: FAILED` ⇒ **RED** with the declared-vs-rostered count mismatch printed.

- [ ] **Step 5: Re-run both to confirm restoration**

```bash
cargo test -p alephcore --lib capability::census 2>&1 | tail -8
```

Expected: `test result: ok`.

- [ ] **Step 6: Commit**

```bash
git add src/capability
git commit -m "capability: close the class with two rule-derived census guards"
```

---

## Task 12: The `core/capability-wiring` diagnostic check

**Files:**
- Create: `src/diagnostics/checks/capability_wiring.rs`
- Modify: `src/diagnostics/checks/mod.rs`, `src/diagnostics/mod.rs:70-84`

**Interfaces:**
- Consumes: `capability::{ALL_SLOTS, MissingSemantics, Outcome, SlotStatus}`,
  `gateway::shutdown_forensics::booted`
- Produces: `pub struct CapabilityWiringCheck` implementing `HealthCheck` with id `core/capability-wiring`

- [ ] **Step 1: Write the failing tests**

Create `src/diagnostics/checks/capability_wiring.rs` with only the test module plus a stub:

```rust
//! `core/capability-wiring` — did boot install the process-global capabilities?

use async_trait::async_trait;

use crate::capability::{MissingSemantics, Outcome};
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/capability-wiring";

/// Placeholder; replaced in step 3.
fn severity_for(_m: MissingSemantics) -> Severity {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Severity is derived from the failure direction, never hand-assigned per
    /// slot — a hand-assigned table is a second source of truth about what a
    /// missing handle costs.
    #[test]
    fn severity_is_derived_from_the_failure_direction() {
        assert_eq!(severity_for(MissingSemantics::FailsOpen), Severity::Error);
        assert_eq!(
            severity_for(MissingSemantics::IndistinguishableDefault { reads_as: "x" }),
            Severity::Warning
        );
        assert_eq!(severity_for(MissingSemantics::ConsumerDecides), Severity::Warning);
        assert_eq!(severity_for(MissingSemantics::FailsClosed), Severity::Info);
    }

    /// The process-truth rule. A test binary never runs `aleph-server start`,
    /// so this exercises exactly the cold-process branch that
    /// `aleph-server doctor` takes.
    #[tokio::test]
    async fn a_process_that_never_booted_reports_info_not_a_pass() {
        // Guard: if some other test in this binary called `mark_boot`, this
        // assertion is meaningless. Skip loudly rather than pass quietly.
        if crate::gateway::shutdown_forensics::booted() {
            eprintln!("SKIP: mark_boot() was called by another test in this binary");
            return;
        }
        let findings = CapabilityWiringCheck::new().run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(
            findings[0].detail.contains("did not"),
            "the cold-process finding must say this process did not boot, not that \
             the wiring is broken; got: {}",
            findings[0].detail
        );
        assert!(findings[0]
            .fix_hint
            .as_deref()
            .is_some_and(|h| h.contains("aleph doctor")));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p alephcore --lib diagnostics::checks::capability_wiring 2>&1 | tail -14
```

Expected: BUILD-ERROR (`CapabilityWiringCheck` not found).

- [ ] **Step 3: Write the implementation**

Replace the stub in `src/diagnostics/checks/capability_wiring.rs`:

```rust
//! `core/capability-wiring` — did boot install the process-global capabilities?
//!
//! # Why this check is three-state
//!
//! `aleph-server doctor` builds a **fresh** `DiagnosticEngine` in a cold
//! process where no capability has been installed; `diagnostics.run` executes
//! inside the daemon where they are live. Same battery, two processes, two
//! truths. Reporting the cold process's empty roster as broken would cry wolf
//! on a healthy machine; reporting it as healthy would be the mistake this
//! whole round exists to remove ("unknown" must never read as "healthy").
//!
//! So the verdict keys on `shutdown_forensics::booted()`:
//!
//! | booted | roster | verdict |
//! |---|---|---|
//! | false | — | `Info`: this process did not boot; ask the daemon |
//! | true | complete | ok |
//! | true | holes | one finding per slot, severity from `MissingSemantics` |
//!
//! The third row is free extra value: `mark_boot()` runs at the *start* of
//! boot and the installs come after, so "booted but incomplete" is a real
//! failure state (boot died or early-returned) that nothing could observe
//! before.

/// Severity is derived from the failure direction, never hand-assigned.
fn severity_for(m: MissingSemantics) -> Severity {
    match m {
        // A gate that silently stopped gating.
        MissingSemantics::FailsOpen => Severity::Error,
        // The round-7 shape: a true sentence hiding a false world.
        MissingSemantics::IndistinguishableDefault { .. } => Severity::Warning,
        // N consumers each inventing an answer.
        MissingSemantics::ConsumerDecides => Severity::Warning,
        // Safe, but the feature is dead and says nothing.
        MissingSemantics::FailsClosed => Severity::Info,
    }
}

#[derive(Default)]
pub struct CapabilityWiringCheck;

impl CapabilityWiringCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HealthCheck for CapabilityWiringCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Capability wiring"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        if !crate::gateway::shutdown_forensics::booted() {
            return vec![Finding::problem(
                ID,
                Severity::Info,
                "Wiring is not observable from this process",
                "This process did not run `aleph-server start`, so no capability handle \
                 was installed here. Reporting the empty roster either way would be \
                 fiction — the daemon is the only process that knows.",
            )
            .with_fix_hint(
                "Run `aleph doctor` (it asks the running gateway over `diagnostics.run`) \
                 rather than `aleph-server doctor`.",
            )];
        }

        let mut findings: Vec<Finding> = Vec::new();
        for slot in crate::capability::ALL_SLOTS {
            match slot.outcome() {
                Some(Outcome::Installed) => {}
                Some(Outcome::Declined { because }) => findings.push(
                    Finding::problem(
                        ID,
                        severity_for(slot.missing()),
                        format!("Capability `{}` was declined", slot.id()),
                        format!(
                            "Boot reached this handle and could not install it: {because}. \
                             Reads observe: {}",
                            describe(slot.missing())
                        ),
                    )
                    .with_tag("capability-declined"),
                ),
                None => findings.push(
                    Finding::problem(
                        ID,
                        severity_for(slot.missing()),
                        format!("Capability `{}` was never reached", slot.id()),
                        format!(
                            "Boot started but nothing installed or declined this handle — \
                             boot may have failed or returned early. Reads observe: {}",
                            describe(slot.missing())
                        ),
                    )
                    .with_tag("capability-unreached"),
                ),
            }
        }

        if findings.is_empty() {
            vec![Finding::ok(
                ID,
                "Capability wiring complete",
                format!("All {} process-global capabilities were installed.", crate::capability::ALL_SLOTS.len()),
            )]
        } else {
            findings
        }
    }
}

/// What a caller actually sees when the handle is absent — the sentence an
/// operator needs, not the enum's name.
fn describe(m: MissingSemantics) -> String {
    match m {
        MissingSemantics::IndistinguishableDefault { reads_as } => {
            format!("{reads_as} — indistinguishable from a deliberate configuration")
        }
        MissingSemantics::ConsumerDecides => {
            "`None`; each consumer decides for itself what that means".into()
        }
        MissingSemantics::FailsClosed => "a closed gate; the feature is inert".into(),
        MissingSemantics::FailsOpen => "an OPEN gate; this check is not being enforced".into(),
    }
}
```

Register it. In `src/diagnostics/checks/mod.rs`:

```rust
pub mod capability_wiring;
pub use capability_wiring::CapabilityWiringCheck;
```

In `src/diagnostics/mod.rs::default_registry()`, append to the `checks` vec:

```rust
            Arc::new(checks::CapabilityWiringCheck::new()),
```

- [ ] **Step 4: Run the tests and the whole diagnostics suite**

```bash
cargo test -p alephcore --lib diagnostics:: 2>&1 | tail -14
```

Expected: green, including the two new tests.

- [ ] **Step 5: Verify the three states on a real machine**

```bash
cargo build --bin aleph-server 2>&1 | tail -3

# State 1 — cold process: must be Info, must NOT be a pass.
./target/debug/aleph-server doctor --json 2>/dev/null \
  | python3 -c "import sys,json; f=[x for x in json.load(sys.stdin)['findings'] if x['check_id']=='core/capability-wiring']; print(f)"

# State 2 — daemon: must pass.
./target/debug/aleph-server start &
sleep 8
./target/debug/aleph doctor --json 2>/dev/null \
  | python3 -c "import sys,json; f=[x for x in json.load(sys.stdin)['findings'] if x['check_id']=='core/capability-wiring']; print(f)"
```

State 1 must show `severity: Info` and a detail naming the daemon. State 2 must show the
ok finding. **If state 1 shows a pass, the check is lying** — that is the exact failure this
design exists to prevent.

For state 3, temporarily add `SOME_SLOT.decline("QA probe");` to boot, rebuild, restart,
and confirm the finding's `detail` contains `QA probe` verbatim (not a generic phrase).
Revert the probe before committing.

- [ ] **Step 6: Commit**

```bash
git add src/diagnostics
git commit -m "diagnostics: add core/capability-wiring, a three-state wiring report

Keys on the existing boot sentinel so a cold `aleph-server doctor` says
'ask the daemon' instead of inventing an answer about an empty roster."
```

---

## Task 13: Adjudicate the Task 3 triage ledger

**Files:**
- Modify: whichever files the ledger names
- Modify: `docs/superpowers/plans/2026-08-24-capability-wiring-triage.md`

**Interfaces:**
- Consumes: the ledger written in Task 3
- Produces: an empty (fully-adjudicated) ledger

- [ ] **Step 1: Work the ledger top to bottom**

For each row, decide CONNECT / CUT / REPORT. Read before writing — a guard that newly sees
dead scaffolding wants it **deleted**, not reconnected. Record the verdict and the reason in
the ledger row.

- [ ] **Step 2: Fix each CONNECT and CUT row, one commit per row**

```bash
git add <files>
git commit -m "<scope>: <what the newly-sighted guard caught>"
```

- [ ] **Step 3: Run the minimum trusted verification set**

```bash
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli
```

- [ ] **Step 4: Commit the completed ledger**

```bash
git add docs/superpowers/plans/2026-08-24-capability-wiring-triage.md
git commit -m "docs: close the Task 3 triage ledger"
```

---

## Task 14: Give every conditional boot install an `else` arm

**Files:**
- Modify: the 20 conditional install sites, including
  `src/bin/aleph-server/commands/start/mod.rs:{455,461,1239,1739,1782,1788,1879,2084}`,
  `src/bin/aleph-server/commands/start/builder/subsystems.rs:{292,319,348,391,396,398}`,
  `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:{682,751}`,
  `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:470`,
  `src/bin/aleph-server/commands/start/mod.rs:109`, `src/bin/aleph-server/main.rs:79`,
  **`src/bin/aleph-server/commands/start/mod.rs:3163-3195`** — a `match` arm, not an
  `if`, and it decides THREE slots at once (see Step 2's ⚠️ below)

**Two decline sites this task's own search shape will not find. Read both before Step 1.**

1. **`start/mod.rs:3163-3195` — three slots inside a `match` arm.**
   ```rust
   match alephcore::tools::result_store::ToolResultStore::new("global") {
       Ok(store) => {
           set_global_tool_result_store(store);   // tools/result-store
           set_global_result_budget_ceiling(…);   // tools/result-budget-ceiling
           set_global_turn_result_budget(budget); // tools/turn-budget
       }
       Err(e) => { tracing::warn!(…, "…Layer 2 + Layer 3 disabled"); }
   }
   ```
   Step 1's `grep` includes `match ` so it will *list* this, but Step 3's guard
   recognised `if` / `if let` openers only and would have **silently exempted all
   three** — widened below. Note also for Task 15: `turn-budget` and
   `result-budget-ceiling` are installed only inside the result-store's `Ok` arm
   though neither depends on the store, so a `ToolResultStore::new` failure
   silently disables the turn cap — a handle Task 8 judged `FailsOpen`.

2. **`src/tools/result_processing.rs::set_global_result_budget_ceiling` — an early
   return inside the library setter, outside this task's walk root entirely.**
   It returns without installing when `ceiling >= DEFAULT_RESULT_BUDGET_TOKENS`,
   with the reason already written down ("a large-window model installs nothing
   and behaves byte-for-byte as it does today"). Step 1 greps
   `src/bin/aleph-server/commands/start/` and Step 3 walks `src/bin/aleph-server`
   only, so **neither reaches it**.

   ⚠️ Boot passes `per_result_tokens`, whose maximum *is* `DEFAULT_RESULT_BUDGET_TOKENS`,
   and the no-`[context_budget]` path passes that constant directly — so this decline
   fires for **every deployment without a small window**, i.e. the common healthy case.
   Until it is converted, a healthy box reports `outcome() == None` on that slot, which
   `capability/mod.rs` defines as "nothing ever reached this slot — either this process
   did not boot, or boot died before getting here". That is the confident-lie direction,
   and it becomes observable the moment Task 11/12 land.

   Task 8 recorded this at the declaration; it is repeated here because that file is
   one this task has no reason to open.

**Interfaces:**
- Consumes: `decline` on every migrated slot
- Produces: nothing consumed later

- [ ] **Step 1: Re-derive the list (do not trust the line numbers above)**

```bash
grep -rn --include='*.rs' -B6 -E '\b(install|init|set)_[a-z_]+\(' \
  src/bin/aleph-server/commands/start/ | grep -E 'if let|if |match ' | head -40
```

- [ ] **Step 2: For each site, decide unconditional-install vs decline**

Two legitimate outcomes, and the choice is per-site:

```rust
// (a) The handle should always exist — make it unconditional, and say why in a
//     comment, the way `spend::install_policy` does. Use this when the guard was
//     protecting against an Option that is never None in production.

// (b) The dependency genuinely may be absent — record why:
if let Some(ref db) = state_db {
    crate::gateway::offset_tracker::set_offset_tracker(db.clone());
} else {
    crate::gateway::offset_tracker::decline_offset_tracker(
        "state database absent: [gateway] state_db is unset",
    );
}
```

Add a `pub fn decline_*(because: &'static str)` wrapper beside each `set_*` wrapper, mirroring
the install wrapper's shape.

⚠️ The `because` string is shown to operators verbatim. Name the **missing input** and the
config key, not the symptom. "state database absent" alone is not actionable;
"`[gateway] state_db` is unset" is.

- [ ] **Step 3: Verify no conditional install is silent**

Add to `src/capability/census.rs`'s test module. Note two deliberate choices: the set of
install wrappers is **derived** (any `pub fn` whose body calls `.install(`), never listed —
a name list rots the first time someone adds a handle; and every scan window ends at its
subject's **syntactic terminus**, never at a line budget — a fixed-size window silently reads
into the next item.

```rust
    /// Production lines of `text`, comment-free.
    fn prod_lines(text: &str) -> Vec<String> {
        strip_comment_lines(&production_prefix(text))
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn indent_of(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }

    /// `(body, index_of_closing_line)` for the block opening at `start`.
    ///
    /// The block ends at the first line that starts with `}` at an indent <=
    /// the opener's — the block's own syntactic terminus. NOT a line budget:
    /// a fixed-size window reads into whatever follows, and this repo has
    /// shipped a guard that passed because its 400-character window read the
    /// neighbouring declaration.
    fn block_at(lines: &[String], start: usize) -> (String, usize) {
        let indent = indent_of(&lines[start]);
        let mut body = vec![lines[start].clone()];
        for (offset, line) in lines[start + 1..].iter().enumerate() {
            if !line.trim().is_empty()
                && indent_of(line) <= indent
                && line.trim_start().starts_with('}')
            {
                return (body.join("\n"), start + 1 + offset);
            }
            body.push(line.clone());
        }
        (body.join("\n"), lines.len().saturating_sub(1))
    }

    /// Names of the `pub fn`s that install a capability, derived crate-wide.
    fn install_wrapper_names() -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for (_, text) in all_sources() {
            let lines = prod_lines(&text);
            for i in 0..lines.len() {
                let t = lines[i].trim_start();
                let Some(rest) = t.strip_prefix("pub fn ").or_else(|| {
                    t.strip_prefix("pub(crate) fn ")
                }) else {
                    continue;
                };
                let Some(name) = rest.split('(').next() else { continue };
                let (body, _) = block_at(&lines, i);
                if body.contains(".install(") {
                    out.insert(name.trim().to_string());
                }
            }
        }
        out
    }

    /// Which conditional opener decided an install — the `else` for one is a
    /// sibling arm for the other, so they cannot share a check.
    enum GuardKind {
        If,
        MatchArm,
    }

    /// Every conditional capability install in boot says why it was skipped.
    #[test]
    fn no_conditional_boot_install_is_silent() {
        let wrappers = install_wrapper_names();
        assert!(
            wrappers.len() >= 30,
            "derived only {} install wrappers; >=30 expected after migration. A \
             derivation that stopped matching makes this guard pass by finding nothing.",
            wrappers.len()
        );

        let boot_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/bin/aleph-server");
        let mut examined = 0usize;
        let mut offenders: Vec<String> = Vec::new();

        for (rel, text) in all_sources_under(&boot_root) {
            let lines = prod_lines(&text);
            for i in 0..lines.len() {
                let calls_installer = wrappers
                    .iter()
                    .any(|w| lines[i].contains(&format!("{w}(")));
                if !calls_installer {
                    continue;
                }
                let my_indent = indent_of(&lines[i]);

                // Nearest enclosing CONDITIONAL opener at a strictly smaller
                // indent. TWO families, not one.
                //
                // ⚠️ This recognised `if` / `if let` only for one revision, and
                // `start/mod.rs:3163` is why that was not enough: three installs
                // sit inside `match ToolResultStore::new { Ok(store) => … ,
                // Err(e) => warn! }`. From each of them the back-walk lands on
                // `Ok(store) => {`, finds no `if`, sets `guard = None`, and takes
                // the `continue` below commented "unconditional install: fine".
                // It is not unconditional — an `Err` decides three slots and says
                // nothing — and `examined >= 15` is satisfied by the genuine
                // `if let` sites, so the vacuity assertion could not report it
                // either. A guard blind to a whole opener family reports "all
                // clear" about sites it never read.
                let mut guard: Option<(usize, GuardKind)> = None;
                for j in (0..i).rev() {
                    if lines[j].trim().is_empty() {
                        continue;
                    }
                    let ji = indent_of(&lines[j]);
                    if ji >= my_indent {
                        continue;
                    }
                    let t = lines[j].trim_start();
                    if t.starts_with("if ") || t.starts_with("if let ") {
                        guard = Some((j, GuardKind::If));
                    } else if t.contains("=>") && t.ends_with('{') {
                        // A match arm. Nothing else in Rust opens a block with
                        // `… => {`; closures spell their params `|a, b|`.
                        guard = Some((j, GuardKind::MatchArm));
                    }
                    break; // first shallower line decides; do not keep walking out
                }
                let Some((g, kind)) = guard else { continue }; // unconditional install: fine
                examined += 1;

                let says_why = match kind {
                    GuardKind::If => {
                        let (_, closing) = block_at(&lines, g);
                        let closer = lines.get(closing).map(String::as_str).unwrap_or("");
                        closer.trim_start().starts_with("} else")
                            && block_at(&lines, closing).0.contains("decline")
                    }
                    // A match arm's "else" is a SIBLING ARM, so the subject is
                    // the whole `match`: step out exactly one level and require a
                    // `decline` somewhere in its body.
                    //
                    // Deliberately weaker than the `If` branch, and say so rather
                    // than assume it away: an arm that declines the WRONG slot
                    // still passes here. Pinning which arm declines what needs
                    // arm-by-arm parsing; the defect worth catching first is "this
                    // match decides a slot and never declines at all".
                    //
                    // `contains("match ")`, not `starts_with`: `let x = match y {`
                    // is the common spelling, and `starts_with` would walk past it
                    // to an unrelated earlier `match`. If the line one step out is
                    // not a match at all, this yields `false` and the site is
                    // REPORTED — over-report rather than silently exempt, which is
                    // the failure this whole widening exists to remove.
                    GuardKind::MatchArm => {
                        let arm_indent = indent_of(&lines[g]);
                        lines[..g]
                            .iter()
                            .rposition(|l| !l.trim().is_empty() && indent_of(l) < arm_indent)
                            .filter(|&m| lines[m].contains("match "))
                            .is_some_and(|m| block_at(&lines, m).0.contains("decline"))
                    }
                };
                if !says_why {
                    offenders.push(format!("{rel}:{}", i + 1));
                }
            }
        }

        assert!(
            examined >= 15,
            "examined only {examined} conditional installs; 20 were measured on \
             2026-08-24. Zero-or-few is how this guard reports 'all clear' about \
             sites it never read."
        );
        assert!(
            offenders.is_empty(),
            "these conditional capability installs never say why they were skipped — \
             add an `else` arm calling the slot's `decline(because)`:\n  {}",
            offenders.join("\n  ")
        );
    }
```

`all_sources_under(&path)` is `all_sources()` with the walk root parameterised — refactor
`all_sources()` to `all_sources_under(root)` and keep `all_sources()` as
`all_sources_under(&manifest.join("src"))`.

- [ ] **Step 3b: Falsify the guard**

```bash
cp src/bin/aleph-server/commands/start/builder/subsystems.rs /tmp/subsystems.rs.bak
# Delete one `decline` call and confirm the site is named.
python3 - <<'EOF'
p='src/bin/aleph-server/commands/start/builder/subsystems.rs'
s=open(p).read()
i=s.index('decline')
j=s.index(';', i)
open(p,'w').write(s[:i]+'()'+s[j:])
EOF
cargo test -p alephcore --lib capability::census::tests::no_conditional_boot_install 2>&1 | tail -12
cp /tmp/subsystems.rs.bak src/bin/aleph-server/commands/start/builder/subsystems.rs
```

Expected: `test result: FAILED` ⇒ **RED** naming `subsystems.rs:<line>`. If instead you get
`examined only 0`, the wrapper derivation broke — fix that before trusting any green from
this guard.

⚠️ **Falsify the `match` branch separately — the `if` branch passing proves nothing about
it.** That is the whole lesson of the revision that added it: the guard was green crate-wide
while structurally blind to three slots.

```bash
cp src/bin/aleph-server/commands/start/mod.rs /tmp/mod.rs.bak
# Delete the `decline` from the Err arm at ~3189 and confirm all THREE installs
# in the Ok arm are named.
cargo test -p alephcore --lib capability::census::tests::no_conditional_boot_install 2>&1 | tail -12
cp /tmp/mod.rs.bak src/bin/aleph-server/commands/start/mod.rs
```

Expected: **RED** naming three lines in `start/mod.rs` (one per slot). A single line means
the back-walk is still resolving only one of them; a green means the `match` opener is not
being recognised at all — check `t.contains("=>") && t.ends_with('{')` against the actual
formatting of that arm before believing it.

- [ ] **Step 4: Run boot tests and the verification set**

```bash
cargo test -p alephcore --lib capability:: 2>&1 | tail -10
cargo test -p alephcore --lib --no-run && cargo clippy --all-targets
```

- [ ] **Step 5: Commit**

```bash
git add src/bin src/capability
git commit -m "boot: make every conditional capability install say why it was skipped

The Rust shape of Cordis's unsatisfied \`static inject\`: a declined slot names
the missing input instead of leaving a silent hole."
```

---

## Task 15: Adjudicate the `None`-handling consumers

**Files:**
- Modify: only those of the 9 `global_session_service()` consumers whose handling is wrong —
  `src/tools/scoped/dispatch.rs:1120`, `src/builtin_tools/sessions/compact_tool.rs:112`,
  `src/gateway/openai_api/completions/agent.rs:342`, `src/gateway/execution_engine/execute.rs:982`,
  `src/gateway/execution_engine/simple.rs:{142,221}`, `src/gateway/execution_engine/fast_path.rs:{46,157}`,
  `src/gateway/execution_engine/run_loop/inner.rs:1222`

**Interfaces:**
- Consumes: everything above
- Produces: nothing

- [ ] **Step 1: Read each consumer and answer one question**

> When this handle is absent, is silently skipping the right behaviour *for this
> call site*?

`fail-soft skipping is not evidence of absence` — but for several of these it may still be
correct (a projection that has nothing to project). Record the verdict per site in the triage
ledger. **Do not blanket-rewrite** (spec §2 non-goal 5).

- [ ] **Step 2: Change only the sites whose verdict is "wrong", one commit each**

For a site that should complain rather than skip:

```rust
// BEFORE
let Some(session_svc) = crate::session::service::global_session_service() else {
    return;
};

// AFTER
let Some(session_svc) = crate::session::service::global_session_service() else {
    // Not "nothing to do": the process-global service is absent, which
    // `core/capability-wiring` reports separately. Say so once rather than
    // dropping the work silently.
    tracing::warn!(
        "session/service capability absent; skipping <what> — see `aleph doctor`"
    );
    return;
};
```

- [ ] **Step 3: Run the affected suites plus the verification set**

```bash
cargo test -p alephcore --lib -- tools::scoped builtin_tools::sessions gateway::execution_engine 2>&1 | tail -12
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "session: adjudicate the capability-absent handling at each consumer"
```

---

## Task 16: Documentation

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§3.1 dsh-round area and §5.9 diagnostics)
- Modify: `CLAUDE.md` (工程判据清单 §0 and §8)

**Interfaces:**
- Consumes: the finished implementation
- Produces: the record the next round reads before re-deriving any of this

- [ ] **Step 1: Add the round to `FEATURE_LOCATOR.md`**

Under §3.1, after the Round 8 dsh entry, add a round entry recording: the mechanism, the 46-member
inventory with the three measurement passes and why each was low, the 276-file blind spot, the
three-state doctor rule, and — explicitly — that this does **not** reopen the "architecture not
ported" ruling (spec §8).

Under §5.9, add `core/capability-wiring` to the diagnostics check list with its three-state contract.

- [ ] **Step 2: Add two criteria to `CLAUDE.md` §0**

Draft (tighten to the file's voice):

> - **一个源码级守卫取「生产前缀」的方式，决定它看得见哪 84% 的仓库** —— `split("#[cfg(test)]").next()` 不是分割点，是**第一个测试属性出现的位置**：1,734 个含该标记的文件里，73 个整份被丢弃（顶部 `#[cfg(test)] mod tests;`）、203 个被任意截断（首个标记是文件中段的测试项）。`utils/paths.rs` 的测试专用互斥量在文件 5% 处 ⇒ 该文件 95% 的生产代码对每一条 prefix-split 守卫不可见。⚠️ **这一族的量具会连着骗你三次**：同一个类被量了三遍（裸类型名 + 截断前缀 ⇒ 29；全限定路径 + 截断前缀 ⇒ 38；全限定路径 + 大括号配对 ⇒ **40**），每一遍都偏低，而**低的方向不会引起怀疑**。判据：**写下一个类的大小之前，先说出你的量具会漏掉哪一种形状**。单一源 `utils::source_scan::production_prefix`。
> - **一个进程级句柄的「没装过」与「装成了这个值」，只有安装侧能分开** —— 读侧按定义分不开（这正是缺陷）。判据：**这个句柄的未安装回退值，落在某个合法配置的取值范围内吗**；落在里面就必须让安装侧记账（`CapabilitySlot::install` 写值与盖戳是同一个动作，`decline(because)` 让条件安装说出缺了什么）。⚠️ 配套：**报告它的那个传感器必须先回答「我这个进程知道吗」**——`aleph-server doctor` 在冷进程里建 registry，句柄全空，报 pass 或报故障都是虚构；三态判据钉在 `shutdown_forensics::booted()` 上。

- [ ] **Step 3: Verify the docs match the code**

```bash
grep -n 'core/capability-wiring' docs/reference/FEATURE_LOCATOR.md src/diagnostics/checks/capability_wiring.rs
grep -c 'CapabilitySlot' docs/reference/FEATURE_LOCATOR.md
```

Any number quoted in the docs must be one the guards assert. A doc number with no guard
behind it drifts, and the drifted copy is the one people read.

- [ ] **Step 4: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: record the capability-wiring round and its two criteria"
```

---

## Task 17: Final verification and merge readiness

**Files:** none

- [ ] **Step 1: Full verification set from a clean build**

```bash
cargo clean -p alephcore
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli
cargo test -p alephcore --lib 2>&1 | tail -5
```

- [ ] **Step 2: Confirm the harness redline held**

```bash
git diff main...capability-wiring --stat -- src/harness/
```

Expected: **empty**. Any output violates the global constraint; revert those hunks.

- [ ] **Step 3: Confirm no new dependency**

```bash
git diff main...capability-wiring -- Cargo.toml Cargo.lock | grep -E '^\+' | grep -vE '^\+\+\+' | head
```

Expected: empty (or only version-neutral churn).

- [ ] **Step 4: Compare against the Task 0 baseline**

```bash
diff <(sort /tmp/baseline-lib.txt) <(cargo test -p alephcore --lib 2>&1 | tail -3 | sort) || true
```

Every difference must be explained by a triage-ledger row.

- [ ] **Step 5: Report, do not merge**

Summarise for the human: tasks completed, defects found and their verdicts, anything left
open, and the exact numbers the guards now assert. Merging is their call.
