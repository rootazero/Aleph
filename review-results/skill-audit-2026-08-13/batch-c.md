# Batch C — Runtime guards & registry

Scope: `src/skill/registry.rs` (184), `src/skill/eligibility.rs` (368), `src/skill/guard.rs` (423).
Worktree `/home/zou/data/workspace/Aleph/.worktrees/skill-audit`, read-only, no build run.

> **Tree note.** HEAD moved during this audit to `59ce3e7 skill: sever three dead abstractions the audit caught`,
> which deleted `SkillSnapshot.ineligible`, `SkillSnapshot.built_at`, and `SkillSystemError`.
> The three files in this batch are **byte-identical before and after** that commit; every line number below is
> against `59ce3e7`. A prior revision of this file existed with pre-commit line numbers — this report supersedes it,
> re-verified end to end, and revises two of its calls (see [C-1] and the note under [C-3]).

## Summary

| Severity | Count |
|----------|-------|
| Critical | 1 |
| High     | 2 |
| Medium   | 5 |
| Low      | 3 |
| **Total**| **11**|

## Findings (highest severity first)

### [C-1]. The guard's hidden-file skip and the loader's skill-file predicate disagree — `.<name>.skill.md` installs completely unscanned
- **File:line**: src/skill/guard.rs:236-242
- **Category**: gate-bypass / predicate-drift
- **Severity**: CRITICAL
- **Description**: `scan_skill_directory_inner` `continue`s on **any** entry whose name starts with `.` — files as well as directories — before the file/dir branch. The loader it gates (`SkillLoader::find_skill_files`, src/tools/markdown_skill/loader.rs:109-120) prunes hidden **directories only** (`if e.depth() > 0 && e.file_type().is_dir()`), and its file predicate is `name.eq_ignore_ascii_case("SKILL.md") || name.to_lowercase().ends_with(".skill.md")` (:136). `".evil.skill.md"` fails the guard's read and passes the loader's match. So a git/zip bundle shipping `evil.skill.md` renamed to `.evil.skill.md` is scanned **zero times** → verdict `Safe` → `install_allowed(Safe, Community) == true` → `load_skills_from_dir` registers it as a model-visible `MarkdownCliTool` whose body is injected as trusted instructions. This needs no model cooperation and no second step; the attacker controls only the filename. `scan_skill_directory` has exactly one production caller and it is this path.
- **Verification grep**:
  ```
  sed -n '233,245p' src/skill/guard.rs
  #   .is_some_and(|n| n.starts_with('.'))  → continue    ← applies to files too
  sed -n '109,137p' src/tools/markdown_skill/loader.rs
  #   filter_entry: `if e.depth() > 0 && e.file_type().is_dir() { return !name.starts_with('.') }`
  #   is_skill_file_static: `name.to_lowercase().ends_with(".skill.md")`
  grep -rn --include='*.rs' 'scan_skill_directory' .
  #   src/skill/guard.rs:219,221,225,245 + tests; src/gateway/handlers/markdown_skills.rs:303  ← sole prod caller
  ```
  `".evil.skill.md".to_lowercase().ends_with(".skill.md")` is `true`; `".evil.skill.md".starts_with('.')` is `true`.
- **Triage**: CONNECT
- **Proposed fix**: Derive the guard's skip set from the loader's predicate instead of restating it — scan every entry the loader would load, and keep the dot-skip only for directories (`.git`, `.obsidian`) plus a named file exception set (`.clawhub.json`). A source-level guard asserting the two predicates stay in sync is cheap here: both are single functions.

---

### [C-2]. `scan_skill_directory` is skipped entirely for single-file (`LocalPath`) installs, which the loader still loads
- **File:line**: src/skill/guard.rs:219 (contract); gate at src/gateway/handlers/markdown_skills.rs:302-325
- **Category**: gate-bypass / stub-far-end
- **Severity**: HIGH
- **Description**: The only production call to `scan_skill_directory` sits behind `if load_path.is_dir()`. `detect_source_type` (markdown_skills.rs:99-111) routes anything that is not `*.zip` / `http(s)://` / `*.git` to `SourceType::LocalPath`, which is used verbatim as `load_path`. Point `skills.install` at a local **file** — `/tmp/downloaded.skill.md` — and `is_dir()` is false, so the whole guard block is jumped. `load_skills_from_dir` then runs `WalkDir::new(<file>)`, which yields the file itself at depth 0, passes `is_file()`, matches `is_skill_file_static`, and registers it. Same terminal state as [C-1] via a different door: the gate is not bypassed by a trick, it is simply never entered for one of the three source types.
- **Verification grep**:
  ```
  sed -n '298,325p' src/gateway/handlers/markdown_skills.rs
  #   if load_path.is_dir() { let verdict = scan_skill_directory(&load_path); ... }
  grep -n 'fn detect_source_type' -A 13 src/gateway/handlers/markdown_skills.rs
  #   else { SourceType::LocalPath }        ← catch-all, no is_dir() assertion
  sed -n '96,124p' src/tools/markdown_skill/loader.rs
  #   WalkDir::new(&base_dir) ... .filter(|e| e.file_type().is_file())
  ```
- **Triage**: CONNECT
- **Proposed fix**: Replace the `is_dir()` guard with an unconditional one: `let verdict = if load_path.is_dir() { scan_skill_directory(&load_path) } else { scan_content(<file name>, &fs::read(&load_path)?) };` — i.e. make "what gets scanned" a total function of `load_path` rather than a branch that has a silent third case.

---

### [C-3]. `IneligibilityReason::OsNotSupported` and `::Disabled` are produced but matched nowhere
- **File:line**: src/skill/eligibility.rs:31 / :33 (variants), produced at :92 and :105
- **Category**: lossy-projection / dead-variant
- **Severity**: HIGH
- **Description**: The only destructuring of `IneligibilityReason` in the repo is `SkillStatusEntry::build` (src/skill/status.rs:81-88), which handles the four `Missing*` variants and swallows the other two with `_ => {}`. `MissingRequirements` (status.rs:20-25) has `bins` / `env` / `config` and nothing that could carry them. Symptoms, both user-visible: **(a)** a manifest declaring `eligibility.os: [darwin]` on Linux renders as `eligible: false`, `disabled: false`, `missing: {[],[],[]}` → `matches_filter(NeedsSetup)` is true (status.rs:143), so Panel/CLI/`skill_status` file it under "Needs Setup" **with an empty requirements list** and zero install options, inviting remediation that cannot succeed; **(b)** a manifest-level `eligibility.enabled: false` produces `IneligibilityReason::Disabled`, but `disabled` is computed only from `entry_config` (status.rs:70), so the skill lands in the **NeedsSetup** bucket rather than **Disabled**. Severity is raised from the prior revision's MEDIUM because HEAD (`59ce3e7`) deleted `SkillSnapshot.ineligible`, which was the last field in the repo that even held these reasons — there is now no remaining escape route for the information.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'IneligibilityReason' .
  #   producers: eligibility.rs:92 Disabled, :105 OsNotSupported, :112/:120/:127/:134 Missing*
  #   consumers: status.rs:82,83,86,87  → Missing* only; `_ => {}` at :88
  #   (post-59ce3e7: zero hits in snapshot.rs)
  grep -rn --include='*.rs' 'OsNotSupported' .        # decl + construct only, no match arm
  grep -n 'pub struct MissingRequirements' -A 5 src/skill/status.rs   # bins / env / config
  ```
- **Triage**: CONNECT
- **Proposed fix**: Add `os: Vec<String>` to `MissingRequirements`, OR the `Disabled` arm into `SkillStatusEntry.disabled`, and **replace the `_ => {}` with exhaustive arms** so the next variant added to `IneligibilityReason` is a compile error instead of a silent drop — the `_` is what let both of these through.

---

### [C-4]. `TrustLevel::Builtin` has zero producers — the `(Dangerous, Builtin)` arm is unreachable
- **File:line**: src/skill/guard.rs:23 (variant), :206 (arm)
- **Category**: dead-code / stub-far-end
- **Severity**: MEDIUM
- **Description**: `TrustLevel` is constructed at exactly three production sites: `markdown_skills.rs:304` (`Community`) and `skill_manage.rs:195`, `:526` (`Trusted`). `Builtin` appears only in its own declaration, the `install_allowed` arm, and one unit test (guard.rs:370). The carve-out that lets Aleph-shipped skills carry `Dangerous` content is structurally dead — and correspondingly, bundled skills never reach the guard at all: they flow through the dir-based `skill_system.init` path, which calls nothing in `guard.rs`.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'TrustLevel::Builtin' .
  #   src/skill/guard.rs:206 (arm), :370 (test)          ← no producer
  grep -rn --include='*.rs' 'scan_skill_directory\|scan_content\|install_allowed' src/bundled/ src/extension/
  #   (no output)
  ```
- **Triage**: DECIDE — bundled content is trusted by construction and never scanned today, so `Builtin` has neither a producer nor a use case.
- **Proposed fix**: Delete `TrustLevel::Builtin` and the `(Dangerous, Builtin) => true` arm; if bundled skills should instead be scanned, add the call at the bundled-copy site in the same change rather than leaving the variant as a placeholder.

---

### [C-5]. `Finding.file` is written on every finding and read by nobody
- **File:line**: src/skill/guard.rs:33
- **Category**: inert-field
- **Severity**: MEDIUM
- **Description**: Both `scan_content` (:147, :162) and the oversize path (:258) allocate a per-finding `String` label, but all three consumers project the vector to `pattern_id` only. This bites hardest on the one caller that scans a whole tree: `markdown_skills.rs:307` rejects with `"skill bundle blocked by security scan (Dangerous): reverse_shell_devtcp"` — the operator is never told **which** file in the bundle tripped it, even though the scanner knew and paid to record it.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'findings' src/gateway/handlers/markdown_skills.rs src/builtin_tools/skill_manage.rs
  #   markdown_skills.rs:307   .map(|f| f.pattern_id)
  #   skill_manage.rs:194,527  .map(|f| f.pattern_id)
  grep -rn --include='*.rs' '\.file\b' src/gateway/handlers/markdown_skills.rs src/builtin_tools/skill_manage.rs
  #   (no output)   ← zero readers repo-wide
  ```
- **Triage**: CONNECT
- **Proposed fix**: Render `format!("{}:{}", f.file, f.pattern_id)` (deduped by pair) at markdown_skills.rs:307; the two single-file callers in `skill_manage` can keep the bare id since the filename is already in the request.

---

### [C-6]. `ThreatLevel::Caution` branch in the markdown-skills install path is unreachable
- **File:line**: src/gateway/handlers/markdown_skills.rs:318-323 (contract owned by src/skill/guard.rs:201-209)
- **Category**: dead-branch / contract-drift
- **Severity**: MEDIUM
- **Description**: `install_allowed(Caution, Community)` is `false` by construction (guard.rs:204), so the block at :304 returns an error for **every** `Caution` verdict. The follow-on `if matches!(verdict.level, ThreatLevel::Caution)` can therefore never be true, and its `warn!` text — *"caution-level security findings; proceeding"* — describes a state this call site cannot reach. A reader concludes Community installs warn-and-proceed on `Caution`; they are rejected. Reported here because verifying `install_allowed`'s call sites is this batch's item #4; the edit lands in the handler.
- **Verification grep**:
  ```
  sed -n '300,325p' src/gateway/handlers/markdown_skills.rs
  sed -n '201,209p' src/skill/guard.rs
  #   (ThreatLevel::Caution, TrustLevel::Community) => false
  ```
- **Triage**: CUT (or DECIDE, if Community was meant to be warn-and-proceed)
- **Proposed fix**: Delete the unreachable block and fold the `Caution` pattern list into the rejection message at :311, so the operator at least learns which patterns fired.

---

### [C-7]. `SkillRegistry::{len, is_empty, clear}` have no non-test callers
- **File:line**: src/skill/registry.rs:69, :75, :85
- **Category**: dead-code
- **Severity**: MEDIUM
- **Description**: `SkillRegistry` is referenced from exactly two modules — `mod.rs` and `snapshot.rs` — and every method call from them is enumerated in the wired list below. `len`, `is_empty` and `clear` appear only in this file's own `#[cfg(test)]` module. `clear` is specifically not needed by the swap path: `rescan_dirs` builds a fresh registry and assigns over the old one (mod.rs:498-510).
- **Verification grep**:
  ```
  grep -rn 'registry\.' src/skill/ | grep -v '^src/skill/registry.rs'
  #   mod.rs: replace:125 get:153,398,454 register:195(cfg(test)) list_all:161,217 remove:463 register_all:503
  #   snapshot.rs: iter:87
  grep -rn --include='*.rs' 'registry.len()\|registry.is_empty()\|registry.clear()' src/skill/
  #   (no output)
  ```
- **Triage**: CUT
- **Proposed fix**: Delete the three methods; the file's own tests can assert via `list_all().len()` and re-instantiation.

---

### [C-8]. `EligibilityResult::is_eligible()` has no non-test callers
- **File:line**: src/skill/eligibility.rs:22
- **Category**: dead-code
- **Severity**: MEDIUM
- **Description**: Every production consumer matches the enum directly rather than calling the convenience predicate — `status.rs:78-79` and `snapshot.rs:99-101`. The only `is_eligible()` call sites in the repo are the ten assertions inside `eligibility.rs`'s own test module.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'is_eligible' . | grep -v 'src/skill/eligibility.rs'
  #   src/domain/skill.rs:797  ← a test *name* (test_eligibility_spec_default_is_eligible), not a call
  ```
- **Triage**: CUT
- **Proposed fix**: Delete `is_eligible`; the tests can use `matches!(result, EligibilityResult::Eligible)`, which `present_config_key_passes` (:326) already does.

---

### [C-9]. `merge_verdicts` is `pub` + re-exported with zero external callers
- **File:line**: src/skill/guard.rs:181 (re-export src/skill/mod.rs:27)
- **Category**: dead-api-surface
- **Severity**: LOW
- **Description**: The only caller is `scan_skill_directory` at guard.rs:222, one line below in the same file. The function is live; its `pub` visibility and the `mod.rs` re-export are not — no code outside `guard.rs` merges verdicts, because no code outside `guard.rs` ever holds more than one.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' '\bmerge_verdicts\b' .
  #   src/skill/mod.rs:27 (re-export), src/skill/guard.rs:181 (decl), :222 (only call)
  ```
- **Triage**: CUT (visibility only)
- **Proposed fix**: Make it private (`fn merge_verdicts`) and drop it from the `pub use guard::{…}` list in mod.rs:26-29.

---

### [C-10]. `MAX_SCAN_BYTES` is `pub` with no reader outside its own file
- **File:line**: src/skill/guard.rs:137
- **Category**: dead-api-surface
- **Severity**: LOW
- **Description**: Read only at guard.rs:254, and not re-exported from `mod.rs`, so no crate-external consumer can even name it. (The same-named `sandbox::command_policy::MAX_SCAN_BYTES` is a private `usize` in an unrelated module — no relation, but the collision is worth knowing before grepping.)
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'MAX_SCAN_BYTES' . | grep -v 'sandbox/command_policy'
  #   src/skill/guard.rs:137 (decl), :254 (only use)
  ```
- **Triage**: CUT (visibility only)
- **Proposed fix**: Demote to a private `const`. Keep the doc comment — it documents a real OOM guard.

---

### [C-11]. `EligibilityService::evaluate_spec` is `pub` with no external caller
- **File:line**: src/skill/eligibility.rs:86
- **Category**: dead-api-surface
- **Severity**: LOW
- **Description**: Called once from `evaluate` (:79) and otherwise only from two tests in this file (:308, :325). Every production path enters through `evaluate(&manifest, &config)`. Lower confidence than C-9/C-10 that this should be cut: taking an `EligibilitySpec` directly is a plausible seam for a manifest-less caller, and it is exactly what the two `required_config` tests exercise.
- **Verification grep**:
  ```
  grep -rn --include='*.rs' 'evaluate_spec' .
  #   src/skill/eligibility.rs:79 (internal), :86 (decl), :308, :325 (tests)
  ```
- **Triage**: DECIDE
- **Proposed fix**: Demote to private and have the two tests go through `evaluate` with a synthetic manifest (the file already has `manifest_with_eligibility` for that), or keep `pub` and document why the spec-level seam exists.

---

## Verified wired (no-op, do NOT re-flag)

**`src/skill/registry.rs`** — live consumers are `mod.rs` and `snapshot.rs` only:
- `SkillRegistry::new` — mod.rs:92, :498
- `SkillRegistry::register_all` — mod.rs:503 (`rescan_dirs`)
- `SkillRegistry::register` — live **only** via `register_all` (registry.rs:46); its sole direct call sites are mod.rs:195 (inside `#[cfg(test)] insert_manifests_for_test`) and snapshot.rs tests. Not flagged: it is the primitive `register_all` is built from.
- `SkillRegistry::replace` — mod.rs:125 (`reload_file` hot-reload path)
- `SkillRegistry::get` — mod.rs:153 (`get_skill`), :398 (`install_dependency`), :454 (`remove_skill`)
- `SkillRegistry::list_all` — mod.rs:161 (`list_skills`), :217 (`full_status`)
- `SkillRegistry::remove` — mod.rs:463
- `SkillRegistry::iter` — snapshot.rs:87

**`src/skill/eligibility.rs`**
- `EligibilityService` / `::new` — mod.rs:70 (field), :96; snapshot.rs:78 (param)
- `EligibilityService::evaluate` — mod.rs:220 (`full_status`), snapshot.rs:99 (`SkillSnapshot::build`)
- `EligibilityResult::{Eligible, Ineligible}` — status.rs:78-79, snapshot.rs:100/121, builtin_tools/skill_status.rs:150
- `IneligibilityReason::{MissingBinary, MissingAnyBinary, MissingEnv, MissingConfig}` — status.rs:82-87
- `current_os()` — installer.rs:84 (`filter_install_specs_for_current_os`), :332
- `config_get_path` — private helper, used at :133

**`src/skill/guard.rs`**
- `scan_content` — skill_manage.rs:193 (`vet_skill_md`), :525 (`write_file`)
- `scan_skill_directory` — markdown_skills.rs:303 (sole production caller; see C-1/C-2 for its coverage gaps)
- `install_allowed` — markdown_skills.rs:304, skill_manage.rs:195, :526
- `ScanVerdict` (skill flavour) — constructed and consumed at all four sites above. Note the unrelated same-named types in `gateway/resume_coordinator.rs` and `memory/content_scanner.rs`; `memory::content_scanner` also exports a different `scan_content`.
- `ScanVerdict.level` / `.findings` — read at markdown_skills.rs:304/307/318, skill_manage.rs:194/195/201/526/527
- `ThreatLevel::{Safe, Caution, Dangerous}` — all three produced by `scan_content`; `Caution` is reachably matched at skill_manage.rs:201, since `install_allowed(Caution, Trusted) == true`
- `TrustLevel::{Trusted, Community}` — skill_manage.rs:195/526, markdown_skills.rs:304
- `Finding.pattern_id` / `Finding.level` — `pattern_id` read at three call sites; `level` drives the max in `scan_content` and `merge_verdicts`
- `PATTERNS` / `PATTERN_SET` / `scan_skill_directory_inner` — internal, live
