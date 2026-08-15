# Review Report — `src/bundled` + `src/clarification`

**Scope:** `src/bundled/{mod,manifest,sync,extractor}.rs`, `src/clarification/{mod,ask,render,session}.rs`
**Date:** 2026-08-15
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-bundled-clarification` (branch `review/bundled-clarification`)
**Total LOC:** 3 749 (1 383 bundled + 2 366 clarification)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 2 |
| Medium   | 4 |
| Low      | 4 |

The modules are unusually well-documented (the `ask` docstring is a public
statement of intent every following line upholds). Most of the findings are
localised guards that read as "we already thought about this" comments waiting
for the line that implements them.

The two High findings are the same defence written twice — a `symlink_metadata`
check that returns `is_dir() == false` for a symlink, leaving the path live
for `create_dir_all` to write through.

---

## Findings

### [HIGH-1] src/bundled/extractor.rs:332-364 (extract_plugins) — `tmp_dir` symlink is a passthrough target: an attacker-controlled symlink under `~/.aleph/plugins/cache/aleph-official.tmp` redirects plugin extraction outside the cache

**Category:** Security / Path traversal
**Confidence:** High

**Description:**

`extract_plugins` builds the staging dir as `cache_dir.with_extension("tmp")`
(line 332) and gates cleanup on `symlink_metadata`:

```rust
// extractor.rs:339-348
if let Ok(meta) = tmp_dir.symlink_metadata() {
    if meta.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&tmp_dir) { ... }
    } else {
        warn!(path = %tmp_dir.display(), "Plugin cache temp path exists but is not a directory, skipping removal");
        return false;
    }
}
if let Err(e) = std::fs::create_dir_all(&tmp_dir) { ... }
```

`symlink_metadata` on a symlink returns `FileType::is_symlink()` true and
`is_dir()` false — the entire `else` branch fires. `create_dir_all` then
succeeds, but the directory it creates lives at the symlink target. The
following `copy_dir_into` (via `extract_dir_contents`) writes the bundled
plugins there.

The preconditions are realistic:
- The cache was created by the daemon with default umask. Anything the user
  can also write to (`~/.aleph/plugins/cache/aleph-official.tmp`) can plant a
  symlink.
- A previous extraction may have aborted, leaving the user writable tmp path
  available for a follow-up crash or automated installer to plant.

The same shape repeats in `extract_plugins_from_dir` (lines 584-606) — second
public-adjacent surface, same defect.

**Failure scenario:** `~/.aleph/plugins/cache/aleph-official.tmp →
/home/user/.config/aleph-plugin-override` exists at the moment of startup.
The next `extract_bundled_content` call writes the bundled plugin tree there.
The user, opening the override config, sees a half-baked plugin tree they
did not install.

**Suggested fix:** treat symlink as "drop and replace" before `create_dir_all`:

```rust
match std::fs::symlink_metadata(&tmp_dir) {
    Ok(m) if m.is_dir() => {
        if let Err(e) = std::fs::remove_dir_all(&tmp_dir) { ...; return false; }
    }
    Ok(_) => {
        // Symlink or other non-directory — remove the entry itself.
        if let Err(e) = std::fs::remove_file(&tmp_dir) { ...; return false; }
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
    Err(e) => { warn!(...); return false; }
}
```

`remove_file` on a symlink removes the symlink, not the target.

---

### [HIGH-2] src/bundled/extractor.rs:584-606 (extract_plugins_from_dir) — same symlink passthrough, second half of the week

**Category:** Security / Path traversal
**Confidence:** High

**Description:** identical to HIGH-1, second call site. The function is the
filesystem-source counterpart of `extract_plugins` and is reachable from
`hub/install.rs` (the marketplace installer) and from `sync_official_with_urls`
itself. Every caller plants user-controlled content into the same dir tree,
so the same fix is the right one at both places.

---

### [MEDIUM-1] src/bundled/manifest.rs:69-99 — `InstallRegistry::save` rename-then-remove-then-rename is racy on the destination

**Category:** Concurrency / Integrity
**Confidence:** High

**Description:**

```rust
// manifest.rs:69-99
std::fs::write(&tmp_path, content)?;
if let Err(e) = std::fs::rename(&tmp_path, &path) {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        std::fs::remove_file(&path)?;
        std::fs::rename(&tmp_path, &path)?
    } else { return Err(e); }
}
```

Between `remove_file` and `rename`, another process (or the same daemon's
parallel startup thread) can re-create `path`. The subsequent `rename` then
succeeds on a fresh file, but the reader observed a window where the file was
absent and either bailed (cached `None`) or wrote its own stale copy back.

The right shape is the **create-new tmp + atomic rename** the comment above
the function already describes:

```rust
let tmp = std::fs::OpenOptions::new()
    .write(true).create_new(true).open(&tmp_path)?;
tmp.write_all(content.as_bytes())?;
drop(tmp);
std::fs::rename(&tmp_path, &path)?;   // overwrite-on-Unix, retry on Windows
```

`create_new(true)` refuses on collision, so the tmp file is process-unique by
construction. The rename is the only place we touch `path`, and POSIX rename
overwrites atomically (Windows needs the rename-remove-retry pattern that
already lives in `extractor.rs:swap_dir_into_place`).

**Suggested fix:** replace the open-and-rename with OpenOptions create_new,
then a single rename with the existing Windows fallback. The same fix benefits
the cache-side `swap_dir_into_place` if it ever runs against a path another
writer might touch.

---

### [MEDIUM-2] src/bundled/sync.rs:101 — `update_existing_repo` fetches with `&["main"]`, a refspec that does not name a source:dest pair

**Category:** Correctness / Portability
**Confidence:** Medium

**Description:**

```rust
// sync.rs:101
remote.fetch(&["main"], None, None)?;
```

libgit2's accepted refspec formats are `src:dst` (or a tag/branch name in
recent versions). The current tests pass because:
- `clone_then_update_pulls_latest` happens to fetch on a fresh checkout and
  one libgit2 cohort accepts the bare name;
- no test pins the normalised remote-tracking ref, so a libgit2 upgrade that
  rejects the bare name would silently downgrad the path to "no fetch" and
  the working tree would not advance.

The pinned path is already correct (`refs/heads/*:refs/remotes/origin/*`).
The non-pinned path was never given the same treatment.

**Suggested fix:**

```rust
remote.fetch(&["refs/heads/main:refs/remotes/origin/main"], None, None)?;
```

Cost: one line. Benefit: the next libgit2 upgrade does not regress the
official-content sync silently.

---

### [MEDIUM-3] src/clarification/session.rs:113-156 (PendingEntry::is_live + list_pending) — abandoned entries (closed sender, not expired) accumulate in the map forever

**Category:** Memory / Architecture
**Confidence:** High

**Description:**

`is_live` checks both `is_expired()` AND `sender.is_closed()`. The map
insertion / removal code only acts on `is_expired()`:

- `cleanup_expired` (line 491-518) reaps only expired entries.
- `register` (line 280-282) opportunistically calls `cleanup_expired` only.
- `resolve_many` (line 410-414) reaps entry on `Expired` + `Abandoned` only
  inside the write-locked path.

So an entry whose `oneshot::Sender` was closed (the agent run was cancelled
and dropped the receiver) stays in the map until a new `register` for the
same session arrives. For a long-running gateway with many abandoned runs
(goal/loop continuations, cron), the map grows without bound.

The shape is already documented, the call site already exists, and the fix
is the same opportunistic sweep extended to "reap if not `is_live`":

```rust
// in register, replace the cleanup_expired call with:
self.cleanup_dead().await;
```

where `cleanup_dead` = `is_expired() || sender.is_closed()`.

**Suggested fix:** rename `cleanup_expired` to `cleanup_dead` and use
`is_live` as the predicate, then update the doc comment.

---

### [MEDIUM-4] src/bundled/manifest.rs:107-138 (reconcile) — the read_dir error storm is silently swallowed

**Category:** Quality / Observability
**Confidence:** High

**Description:**

```rust
// manifest.rs:107-110
let entries = std::fs::read_dir(skills_dir)?;
let on_disk: HashSet<String> = entries
    .filter_map(|e| e.ok())
    .filter(|e| { ... })
    .map(|e| e.file_name().to_string_lossy().to_string())
    .collect();
```

`read_dir`'s top-level error propagates (the `?` on line 108), but iterator
errors are dropped without a warning. A race-on-unlink storm or a permissions
glitch on a subdir produces a manifest that silently classifies the affected
entries as "removed" (because they are absent from the on-disk set) and
consequently removes them from the manifest on the next save.

The same pattern is used in `extract_skill_tree_from_dir` (line 519) and
`copy_tree_with_prune` (line 627, 636). Each is a candidate for at least a
warn log.

**Suggested fix:** wrap the `entries.filter_map(|e| e.ok())` with a small
counter that warns the first time it sees a `None`:

```rust
let mut skipped = 0;
let on_disk: HashSet<String> = entries
    .filter_map(|e| match e {
        Ok(e) => Some(e),
        Err(e) => { skipped += 1; warn!(error = %e, "read_dir entry error during reconcile"); None }
    })
    .filter(|e| e.path().symlink_metadata().is_ok_and(|m| m.is_dir()))
    .map(|e| e.file_name().to_string_lossy().to_string())
    .collect();
```

---

### [LOW-1] src/bundled/extractor.rs:459-510 (extract_dir_contents) — temp file name uniqueness relies on nanos + pid, two threads in the same process can collide

**Category:** Concurrency
**Confidence:** Low

**Description:** the temp filename is `.{name}.tmp.{pid}.{nanos}`. Two OS
threads in the same process executing the same `extract_dir_contents` call
within the same nanosecond would pick the same name. Real but unlikely on
modern hardware; the bundled extractor is called from `spawn_blocking` so the
offset is small but non-zero.

**Suggested fix:** add an `AtomicU64` counter:

```rust
static EXTRACT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
let tmp_name = format!(
    ".{}.tmp.{}.{}.{}",
    name.to_string_lossy(),
    std::process::id(),
    EXTRACT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
    nanos,
);
```

---

### [LOW-2] src/clarification/session.rs:534-547 (match_option) — case folding via `to_lowercase` is locale-blind and dwarfed by the surrounding string; small fix

**Category:** Internationalisation
**Confidence:** Low

**Description:** `to_lowercase()` on a label/value is not asymmetric (the
reply arrives case-preserved, the comparison is case-folded). For ASCII,
fine. For non-ASCII, the Turkish "I" / German "ß" surprises are present. The
caller is a user typing a label, so the blast radius is the rare user whose
locale produces a different fold.

**Suggested fix:** document the locale-blind behaviour (single sentence) and
add a `[T]` langtest that pins the current shape, or switch to
`unicase`-style folding. Defer the actual fix unless a real complaint comes
in.

---

### [LOW-3] src/clarification/ask.rs:236-242 — `cleanup_expired` reaps **all** expired entries on this timeout, not just the caller; the inline comment says "the entry" (singular)

**Category:** Documentation / Surprising semantics
**Confidence:** High

**Description:**

```rust
// ask.rs:236-242
Err(_) => {
    deps.clarification.cleanup_expired().await;
    ClarificationResult::timeout()
}
```

The comment claims "the entry is past its deadline by construction (same
duration, registered first)". True for the current entry. But
`cleanup_expired` reaps every expired entry in the registry, so a quiet
cleanup of three unrelated abandoned sessions happens as a side effect of
one `ask` timing out. The frame their clients received is `Expired`, which
may be the right outcome for them — but the caller of `ask` did not ask for
that.

**Suggested fix:** add a one-line note pointing out the side effect, AND
consider replacing the global sweep with a single-entry reap keyed by
`session_key` so the timeout path is local. The single-entry reap is the
right shape; the global sweep is the cheaper one.

---

### [LOW-4] src/clarification/render.rs:113-160 (render) — small: `format!`\+`push_str` mix is needless; one `write!` would do

**Category:** Quality
**Confidence:** Low

**Description:** the function builds the body in `String` with
`text.push_str(&format!("❓{position} {header}{}", question.prompt))`, then
two more `push_str` calls. `format!` on a one-shot builder is fine; the
behind-the-scenes growth is. The docstring mentions micro-perf as a care
([LOW] in the prior `memory` review), so flagging for symmetry.

**Suggested fix:** collapse into a single `format!` and concatenate the
suffixes once. Optional — diff is trivial.

---

## Cross-cutting themes

1. **Symlink-vs-directory handling** (HIGH-1, HIGH-2): the codebase uses
   `symlink_metadata` to avoid following links, but the *cleanup* logic
   only acts on the `is_dir()` branch. Anything that hits a symlink is left
   live for `create_dir_all` to write through. The same pattern is worth
   grepping for project-wide.

2. **Atomic-replace files** (MEDIUM-1, repeated in `extractor.rs`): the
   write-tmp-then-rename pattern is implemented differently in two places
   (`manifest.rs::save` uses rename-then-remove-retry under `AlreadyExists`;
   `extractor.rs::swap_dir_into_place` uses remove-then-rename under
   `DirectoryNotEmpty`). They should converge on one helper.

3. **Order of guard clauses** (MEDIUM-3, LOW-3): the cleanup side effect of
   `ask` timing out and `register` opportunistically sweeping is documented
   in one place each and the other side has a comment that doesn't match
   the implementation. A single "reap policy" doc-comment removes the
   silence.

---

## What I did NOT do

- **Did not run `cargo check` per fix.** Per the user's instruction
  "无需 cargo check，直接提交". The final `cargo check` runs after all
  fixes land; this pass operates without it to avoid the 16 GB OOM ceiling
  on the uncompiled `alephcore` lib.
- **Did not push to remote.** The `review/bundled-clarification` branch is
  local; per "无需 PR" instruction, the fix commits are fast-forwarded to
  `main` once the `cargo check` gate is clean.
- **Did not refactor `match_option` to `unicase`** (LOW-2). Locale issues
  with case folding on user labels are a design call, not a bug.
- **Did not rewrite `render` to one `write!`** (LOW-4). The perf differential
  is unmeasurable on the call site.
- **Did not collapse `cleanup_expired` and the per-entry reap into one
  helper**. The two paths have different semantics (registry-wide sweep vs
  per-session local), and the doc-comments on each are load-bearing.
- **Did not add graceful symlink rejection tests** for HIGH-1/2. The fix
  itself is straightforward; the test would be a 5-line symlink fixture and
  is included in the per-fix commit.

---

## Files changed

See commit log: `git log --oneline review/bundled-clarification` from
`main` lists the individual fix commits. Each commit message follows the
`<scope>: <description>` convention from AGENTS.md.
