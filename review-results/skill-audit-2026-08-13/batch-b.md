# Batch B — Parsing pipeline

Scope: `src/skill/manifest.rs`, `src/skill/preprocess.rs`, `src/skill/prompt.rs`.
Tree: `.worktrees/skill-audit` @ `59ce3e702` (a fix commit from another batch landed
mid-audit; it touched `mod.rs` / `snapshot.rs` only — none of my three files. All
greps below were re-run against the post-commit tree).

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 4 |
| Low      | 2 |
| **Total**| **7**|

## Findings (highest severity first)

### [B-1]. `${ALEPH_SESSION_ID}` never resolves — `with_session` has zero production callers
- **File:line**: src/skill/preprocess.rs:80 (producer) / src/builtin_tools/skill_reader/read.rs:366 (the only production consumer)
- **Category**: severed-wire / stub-far-end
- **Severity**: HIGH
- **Description**: `SkillPreprocessContext.session_id` is `None` on every production
  path: the sole production constructor is `SkillPreprocessContext::new(skill_dir)`
  with no `.with_session(..)`. Consequences, all silent: `expand_template_vars`
  takes the `!has_session` early return so `${ALEPH_SESSION_ID}` is left literal in
  every skill body, and `run_snippet`'s `command.env("ALEPH_SESSION_ID", …)`
  (preprocess.rs:244) never executes, so opted-in inline-shell snippets never see
  the variable either. Both are documented capabilities (module doc lines 9–16,
  `with_session` doc line 78, and `read.rs:360`'s own comment claims the token is
  expanded). Nothing errors; the token just renders as literal text to the model.
- **Verification grep**:
  `grep -rn --include="*.rs" "with_session\b" . | grep -v ^./target/` → the only
  `SkillPreprocessContext` hits are the definition (preprocess.rs:80) and one unit
  test (preprocess.rs:308). Every other hit is an unrelated `with_session` on
  `RawMemory` / `EventFilter` / `PtyManager`.
  `grep -rn "SkillPreprocessContext::new" .` → read.rs:366 + preprocess.rs:279 (test helper).
- **Triage**: DECIDE — this is backlog item H9 in `docs/reference/FEATURE_LOCATOR_BACKLOG.md:142`
  ("线入 session id **或** 删 session_id/with_session/token 死面"), re-confirmed live today.
  The decision has not been made; the wire is still cut.
- **Proposed fix**: CONNECT — `read.rs:366` already runs inside a tool call, so thread
  the ambient session key in: `SkillPreprocessContext::new(skill_dir.clone()).with_session(<ambient session id>)`.
  If no session id is reachable at that seam, CUT: delete `session_id`,
  `with_session`, `TOKEN_SESSION_ID`, the `command.env("ALEPH_SESSION_ID", …)` arm
  and the three doc paragraphs that promise it.

### [B-2]. `build_skills_prompt_xml` (no-budget wrapper) is unreachable in production
- **File:line**: src/skill/prompt.rs:195 (fn) / src/thinker/layers/skill_instructions.rs:85 (its only caller)
- **Category**: dead-code / unreachable-arm
- **Severity**: MEDIUM
- **Description**: The layer picks `build_skills_prompt_xml_with_budget` when
  `config.skill_prompt_budget` is `Some`, else falls back to `build_skills_prompt_xml`.
  That `None` arm cannot be taken: both `skill_prompt_budget` and `eligible_skills`
  are derived from the *same* `skill_snapshot: Option<_>` (prompt_build.rs:516–519),
  and the layer early-returns (skill_instructions.rs:32–35) unless `eligible_skills`
  is `Some(non-empty)`. `eligible_skills.is_some()` ⟹ snapshot was `Some` ⟹ budget
  is `Some`. The only other production `PromptConfig` producer
  (`subagent_spawner/mod.rs:411`) uses `..PromptConfig::default()`, i.e.
  `eligible_skills: None` → the layer returns before reaching either call.
- **Verification grep**:
  `grep -rn --include="*.rs" "PromptConfig {" . | grep -v ^./target/` → 2 production
  producers (`prompt_build.rs:561`, `subagent_spawner/mod.rs:411`); all others are
  `#[cfg(test)]` or `tests/`.
  `sed -n '514,520p' src/orchestrator/harness_bridge/prompt_build.rs` → both fields
  derived from one `skill_snapshot`.
- **Triage**: CUT
- **Proposed fix**: Collapse the layer to a single call —
  `build_skills_prompt_xml_with_budget(&filtered, &input.config.skill_prompt_budget.unwrap_or_default())`
  — and delete `build_skills_prompt_xml` plus its `pub use` in `src/skill/mod.rs:36`,
  rewriting the ~10 tests in prompt.rs that use it to call the budgeted form with
  `SkillPromptBudget::default()`.

### [B-3]. `SkillPromptBudget::unlimited()` has zero non-test callers
- **File:line**: src/skill/prompt.rs:121
- **Category**: dead-code
- **Severity**: MEDIUM
- **Description**: A `pub const fn` constructor for the "no limits" budget. Both
  call sites repo-wide are unit tests inside prompt.rs itself. Nothing in config,
  snapshot, or the prompt layer can produce it — an operator wanting an unlimited
  budget writes `max_skills = 0` / `max_chars = 0` in `skills.toml` instead, which
  is the same value without going through this constructor.
- **Verification grep**:
  `grep -rn --include="*.rs" "SkillPromptBudget::unlimited\|budget.unlimited" . | grep -v ^./target/`
  → prompt.rs:479 and prompt.rs:607, both inside `mod tests`.
- **Triage**: CUT
- **Proposed fix**: Delete `unlimited()`; the two tests construct
  `SkillPromptBudget { max_skills: 0, max_chars: 0 }` inline (that literal already
  appears in four other tests in the same file).

### [B-4]. The derived-id traversal predicate lives at 4 consumers and 0 producers
- **File:line**: src/skill/manifest.rs:162-169 (`parse_skill_content` id derivation)
- **Category**: logic-bug / duplicated-predicate
- **Severity**: MEDIUM
- **Description**: The id is built from the model-/author-supplied `name:` frontmatter
  by lowercasing and joining whitespace-separated tokens with `-`. Path separators
  and `..` survive verbatim — `name: ../../evil` yields the id `../../evil`. The
  parser ships no guard; instead the *same* three-clause predicate
  (`contains("..") || contains('/') || contains('\\')`) is written out four separate
  times at consumers: `skill/mod.rs:255` (`owning_dir`), `skill/mod.rs:285`
  (`skill_dir_for_id`), `skill_manage.rs:175` (`require_skill_id`), `skill_manage.rs:397`
  (`create`, whose comment explicitly documents that the parser does not do this).
  No current consumer is unguarded — so this is not exploitable today — but the
  producer is the only place that knows the id's provenance, and a fifth consumer
  that joins a registry-sourced id onto a root inherits the hole silently.
- **Verification grep**:
  `grep -rn --include="*.rs" 'contains("\.\.")' src/skill/ src/builtin_tools/skill_manage.rs`
  → 4 hits, all identical three-clause forms; `grep -n "fn new" -A 4 src/domain/skill.rs`
  → `SkillId::new` is a bare newtype wrapper with no validation.
- **Triage**: CONNECT
- **Proposed fix**: Make the parser refuse or strip the characters as it slugifies
  (filter `/`, `\`, and `.` runs out of the token stream before `SkillId::new`), then
  have the four consumers call one shared `SkillId::is_path_safe()` rather than each
  re-spelling the predicate.

### [B-5]. A UTF-8 BOM defeats `split_frontmatter`; the skill never loads and the warning misattributes the cause
- **File:line**: src/skill/manifest.rs:366-369
- **Category**: logic-bug / silent-failure
- **Severity**: MEDIUM
- **Description**: `content.trim_start()` does not strip U+FEFF — the BOM is not
  `White_Space` in Unicode, and this repo's own `security/unicode_guard.rs:32`
  classifies it as an *invisible* char, not whitespace. A SKILL.md saved by a
  Windows editor or produced by PowerShell `>` redirection therefore fails
  `starts_with("---")` and returns `NoFrontmatter`. The whole skill is then dropped
  at every entry point: `mod.rs:609` warns `"no YAML frontmatter found"` (which is
  false — the frontmatter is right there, one byte in), `mod.rs:305` and
  `skill_reader/list.rs:140` swallow it entirely, and `preprocess.rs:142`'s
  `frontmatter_allows_inline_shell` silently reads `false`. The misleading message
  is what makes this expensive: the author looks for a YAML problem that isn't there.
- **Verification grep**:
  `grep -rn --include="*.rs" "feff\|FEFF\|strip_bom" src/` → 10 hits, none in any
  frontmatter parser (`skill/manifest.rs`, `agents/loader.rs`, `thinker/soul.rs`,
  `memory/notes/note/parsing.rs` all lack it); only `file_ops/apply_patch.rs:897`
  strips a leading BOM.
- **Triage**: CONNECT
- **Proposed fix**: In `split_frontmatter`, strip a leading BOM before trimming:
  `let trimmed = content.trim_start_matches('\u{feff}').trim_start();`. One line,
  and it fixes the inline-shell opt-in probe at the same time.

### [B-6]. Comment claims hyphen collapsing that the code does not do
- **File:line**: src/skill/manifest.rs:158-168
- **Category**: name-drift / documentation-drift
- **Severity**: LOW
- **Description**: The comment says the id is built by "lowercase, replace any
  Unicode whitespace run with a single hyphen, **then collapse consecutive
  hyphens**". The code only does the first two: `split_whitespace().join("-")`. A
  name like `Foo - Bar` tokenises to `["foo","-","bar"]` and yields `foo---bar`;
  `Foo -- Bar` yields `foo----bar`. Per CLAUDE.md §0, the comment is the lying half.
- **Verification grep**: read of manifest.rs:162-169 — no `dedup`, no
  `replace("--", "-")`, no regex anywhere in the function.
- **Triage**: DECIDE
- **Proposed fix**: Either delete the "then collapse consecutive hyphens" clause, or
  implement it (`while id_str.contains("--") { id_str = id_str.replace("--", "-"); }`)
  — but note that changing the id changes on-disk directory lookups for any
  already-installed skill whose name contains a bare `-` token, so the cheap fix is
  to correct the comment.

### [B-7]. Four `pub` items in these files have no reader outside their own module
- **File:line**: src/skill/preprocess.rs:110 (`expand_template_vars`), :134 (`frontmatter_allows_inline_shell`); src/skill/prompt.rs:76 (`DEFAULT_MAX_SKILLS_IN_PROMPT`), :84 (`DEFAULT_MAX_SKILLS_PROMPT_CHARS`)
- **Category**: over-exposed-api
- **Severity**: LOW
- **Description**: All four live in `pub mod`s (so they are crate-external API) but
  every non-test reference is inside their defining file: the two functions are
  called only by `preprocess_skill_content` (preprocess.rs:94, :98) and are not in
  the `mod.rs:35` re-export list; the two consts are read only by
  `SkillPromptBudget::default()` (prompt.rs:112-113) and by rustdoc `[…]` links.
  Not a severed wire — just surface that invites a second caller with different
  expectations.
- **Verification grep**: the per-symbol sweep in the transcript — for each of the four,
  `grep -rn --include="*.rs" "\b<sym>\b" . | grep -v ^./target/` returns only
  `src/skill/preprocess.rs` / `src/skill/prompt.rs` lines (definition, internal
  call, `mod tests`).
- **Triage**: CUT (visibility only — keep the code)
- **Proposed fix**: Demote all four to `pub(crate)` (or `pub(super)`), which also
  makes any future external consumer a compile error rather than a silent second
  entry point.

## Verified wired (no-op, do NOT re-flag)

- `parse_skill_content` — 5 non-test consumers: `skill_manage.rs:383/451/498`,
  `skill_reader/list.rs:140`, `run_loop/project_context.rs:112`,
  `hub/official_skills.rs:64`. Live.
- `parse_skill_file` — genuinely a distinct wrapper (adds `read_to_string`), and it
  has its own consumers that `parse_skill_content` does not serve:
  `skill/mod.rs:122` (`reload_file`), `:305` (`skill_dir_for_id`), `:609`
  (`scan_directory`), plus `manifest.rs:338` (`automation_notice`). Not a redundant
  double. (The identically named `src/tools/markdown_skill/parser.rs::parse_skill_file`
  is an unrelated function on a different type — name collision only.)
- `SkillParseError` — all three variants are constructed on live paths: `Io` via
  `From<io::Error>` at manifest.rs:146, `Yaml` via `From<serde_yaml::Error>` at
  :156, `NoFrontmatter` at :368/:374/:395. No orphan variant. The enum itself has
  4 external consumers.
- `automation_notice` — one live consumer, `builtin_tools/hub/install_run.rs:112`,
  and the `path` it receives is the skill *directory* (`install_git_skill` returns
  the dir; `hub/install.rs:343`, asserted by `install_git_skill_clones_subdir_and_stamps_source`
  which checks `path.join("SKILL.md").exists()`), so the `skill_dir.join("SKILL.md")`
  inside `automation_notice` resolves. Wired end to end.
- `split_frontmatter` — consumed by `parse_skill_content` (manifest.rs:155) and by
  `preprocess::frontmatter_allows_inline_shell` (preprocess.rs:142). (The
  same-named functions in `agents/loader.rs`, `thinker/soul.rs`,
  `memory/notes/note/parsing.rs` are separate, unrelated implementations.)
- `preprocess_skill_content` / `SkillPreprocessContext` — live at
  `builtin_tools/skill_reader/read.rs:366-367`. (Only the `with_session` builder is
  cut — see B-1.)
- `build_skills_prompt_xml_with_budget` — live at `thinker/layers/skill_instructions.rs:84`.
- `SkillPromptBudget` (the type and both fields) — fully threaded:
  `SkillsConfig.prompt_budget` (config.rs:46) → `SkillSystem` rebuild (mod.rs:563/592)
  → `SkillSnapshot.prompt_budget` (snapshot.rs:42) → `prompt_build.rs:516` →
  `PromptConfig.skill_prompt_budget` (prompt_builder/mod.rs:59) → layer:83. Both
  `max_skills` and `max_chars` are read at prompt.rs:238-239/256-265. Only the
  `unlimited()` constructor is dead (B-3).
- `DEFERRED_LOADING_GUIDANCE` — injected at `skill_instructions.rs:92`.
- Manifest fields parsed and read downstream: `scope` → skill_instructions.rs:61;
  `bound_tool` → skill_instructions.rs:64; `when_to_use` → prompt.rs:138/164;
  `primary_env` → status.rs:95/130 + mod.rs:179; `homepage` → status.rs:125;
  `emoji` → status.rs:122; `install_specs` → status.rs:101 + mod.rs:400;
  `eligibility.enabled` → eligibility.rs:91; `user_invocable` →
  `tool_catalog_init.rs:160`; `disable_model_invocation` → `domain/skill.rs:633`
  (`is_model_visible`); `automation` → `automation_notice`. No parsed-but-unread field.
- `InvocationPolicy.command_dispatch` is hardcoded `None` at manifest.rs:198 and has
  no `Some` producer repo-wide — but `DispatchSpec` already carries an explicit
  `#[deprecated(note = "No production code consumes DispatchSpec …")]`
  (`domain/skill.rs:371`), i.e. it is a known, annotated cut awaiting removal in the
  domain layer, not a new finding for this batch.

## Scope notes / not investigated

- Whether `automation:` blocks in **plugin-bundled** skills (installed via
  `InstallOutcome::Plugin`, which does not call `automation_notice`) or in the
  `skills/` submodule ever surface: the `skills/` and `plugins/` submodules are not
  checked out in this worktree (`ls skills/` → 0 entries), so I could not confirm
  whether any bundled SKILL.md declares one. Flagging as a question for whoever owns
  the hub/install batch rather than as a finding.
- No `cargo check` / `cargo test` was run, per instructions. All findings are static.
