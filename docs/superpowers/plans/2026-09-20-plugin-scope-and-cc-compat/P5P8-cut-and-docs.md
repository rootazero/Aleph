# Plan — Phase P5 (CUT list / entropy reduction) + Phase P8 (documentation)

> Writer: plan-P5P8. Base: `3ddc1f2e7` (worktree HEAD `35e5f8bca` adds only the spec; `git diff --stat 3ddc1f2e7 HEAD -- src tests interfaces shared qa` is empty, verified).
> Spec: `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md` §3.9, §1.2, §7, §8.
> Evidence: `…-evidence/scan-aleph-plugins.md` §5, §7 rows 3/4/11–19, `rpc_census.sh`.
> Read-only on the repo; every `file:line` below was re-read at HEAD and quoted.

## ⚠️ Two evidence corrections the tasks below are built on (判据 §18 — a number carries its predicate and its commit)

1. **`command.execute` is NOT a permanent error stub.** Evidence §7 row 19 says "`commands.list` IS overridden at `tool_catalog_init.rs:303`, **`command.execute` never is**". At `3ddc1f2e7` it IS overridden: `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:475-483` registers `command.execute` → `commands::handle_execute(req, parser, registry)`. The base registration at `handlers/mod.rs:346-352` is the same kind of ToolCatalog-less fallback as `commands.list` (`:342-344`). What IS true: **zero clients** (multi-line grep over Panel/TUI/CLI/qa/shared, Task P5.4 Step 1). The CUT still stands on the zero-client ground (`chat.send` + `commands.list` answer the same question), but the premise in the task is the corrected one. The lead should re-confirm (Open question 1).
2. **The grep number.** Evidence §5.1's 286 lines / 156 files is for `openclaw|clawhub|claw` (case-insensitive). For the two names that actually get cut, `rg -n -i 'openclaw|clawhub' src | wc -l` = **245 lines / 128 files** at `3ddc1f2e7`, of which **16 are code lines** (not `//`-led) and only **4 of those survive** the cut (Task P5.1 Step 1 lists them). The evidence's ≈50-LOC estimate for OpenClaw-only code is confirmed (spec.rs `:85-124` = 40 lines + 1 field + fixtures).

Everything else in evidence §5 and §7 rows 3, 4, 11–18 re-verified as stated.

---

## Phase P5 — CUT list (entropy reduction)

Ownership boundaries (do NOT duplicate here): `PluginStatus::Overridden` removal → **P3**; `reload_plugin(id)` narrow twin removal → **P1**; `plugins.{load,unload}` → **P1.10**; `plugins.executeCommand` + the WASM command chain → **P4.7d** (R5.1); Task P5.3 is only the post-condition census for that class.

### Task P5.1: OpenClaw DTOs + attribution comments + fixture wording + test renames, with a source-level census guard

**Files:**
- Modify: `src/tools/markdown_skill/spec.rs:1-4, 28, 37, 40, 47-50, 85-124, 253-256, 268`
- Modify: `src/tools/markdown_skill/mod.rs:4`
- Modify: `src/tools/markdown_skill/loader.rs:166`
- Modify: `src/tools/markdown_skill/tool_adapter.rs:143`
- Modify: `src/tools/markdown_skill/executor.rs:690`
- Modify: `src/skill/guard.rs:236`
- Modify: `src/hub/catalog_client.rs:326, 345`
- Modify: `src/hub/types.rs:205`
- Modify: `src/security/content_sanitizer.rs:862`
- Modify: `src/gateway/server/flood_guard.rs:80`
- Modify: `src/gateway/channel_health_monitor.rs:431`
- Modify: `tests/fixtures/markdown_skills/echo-basic/SKILL.md:3, 11`
- Modify: `docs/reference/SKILL_MODEL_TAXONOMY.md:14, 65, 98-100` (the type this task deletes is named there — same commit, 判据 §1; the lead's brief said `:100-110` for the deadline block — at `3ddc1f2e7` the heading is `:108` and the date line `:115`, see P5.8)
- Test: `src/tools/markdown_skill/spec.rs` (new census test in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from other phases.
- Produces: `SkillMetadata` loses field `openclaw`; types `OpenClawMetadata`, `OpenClawInstallSpec` no longer exist. No other phase names them (verified: `rg -n 'OpenClawMetadata|OpenClawInstallSpec|\.openclaw\b' src tests interfaces shared qa crates` → only the lines listed above).

Current code being removed (quoted at `3ddc1f2e7`):

```rust
// spec.rs:1-4
//! Aleph Skill Specification
//!
//! Data structures for parsing and representing Markdown-based CLI skills.
//! Compatible with `OpenClaw` SKILL.md format while adding Aleph-specific extensions.

// spec.rs:28
    /// `OpenClaw` + Aleph metadata

// spec.rs:37
/// Skill metadata (`OpenClaw` compatible + Aleph extensions)

// spec.rs:40
    /// `OpenClaw` compatibility: required binaries

// spec.rs:47-50
    /// `OpenClaw` metadata namespace (`ClawHub` compatibility)
    #[serde(default)]
    pub openclaw: Option<OpenClawMetadata>,
}

// spec.rs:85-124  (two structs, 40 lines — deleted whole)
/// `OpenClaw` metadata namespace — compatible with `ClawHub` skill format.
///
/// Allows SKILL.md files from `ClawHub` to work natively in Aleph.
/// Both `aleph` and `openclaw` namespaces can coexist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenClawMetadata { … }
/// `OpenClaw` install specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawInstallSpec { … }

// spec.rs:253-256 (bridge comment)
// metadata (requires.bins, aleph.security, openclaw.*, docker config) does
// not currently map onto SkillManifest's DDD-aggregate fields (EligibilitySpec,
// InvocationPolicy, InstallSpec). Phase 2 absorbs those fields onto
// SkillManifest as `markdown_cli_extras` and `openclaw_compat`; until then,

// spec.rs:268
            // Default to Global (most clawhub-installed skills land in ~/.aleph/skills/).

// mod.rs:4
//! Compatible with `OpenClaw` ecosystem while adding Aleph-specific extensions.

// loader.rs:166
            .is_none_or(|a| matches!(a.security.sandbox, SandboxMode::Host)); // Default: OpenClaw style (host execution)

// tool_adapter.rs:143
            false // OpenClaw skills default to no confirmation

// executor.rs:690
                openclaw: None,

// guard.rs:236
        // Skip hidden files/dirs (e.g. .git, .clawhub.json)

// catalog_client.rs:326
      "via":"clawhub"}]}"#;
// catalog_client.rs:345
        assert_eq!(e.via.as_deref(), Some("clawhub")); // wire `via` wins

// hub/types.rs:205
    /// Upstream provenance label (e.g. "clawhub", "github:owner"); filled from

// content_sanitizer.rs:862
    fn scrub_covers_the_openclaw_parity_families() {
// flood_guard.rs:80
    fn default_budget_matches_openclaw() {
// channel_health_monitor.rs:431
    fn config_default_matches_openclaw_reference() {
```

- [ ] **Step 1: Record the pre-cut measurement and write the failing census test**

Record (paste into the commit message):

```bash
rg -n -i 'openclaw|clawhub' src | wc -l          # 245 at 3ddc1f2e7
rg -l -i 'openclaw|clawhub' src | wc -l          # 128
rg -n -i 'openclaw|clawhub' tests | wc -l        # 2
# code lines (not //-led), 16 at 3ddc1f2e7; 4 must survive:
rg -n -i 'openclaw|clawhub' src | grep -v -E '^[^:]+:[0-9]+:\s*//'
```

Append to `src/tools/markdown_skill/spec.rs` `mod tests` (after `test_from_aleph_skill_spec_for_skill_manifest`):

```rust
    /// Source-level census: the retired upstream skill dialect must not come
    /// back as code. Its two DTOs were parsed and never read for six months;
    /// a `metadata.<dialect>.*` block in a SKILL.md is now an unknown key that
    /// serde ignores, and that is the whole compatibility story.
    ///
    /// Needles are assembled with `concat!` so this file cannot match itself.
    /// Comment lines and `// …` tails are stripped first: a parity remark
    /// ("mirrors <dialect>'s default") is attribution, not code. What remains
    /// is allowed in exactly two files — the ACP preset that HOSTS that agent
    /// (R3, not plugin compat) and the harness-name list beside it.
    #[test]
    fn the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset() {
        const NEEDLES: [&str; 2] = [concat!("open", "claw"), concat!("claw", "hub")];
        const ALLOWED: [&str; 2] = [
            "src/config/types/acp.rs",
            "src/builtin_tools/team/member_add.rs",
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders: Vec<String> = Vec::new();
        let mut allowed_hits = 0usize;
        let mut checked_files = 0usize;

        let mut stack = vec![root.join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                checked_files += 1;
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let is_allowed = ALLOWED.contains(&rel.as_str());
                for (lineno, line) in src.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    // Drop a trailing `// …` remark; the needle never sits in a
                    // string that also contains " // ".
                    let code = code.split(" //").next().unwrap_or(code);
                    let lower = code.to_lowercase();
                    if !NEEDLES.iter().any(|n| lower.contains(n)) {
                        continue;
                    }
                    if is_allowed {
                        allowed_hits += 1;
                    } else {
                        offenders.push(format!("{rel}:{} — {}", lineno + 1, code.trim()));
                    }
                }
            }
        }

        assert!(
            checked_files > 100,
            "census scanned only {checked_files} files — it is not looking where it thinks it is"
        );
        // Self-check: the ACP preset is the reason the allow-list exists. If it
        // is ever removed, this census must say so rather than pass vacuously.
        assert!(
            allowed_hits >= 4,
            "expected the ACP preset + harness list to produce >= 4 hits, found {allowed_hits} — \
             the allow-list is stale, re-derive it"
        );
        assert!(
            offenders.is_empty(),
            "the retired skill dialect is back as code (fields, types, fixtures or test names). \
             Parity comments are fine; code is not:\n  {}",
            offenders.join("\n  ")
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tools::markdown_skill::spec::tests::the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset -- --nocapture`
Expected: FAIL, offenders list contains (at least) `src/tools/markdown_skill/spec.rs:50`, `:90`, `:102`, `:107`, `src/tools/markdown_skill/executor.rs:690`, `src/hub/catalog_client.rs:326`, `:345`, `src/security/content_sanitizer.rs:862`, `src/gateway/server/flood_guard.rs:80`, `src/gateway/channel_health_monitor.rs:431` (10 lines; `tool_adapter.rs:143` / `loader.rs:166` are already cut by the `" //"` split and appear only in the raw grep).

- [ ] **Step 3: Make the cut**

`src/tools/markdown_skill/spec.rs`:

```rust
// :1-4 →
//! Aleph Skill Specification
//!
//! Data structures for parsing and representing Markdown-based CLI skills
//! (SKILL.md frontmatter + Aleph-specific `metadata.aleph.*` extensions).

// :28 →
    /// Skill metadata (`requires.bins` + Aleph extensions)

// :37 →
/// Skill metadata (required binaries + Aleph extensions)

// :40 →
    /// Required binaries (`metadata.requires.bins`)

// :47-50 → delete the three lines `/// \`OpenClaw\` metadata namespace…`, `#[serde(default)]`, `pub openclaw: …` (keep the closing `}`); `SkillMetadata` becomes:
pub struct SkillMetadata {
    /// Required binaries (`metadata.requires.bins`)
    #[serde(default)]
    pub requires: RequiresSpec,

    /// Aleph extensions (optional)
    #[serde(default)]
    pub aleph: Option<AlephExtensions>,
}

// :85-124 → delete both structs entirely (40 lines).

// :253-256 →
// metadata (requires.bins, aleph.security, docker config) does
// not currently map onto SkillManifest's DDD-aggregate fields (EligibilitySpec,
// InvocationPolicy, InstallSpec). Phase 2 absorbs those fields onto
// SkillManifest as `markdown_cli_extras`; until then,

// :268 →
            // Default to Global (most installed skills land in ~/.aleph/skills/).
```

`src/tools/markdown_skill/mod.rs:4` → `//! SKILL.md frontmatter (`metadata.requires` / `metadata.aleph.*`) plus a Markdown body.`

`src/tools/markdown_skill/loader.rs:166` → `.is_none_or(|a| matches!(a.security.sandbox, SandboxMode::Host)); // Default: host execution`

`src/tools/markdown_skill/tool_adapter.rs:143` → `false // No \`metadata.aleph.security\` block ⇒ no confirmation gate (default)`

`src/tools/markdown_skill/executor.rs:690` → delete the line `openclaw: None,`.

`src/skill/guard.rs:236` → `// Skip hidden files/dirs (e.g. .git, .DS_Store)`

`src/hub/catalog_client.rs:326` → `"via":"upstream-hub"}]}"#;` and `:345` → `assert_eq!(e.via.as_deref(), Some("upstream-hub")); // wire \`via\` wins`

`src/hub/types.rs:205` → `/// Upstream provenance label (e.g. "github:owner", "upstream-hub"); filled from`

Test renames (constants untouched):
- `src/security/content_sanitizer.rs:862` `scrub_covers_the_openclaw_parity_families` → `scrub_covers_the_llama_harmony_and_gemma_families`
- `src/gateway/server/flood_guard.rs:80` `default_budget_matches_openclaw` → `default_budget_is_ten_strikes`
- `src/gateway/channel_health_monitor.rs:431` `config_default_matches_openclaw_reference` → `config_default_is_300s_300s_10_per_hour`

`tests/fixtures/markdown_skills/echo-basic/SKILL.md`: `:3` → `description: Basic echo command (frontmatter smoke fixture)`; `:11` → `A simple echo command for testing SKILL.md frontmatter parsing.`

`docs/reference/SKILL_MODEL_TAXONOMY.md` (the type deleted above is named here; same commit):
- `:14` `| Parse an OpenClaw-style Markdown CLI tool from a SKILL.md frontmatter |` → `| Parse a Markdown CLI tool (SKILL.md frontmatter with \`metadata.aleph.input_hints\`) |`
- `:65` `A second SKILL.md frontmatter parser, originally written for OpenClaw-style Markdown CLI tools. It overlaps with Layer 2's parser at the identity + content level and diverges on metadata (it carries \`RequiresSpec\`, \`AlephExtensions { security, input_hints, docker }\`, \`OpenClawMetadata\`).` → `A second SKILL.md frontmatter parser for Markdown CLI tools. It overlaps with Layer 2's parser at the identity + content level and diverges on metadata (it carries \`RequiresSpec\` and \`AlephExtensions { security, input_hints, docker }\`; the upstream-dialect DTO it once carried was CUT 2026-09-20 — it had zero readers).`
- `:98` `        // SkillSource defaults to Global (matches typical clawhub install path).` → `        // SkillSource defaults to Global (matches the typical ~/.aleph/skills install path).`
- `:99-100` `        // CLI-tool metadata (requires.bins, security, docker, input_hints,` / `        // openclaw.*) is dropped until Phase 2 absorbs those onto SkillManifest.` → the single line `        // CLI-tool metadata (requires.bins, security, docker, input_hints) is dropped until Phase 2 absorbs those onto SkillManifest.`

- [ ] **Step 4: Run test to verify it passes, plus the whole module and the two renamed test files**

Run: `cargo test -p alephcore --lib tools::markdown_skill -- --nocapture`
Expected: PASS (all `markdown_skill` tests incl. the census).
Run: `cargo test -p alephcore --lib security::content_sanitizer gateway::server::flood_guard gateway::channel_health_monitor hub::catalog_client`
Expected: PASS, and the three renamed tests appear under their new names.
Run: `cargo test -p alephcore --features test-helpers --test '*' --no-run` (fixture file is consumed by integration tests).
Expected: compiles.

Post-cut measurement (paste into the commit message with the command):

```bash
rg -n -i 'openclaw|clawhub' src | wc -l          # expected 218 (245 − 27 lines this task touches)
rg -n -i 'openclaw|clawhub' tests | wc -l        # expected 0
rg -n -i 'openclaw|clawhub' src | grep -v -E '^[^:]+:[0-9]+:\s*//' | grep -v -E ' //.*(openclaw|clawhub)'   # expected exactly 4 lines: acp.rs:422,423,424 + member_add.rs:102
```

The 218 is derived (245 − 16 spec.rs − 1 mod.rs − 1 loader.rs − 1 tool_adapter.rs − 1 executor.rs − 1 guard.rs − 2 catalog_client.rs − 1 types.rs − 3 test names); the implementer records the measured number and, if it differs, names the line that made it differ.

Allowed remainder after this task (all comments unless noted): parity comments in gateway/providers/tools/builtin_tools/security/tasks/extension/cluster (≈214 lines), `src/config/types/acp.rs:420-429` ACP preset (**code**, 3 lines), `src/builtin_tools/team/member_add.rs:102` harness list (**code**, 1 line), `src/hub/trust.rs:128` (comment pointing at the real file `docs/engineering-reports/review-results/clawhub.md`), `ClawTeam` / `clawshell` (different projects, not matched by this needle), `interfaces/webchat/src/components/settings_sidebar.rs:287-291 clawhub_tab_is_removed` (a guard that keeps the tab deleted — `interfaces/` is outside the census root on purpose).

- [ ] **Step 5: Mutation step (record the red)**

Re-add `pub openclaw: Option<()>,` under `pub aleph` in `SkillMetadata` → run the census → expected RED: `the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset` with offender `src/tools/markdown_skill/spec.rs:<line> — pub openclaw: Option<()>,`. Revert.

- [ ] **Step 6: Commit**

```bash
git add src/tools/markdown_skill/spec.rs src/tools/markdown_skill/mod.rs src/tools/markdown_skill/loader.rs src/tools/markdown_skill/tool_adapter.rs src/tools/markdown_skill/executor.rs src/skill/guard.rs src/hub/catalog_client.rs src/hub/types.rs src/security/content_sanitizer.rs src/gateway/server/flood_guard.rs src/gateway/channel_health_monitor.rs tests/fixtures/markdown_skills/echo-basic/SKILL.md docs/reference/SKILL_MODEL_TAXONOMY.md
git commit -m "markdown_skill: cut the unread upstream-dialect DTOs and their attribution

OpenClawMetadata / OpenClawInstallSpec were parsed and never read (zero
readers of SkillMetadata.openclaw). Serde ignores the unknown key, so
files carrying that block still parse. Attribution comments, one fixture
value, one fixture prose line and three test NAMES follow; the constants
those tests pin are Aleph's and stay. Census guard added; mutation red
recorded. rg -i 'openclaw|clawhub' src: 245 -> <measured> lines (predicate
in the test doc).

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.2: Zero-client RPC group (a) — `plugin.*` singular orphans + the reversed namespace comment

**Files:**
- Modify: `src/gateway/handlers/mod.rs:364, 376-377, 379, 382-383, 385-389`
- Modify: `src/gateway/handlers/plugins/handlers/manage.rs:23-128` (delete `handle_config_get` `:23-76` / `handle_config_set` `:78-128`)
- Modify: `src/gateway/method_census.rs:312-315, 317-318`
- Modify: `src/gateway/method_authz.rs:238`
- Modify: `docs/reference/PLUGIN_SYSTEM.md:464, 468-469, 476-477, 479, 662` (same commit — the table describes the deleted methods)
- Test: `src/gateway/handlers/mod.rs` (`mod tests`), `src/gateway/method_census.rs` (existing census is the second guard)

**Interfaces:**
- Consumes: nothing.
- Produces: the six methods `plugin.list`, `plugin.installFromZip`, `plugin.enable`, `plugin.disable`, `plugin.config.get`, `plugin.config.set` no longer exist on the wire. `plugin.install` / `plugin.uninstall` / `plugin.update` / `plugin.reload` / `plugin.marketplace.*` **stay** (each has a client: `qa/plugins/drive_plugins.py:178`, `:177`, `interfaces/cli/src/commands/plugins_cmd.rs:359`, `:378`, Panel `settings/plugins.rs:909`). `plugin_manage` tool keeps `config_get`/`config_set` (`src/builtin_tools/plugin_manage.rs:212-276` calls `manager.plugin_settings` / `set_plugin_settings` directly — the tool face is the surviving face, 判据 §9).

Current code (quoted at `3ddc1f2e7`):

```rust
// handlers/mod.rs:364
        // Plugin handlers (plural — legacy namespace, kept for backward compatibility)
// handlers/mod.rs:376-377
        // Plugin handlers (singular — canonical CC-compatible namespace)
        registry.register("plugin.list", plugins::handle_list);
// :379
        registry.register("plugin.installFromZip", plugins::handle_install_from_zip);
// :382-383
        registry.register("plugin.enable", plugins::handle_enable);
        registry.register("plugin.disable", plugins::handle_disable);
// :385-389
        // Per-plugin configuration. The manifest could declare a
        // `config_schema` since the type existed; nothing could read or write
        // a value against it until these two.
        registry.register("plugin.config.get", plugins::handle_config_get);
        registry.register("plugin.config.set", plugins::handle_config_set);

// method_census.rs:312-318
        ("plugin.config.get", Class::Admin),
        ("plugin.config.set", Class::Admin),
        ("plugin.disable", Class::Admin),
        ("plugin.enable", Class::Admin),
        ("plugin.install", Class::Admin),
        ("plugin.installFromZip", Class::Admin),
        ("plugin.list", Class::Admin),

// method_authz.rs:238
            ("plugin_manage", "plugin.enable"),
```

The comment at `:364` describes the opposite of reality: every client calls the plural (`plugins.list` — qa ×5, CLI ×2, Panel ×1; `plugins.enable`/`disable` — CLI + Panel; `plugins.installFromZip` — CLI; `plugins.install`/`uninstall` — CLI + Panel + rate-limiter/lane tables). The singular set is the orphan except where a client exists (listed above).

- [ ] **Step 1: Multi-line re-verification (the census script matched single-line literals only)**

Run and paste the output into the commit message:

```bash
for m in 'plugin\.list' 'plugin\.installFromZip' 'plugin\.enable' 'plugin\.disable' 'plugin\.config\.get' 'plugin\.config\.set'; do
  echo "== $m =="; rg -n -U "\"$m\"" src interfaces shared qa crates desktop; rg -n -U "register\(\s*\"$m\"" src; done
rg -n 'plugin\.config|format!\("plugin\.' interfaces shared qa   # dynamic construction: expected 0
```

Expected (at `3ddc1f2e7`): each method matches ONLY `src/gateway/handlers/mod.rs` (registration) and `src/gateway/method_census.rs` (ruling row); `plugin.enable` additionally `src/gateway/method_authz.rs:238`. Zero hits in `interfaces/`, `shared/`, `qa/`, `crates/`, `desktop/`. If any client hit appears, STOP and report — that method leaves this task.

- [ ] **Step 2: Write the failing test**

Add to `src/gateway/handlers/mod.rs` `mod tests` after `test_plugin_handlers_registered`:

```rust
    /// The singular `plugin.*` namespace was described as "canonical" while
    /// every client called the plural. The six verbs no client ever called
    /// are gone; the ones with a client (install/uninstall/update/reload,
    /// marketplace.*) stay. A method that is registered but never called is
    /// not compatibility, it is surface.
    #[test]
    fn the_uncalled_singular_plugin_verbs_are_not_registered() {
        let registry = HandlerRegistry::new();
        for orphan in [
            "plugin.list",
            "plugin.installFromZip",
            "plugin.enable",
            "plugin.disable",
            "plugin.config.get",
            "plugin.config.set",
        ] {
            assert!(
                !registry.has_method(orphan),
                "{orphan} is registered again — it had zero clients when it was cut (2026-09-20)"
            );
        }
        for kept in ["plugin.install", "plugin.uninstall", "plugin.update", "plugin.reload"] {
            assert!(registry.has_method(kept), "{kept} has a client and must stay");
        }
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::handlers::tests::the_uncalled_singular_plugin_verbs_are_not_registered`
Expected: FAIL with `plugin.list is registered again`.

- [ ] **Step 4: Delete registrations, handlers, rulings; fix the pairing and the docs**

`src/gateway/handlers/mod.rs`:
- `:364` → `// Plugin handlers (plural — this is the namespace every client calls: Panel, CLI, qa)`
- `:376` → `// Plugin handlers (singular — only the verbs that have a client; the rest were cut 2026-09-20)`
- delete `:377`, `:379`, `:382`, `:383`, `:385-389` (the three comment lines + two `config` registrations).

`src/gateway/handlers/plugins/handlers/manage.rs`: delete `:23-128` — the doc block + body of `handle_config_get` (`:23-76`) and of `handle_config_set` (`:78-128`). Keep `is_safe_plugin_name` (`:14-21`, still used at `:190`, `:254`, `:300`) and `handle_list` (`:130-`).

`src/gateway/method_census.rs`: delete rows `:312`, `:313`, `:314`, `:315`, `:317`, `:318`. Keep `:316 ("plugin.install", …)`.

`src/gateway/method_authz.rs:238` → `("plugin_manage", "plugins.enable"),` (the pairing needs a registered admin-gated method of the family; `plugins.` prefix is Admin at `method_admin.rs:235`).

`docs/reference/PLUGIN_SYSTEM.md` "## Gateway RPC 方法" table:
- `:464` `| \`plugin.list\` / \`plugins.list\` | 列出已安装插件 |` → `| \`plugins.list\` | 列出已安装插件 |`
- `:468` → `| \`plugins.enable\` | 启用插件 |`; `:469` → `| \`plugins.disable\` | 禁用插件 |`
- delete `:476-477` (the two `plugin.config.*` rows)
- `:479` `\`plugin.*\`（单数）是 CC 兼容方法名，\`plugins.*\`（复数）保留作为向后兼容别名。` → `\`plugins.*\`（复数）是每个客户端实际调用的命名空间；\`plugin.*\`（单数）只剩有客户端的四个动词（\`install\` / \`uninstall\` / \`update\` / \`reload\`）与 \`plugin.marketplace.*\`。2026-09-20 之前单数还注册着 \`list\` / \`installFromZip\` / \`enable\` / \`disable\` / \`config.get\` / \`config.set\` 六个零客户端动词，且这里的注释把"谁是遗留"说反了；插件配置的唯一面现在是 \`plugin_manage(config_get / config_set)\`。`
- `:662` `| \`plugin.config.get\` RPC | **存储形态** | 这段文字进 Panel |` → delete the row (Panel never called it; the guard on `gateway/handlers/` in `plugin_secrets.rs` is directory-wide and does not name it).

- [ ] **Step 5: Run tests to verify they pass, including the census that must go from stale→green**

Run: `cargo test -p alephcore --lib gateway::handlers::tests`
Expected: PASS (new test green; `test_plugin_handlers_registered` untouched).
Run: `cargo test -p alephcore --lib gateway::method_census gateway::method_authz gateway::method_admin`
Expected: PASS — `every_registered_rpc_method_has_a_recorded_ruling` green (no `stale`), `every_tool_face_of_an_admin_rpc_family_is_operator_gated` green with the new pairing.
Run: `cargo test -p alephcore --lib gateway::handlers::plugins`
Expected: PASS.
Zero-reference grep: `rg -n 'handle_config_get|handle_config_set|"plugin\.list"|"plugin\.installFromZip"|"plugin\.enable"|"plugin\.disable"|"plugin\.config\.' src interfaces shared qa crates` → expected exactly 6 hits, all of them the string literals inside `the_uncalled_singular_plugin_verbs_are_not_registered` in `handlers/mod.rs`; `handle_config_get|handle_config_set` must have 0.

- [ ] **Step 6: Mutation step (record the red)**

Re-add `registry.register("plugin.list", plugins::handle_list);` → run `cargo test -p alephcore --lib gateway::method_census` → expected RED: `every_registered_rpc_method_has_a_recorded_ruling` with `newly registered RPC methods with no recorded ruling … ["plugin.list"]`; and `the_uncalled_singular_plugin_verbs_are_not_registered` RED. Revert.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/mod.rs src/gateway/handlers/plugins/handlers/manage.rs src/gateway/method_census.rs src/gateway/method_authz.rs docs/reference/PLUGIN_SYSTEM.md
git commit -m "gateway/plugins: cut the six uncalled singular plugin.* verbs; fix the reversed namespace comment

plugin.{list,installFromZip,enable,disable,config.get,config.set} had zero
clients across Panel, TUI, CLI, qa and shared (multi-line grep in the
task log). The comment called the plural 'legacy'; the plural is what
every client calls. plugin config keeps its tool face (plugin_manage).
Census rulings and the tool-face pairing follow; PLUGIN_SYSTEM.md table
updated in the same commit.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.3: Post-condition census — the runtime-bypass verbs and the WASM "command handler" chain are gone (deleted by P1.10 / P1.11 / P4.7d)

> **Reconciled (R5.1):** this task no longer deletes anything. `plugins.load` / `plugins.unload` (+ `LoadPluginParams` / `UnloadPluginParams`, their tests, census rows) are deleted by **P1.10**; `load_runtime_plugin` / `unload_runtime_plugin` by **P1.11**; `plugins.executeCommand` + `execute_plugin_command` + `PluginLoader::execute_command` + `DirectCommandResult` + `ExecuteCommandParams` by **P4.7d**. P5 runs after both, so this task is the census that proves the whole class is gone and no doc line still describes it. The two doc replacement texts are kept below because P1.10 and P4.7d copy them into their own commits.

**Files:**
- Modify: none, unless Step 1 finds a leftover (then only that line, in one `docs:`/`gateway:` commit).
- Test: `src/gateway/method_census.rs::tests::every_registered_rpc_method_has_a_recorded_ruling` (already green after P1.10 / P4.7d — this task re-runs it as the post-condition).

**Interfaces:**
- Consumes: P1.10, P1.11, P4.7d landed.
- Produces: nothing.

- [ ] **Step 1: The census (paste the output into the P5.4 commit message if nothing is found, or into this task's own commit if something is)**

```bash
rg -n '"plugins\.load"|"plugins\.unload"|"plugins\.executeCommand"|plugins\.load\b|plugins\.unload\b|plugins\.executeCommand\b' src interfaces shared qa crates docs/reference   # expected 0 outside history prose
rg -n 'execute_plugin_command|load_runtime_plugin|unload_runtime_plugin|DirectCommandResult|ExecuteCommandParams|LoadPluginParams|UnloadPluginParams|handle_load\b|handle_unload\b|handle_execute_command' src interfaces shared qa crates docs/reference   # expected 0
rg -n 'plugins\.load|executeCommand|Direct Commands' docs/reference/EXTENSION_SYSTEM.md docs/reference/PLUGIN_SYSTEM.md   # expected: only the "已删除" history block written by P4.7d and the P1.10 namespace paragraph
cargo test -p alephcore --lib gateway::method_census   # expected PASS with no `stale` and no `missing`
```

If any grep hit is a live reference (a doc table row, a comment that says the method exists), fix that one line in this task and commit it; the hit list is the deliverable either way.

- [ ] **Step 2: Doc replacement texts (carried by other phases — do NOT apply here)**

**Carried by P1.10** — `docs/reference/PLUGIN_SYSTEM.md:481-483` (the "两个命名空间的能力集并不相等" paragraph), replacement:

```markdown
⚠️ **两个命名空间的能力集并不相等**：`callTool` **只**在复数上，`update` / `reload` / `marketplace.*` **只**在
单数上（`executeCommand` / `load` / `unload` 于 2026-09-20 CUT——零客户端，且 `load`/`unload` 绕过 registry
直接对 WASM loader 寻址，与 mount/unmount 生命周期相悖）。
```

**Carried by P4.7d** — `docs/reference/EXTENSION_SYSTEM.md:613-709` (the whole "## Direct Commands (P0.5)" section, from the heading through the `---` before "## Background Services (P1)"), replacement:

```markdown
## Direct Commands —— ❌ 已删除（2026-09-20）

这里曾有一节（~95 行）描述 `[[commands]] handler = "handlePing"` 式的 manifest、TypeScript
`DirectCommandArgs` / `DirectCommandResult` 签名、以及 `plugins.executeCommand` RPC。

**三者都不再存在。** `CommandRegistration` 在 2026-07-17 折进 `SkillRegistration { skill_type: Command }`
（插件 `commands/*.md` 的 markdown 正文就是那个"handler"），之后 `plugins.executeCommand` 把一段
markdown 当 WASM 导出名去调，零客户端，2026-09-20 连同 `ExtensionManager::execute_plugin_command`、
`PluginLoader::execute_command`、`DirectCommandResult` 一起 CUT。

插件命令今天的形状：`commands/<name>.md` → 注册为 slash 条目 → 用户 `/name args` 经 `chat.send` →
正文经 `SkillTemplate` 展开（`$ARGUMENTS` / `$N` / `@file` / `` !`cmd` ``）后作为本轮用户内容注入模型
（见 PLUGIN_SYSTEM.md「commands / agents 正文」）。**没有绕过模型的直接命令**——要确定性执行，写一个
工具（WASM 导出或 MCP tool），不要写命令。

> 保留这一节的标题而不是删干净，理由同下方 Channel / Provider 那节。
```

- [ ] **Step 3: Commit (only if Step 1 found a leftover)**

```bash
git add <the one file>
git commit -m "docs: remove the last reference to the retired plugins.load/unload/executeCommand verbs

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.4: Zero-client RPC group (c) — `command.execute` (base stub + startup override + `handle_execute` + helpers + tests)

> **Reconciled (R5.2 Q1):** the CUT stands on the zero-client ground. **(R5.2 Q5):** this task also CUTs `ToolCatalog::{is_namespace, list_namespace_children}` (their only production caller was `handle_execute`) with their five tests.

**Corrected premise** (see the header): `command.execute` is overridden at startup with a real handler; the stub at `handlers/mod.rs:346-352` is only the pre-startup fallback. The CUT ground is **zero clients** — the TUI, CLI, Panel, channels and qa all resolve `/cmd` through `chat.send` (the same `CommandParser` is injected into it via `command_parser_cell`, `agent_init/mod.rs:906-909`) and enumerate through `commands.list`. This task removes the second face; it does not touch `CommandParser` or `commands.list`.

**Files:**
- Modify: `src/gateway/handlers/mod.rs:346-352`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:9-14, 466-487`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:906-908` (comment)
- Modify: `src/gateway/handlers/commands.rs:1-15, 17-26, 66-86, 308-338, 340-363, 365-502, 709-973, 1038-1058, 1059-1094`
- Modify: `src/gateway/method_census.rs:140`
- Modify: `src/gateway/method_admin.rs:940`
- Modify: `src/tool_metadata/registry/mod.rs:264-272` (the two `ToolCatalog` wrappers), `src/tool_metadata/registry/query.rs:304-338` (the two query impls), `src/tool_metadata/registry/tests.rs:888-942` (the "Namespace Query Tests" banner + five tests)
- Modify: `docs/reference/FEATURE_LOCATOR.md:1017` (§3.5 打磨话术 names `command.execute`; same commit)
- Test: `src/gateway/handlers/mod.rs` `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `ToolCatalog::is_namespace` / `list_namespace_children` are **deleted** (R5.2 Q5) — `handle_execute` was their only production caller; `suggest_commands` keeps its channel-router caller and stays.

Current code (quoted):

```rust
// handlers/mod.rs:346-352
        registry.register("command.execute", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "command.execute requires ToolRegistry — wire in Gateway startup".to_string(),
            )
        });

// tool_catalog_init.rs:466-487
    // Wire command.execute to resolve slash commands via CommandParser + ToolRegistry
    {
        let parser = Arc::new(alephcore::command::CommandParser::new(tool_catalog.clone()));

        // Inject parser into chat.send handler (created earlier, uses deferred cell)
        {
            let mut cell = command_parser_cell.write().await;
            *cell = Some(parser.clone());
        }

        let reg = tool_catalog.clone();
        server
            .handlers_mut()
            .register("command.execute", move |req| {
                let p = parser.clone();
                let r = reg.clone();
                async move {
                    alephcore::gateway::handlers::commands::handle_execute(req, p, r).await
                }
            });
        if !daemon {
            println!("  command.execute: wired to unified command parser + registry");
        }
    }

// method_census.rs:140
        ("command.execute", Class::Open),
// method_admin.rs:940 (inside the "must stay open to members" list)
            "command.execute",
```

- [ ] **Step 1: Multi-line re-verification**

```bash
rg -n -U '"command\.execute"' src interfaces shared qa crates desktop
rg -n -U 'register\(\s*"command\.execute"' src
rg -n 'command\.execute|command_execute' interfaces/tui/src interfaces/cli/src interfaces/webchat/src qa shared   # expected 0
```

Expected: hits only in `handlers/mod.rs:346`, `tool_catalog_init.rs:477`, `method_census.rs:140`, `method_admin.rs:940`, `handlers/commands.rs` (its own tests `:724, :763, :804, :844`). Zero client hits.

- [ ] **Step 2: Write the failing test**

Add to `src/gateway/handlers/mod.rs` `mod tests`:

```rust
    /// `command.execute` was a second face for what `chat.send` already does
    /// with the same `CommandParser`, and no client ever called it. The
    /// ToolCatalog-less base stub and the startup override both go.
    #[test]
    fn command_execute_is_not_registered() {
        let registry = HandlerRegistry::new();
        assert!(!registry.has_method("command.execute"));
        assert!(registry.has_method("commands.list"), "commands.list is the surviving face");
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::handlers::tests::command_execute_is_not_registered`
Expected: FAIL (`assertion failed: !registry.has_method("command.execute")`).

- [ ] **Step 4: Delete**

`src/gateway/handlers/mod.rs`: delete `:346-352`.

`src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:466-487` → keep only the parser injection:

```rust
    // Build the one CommandParser and hand it to chat.send / agent.run via the
    // deferred cell (created earlier). `command.execute` — the second RPC face
    // for the same parser — was cut 2026-09-20 (zero clients).
    {
        let parser = Arc::new(alephcore::command::CommandParser::new(tool_catalog.clone()));
        let mut cell = command_parser_cell.write().await;
        *cell = Some(parser);
        if !daemon {
            println!("  CommandParser: injected into chat.send / agent.run");
        }
    }
```

`tool_catalog_init.rs:9-11` module doc → `//! Side effects preserved exactly: registers \`commands.list\` / \`tools.catalog\`` / `//! / \`tools.invoke\` / \`tools.effective\` / \`tools.cancel_call\` /` / `//! \`tools.in_flight\` handlers, injects the \`CommandParser\`` (drop `/ \`command.execute\``).

`agent_init/mod.rs:906-907` → `// Share the one parser cell with \`chat.send\` and \`agent.run\`, so the` / `// slash surfaces resolve \`/foo\` identically.`

`src/gateway/handlers/commands.rs`:
- `:1-4` module doc → `//! Commands RPC Handlers` / `//!` / `//! Handlers for command listing and discovery (\`commands.list\`).` / `//! Returns hierarchical tree structure for namespaced commands.`
- delete `:13` `use crate::command::CommandParser;`
- `:15` → `use crate::tool_metadata::{ChannelType, ToolCatalog, UnifiedTool};`
- delete `:17-26` (`source_type_to_string`)
- delete `:66-86` (`split_namespace_action` + its doc `:66-74`; keep `TOOL_NAMESPACES` at `:61-64` — used by `build_command_tree :117` and `render_command_help :224`)
- delete `:308-338` (the `command.execute` banner `:308-310`, `ExecuteParams` `:312-320`, `ResolvedCommandInfo` `:322-338`)
- delete `:340-363` (`build_namespace_children`)
- delete `:365-502` (`handle_execute` doc `:365-386` + body `:387-502`)
- `:8` `use serde::{Deserialize, Serialize};` → check remaining derives; if none, delete the line.
- `:14` `use crate::sync_primitives::Arc;` → only `handle_execute` used it; delete if unused.
- tests (each range = the `#[tokio::test]` attribute through the closing `}`, plus the `///` doc lines that precede the NEXT deleted test): `test_execute_resolved` (`:709-738`), `test_execute_namespace_only` (`:740-777`) with the doc at `:778-781`, `test_execute_namespace_only_with_at_bot_suffix` (`:782-817`) with the doc at `:818-821`, `test_execute_bad_subcommand_with_at_bot_suffix` (`:822-861`), `test_execute_bad_subcommand` (`:863-901`), `test_execute_unknown_command` (`:903-923`), `test_execute_unknown_command_offers_suggestions` (`:925-951`), `test_execute_empty_input` (`:953-962`), `test_execute_params_deserialization` (`:964-973`), `test_split_namespace_action` (`:1038-1058`), the doc at `:1059-1062` + `test_execute_resolved_namespace_and_internal_id` (`:1063-1094`). Keep `test_channel_from_interface_mapping` (`:509-522`), the three `test_list_from_registry_*`, `the_live_response_parses_as_the_shared_contract`, `test_build_command_tree_mixed` (`:975-1029`), `test_capitalize` (`:1031-1036`), the doc at `:1095-1100` + `render_command_help_curates_and_folds` (`:1101-1160`).

`src/gateway/method_census.rs`: delete `:140`.
`src/gateway/method_admin.rs`: delete `:940` `"command.execute",` from the must-stay-open list.

`src/tool_metadata/registry/mod.rs:264-272` — delete both wrappers (quoted):

```rust
    /// Check if a name is a namespace (has active tools with that prefix)
    pub async fn is_namespace(&self, name: &str) -> bool {
        self.query.is_namespace(name).await
    }

    /// List direct children of a namespace
    pub async fn list_namespace_children(&self, namespace: &str) -> Vec<UnifiedTool> {
        self.query.list_namespace_children(namespace).await
    }
```

`src/tool_metadata/registry/query.rs:304-338` — delete `is_namespace` (`:304-313`, doc + body) and `list_namespace_children` (`:315-338`, doc + body); `list_root_commands` at `:340` stays.

`src/tool_metadata/registry/tests.rs:888-942` — delete the `// Namespace Query Tests` banner (`:888-890`) and the five tests `test_is_namespace_true` (`:892-899`), `test_is_namespace_false` (`:901-907`), `test_is_namespace_case_insensitive` (`:909-916`), `test_list_namespace_children` (`:918-933`), `test_list_namespace_children_empty` (`:935-942`). The slash-command surfacing tests from `:944` stay.

`docs/reference/FEATURE_LOCATOR.md:1017` — in the §3.5 打磨话术 replace `\`command.execute\` 返回的 \`internal_id\` = \`ParsedCommand.tool_id\`（canonical 注册 id，别再 \`{source}:{name}\` 重建）；\`namespace\`/\`action\` 经 \`split_namespace_action\`（认 \`TOOL_NAMESPACES\`，**不是** \`.\` 分隔）。` with `\`ParsedCommand.tool_id\` 是 canonical 注册 id（别再 \`{source}:{name}\` 重建）；它的唯一消费面是 \`chat.send\`——\`command.execute\` RPC 于 2026-09-20 CUT（零客户端，第二张脸），\`split_namespace_action\` 随之删除。`

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::handlers::commands gateway::handlers::tests gateway::method_census gateway::method_admin`
Expected: PASS.
Run: `cargo test -p alephcore --bins`
Expected: PASS (the `src/bin` census sees the startup registration gone).
Run: `cargo clippy -p alephcore --all-targets` → no `unused import` in `commands.rs` / `tool_catalog_init.rs`.
Run: `cargo test -p alephcore --lib tool_metadata::registry` → PASS (the namespace tests are gone; `suggest_commands` tests remain).
Zero-reference grep: `rg -n 'handle_execute\b|ExecuteParams|ResolvedCommandInfo|build_namespace_children|split_namespace_action|source_type_to_string|is_namespace\(|list_namespace_children|"command\.execute"' src interfaces shared qa crates` → expected: only the literal inside `command_execute_is_not_registered` (1 hit). (`is_namespace:` as a struct FIELD of `CommandTreeNode` in `aleph_protocol` is a different identifier — the pattern above has the `(`.)

- [ ] **Step 6: Mutation step (record the red)**

Re-add the base stub at `handlers/mod.rs` → RED: `command_execute_is_not_registered` and `every_registered_rpc_method_has_a_recorded_ruling` (`missing: ["command.execute"]`). Revert.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs src/gateway/handlers/commands.rs src/gateway/method_census.rs src/gateway/method_admin.rs src/tool_metadata/registry/mod.rs src/tool_metadata/registry/query.rs src/tool_metadata/registry/tests.rs docs/reference/FEATURE_LOCATOR.md
git commit -m "gateway/commands: cut the command.execute RPC face (zero clients) and the namespace queries it alone used

Correction to the scan evidence: command.execute WAS overridden at
startup (tool_catalog_init.rs) with the real handler; the base stub was
only the pre-startup fallback. What made it dead is that no client ever
called it — every surface resolves /cmd through chat.send with the same
CommandParser. Handler, params, namespace helpers and their tests go;
CommandParser injection into chat.send is kept verbatim.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P5.5: Zero-client RPC group (d) — all eleven `mcp.*` server-management/aggregation verbs

> **Reconciled (R5.2 → U-a):** the user ruled that the nine tool-face-less verbs are CUT as well, so all eleven go. `mcp.list` (CLI client `interfaces/cli/src/commands/doctor.rs:769`) and the three approval verbs `mcp.{list_pending_approvals,respond_approval,cancel_approval}` (CLI `mcp_cmd.rs:13,70,100,122`) are NOT in the eleven and stay. Per-verb evidence kept for the record: `mcp.prompts` / `mcp.resources` have tool faces (`mcp_list_prompts` `src/builtin_tools/mcp_prompt.rs:245`, `mcp_list_resources` `src/builtin_tools/mcp_resource.rs:281`); `mcp.tools` twins `tools.catalog`; `mcp.add/update/delete` twin the Panel's `mcp_config.create/update/delete` (`interfaces/webchat/src/api/mcp.rs:67-85`); `mcp.logs` is an honest stub (`handlers/mcp.rs:176-193`: `// Log retrieval is not implemented yet — the MCP actor keeps no log buffer.`, answers `{"logs": [], "implemented": false}`); `mcp.status/start/stop/restart` have no equivalent and no caller.

**The eleven at `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:31-45`** (quoted):

```rust
    // Lifecycle
    reg!("mcp.list", mcp::handle_list);          // :32  — STAYS (CLI client)
    reg!("mcp.add", mcp::handle_add);            // :33
    reg!("mcp.update", mcp::handle_update);      // :34
    reg!("mcp.delete", mcp::handle_delete);      // :35
    reg!("mcp.status", mcp::handle_status);      // :36
    reg!("mcp.logs", mcp::handle_logs);          // :37
    reg!("mcp.start", mcp::handle_start);        // :38
    reg!("mcp.stop", mcp::handle_stop);          // :39
    reg!("mcp.restart", mcp::handle_restart);    // :40

    // Capability aggregation
    reg!("mcp.tools", mcp::handle_list_tools);            // :43
    reg!("mcp.resources", mcp::handle_list_resources);    // :44
    reg!("mcp.prompts", mcp::handle_list_prompts);        // :45
```

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:1-14 (doc), 31-45`
- Modify: `src/gateway/handlers/mcp.rs:1-7 (module doc), 9-13 (imports), 15-47 (param types), 61-299 (eleven handlers), 386-496 (their param tests)`
- Modify: `src/gateway/method_census.rs:269, 271, 274-276, 278-283`
- Modify: `src/gateway/method_admin.rs:726` (`"mcp.add",` in the admin-list test)
- Test: `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` (new `#[cfg(test)]` pin) + `src/gateway/method_census.rs` (existing census)

**Interfaces:**
- Consumes: nothing.
- Produces: `McpManagerHandle::aggregate_{tools,resources,prompts}` (`src/mcp/manager/handle.rs:277, 292, 307`) lose their only callers → cascaded in Task P5.6. Every other handle method the eleven used keeps callers elsewhere (verified at `3ddc1f2e7`): `remove_server` (`hub/official_mcp.rs:148`, `mcp_config.rs:393,424`, `extensions/lifecycle.rs:109`), `add_server` (`mcp_config.rs:74`), `start_server` (`extensions/lifecycle.rs:57`, `extensions/install.rs:276`), `stop_server` (`extensions/lifecycle.rs:59`), `restart_server` (`builtin_tools/mcp_login.rs:234`), `get_status` (`builtin_tools/mcp_login.rs:176`). `McpManagerConfig` (imported by the param types) stays — it is the manager's own type.

- [ ] **Step 1: Multi-line re-verification for all eleven (paste output into the commit message)**

```bash
for m in 'mcp\.add' 'mcp\.update' 'mcp\.delete' 'mcp\.status' 'mcp\.logs' 'mcp\.start' 'mcp\.stop' 'mcp\.restart' 'mcp\.tools' 'mcp\.resources' 'mcp\.prompts'; do
  echo "== $m =="; rg -n -U "\"$m\"" src interfaces shared qa crates desktop | grep -v 'method_census.rs\|method_admin.rs'; done
rg -n '"mcp\."|format!\("mcp\.' interfaces shared qa    # dynamic: expected 0
rg -n -o 'rpc\.call\("mcp[a-z_.]*"' qa | sort -u        # expected: empty (qa never calls mcp.*)
rg -n '"mcp\.list"' interfaces                             # expected: interfaces/cli/src/commands/doctor.rs:769 — the one that stays
```

Expected (at `3ddc1f2e7`): every one of the eleven matches only `builder/handlers/mcp.rs` (its `reg!` line). If any client hit appears, STOP and report.

- [ ] **Step 2: Write the failing test**

Add to `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` (end of file):

```rust
#[cfg(test)]
mod tests {
    /// The eleven `mcp.*` management / aggregation RPCs had zero clients
    /// (user ruling 2026-09-20). Only `mcp.list` — the CLI's `doctor` calls
    /// it — is registered from this file. Source-level because
    /// `register_mcp_handlers` needs a live actor to run.
    #[test]
    fn only_mcp_list_is_registered_here() {
        let src = include_str!("mcp.rs");
        let registered: Vec<&str> = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("reg!("))
            .collect();
        assert_eq!(
            registered.len(),
            1,
            "exactly one reg!() line expected (mcp.list); found:\n{}",
            registered.join("\n")
        );
        assert!(
            registered[0].contains("\"mcp.list\""),
            "the surviving registration must be mcp.list, got: {}",
            registered[0]
        );
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --bins only_mcp_list_is_registered_here`
Expected: FAIL with `exactly one reg!() line expected (mcp.list); found:` followed by 12 lines.

- [ ] **Step 4: Delete**

`src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:31-45` → replace with:

```rust
    // `mcp.list` is the only server-management verb with a client (the CLI's
    // `doctor`). The other eleven (`add/update/delete/status/logs/start/stop/
    // restart`, `tools/resources/prompts`) were cut 2026-09-20: zero clients;
    // persistent CRUD lives on `mcp_config.*`, the model already sees prompts /
    // resources through `mcp_list_*` tools and tools through the catalog.
    reg!("mcp.list", mcp::handle_list);
```

and the fn doc `:5-6` `/// Register the \`mcp.*\` server-management JSON-RPC handlers against a live` → `/// Register the \`mcp.list\` JSON-RPC handler against a live`.

`src/gateway/handlers/mcp.rs`:
- `:1-7` module doc → `//! MCP RPC Handlers` / `//!` / `//! \`mcp.list\` (server inventory) and the three approval verbs. The eleven` / `//! management / aggregation verbs were cut 2026-09-20 (zero clients).`
- `:15-47` delete the `// Param Types` banner, `AddParams`, `UpdateParams`, `IdParams`, `LogsParams`, `default_max_lines` (all only used by the deleted handlers).
- `:61-299` delete the ten banners + handlers `handle_add` (`:61-81`), `handle_update` (`:83-129`), `handle_delete` (`:131-149`), `handle_status` (`:151-171`), `handle_logs` (`:173-194`), `handle_start` (`:196-214`), `handle_stop` (`:216-234`), `handle_restart` (`:236-254`), `handle_list_tools` (`:256-269`), `handle_list_resources` (`:271-284`), `handle_list_prompts` (`:286-299`). Keep `handle_list` (`:49-59`) and everything from `// Approval Handlers` (`:301`) on.
- `:9-13` imports: `serde::Deserialize` stays (approval params derive it); `McpManagerConfig` goes (`use crate::mcp::manager::McpManagerHandle;`); `RESOURCE_NOT_FOUND` — check with `cargo test --no-run`, delete if unused.
- tests `:386-496`: delete `test_add_params_deserialize`, `test_add_params_with_args`, `test_id_params_deserialize`, `test_logs_params_defaults`, `test_logs_params_custom_max_lines`, `test_update_params_deserialize`, `test_add_params_http_transport`, `test_add_params_with_env`. Keep the approval tests from `:502`.

`src/gateway/method_census.rs`: delete rows `:269 mcp.add`, `:271 mcp.delete`, `:274 mcp.logs`, `:275 mcp.prompts`, `:276 mcp.resources`, `:278 mcp.restart`, `:279 mcp.start`, `:280 mcp.status`, `:281 mcp.stop`, `:282 mcp.tools`, `:283 mcp.update`. Keep `:270 mcp.cancel_approval`, `:272 mcp.list`, `:273 mcp.list_pending_approvals`, `:277 mcp.respond_approval`.

`src/gateway/method_admin.rs:726` delete `"mcp.add",` from the "extension / capability install" admin list (it would be a ghost entry; the `"mcp."` prefix at `:231` stays for `mcp.list` + approvals).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --bins`
Expected: PASS (new pin green; the bins census sees one `reg!`).
Run: `cargo test -p alephcore --lib gateway::method_census gateway::method_admin gateway::handlers::mcp`
Expected: PASS; no `stale` rows; `the_role_tables_have_no_ghost_entries` green.
Run: `cargo clippy -p alephcore --all-targets` → no unused imports in `handlers/mcp.rs`.
Zero-reference grep: `rg -n 'handle_add\b|handle_update\b|handle_delete\b|handle_status\b|handle_logs\b|handle_start\b|handle_stop\b|handle_restart\b|handle_list_tools|handle_list_resources|handle_list_prompts|AddParams|UpdateParams|IdParams|LogsParams' src/gateway/handlers/mcp.rs src/bin` → 0; `rg -n '"mcp\.(add|update|delete|status|logs|start|stop|restart|tools|resources|prompts)"' src interfaces shared qa crates` → 0.

- [ ] **Step 6: Mutation step (record the red)**

Re-add `reg!("mcp.prompts", mcp::handle_list);` → RED: `only_mcp_list_is_registered_here` (found 2 lines) and (after `cargo test --lib`) `every_registered_rpc_method_has_a_recorded_ruling` (`missing: ["mcp.prompts"]`). Revert.

- [ ] **Step 7: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers/mcp.rs src/gateway/handlers/mcp.rs src/gateway/method_census.rs src/gateway/method_admin.rs
git commit -m "gateway/mcp: cut the eleven zero-client mcp.* management/aggregation RPCs; mcp.list stays

User ruling 2026-09-20. add/update/delete twin mcp_config.*; prompts /
resources have tool faces; tools twins tools.catalog; logs was an honest
implemented:false stub; status/start/stop/restart had no caller anywhere.
Census rulings and the admin-list test entry follow.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.6: Cascade of P5.5 into `src/mcp/manager/` — the three aggregate wrappers that just lost their last caller

> **Reconciled (R5.2 Q6):** `mcp.tools` is cut too, so `aggregate_tools` / `AggregateTools` join the cascade. The positive control for the pin becomes `AggregateInstructions` (its caller is the prompt builder: `src/orchestrator/harness_bridge/prompt_build.rs:478` → `handle.aggregate_instructions()` → `McpInstructionsLayer`; verified at `3ddc1f2e7`). `aggregate_from_healthy` (`actor.rs:1043`) keeps that one user (`:1098`).

**Files:**
- Modify: `src/mcp/manager/handle.rs:19, 271-317`
- Modify: `src/mcp/manager/types.rs:18, 546-562, 637-639`
- Modify: `src/mcp/manager/actor.rs:33, 429-440, 1073-1089, 1287-1315`
- Test: `src/mcp/manager/actor.rs` `mod tests` (new pin; `test_aggregate_tools_empty` is deleted with its subject)

**Interfaces:**
- Consumes: Task P5.5 landed (no caller of `aggregate_{tools,resources,prompts}` on the handle).
- Produces: `McpCommand` loses variants `AggregateTools`, `AggregateResources`, `AggregatePrompts`. `aggregate_instructions` / `AggregateInstructions` stay (prompt-builder caller). `aggregate_from_healthy` (`actor.rs`, the shared helper) stays because `aggregate_instructions` uses it — verify at `:1091-1100`; if it is the only remaining user, keep it (one user is a user).

Current code (quoted at `3ddc1f2e7`):

```rust
// handle.rs:271-317
    // ===== Aggregation Methods (P1) =====

    /// Get aggregated tools from all healthy servers
    …
    pub async fn aggregate_tools(&self) -> Result<Vec<McpTool>> { … }     // :277-287
    /// Get aggregated resources from all healthy servers
    …
    pub async fn aggregate_resources(&self) -> Result<Vec<McpResource>> { … }  // :292-302
    /// Get aggregated prompts from all healthy servers
    …
    pub async fn aggregate_prompts(&self) -> Result<Vec<McpPrompt>> { … }      // :307-317

// types.rs:546-562
    /// Get aggregated tools from all servers
    AggregateTools { respond_to: oneshot::Sender<Vec<McpTool>> },            // :546-550
    /// Get aggregated resources from all servers
    AggregateResources { respond_to: oneshot::Sender<Vec<McpResource>> },    // :552-556
    /// Get aggregated prompts from all servers
    AggregatePrompts { respond_to: oneshot::Sender<Vec<McpPrompt>> },        // :558-562
// types.rs:637-639
            Self::AggregateTools { .. } => f.debug_struct("AggregateTools").finish(),
            Self::AggregateResources { .. } => f.debug_struct("AggregateResources").finish(),
            Self::AggregatePrompts { .. } => f.debug_struct("AggregatePrompts").finish(),

// actor.rs:429-440  (three match arms)
            McpCommand::AggregateTools { respond_to } => { let tools = self.aggregate_tools().await; let _ = respond_to.send(tools); }
            McpCommand::AggregateResources { respond_to } => { … }
            McpCommand::AggregatePrompts { respond_to } => { … }
// actor.rs:1073-1089  (three private fns, each one call to aggregate_from_healthy)
// actor.rs:1287-1315  test_aggregate_tools_empty / test_aggregate_resources_empty / test_aggregate_prompts_empty
```

- [ ] **Step 1: Verify the wrappers are now zero-caller (pre-condition)**

Run: `rg -n '\.aggregate_(tools|resources|prompts)\(|Aggregate(Tools|Resources|Prompts)\b' src interfaces --glob '!*.md'`
Expected: hits ONLY inside `src/mcp/manager/{handle,types,actor}.rs` (definition, variant, arm, own test). If any hit outside `src/mcp/manager/` remains, STOP — P5.5 did not land.
Run: `rg -n 'aggregate_instructions\(|AggregateInstructions' src --glob '!*.md' | grep -v 'src/mcp/manager/'` → at least one hit (the prompt builder) — that is the positive control's caller.

- [ ] **Step 2: Write the failing test**

Add to `src/mcp/manager/actor.rs` `mod tests`:

```rust
    /// `McpCommand` is the actor's whole vocabulary. Three words nobody
    /// spoke after the `mcp.{tools,resources,prompts}` RPCs were cut
    /// (`AggregateTools`, `AggregateResources`, `AggregatePrompts`) are gone;
    /// this pins the vocabulary so a resurrected arm needs a caller first.
    /// Source-level: an enum variant with no constructor site compiles fine
    /// and looks like capability. `AggregateInstructions` is the positive
    /// control — the prompt builder still speaks it.
    #[test]
    fn the_command_vocabulary_has_no_uncalled_aggregate_words() {
        let types_src = include_str!("types.rs");
        for gone in ["AggregateTools", "AggregateResources", "AggregatePrompts"] {
            let n = types_src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| l.contains(gone))
                .count();
            assert_eq!(n, 0, "{gone} is back in McpCommand without a caller");
        }
        assert!(
            types_src.contains("AggregateInstructions {"),
            "AggregateInstructions is the positive control (the prompt builder still calls it)"
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib mcp::manager::actor::tests::the_command_vocabulary_has_no_uncalled_aggregate_words`
Expected: FAIL with `AggregateTools is back in McpCommand without a caller`.

- [ ] **Step 4: Delete**

- `handle.rs`: delete `:271-317` (the `// ===== Aggregation Methods (P1) =====` banner and the three wrappers; `aggregate_instructions` at `:319` stays — move the banner above it as `// ===== Aggregation (instructions only) =====` if you keep a banner at all); `:19` → `use crate::mcp::{McpClient, McpTool};` only if `McpTool` is still used elsewhere in the file, else drop it too — `cargo test --no-run` decides; `McpPrompt, McpResource` go.
- `types.rs`: delete `:546-562` and `:637-639`; `:18` keeps `McpPrompt, McpResource, McpTool` if `:405-407` (the aggregated-capabilities struct fields) still use them — they do at `3ddc1f2e7`, so the import stays.
- `actor.rs`: delete `:429-440`, `:1073-1089`, and the three tests `test_aggregate_tools_empty` (`:1287-1295`), `test_aggregate_resources_empty` (`:1297-1305`), `test_aggregate_prompts_empty` (`:1307-1315`). `:33` import: keep `McpPrompt, McpResource, McpTool` if other code in the file uses them; otherwise drop (compiler tells you).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib mcp::manager`
Expected: PASS (the pin green; `aggregate_instructions` tests, if any, still green).
Run: `cargo test -p alephcore --lib --no-run && cargo clippy -p alephcore --all-targets`
Expected: no `unused import`, no `dead_code` (in particular `aggregate_from_healthy` must still have `aggregate_instructions` as a user — if clippy says it is dead, `aggregate_instructions` does not use it and it goes too).
Zero-reference grep: `rg -n 'aggregate_tools|aggregate_resources|aggregate_prompts|AggregateTools|AggregateResources|AggregatePrompts' src interfaces shared qa crates` → expected 3 hits, all string literals inside the new pin.

- [ ] **Step 6: Mutation step (record the red)**

Re-add the `AggregatePrompts { respond_to: oneshot::Sender<Vec<McpPrompt>> }` variant to `McpCommand` (and a `Debug` arm so it compiles) → RED: `the_command_vocabulary_has_no_uncalled_aggregate_words`. Revert.

- [ ] **Step 7: Commit**

```bash
git add src/mcp/manager/handle.rs src/mcp/manager/types.rs src/mcp/manager/actor.rs
git commit -m "mcp/manager: drop the AggregateTools/Resources/Prompts actor words

Their only speakers were the mcp.tools / mcp.resources / mcp.prompts RPCs
cut in the previous commit. AggregateInstructions keeps its prompt-builder
caller and stays.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.7: Phantom Node runtime — delete `packages/plugin-sdk/`, delete `examples/plugins/media-video/`, tell the truth in EXTENSION_SYSTEM.md

**Decision on the example (read it; recorded here):** `examples/plugins/media-video/src/index.js:120-171` speaks a made-up line-delimited JSON-RPC protocol (`method === "plugin.call"` with `params.handler`, plus a `"ready"` frame at `:176-180`). No host ever spoke it — `PluginKind` is `{Wasm, Mcp, Static}` (`src/extension/types/plugins.rs:125-132`), so `kind = "nodejs"` (`aleph.plugin.toml:6`) fails serde as `unknown variant` and the plugin never loaded. Its `onPreToolUse` hook handler (`index.js:61-99`) has no channel either (MCP has none; `hooks.json` is the CC shape). Converting to `runtime = "mcp"` would mean rewriting `index.js` as a real MCP stdio server (initialize / tools/list / tools/call) with no SDK dependency in `package.json` — a new deliverable whose only purpose would be to be an example, while the sibling repo already has seven MCP-kind plugins to point at. **DELETE the directory.** The truthful pointer for "how do I write a Node plugin" is the new EXTENSION_SYSTEM.md section below.

**Sibling-repo follow-up (do NOT touch the submodule; written into the doc):** `plugins/media-office/src/index.js:349` `if (method === "plugin.call") {` — the whole transport is the same phantom protocol, and `:264 onPostToolUse` is a JS hook handler nothing can call. `PLUGIN_SYSTEM.md:618-620` already records the class ("Node.js 插件标记为 `runtime = "mcp"` 但 `src/index.js` 仍是旧 IPC 格式"); this task adds the `path:line`.

**Files:**
- Delete: `packages/plugin-sdk/` — 8 tracked files, 961 lines: `.gitignore` (2), `package-lock.json`, `package.json` (33), `src/__tests__/types.test.ts` (285), `src/helpers.ts` (144), `src/index.ts` (60), `src/types.ts` (417), `tsconfig.json` (20). `packages/` then becomes empty and disappears with it (P6's `packages/pi-aleph/` recreates the directory).
- Delete: `examples/plugins/media-video/` — 4 files: `aleph.plugin.toml`, `package.json`, `README.md`, `src/index.js`. `examples/plugins/` then empty → gone.
- Modify: `docs/reference/EXTENSION_SYSTEM.md:3, 11, 33-43, 58, 76, 143-218, 302, 391-394, 438-439, 607`
- Modify: `docs/reference/PLUGIN_SYSTEM.md:619-620`
- Test: none compilable — the guard is the reference grep in Step 4 (there is no Rust that names any of this; that is the point).

**Interfaces:** none. References checked at `3ddc1f2e7`: root `package.json` has no `workspaces` (it is `{ "devDependencies": { "@playwright/test", "@types/ws", "ws" } }`); `justfile`, `.github/workflows/*.yml`, `Cargo.toml`, `.gitignore`, `.dockerignore` — zero hits for `plugin-sdk|packages/`. `scripts/validate-harness/fixtures/long-text-04.md:193,210` quotes `@aleph/plugin-sdk` inside a **fixture** (test corpus, not a reference) — leave. `docs/plans/2026-03-09-*` and `docs/superpowers/specs/2026-03-25-*`, `2026-05-19-*` name it as history — leave (Tier 3).

- [ ] **Step 1: Pre-condition greps (the "failing" state — these must be non-empty now)**

```bash
ls packages/plugin-sdk examples/plugins/media-video                       # both exist
rg -n 'plugin-sdk|@aleph/plugin' --glob '!node_modules' --glob '!*.lock' . | grep -v '^./docs/\|^./scripts/validate-harness/fixtures/\|^./packages/plugin-sdk'   # expected: EMPTY already (zero consumers)
rg -n -i 'nodejs|node\.js' docs/reference/EXTENSION_SYSTEM.md | wc -l    # 14 at 3ddc1f2e7
```

- [ ] **Step 2: Delete the two trees**

```bash
git rm -r packages/plugin-sdk examples/plugins/media-video
```

- [ ] **Step 3: Rewrite EXTENSION_SYSTEM.md — every place that says the runtime exists**

`:3` `> Plugin architecture with WASM and Node.js runtimes` → `> Plugin architecture: WASM runtime, MCP-kind external servers, static (Markdown) plugins`

`:11` `- **Node.js Plugins**: JavaScript/TypeScript extensions` → `- **MCP-kind Plugins**: any-language external servers (Node.js, Python, …) reached over MCP stdio / HTTP — see "Node plugins run as MCP stdio servers" below`

`:33-43` (inside the architecture diagram) replace the two-box block with:

```
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                     Plugin Runtimes                       │   │
│  │  ┌────────────────────┐  ┌────────────────────┐         │   │
│  │  │    WASM Runtime    │  │  MCP-kind (extern) │         │   │
│  │  │    (Extism)        │  │  stdio / http srv  │         │   │
│  │  │                    │  │                    │         │   │
│  │  │ • Sandboxed        │  │ • Any language     │         │   │
│  │  │ • Fast startup     │  │ • Tools via bridge │         │   │
│  │  │ • Limited I/O      │  │ • No hook channel  │         │   │
│  │  └────────────────────┘  └────────────────────┘         │   │
│  └─────────────────────────────────────────────────────────┘   │
```

`:58` `│   ├── package.json          # (Node.js) or` → `│   ├── .mcp.json             # (MCP-kind: declares the external server) or`

`:76` `type = "nodejs"  # or "wasm"` → `type = "wasm"    # wasm | mcp | static — the real key is [aleph] runtime, see PLUGIN_SYSTEM.md「Runtime 模型」`

`:143-218` (from `## Node.js Runtime` through the closing ```` ``` ```` of the SDK block, i.e. up to but not including the `---` at `:219`) → replace with exactly:

```markdown
## Node plugins run as MCP stdio servers

There is no Node.js runtime in Aleph and there never was one on disk (`ls src/extension/runtime/` →
`wasm/` only; `PluginKind` is `Wasm | Mcp | Static`). A plugin written in Node.js — or Python, Go,
anything — is an **MCP-kind plugin**: `[aleph] runtime = "mcp"` plus a `.mcp.json` naming the command
to spawn; `McpManagerHandle::add_transient_server` starts it, the tool bridge registers its tools, and
`unmount` stops it (EffectScope step `"mcp_server"`). Use the official MCP SDK for your language; do
not speak a private JSON-RPC-over-stdio dialect — nothing on the host side answers it.

MCP has no hook channel: an MCP-kind plugin contributes **tools** (and skills / agents / commands as
static files), not `PreToolUse` / `PostToolUse` handlers. Hooks are `hooks.json` shell commands or
WASM exports.

> Until 2026-09-20 this section described a `NodejsRuntime` at `src/extension/runtime/nodejs/` and an
> `@aleph/plugin-sdk` npm package (`packages/plugin-sdk/`, 961 lines of TypeScript types with no host).
> Both were doc-only; both are gone. The sibling repo still carries the phantom dialect:
> `plugins/media-office/src/index.js:349` (`method === "plugin.call"`) and `:264` (`onPostToolUse`) —
> follow-up in Aleph-plugins, not here.
```

`:302` `│    Node.js → NodejsRuntime              │` → `│    MCP  → McpManager (transient server) │`

`:391-394` delete the four lines of the `"nodejs": { … }` object and the trailing comma on the preceding `}` (`:390` `      },` → `      }`).

`:438` `kind = "nodejs"                     # nodejs | wasm | static` → `kind = "wasm"                       # wasm | mcp | static`
`:439` `entry = "dist/index.js"             # Entry point for nodejs/wasm` → `entry = "plugin.wasm"               # Entry point (wasm only; mcp uses .mcp.json)`

`:607` `3. Add \`kind\` field (\`nodejs\`, \`wasm\`, or \`static\`)` → `3. Add \`kind\` field (\`mcp\`, \`wasm\`, or \`static\`)`

`docs/reference/PLUGIN_SYSTEM.md:619-620`:

```markdown
当前状态：目录结构已迁移到 CC 兼容格式（`.claude-plugin/plugin.toml`），Node.js 插件标记为 `runtime = "mcp"` 但 `src/index.js` 仍是旧 IPC 格式——那个格式（`method === "plugin.call"`）从来没有宿主，例：`plugins/media-office/src/index.js:349`；其 `:264 onPostToolUse` 是无人能调的 JS hook handler。本仓 2026-09-20 删掉了同形状的 `examples/plugins/media-video`。
需要：将每个 Node.js 插件的入口文件改为 MCP Server SDK 实现（兄弟仓 Aleph-plugins 的 follow-up）。
```

- [ ] **Step 4: Verify (the post-condition greps)**

```bash
ls packages examples/plugins 2>&1                                   # both: No such file or directory
rg -n -i 'nodejs|node\.js' docs/reference/EXTENSION_SYSTEM.md         # expected: only lines inside the new section (Node.js named as a language, never as a runtime) — count them and paste
rg -n 'NodejsRuntime|runtime/nodejs|plugin-sdk|createServer' docs/reference/EXTENSION_SYSTEM.md   # expected: only the historical `>` block of the new section (3 hits)
rg -n 'media-video' --glob '!node_modules' . | grep -v '^./docs/'      # expected 0
cargo test -p alephcore --lib --no-run                                 # unaffected, sanity
```

- [ ] **Step 5: Commit**

```bash
git add -A packages examples/plugins docs/reference/EXTENSION_SYSTEM.md docs/reference/PLUGIN_SYSTEM.md
git commit -m "extension: delete the phantom Node runtime — plugin-sdk package, media-video example, doc section

packages/plugin-sdk (961 lines TS) described a protocol with no host;
examples/plugins/media-video declared kind=nodejs (unknown variant, never
loaded) and spoke a private plugin.call dialect. EXTENSION_SYSTEM.md's
'Node.js Runtime' section named a directory that does not exist; replaced
with the truth: Node plugins are MCP-kind stdio servers. Sibling-repo
follow-up (media-office) recorded by path:line.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P5.8: `AlephSkillSpec` Phase-2 deadline — say the true status

**Files:**
- Modify: `docs/reference/SKILL_MODEL_TAXONOMY.md:108-115` (section "## Phase 2 timing rule (≥ 2026-06-03)" — the brief said `:100-110`; at `3ddc1f2e7` the heading is `:108`, the date line `:115`) and `:92` ("Phase 1 bridge (active 2026-05-20 → Phase 2)")
- Modify: `src/tools/markdown_skill/spec.rs:11-19` and `src/tools/markdown_skill/mod.rs:6-9` (the deprecation note and module doc carry the same date — same fact, same commit)

**Current text (quoted at `3ddc1f2e7`):**

```markdown
## Phase 2 timing rule (≥ 2026-06-03)

Phase 2 — the destructive absorption that deletes `AlephSkillSpec` — must not begin until:

1. At least **two weeks** have passed since Phase 1 ships, AND
2. No regression has been reported against `project_skill_system_wiring_shipped` in those two weeks.

The earliest practical Phase 2 start date is **2026-06-03**. See `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md` §4.2 for the full Phase 2 task list.
```

```rust
// spec.rs:11-19
/// **Deprecated:** Phase 1 of skill data model unification (see
/// `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md`
/// and `docs/reference/SKILL_MODEL_TAXONOMY.md`).
/// Phase 2 (earliest 2026-06-03) absorbs the fields into
/// `crate::domain::skill::SkillManifest` and deletes this type.
#[deprecated(
    since = "26.5.20",
    note = "use crate::domain::skill::SkillManifest via From impl; will be removed in Phase 2 (≥2026-06-03) per docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md"
)]
// mod.rs:6-9
//! Phase 1 of skill data model unification deprecates `AlephSkillSpec` in
//! favor of `crate::domain::skill::SkillManifest`; the module itself remains
//! the only legitimate consumer until Phase 2 (≥2026-06-03) absorbs the
//! types. See docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md.
```

- [ ] **Step 1: Pre-condition grep**

`rg -n '2026-06-03' docs/reference/SKILL_MODEL_TAXONOMY.md src/tools/markdown_skill/` → expected 5 hits: `SKILL_MODEL_TAXONOMY.md:108`, `:115`, `spec.rs:14`, `spec.rs:18`, `mod.rs:8` (paste).

- [ ] **Step 2: Replace**

`SKILL_MODEL_TAXONOMY.md:108-115` (heading through the "earliest practical" paragraph) → exactly:

```markdown
## Phase 2 status（截至 2026-09-20：逾期，未排期，DECIDE）

Phase 2 — the destructive absorption that deletes `AlephSkillSpec` — was gated on two conditions
(two weeks after Phase 1; no regression against `project_skill_system_wiring_shipped`). Both were met by
**2026-06-03**. It did not happen. As of **2026-09-20** the module is 3.5 months past that date, still
`#[deprecated(since = "26.5.20")]`, still the only parser of `metadata.aleph.input_hints` and still
**live** (`run_loop/inner.rs join_markdown_skills`, boot `start/mod.rs`).

**Status: overdue, not scheduled.** The 2026-09-20 plugin-scope round only CUT the unread
upstream-dialect DTO out of it (see Layer 3 above); absorbing `input_hints` / `security` / `docker` /
`requires.bins` onto `SkillManifest` and deleting the module is a separate round that needs a user
ruling (spec `2026-09-20-plugin-scope-and-cc-compat-design.md` §8 DECIDE 1). Until that ruling this
section states a date that passed, not a date that is coming. Task list for whenever it is scheduled:
`docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md` §4.2.
```

`SKILL_MODEL_TAXONOMY.md:92` `## Phase 1 bridge (active 2026-05-20 → Phase 2)` → `## Phase 1 bridge (active since 2026-05-20; Phase 2 unscheduled)`

`spec.rs:14-15` → `/// Phase 2 (gated ≥ 2026-06-03, **overdue and unscheduled as of 2026-09-20**) absorbs` / `/// the fields into \`crate::domain::skill::SkillManifest\` and deletes this type.`
`spec.rs:18` note → `note = "use crate::domain::skill::SkillManifest via From impl; slated for removal in Phase 2 (overdue since 2026-06-03, unscheduled as of 2026-09-20 — see docs/reference/SKILL_MODEL_TAXONOMY.md)"`
`mod.rs:8` → `//! the only legitimate consumer until Phase 2 (overdue since 2026-06-03, unscheduled) absorbs the`

- [ ] **Step 3: Verify**

`rg -n '2026-06-03' docs/reference/SKILL_MODEL_TAXONOMY.md src/tools/markdown_skill/` → every remaining hit sits next to the word `overdue`/`逾期` (paste). `cargo test -p alephcore --lib tools::markdown_skill` → PASS (doc-only change in code).

- [ ] **Step 4: Commit**

```bash
git add docs/reference/SKILL_MODEL_TAXONOMY.md src/tools/markdown_skill/spec.rs src/tools/markdown_skill/mod.rs
git commit -m "docs(skill-taxonomy): state the true Phase-2 status — overdue since 2026-06-03, unscheduled

The deadline passed 3.5 months ago and read as if it were coming. The
deprecation note and module doc carried the same date; all three now say
overdue/unscheduled and point at the DECIDE item in the 2026-09-20 spec.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### P5 references (no task here — owned elsewhere)

- `PluginStatus::Overridden` removal (zero producers; `mod.rs:556-580` records `shadowed` on the winner) → **P3** (contract "Status"). The doc claim at `PLUGIN_SYSTEM.md:149-160` is fixed by **P8.3** in this file.
- `reload_plugin(id)` narrow twin (`mod.rs:1303-1341`) → **P1** (contract "Lifecycle"). `handle_reload` (`plugins/handlers/runtime.rs:234-272`) is kept by P5.3 for P1 to rewire.

---

## Phase P8 — documentation (each task one commit; exact text)

> Doc tasks keep the five-step shape: Step 1 is the pre-condition grep (must match now), Step 3 is the exact text, Step 4 is the post-condition grep. No cargo test exists for prose; the grep IS the test and it is stated with its expected output.

### Task P8.1: ARCHITECTURE.md — delete the phantom `src/clawhub/` row

**Files:**
- Modify: `docs/reference/ARCHITECTURE.md:261`

Current line (quoted): `| **clawhub** | \`src/clawhub/\` | ClawHub integration |`

- [ ] **Step 1: Pre-condition** — `ls src/clawhub` → `No such file or directory`; `rg -n 'src/clawhub' docs/reference/ARCHITECTURE.md` → exactly `:261`.
- [ ] **Step 2: Delete `:261`.** No replacement row: the Hub is already listed as `hub` (`src/hub/`, verify with `rg -n '\*\*hub\*\*' docs/reference/ARCHITECTURE.md` — if absent, add `| **hub** | \`src/hub/\` | Aleph Hub — extension catalog + install pipeline ([ALEPH_HUB.md](./ALEPH_HUB.md)) |` in alphabetical position after `group_chat`).
- [ ] **Step 3: Post-condition** — `rg -n 'clawhub' docs/reference/ARCHITECTURE.md` → 0.
- [ ] **Step 4: Commit**

```bash
git add docs/reference/ARCHITECTURE.md
git commit -m "docs(architecture): drop the src/clawhub/ row — the directory never existed

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.2: ALEPH_HUB.md — keep the rulings, drop the OpenClaw comparison framing

**Files:**
- Modify: `docs/reference/ALEPH_HUB.md:3-5, 54, 230-257`

Current text (quoted):

```markdown
<!-- :3-5 -->
> 定位速查见 [FEATURE_LOCATOR.md §5.21](FEATURE_LOCATOR.md)。本文补它不承载的三件：
> **线上契约**、**安装管线各阶段的强制点**、以及 **openclaw `clawhub` 逐项对照表**
> （改这一层前先看那张表，不必重做对比）。
<!-- :54 -->
    "via": "clawhub"              // 可选上游出处标签；给了就胜过 manifest.name
<!-- :230 -->
## 7. openclaw `clawhub` 逐项对照 (Gap Analysis)
<!-- :232-234 -->
参考实现：`T:/Github/openclaw` 的 `src/skills/lifecycle/clawhub.ts`、
`src/infra/clawhub.ts`、`src/security/install-policy.ts`、
`src/state/claw-package-{adoption,lifecycle-lease}.ts`。
<!-- :236-251 the 14-row table "| 维度 | openclaw | Aleph 现状 |" -->
<!-- :253-257 -->
**刻意不移植**：openclaw 的 `install-policy.ts` 是一套可配置的安装期静态扫描 +
外部命令钩子（749 行）。Aleph 的对位面是 `[sandbox.command_policy]` 硬底线 +
exec tier + 披露门，三者已在 `src/tools/scoped/` 有唯一强制点；把第二套策略引擎装进
安装路径会造出第二个强制点（违 SECURITY.md 的单点原则）。
```

- [ ] **Step 1: Pre-condition** — `rg -n -i 'openclaw|clawhub' docs/reference/ALEPH_HUB.md` → 14 hits (evidence §5.1 count for this file; paste).

- [ ] **Step 2: Replace**

`:3-5` →

```markdown
> 定位速查见 [FEATURE_LOCATOR.md §5.21](FEATURE_LOCATOR.md)。本文补它不承载的三件：
> **线上契约**、**安装管线各阶段的强制点**、以及 **§7 的设计裁定清单**
> （改这一层前先看那张表：哪些能力是有意不做的，为什么）。
```

`:54` → `    "via": "github:acme"          // 可选上游出处标签；给了就胜过 manifest.name`

`:230-257` (heading through the 刻意不移植 paragraph, up to the `---` at `:259`) → exactly:

```markdown
## 7. 设计裁定：有意不做的能力（与它们的理由）

这一节曾是一张与某个参考实现的逐项对照表（2026-09-20 起去掉对照框架；裁定本身不变——
每一条的主语是 Aleph 自己的形状，不是"别人有而我们没有"）。

| 能力 | Aleph 的裁定 | 理由 |
|---|---|---|
| 目录检索 | ✅ `hub_catalog_search`（name/description/tags/author，服务端 `matches_query`） | — |
| 安装出处 | ✅ 单份 `install_origin` 表；**有意**不做双文件互证（origin + lock） | 见 §5「为什么这样切」：一份真源，两份就是判据 §1 |
| 内容摘要 | ✅ 复用 `directory_digest`（排序、排除 `.git`、符号链接既不哈希也不拷贝、`/` 归一） | 与 skills 同一把尺 |
| 产物完整性 | ✅ `GitDir.sha256` 在第一次写盘前比对 | — |
| 目录完整性 | ✅ `entry_count` + id 唯一性 + 保留命名空间；`content_hash` 有意 CUT | 单源策展目录下逐产物哈希是第二份真源 |
| 不可变 ref | ⚠️ `git_ref` 生效且 detached、解析不出报错，但**不强制** 40 位 SHA（tag/branch 也接受） | 单源策展目录下 tag 由我们自己发布；强制 SHA 的收益不抵可读性损失 |
| 权威钳制 | ✅ `TrustTier::clamped_to(源上限)` | — |
| 版本/更新 | ⚠️ **检测**已实现（徽标会亮）；**一键更新**未做 | 见 §8 |
| 兼容门（plugin API 版本区间） | ❌ 未做 | Aleph 的 plugin API 无版本区间概念；等真出现破坏性 API 分代再谈（用户裁定 2026-08-19 继续 defer） |
| 并发租约（安装期 sqlite 租约 + 心跳） | ❌ 未做 | 见 §8 |
| owner 限定引用 / 歧义 slug | N/A | 单份策展目录，id 全局唯一由 `validate()` 保证 |
| 安装遥测 | ❌ **有意不做** | 隐私 |
| promotions feed | ❌ **有意不做** | 编辑位应由目录发布端决定，本地 `featured_picks` 只是确定性占位 |
| 安装期策略引擎（可配置静态扫描 + 外部命令钩子） | ❌ **有意不做** | Aleph 的对位面是 `[sandbox.command_policy]` 硬底线 + exec tier + 披露门，三者已在 `src/tools/scoped/` 有唯一强制点；把第二套策略引擎装进安装路径会造出第二个强制点（违 SECURITY.md 的单点原则） |
```

- [ ] **Step 3: Post-condition** — `rg -n -i 'openclaw|clawhub' docs/reference/ALEPH_HUB.md` → 0 (if any hit remains outside `:3-5, :54, :230-257`, quote it and relabel in the same commit — the 14 pre-count says there are none, but the grep decides).

- [ ] **Step 4: Commit**

```bash
git add docs/reference/ALEPH_HUB.md
git commit -m "docs(aleph-hub): §7 keeps every ruling, drops the comparison framing

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.3: PLUGIN_SYSTEM.md — lifecycle four primitives, ClaudeCache, commands/agents body, `Overridden` claim, parity relabels, DEVIATION list

**Files:**
- Modify: `docs/reference/PLUGIN_SYSTEM.md:149-171, 173-179, 334-350, 353-383, 570-578, 672, 683-706 (only :690, :699), 708, 719, 790-820`
- Insert: new section before `:790 ## 进程级投影的单一咽喉`

**Consumes (contract names):** `ExtensionManager::{mount, unmount, reload_plugin, reload}`, `after_transition`, `EffectScope`, `Disposer`, `DisposeReport`, six step labels `registry_row | wasm_module | mcp_server | service | memory_extension | slash_command`, `PluginStatus::Pending { waiting_on }`, `ScopeKey::{Global, Project}`, `VisibilityCtx`, `visible_to`, `PluginOrigin::ClaudeCache`, `AgentDef.system_prompt`, `SkillTemplate::render`, `McpFace::notify_tools_list_changed`, `CC_TOOL_ALIASES`, `MAX_HOOK_TIMEOUT_SECS`.

- [ ] **Step 1: Pre-condition greps**

```bash
rg -n 'openclaw' docs/reference/PLUGIN_SYSTEM.md            # :672, :677, :688, :699, :708, :719 — 6 hits (:688/:699 are inside the CUT-planner history block)
rg -n 'overridden|Overridden' docs/reference/PLUGIN_SYSTEM.md # :155, :159
rg -n 'ClaudeCache|mount\(|unmount\(|after_transition|visible_to|ScopeKey' docs/reference/PLUGIN_SYSTEM.md   # 0
```

- [ ] **Step 2: Edits (exact text)**

**(a) `:149-171` 插件状态** — **written against P3.1's text** (R5.3 / G-2): P3.1 already deletes the `overridden` row, adds the `pending` row and rewrites the blockquote (see plan-P2P3 P3.1 Step 3, "`docs/reference/PLUGIN_SYSTEM.md:149-165`"). P8.3(a) changes only the `含义` cells so each row names the enum variant with **today's names** (`Loaded / Pending / Disabled / Error / Blocked` — no `Active`, no `Failed`), and the mount-failure semantics. Resulting table:

```markdown
## 插件状态（`plugins.list` 的 `status`）

| status | 含义 | 补救 |
|--------|------|------|
| `loaded` | 活跃，capability 对模型可见（`PluginStatus::Loaded`） | — |
| `pending` | 已 mount，但某个声明的依赖尚未到达终态（`PluginStatus::Pending { waiting_on }`：MCP manager 未接上 → `mcp:manager`；server 未完成 `initialize` → `mcp:<server_id>`）。**不因超时变 `error`**（判据 §8：「还没准备好」≠「失败了」） | `status_detail` 列出 `waiting on …`；`aleph doctor` 的 `extension/plugins-activated` 逐个点名 |
| `disabled` | operator 关掉了（`plugins.toml`；`PluginStatus::Disabled`）| `aleph plugin enable <name>` |
| `error` | manifest 解析失败，或 `mount` 某一步失败——`PluginStatus::Error("<step>: <reason>")`，由 `lifecycle.rs::write_failed_row` 这一处写入；已注册的部分已 dispose（**全有或全无**） | `status_detail` 给出 step 与原因 |
| `blocked` | owner trust policy 拒绝了它（`PluginStatus::Blocked(reason)`） | `plugin_manage(action='trust', name=…)` |
```

The blockquote below the table is P3.1's text, unchanged by this task (it already states that `overridden` never had a producer and was removed in 2026-09, and that `PluginRuntimeStatus` is the wire vocabulary). Append one sentence to it: `> \`activation_gate\` 认得的**终态集合从枚举派生**（守卫 G6），不是手写清单；\`Pending\` 计入 \`is_active()\`。`

**(b) `:173-179` Runtime 模型** — the three-row table stays as is; append this paragraph after it (before the `---`):

```markdown
Runtime 与 **origin** 是两个轴。origin（`PluginOrigin`）多了一个值：`ClaudeCache`——`~/.claude/plugins/`
里 Claude Code 已装的插件（读 `installed_plugins.json`，解析到 `cache/<marketplace>/<plugin>/<version>/`）。
**只读发现**：Aleph 永不写 `~/.claude/`，不读 `settings.json`；这一 origin 的插件**默认 disabled**
（`plugins.toml` 里缺席即 disabled，只对这个 origin 如此），`plugin_manage list` 带 origin 露出，
一个动词启用，启用态只写 `plugins.toml`。`installed_plugins.json` 形状变了 → 整个来源跳过 + 一条 warn，
其它来源不受影响。
```

**(c) `:334-350` Scope 管理** — replace the section with (outer fence is four backticks because the text contains a fenced block):

````markdown
## Scope 管理：发现路径 与 可见性是两件事

**发现路径**（谁被扫到，优先级高→低）：

| Scope | 路径 | `ScopeKey` |
|-------|------|-----------|
| `agent-level` | `~/.aleph/agents/<id>/plugins/` | `Global` |
| `local` | `<project>/.aleph/plugins.local/` | `Project(root)` |
| `project` | `<project>/.aleph/plugins/`、`<project>/.claude/` | `Project(root)` |
| `user` | `~/.aleph/plugins/installed/` | `Global` |
| `claude-cache` | `~/.claude/plugins/cache/…`（只读） | `Global` |
| `bundled` | 编译期嵌入 | `Global` |

**可见性**（谁在哪个请求里看得见，2026-09-20 起）：每条 registry 行在发现时带一个 `ScopeKey`；
每张脸在**请求构建时**调同一个谓词 `visible_to(key, ctx)`（`src/extension/visibility.rs`，前身是只服务
hooks 的 `scope.rs::project_scope_allows`）：`Global` 永远可见；`Project(p)` 只对
`ctx.project_root == Some(p)` 的会话可见。五张脸共用：tool index · skills 索引 · agents 解析 · slash 列表 ·
MCP tool bridge（按拥有 server 的插件的 key 过滤；server 进程本身仍是全局的）。hooks 的
`project_scope_allows` 改为调它。**`VisibilityCtx.project_root` 只有一份推导**——hooks 今天那一份
（`executor.rs:918` 上游）抽出来共用，不新造第二个「当前项目」。

⚠️ **行为变更**：**无 project 的会话只见 `Global`**（fail-closed；此前是全部可见）。一个在 `~` 里起的
会话再也看不到某个项目的 `.claude/` 插件——这是有意的。

```bash
aleph plugin install <name> --scope user      # 默认
aleph plugin install <name> --scope project   # 团队共享
aleph plugin install <name> --scope local     # 个人项目
```
````

**(d) `:353-383` 安装第三方 Claude Code 插件** — replace the 支持的组件类型 table rows for `agents` and `commands`, add P4.6's `timeout` row, and append the DEVIATION / CONNECT block after the table (R5.3: DEVIATION = hook timeout 300 s · skills CC-only fields (P4.12's sentence) · agent `permissionMode` · `~/.claude/settings.json` not read · hooks file at `~/.aleph/hooks.json`; `hook_event_name` is CONNECT per U-b):

```markdown
| `agents/*.md` | ✅ 完全支持 | frontmatter → `AgentDef`，**正文 → `AgentDef.system_prompt`**（2026-09-20 起；此前正文丢弃）。`permissionMode` **解析但不应用**（见下方 DEVIATION 3）；`color` 忽略 |
| `commands/*.md` | ✅ 完全支持 | slash 条目随 mount/unmount 注册/撤销；`/cmd args` 时**正文经 `SkillTemplate` 展开后注入本轮**（`$ARGUMENTS` / `$1..$N` / `${N:-d}` / `@file` 经沙箱读 / `` !`cmd` `` 经 shell 同意闸）——展开后的正文**瞬时投递**，转录里持久化的是原始 `/cmd args`（U-c）。`argument-hint` 进列表；`allowed-tools` 作本轮静态 retain；`model` 走请求级 pin；`disable-model-invocation` 只留人类入口 |
| `hooks/hooks.json` `timeout` | ⚠️ 偏离 | Claude Code 默认 600 s；Aleph 默认 **300 s** 且上限 **300 s**（`MAX_HOOK_TIMEOUT_SECS`，`src/extension/hooks/mod.rs`）——hook 在工具派发内运行，本就受 180 s tool budget 约束，更长的值会被钳到 300 并记一条 warn。写 `timeout: 600` 不报错，只是拿不到 600。 |
```

and after the table:

```markdown
**DEVIATION（有意与 Claude Code 不同；验收表 `scan-cc-plugin-format.md` 70 项里标 DEVIATION 的就是这几条）：**

1. **hook `timeout` 默认 300 s、上限 300 s**（CC 600 s）——见上表那一行；理由：hook 跑在工具派发内，受 tool budget 约束。
2. **skills 的 CC 专属字段**：`skills/*/SKILL.md` Claude-Code-only fields (`when_to_use`, `argument-hint`, `arguments`, `disallowed-tools`, `model`, `effort`, `context: fork`, `agent`, `background`, `hooks`, `paths`, `shell`) parse without error and are NOT honoured, except `disable-model-invocation`, `user-invocable`, `allowed-tools` (pre-grant) and `when_to_use` (read). A skill relying on `context: fork` runs inline; one relying on skill-scoped `hooks` gets none.
3. **agent `permissionMode` 不应用**：sub-agent 跑在父的 `ScopedToolService` 上，没有自己的执行档；值被解析并以 `debug!` 记下它本会映射到的档，偏离可见于日志而不是静默。
4. **不读 `~/.claude/settings.json`**：启用态由 `plugins.toml` 决定（U6）。
5. **用户级 hooks 文件在 `~/.aleph/hooks.json`**，不是 `~/.claude/settings.json` 的 `hooks` 键。

**CONNECT（与 CC 对齐，2026-09-20 接线；不是偏离）：**

- **exit code 2 = block**：JSON 决策与 exit-code 决策在同一个函数里派生；Interceptor 型事件 exit 2 → `blocked { reason: stderr }`（stderr 空也 block，通用原因）；其它非零 → 非阻塞警告；Observer 型只记日志。`hookSpecificOutput.updatedInput` 与 `update_input:` 前缀走同一条路；`permissionDecision: "block"` 亦读作 Block。
- **`hook_event_name` = hook 注册时用的那个拼法**（U-b）：注册为 `PreToolUse` 的 hook 收到 `"hook_event_name":"PreToolUse"`，注册为 `before_tool_call` 的收到 `before_tool_call`——注册行上一个字段、一份推导，Aleph 原生脚本不变；别名表 `CC_TOOL_ALIASES` 只在 matcher 派发时把 CC 名（`Bash` / `Edit` / `Write` / `mcp__srv__tool`）翻成 Aleph 名。
- **`permission_mode`** 从会话执行档映射，`ExecTier::Auto → "auto"`（CC 六值枚举里的真值）。
- **`allowed-tools` 双语义**：command → 本轮限制；skill → 预授权跳审批、不限制。
- **无 project 的会话只见 `Global` 插件**（行为变更，见 Scope 管理；CC 没有对应概念，不列为偏离）。
```

**(e) `:570-578` MCP Runtime Wiring** — replace the 注册编排 and 卸载清理 bullets:

```markdown
- **注册编排（2026-09-20 起走 lifecycle）**：`mount(id)` 对 MCP-kind 插件调 `add_transient_server`，返回的 `Disposer` 记入该插件的 `EffectScope`（step `"mcp_server"`）；`unmount(id)` 逆序 dispose 即 `remove_transient_server`。此前 `set_plugin_enabled(true)` 什么都不做、只有 `reload()` 调 `sync_mcp_plugin_servers`（判据 §14 闸的两个方向不对称）——那条路已删。
- **卸载清理**：不再有单独的「捕获 server id 再拆」逻辑——server id 住在 disposer 闭包里，dispose 就是拆。
```

**(f) `:672` heading** `## Manifest 解析缓存（openclaw parity）` → `## Manifest 解析缓存`; and `:677-679` `openclaw 也有相同模式（\`plugin-cache-primitives.createPluginCacheKey\`），但 Aleph 版本借助 Rust 类型系统多加了 \`dev\`/\`ino\` 字段以对抗硬链接替换。` → `key 里带 \`dev\`/\`ino\` 是为了对抗硬链接替换（`canonicalize` 关不掉硬链接——附录 E.3）。`

**(g) `:688`, `:699`** inside the CUT-planner history: `曾经有过一个 openclaw \`activation-planner.ts\` 的 Rust 移植` → `曾经有过一个从参考实现的 activation planner 移植来的 Rust 版`; `请从 openclaw 的 \`activation-planner.ts\` 和` → `请从参考实现的 activation planner 和` (the section stays — it is the record that lazy activation was built, never fired, and deleted). `:677` is covered by (f).

**(h) `:708` heading** `## Owner Trust Policy（P3.5 — openclaw parity）` → `## Owner Trust Policy（P3.5）`; `:719` `这对应 openclaw 的 \`passesManifestOwnerBasePolicy\` + bundled 短路。` → `Bundled / Config origin 短路、其余按 allowlist——这是 Aleph 自己的规则，不再标注出处。`

**(i) Insert before `:790`** the new section:

```markdown
## 生命周期四原语（2026-09-20，`src/extension/lifecycle.rs`）

| 原语 | 语义 |
|---|---|
| `mount(id: &str) -> Result<PluginStatus, MountError>` | 解析 manifest → owner-trust / enabled 门（`plugins.toml`；`ClaudeCache` origin 按 origin 判默认 disabled）→ 新建 `EffectScope` → **按固定顺序**注册六种效果 → 任一步失败即 `dispose` 已注册部分（**全有或全无**）→ `write_failed_row` 写 `Error("<step>: <reason>")`；成功写 `Loaded`（MCP-kind 先 `Pending { waiting_on }`，server 完成 `initialize` 后由 `watch_server_starts` 改 `Loaded`） |
| `unmount(id: &str) -> Result<DisposeReport, UnmountError>` | 从 `scopes`（`Mutex<HashMap<String, EffectScope>>`）取出该插件的 `EffectScope` → `dispose`（逆序；单条失败记日志带 step 标签、**不停**）→ 写 `Disabled` / 移除行 |
| `reload_plugin(id)` | `unmount` + `mount`。2026-09-20 前的窄孪生（只刷 tool index、跳过 hooks / projections / MCP / services）已删 |
| `reload()` | 对每个已发现插件 `unmount` + `mount`；`stop_orphaned_services` 变成 dispose 的自然结果 |

六种效果与它们的逆（注册顺序即下表顺序；dispose 逆序，所以 registry 行最后撤——视图重算时它已不在）：

| step 标签 | 注册 ↔ 逆 |
|---|---|
| `registry_row` | `PluginRegistry::register_plugin` ↔ `unregister_plugin` |
| `wasm_module` | `PluginLoader` load ↔ unload |
| `mcp_server` | `McpManagerHandle::add_transient_server` ↔ `remove_transient_server` |
| `service` | `service_manager` start ↔ stop |
| `memory_extension` | `MemoryExtensionRegistry::register*` ↔ `unregister(plugin_id)`（**2026-09-20 新增**——此前 disable 后 `[memory]` 扩展仍挂着） |
| `slash_command` | ToolCatalog `register_skills` ↔ `unregister_skills(&[String])`（**新增**；disposer 持有它注册的那些 id——此前只在 boot 注册一次） |

**规则只有一句**：它有逆操作吗？有 → **效果**，注册函数返回 `#[must_use] Disposer`，由 lifecycle 放进该插件的
`EffectScope`；没有但能从 registry 重算 → **视图**（tool index 快照、`PLUGIN_SKILL_DIRS`、`PLUGIN_SUBAGENTS`、
`HookExecutor`），由下一节那一个函数派生；两者都不是 → 它不该由插件写入运行时。

**每次迁移之后，且只在 `after_transition()` 这一处（每个公共原语跑一次）**：`republish_plugin_projections()` +
`sync_hooks_from_registry()` + `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed() }`。
迁移在既有的 `load_guard` 上串行。`set_plugin_enabled(true/false)`、watcher、`plugin.reload` / `hooks.reload` RPC
都只调这四个原语——没有第五条改激活态的路。插件 id 就是 `String`（无 newtype）。

守卫：G1 census（`registrar/` `service_ops.rs` `src/extension/loader.rs` `memory/extensions/` 里产生运行时副作用的
`pub fn` 必须返回 `Disposer`）· G2 往返（夹具插件覆盖六种效果，`mount` → 六面快照 → `unmount` → 快照 == mount 前）·
G3（`publishing_plugin_projections_has_exactly_one_author` 改钉 `lifecycle.rs` 里的那个调用点）。
```

**(j) `:806-812`** in 进程级投影的单一咽喉 — replace the Cordis paragraph:

```markdown
这些是 **effect 不是返回值**——之后的任何一次调用都不会提醒你它们还装在那儿。2026-08-16 的答案是
「一个函数从 registry 派生整套投影，每一条能改变插件激活状态的路径都调它」，并刻意不引入 fiber 运行时。
**那个答案只对了一半**（2026-09-20）：它证明的是「改激活态的路径都调了那一个函数」，证不出「那个函数
盖住了所有面」——派生列举了三个面，三个月里在派生之外漏了四处（memory extension 无 unregister、slash
只在 boot 注册、MCP disable→enable 不重挂、`reload_plugin` 窄孪生）。这是列举法（判据 §5）。
现在**效果归 `EffectScope`、视图归派生**（上一节）：派生函数仍然只写一遍谓词，但它只负责**可重算**的
东西，且**唯一的触发点是 `lifecycle.rs::after_transition`**。DI 容器 / Proxy 上下文 / 级联重启仍不采。
```

- [ ] **Step 3: Post-condition greps**

```bash
rg -n 'openclaw' docs/reference/PLUGIN_SYSTEM.md                          # 0
rg -n 'overridden' docs/reference/PLUGIN_SYSTEM.md                         # only inside the "于 2026-09-20 删除" blockquote (2 hits)
rg -n 'after_transition|EffectScope|visible_to|ClaudeCache|system_prompt|CC_TOOL_ALIASES' docs/reference/PLUGIN_SYSTEM.md | wc -l   # >= 8
```

- [ ] **Step 4: Commit**

```bash
git add docs/reference/PLUGIN_SYSTEM.md
git commit -m "docs(plugin-system): lifecycle four primitives, ClaudeCache, visibility, commands/agents body, DEVIATION list; drop the Overridden claim and parity labels

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.4: EXTENSION_SYSTEM.md — "Effects and EffectScope" + "ScopeKey and visibility" sections

**Files:**
- Modify: `docs/reference/EXTENSION_SYSTEM.md` — insert two sections immediately after the "Node plugins run as MCP stdio servers" section written by P5.7 (i.e. before `## Plugin Discovery`), and fix `:403-413` (`plugins.reload` never existed; the table names it).

**Consumes:** contract signatures verbatim (`Disposer`, `sync_disposer`, `async_disposer`, `EffectScope`, `DisposeReport`, `ScopeKey`, `VisibilityCtx`, `visible_to`).

- [ ] **Step 1: Pre-condition** — `rg -n 'EffectScope|visible_to' docs/reference/EXTENSION_SYSTEM.md` → 0; `rg -n 'plugins\.reload' docs/reference/EXTENSION_SYSTEM.md` → `:412`.

- [ ] **Step 2: Insert (exact text)** — outer fence is four backticks because the text contains fenced blocks:

````markdown
## Effects and `EffectScope` (temporal composability)

**Location**: `src/extension/effects/{mod.rs, scope.rs, disposer.rs}` (2026-09-20)

```rust
/// One reversible side effect a plugin made on the running process.
/// Dispose is async (MCP server removal, service stop) and reports failure
/// instead of panicking; the scope records the Err and keeps going.
pub type DisposeOutcome = Result<(), String>;
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, DisposeOutcome> + Send>;
pub fn sync_disposer(f: impl FnOnce() -> DisposeOutcome + Send + 'static) -> Disposer;
pub fn async_disposer<F, Fut>(f: F) -> Disposer
where F: FnOnce() -> Fut + Send + 'static, Fut: Future<Output = DisposeOutcome> + Send + 'static;

pub type PluginId = String;            // bare String, no newtype
pub const STEP_LABELS: [&str; 6];      // the six step labels, in registration order
pub struct EffectScope { /* plugin_id, disposers: Vec<(&'static str, Disposer)>, skipped */ }
impl EffectScope {
    pub fn new(plugin_id: PluginId) -> Self;
    pub fn effect(&mut self, step: &'static str, d: Disposer);
    /// A step the plugin declares but this process cannot provide (e.g. no MCP
    /// handle): recorded, not an error; `Pending { waiting_on }` derives from it.
    pub fn skip(&mut self, step: &'static str, why: impl Into<String>);
    /// Reverse registration order. A failing/panicking disposer is recorded
    /// and does NOT stop the rest. Consumes self: a scope cannot be half-disposed.
    pub async fn dispose(self) -> DisposeReport;
}
pub struct DisposeReport { pub plugin_id: PluginId, pub steps: Vec<(&'static str, DisposeOutcome)> }  // all_ok() / failures()
```

**The one rule — has an inverse → effect; recomputable from the registry → view.** Every registrar
function that puts something into the running process (`registrar/`, `service_manager.rs`,
`src/extension/loader.rs`, `memory/extensions/`) returns `#[must_use] Disposer`; the caller
(`lifecycle.rs::mount`) pushes it into the plugin's `EffectScope` under one of six fixed step labels,
in this order: `registry_row`, `wasm_module`, `mcp_server`, `service`, `memory_extension`,
`slash_command`. `unmount` disposes in reverse, so the registry row is the last thing to go and every
view recomputed afterwards already sees the plugin gone. Anything with no inverse that can be
recomputed from `PluginRegistry` (tool-index snapshot, `PLUGIN_SKILL_DIRS`, `PLUGIN_SUBAGENTS`,
`HookExecutor`) is a **view**, derived by `projection.rs` from `after_transition()` only.

This is the ownership rule from DeepSeek Harness / Cordis (`scan-dsh-cordis.md` §1), expressed as
signatures. It is **not** a fiber runtime: no DI container, no Proxy context, no cascade restart, no
HMR (the three earlier rounds' rulings stand — HARNESS_PHILOSOPHY.md §8 第五课, narrowed 2026-09-20).
Guard: `effects::census::every_crate_visible_registration_returns_a_disposer` (source-level census, G1;
mutation: an extra `pub fn register_extra` in `registrar/api.rs` goes red by name) and the six-effect
round-trip (G2, `tests/plugin_lifecycle_roundtrip.rs` + the two fixtures in P1.13).

---

## `ScopeKey` and visibility (spatial composability)

**Location**: `src/extension/visibility.rs` (renamed from `scope.rs`, which served only hooks)

```rust
pub enum ScopeKey { Global, Project(PathBuf /* canonicalized root */) }
pub struct VisibilityCtx { pub project_root: Option<PathBuf> }
/// Global → always visible. Project(p) → visible iff ctx.project_root == Some(p).
/// A session with no project sees Global only (fail-closed).
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool;
```

Every registry row carries a `scope_key` derived at discovery: `Project(root)` only for
`<project>/.claude/` and `<project>/.aleph/plugins{,.local}`; every other origin — `Bundled`, `Config`,
`Global`, marketplace installs, and the new `ClaudeCache` — is `Global`. Project level is the only
level (no session / agent sub-scopes — user ruling U4). The five faces that present plugin capability
to a request (tool index, skills index, agent resolution, slash list, MCP tool bridge) all call
`visible_to` at request-build time; hooks' `project_scope_allows` calls the same predicate.
`VisibilityCtx.project_root` has exactly one derivation — the one hooks already used upstream of
`executor.rs:918` — extracted, not duplicated.

**Behaviour change (2026-09-20)**: a session with no project root sees `Global` plugins only. Before,
discovery was the union of every project and every session saw everything.

---
````

`:403-413` (Plugin RPC Methods table): replace the last row `| \`plugins.reload\` | Reload plugin |` with `| \`plugin.reload\` | Reload one plugin (unmount + mount) |` and add `| \`plugins.callTool\` | Call a tool on a loaded runtime plugin (CLI) |`.

- [ ] **Step 3: Post-condition** — `rg -n 'EffectScope|visible_to|has an inverse' docs/reference/EXTENSION_SYSTEM.md | wc -l` → ≥ 6; `rg -n 'plugins\.reload' docs/reference/EXTENSION_SYSTEM.md` → 0.

- [ ] **Step 4: Commit**

```bash
git add docs/reference/EXTENSION_SYSTEM.md
git commit -m "docs(extension-system): Effects/EffectScope and ScopeKey/visibility sections; fix the RPC table

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P8.5: GATEWAY.md — "MCP 面 (`src/gateway/mcp_face/`)" section

**Files:**
- Modify: `docs/reference/GATEWAY.md` — insert a `###` subsection under `## HTTP Server` (`:1150`), after `### Channel webhook ingestion` (`:1276-1352`) and before `## See Also` (`:1354`). No new reference file (CLAUDE.md 写入纪律).

**Consumes:** plan-P6 (reconciled): `McpServerConfig { enabled, expose }` `[mcp_server]` (P6.1, `DEFAULT_EXPOSE_EXCLUDES` name-by-name subtractions), session table `Mcp-Session-Id` ↔ Aleph session + `McpClient` (P6.2), per-request auth = the `connect` rules (P6.3), `McpFace` + `try_mcp_face() -> Option<&'static Arc<McpFace>>` + `MCP_APPROVAL_HINT` + `OperatorPresence` probe (P6.4), `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]` (P6.5), `POST/GET(SSE)/DELETE /mcp` with the `/ws` guards and status map 401/403/404/426 + the remote-only limiter `MCP_REMOTE_POSTS_PER_MINUTE = 120` → 429 + `Retry-After` (P6.6, R6.4), `DEFAULT_EXPOSE_EXCLUDES` = `tool_usage` / `config_audit` / `node_list` / `user_profile` ⇒ 36 pinned names (P6.1), `packages/pi-aleph/README.md` as the client-side how-to (P6.8), `after_transition` → `notify_tools_list_changed` (R6.1), **live-apply of `[mcp_server].expose`** (P6.9: `mcp_server.expose` in `LIVE_SUBSECTIONS`; `enabled` stays `Restart`). The pi-aleph pointer sentence is carried HERE (lead addendum: the section is P8.5's, so P6.8 cannot write into it). Version literals verified at `3ddc1f2e7`: `src/mcp/protocol.rs:658 MCP_LEGACY_PROTOCOL_VERSION = "2025-03-26"`; `src/mcp/modern/mod.rs:42 MCP_MODERN_PROTOCOL_VERSION = "2026-07-28"` is the handshake-less/sessionless dialect and is **not spoken** (R6.2). Cites spec §3.7.

- [ ] **Step 1: Pre-condition** — `rg -n 'mcp_face|/mcp\b' docs/reference/GATEWAY.md` → 0.

- [ ] **Step 2: Insert (exact text)**

```markdown
### MCP 面（`src/gateway/mcp_face/`，2026-09-20）

Aleph 对外**是一个 MCP server**：dsh 一行 `cordis.yml`、pi 一行 `pi-mcp-adapter` 配置、Claude Code 一行
`.mcp.json`（`type: http`）都能挂上，零专属代码。**客户端侧的接法只写一份**：`packages/pi-aleph/README.md`——
一个 JS-free 的 pi 包（`package.json#pi = { skills, mcp }` + `mcp.json` + `skills/aleph/SKILL.md`），README 同时给出
pi（经 pi-mcp-adapter）、Claude Code `.mcp.json`（`type: http`）与 dsh `cordis.yml` 的三行配置；本节不抄那三行。设计母本：spec
`docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md` §3.7。它是一张**接口脸**（R4：
纯 I/O），把 `tools/call` 翻成 Aleph 自己的工具调用路径；**wire 类型复用 `src/mcp/{jsonrpc,protocol,types}.rs`，
信封复用网关自己的 `protocol::JsonRpc{Request,Response}`（客户端侧的 `mcp/jsonrpc.rs` 的 id 是 `u64`，回不了字符串 id），不复制**（CLAUDE.md 禁用清单 2026-09-20 那一行）。

| 项 | 决定 |
|---|---|
| 传输 | **只做 Streamable HTTP**，挂网关已有 HTTP 监听的 `/mcp`：`POST /mcp`（JSON-RPC，一请求一响应；通知 → `202`）+ `GET /mcp`（SSE 通知流）+ `DELETE /mcp`（结束 session）+ `Mcp-Session-Id` 头。**stdio 刻意不做**——宿主 spawn 第二个 `aleph-server` 会撞单例 flock（PROCESS_MANAGEMENT.md）；legacy SSE 传输 dsh 不支持 |
| 协议版本 | 协商：客户端提出的版本在支持集内则接受，否则回**最新**（`2025-11-25`）由客户端决定。支持集 `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26"]`（最老的一个就是客户端栈自己的 `MCP_LEGACY_PROTOCOL_VERSION`；pi-mcp-adapter 默认 `2025-03-26`）。`src/mcp/modern/` 的 `2026-07-28` 是**无握手、无 session** 的方言（`server/discover` + 每请求 `_meta`），这一面**不说它**——`server/discover` 答 `-32601`，adapter 的 `auto` 探测会退回 `initialize` |
| 能力 | 只宣告 `tools: { listChanged: true }` + 短 `instructions`；prompts / resources / sampling / elicitation / roots / tasks **不做**（YAGNI） |
| 认证与护栏 | **复用 `connect` 那一份**（上文「Connect handshake」），逐请求判：loopback 免凭据；远程须 `Authorization: Bearer <token>`（device token / 共享 token），dsh 与 adapter 都是静态 `headers`——无凭据或坏凭据 → **401**（`WWW-Authenticate: Bearer realm="aleph"`）。`/ws` 的三道护栏原样复用：明文远程且未放行 → **426**；Origin 不允许 → **403**；`local` 位来自 `trusted_proxy::resolve_client`，**不是** `ip.is_loopback()`。远程 `POST /mcp` 另有一个私有限流桶：`MCP_REMOTE_POSTS_PER_MINUTE = 120`，超出 → **429** + `Retry-After`，loopback 免（形状同 artifact 路由的桶） |
| 暴露 | `[mcp_server] enabled = true / expose = ["…"]`（`McpServerConfig`）。`expose` 是白名单，**默认集合＝按谓词从工具目录挑出的无副作用工具，再按名字减去 `DEFAULT_EXPOSE_EXCLUDES`**（`tool_usage` / `config_audit` / `node_list` / `user_profile`，每条带理由）——结果是 **36 个名字**，由 P6.1 的逐名测试钉住（数字带谓词：`3ddc1f2e7` 的工具目录；目录变了先红的是那条测试，不是这句）；`bash` / 文件写 / 浏览器须显式加入。启动时校验 `expose ⊆ 工具目录`（守卫 G5：写一个不存在的工具名 → 红 + boot warn）。**`expose` 改动即时生效**（`mcp_server.expose` 在 `LIVE_SUBSECTIONS` 里，面上一份 `ArcSwap<BTreeSet<String>>`），重跑 G5 校验后向每条活着的 session 广播 `list_changed`；**`enabled` 仍是 `Restart`**——开关这张脸要重启 |
| 会话 / principal | 每个 MCP session ↔ 一个 Aleph 会话；principal `McpClient { client_name }`（来自 `initialize.clientInfo.name`）。调用走**同一条** scoped dispatch（审批门 + spend ledger + hooks）——不是第二条执行路径 |
| 审批 | 「有没有 operator 能答」是**连接表**上的事实：`McpFace` 每次调用问 `OperatorPresence` 探针（`GatewayServer::operator_presence_probe()`），派发时 `unattended = !present`——无 operator 界面时 confirm 门**立刻** fail-closed，结果 `isError: true`，文本末尾附 `MCP_APPROVAL_HINT`（告诉对端模型卡本会去哪、怎么开）。⚠️ 这一步**不是** `OperatorApprovalRequester` 的「零订阅者即拒绝」——boot 给事件总线挂了内部消费者，那条臂在真服务器上**永远不触发**（FL 附录 D.0.199）。**有 operator 在线时**：卡按现有审批流升起，**没有死线**（2026-08-28 裁定），MCP 客户端那边等的是它自己的超时（dsh 60 s / adapter 30 s）；客户端放弃后卡**仍挂在 Panel 上**——用户裁定接受这个形状（U-d），「handler future 掉落时收回卡」记为 follow-up，未建 |
| 错误形状 | 工具执行错误 → `isError: true` 的结果（**不是**协议错误）；不在 `expose` → tool not found；body 不是 JSON-RPC → 400 + JSON-RPC error body；非 `initialize` 请求缺 session 头 → 400；未知 / 过期 session → 404 |
| 通知 | 插件 mount/unmount/reload（`lifecycle.rs::after_transition` 里的 `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed() }`，G3 钉住这个调用点）与 `expose` 变更 → 向每条活着的 MCP session 广播 `notifications/tools/list_changed`。进程级句柄 `try_mcp_face() -> Option<&'static Arc<McpFace>>`（同 spend 的 `install_ledger` 形状，FL §5.25）让 lifecycle 不必穿 7 个构造点 |
| 名字 | Aleph 工具名已是 `[a-z0-9_]`，满足 dsh 的 64 字符规则（dsh 侧展示为 `mcp__<server>__<tool>`）；P6 逐名验证无超长名 |

**有 operator 在线时的审批卡是「挂着的」，这是接受下来的形状（用户裁定 2026-09-20，U-d）。** 一个需要确认的
`tools/call` 在 Panel 上升起一张没有死线的卡（2026-08-28 裁定：审批不超时）；MCP 客户端那边等的是**它自己的**超时
（dsh 60 s、pi-mcp-adapter 30 s），到点后客户端把这次调用当失败，而卡**仍挂在 Panel 上**——operator 点批准时，那个
`tools/call` 的 handler future 早已被丢弃，批准落在一个没有人在等的动作上。两种后果都比它们的替代品便宜：立刻拒绝
会把每一个 attended 调用变成 unattended（那正是 `deny` 阶段要证的另一臂），给卡加死线会推翻 08-28 的裁定。
「handler future 掉落时收回卡」记为 follow-up，未建；在那之前，Panel 上一张来自 `McpClient` 的旧卡读作
「对端已经放弃了」，不是「还在等」。

真机：`qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`——每阶段证明什么见
[`qa/README.md`](../../qa/README.md)。

---
```

- [ ] **Step 3: Post-condition** — `rg -n 'mcp_face' docs/reference/GATEWAY.md | wc -l` → ≥ 3; `rg -n '2026-07-28' docs/reference/GATEWAY.md` → exactly 1 and on the line that says it is NOT spoken; `rg -c 'SUPPORTED_PROTOCOL_VERSIONS' docs/reference/GATEWAY.md` → 1 (the set is named by its constant, so a change in P6.5 has one doc line to chase); `rg -n 'packages/pi-aleph/README.md' docs/reference/GATEWAY.md` → 1 (the client-side how-to is pointed at, not copied); `rg -n 'MCP_REMOTE_POSTS_PER_MINUTE|DEFAULT_EXPOSE_EXCLUDES|LIVE_SUBSECTIONS' docs/reference/GATEWAY.md | wc -l` → 3 (each fact named by its constant once).

- [ ] **Step 4: Commit**

```bash
git add docs/reference/GATEWAY.md
git commit -m "docs(gateway): MCP face section — Streamable HTTP /mcp, expose whitelist, auth reuse, list_changed

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.6: HARNESS_PHILOSOPHY.md §8 第五课 — the 2026-09-20 narrowing paragraph

**Files:**
- Modify: `docs/reference/HARNESS_PHILOSOPHY.md:350` — append a new paragraph immediately after 第五课 (`:350`) and before the blank line + 第六课 (`:352`).

Current anchor (quoted, `:350` begins): `**第五课：对一个「everything is a plugin」的 harness 做完 10 维对照，落点依旧全在循环之外（2026-08-15，deepseek-harness/Cordis）**。` … ends `逐项见 FEATURE_LOCATOR §3.1 Round 8。`

- [ ] **Step 1: Pre-condition** — `rg -n '第五课' docs/reference/HARNESS_PHILOSOPHY.md` → `:350` only; `rg -n '2026-09-20' docs/reference/HARNESS_PHILOSOPHY.md` → 0.

- [ ] **Step 2: Insert after `:350` (exact text)**

```markdown

**第五课补注（2026-09-20，插件宿主作用域化轮）：那条「架构本身不移植」的裁定被收窄了一次，收窄的是一句话，不是立场。** 三轮（08-15 / 08-16 / 08-19）都裁「对照 Cordis 但不引 fiber」，代码侧的表述在 `projection.rs:14-24`：「等价保证更便宜——一个函数从 registry 派生整套投影，每条改激活态的路径都调它」。守卫 `publishing_plugin_projections_has_exactly_one_author` 一直是绿的，而它证明的是**调用**不是**覆盖**：派生列举了三个面，三个月里在派生之外漏了四处（memory extension 无 unregister、slash 只在 boot 注册、MCP disable→enable 不重挂、`reload_plugin` 窄孪生——FEATURE_LOCATOR 附录 D.0.196）。本轮**采的只是 dsh 的那一条所有权规则**：每个进运行时的注册返回一个 `Disposer`，由注册方持有的 `EffectScope` 逆序执行；在 Rust 里它是一个返回值类型，不是运行时。**仍然不采**：DI 容器、Proxy 上下文、级联重启、HMR、插件依赖声明与 host API 版本闸（用户 2026-08-19 裁定继续 defer）。落点：`src/extension/effects/` + `lifecycle.rs`——**`src/harness/` 增删 0 行，本轮亦然**（`budget.rs::CEILING` 不动；**第几次刻意不数**——第六课已经说了那是结论不是目标，而一个计数会在下一轮安静地变错）。教训与第六课同源：**一条守卫绿着，先问它变红的条件里包不包括「多了一个它不认识的面」**；不包括，它守的是产地不是覆盖。
```

- [ ] **Step 3: Post-condition** — `rg -n '第五课补注' docs/reference/HARNESS_PHILOSOPHY.md` → 1; `rg -n '第六课' docs/reference/HARNESS_PHILOSOPHY.md` → still exactly one line, after the new paragraph.

- [ ] **Step 4: Commit**

```bash
git add docs/reference/HARNESS_PHILOSOPHY.md
git commit -m "docs(harness-philosophy): 第五课补注 — ownership rule adopted, fiber/DI/cascade still not, harness 0 lines

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P8.7: FEATURE_LOCATOR.md — §3.10 round entry, new §5.27 MCP face, Appendix D.0.196–199, Appendix E triggers

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` at four anchors (line numbers at `3ddc1f2e7`; P5.4 already edited `:1017`, so re-locate by heading, not by number):
  - (a) end of `### 3.10 插件系统` (last bullet ends at `:1243`; `### 3.11` at `:1245`) — append the round entry.
  - (b) after `### 5.26 Gateway 深度加固轮…` (`:3713-3803`), before `## 6. UI / Panel` (`:3804`) — **next free number is §5.27** (the §5 headings run 5.1–5.26 with a `5.23b`; verified by `grep -n '^### 5\.'`).
  - (c) after `**附录 D.0.195**` (`:5149`), before `### 附录 D.1` — append D.0.196, D.0.197, D.0.198, **D.0.199** (R5.4: P6's operator-presence finding) (last existing number is **195**, verified `grep -o '附录 D\.0\.[0-9]*\*\*' | sort -n | tail -1`).
  - (d) E.0 (four trigger lines after `:5656`, the last E.0 bullet), E.3 (one line before `:5759 ### 附录 E.4`), E.4 (one line before `:5816 ### 附录 E.5`), E.9 (one line before `:5958 ### 附录 E.10`). Cross-group pointers from E.3 to a D.0.x entry have precedent (`:5758` area points at `附录 D.4.43`).

**Consumes:** every name from the contract; numbers only where measured (grep counts from P5.1 with predicate + commit).

- [ ] **Step 1: Pre-condition greps**

```bash
grep -n '^### 5\.' docs/reference/FEATURE_LOCATOR.md | tail -2        # 5.25, 5.26 — no 5.27 yet
grep -o '附录 D\.0\.[0-9]*\*\*' docs/reference/FEATURE_LOCATOR.md | sort -t. -k3 -n | tail -1   # D.0.195
rg -n 'mcp_face|EffectScope|ClaudeCache' docs/reference/FEATURE_LOCATOR.md   # 0
```

- [ ] **Step 2 (a): §3.10 round entry** — append after the last bullet of §3.10 (exact text):

```markdown
- **插件宿主作用域化 + CC 兼容补齐 + 砍 OpenClaw + MCP server 面 (2026-09-20，worktree `plugin-scope-round`，spec `2026-09-20-plugin-scope-and-cc-compat-design.md`，四份只读扫描证据在同名 `-evidence/`)**：用户八条裁定（U1–U8）定边界：**效果作用域 + 可撤销注册，不动 harness 循环**；项目级可见性、所有能力种类一个谓词；agents 正文接上（推翻上文「刻意仍不做」）；只读发现 `~/.claude/plugins/`、不读 `settings.json`；MCP 面配置驱动白名单；核心机制 **C：效果归 scope、视图归派生**。
  - **① 上一轮的答案只对了一半（附录 D.0.196）**。2026-08-16 round-2 把「注册是可逆 effect」落成「单函数派生 + 每条路都调它」，守卫 `publishing_plugin_projections_has_exactly_one_author` 一直绿——它证明的是**调用**不是**覆盖**。派生列举三个面，之外漏了四处，每处两端完整中间没线（§7）：`[memory]` 扩展只有 `register` 无 `unregister`（`memory/extensions/registry.rs:95-114`）· 插件 slash 只在 boot `tool_catalog_init.rs:216-247` 注册一次 · MCP 插件 disable 走 `unload_runtime_plugin`、enable 什么都不做（`plugin_ops.rs:586-641`），只有 `reload()` 才 `sync_mcp_plugin_servers`（§14）· `reload_plugin(id)` 是 `reload()` 的窄孪生（`mod.rs:1303-1341`，§16）。**修法是一句所有权规则，不是更长的清单**：新 `src/extension/effects/`（`Disposer` / `EffectScope` / `DisposeReport`），每个产生运行时副作用的注册函数返回 `#[must_use] Disposer`，lifecycle 按固定顺序（`registry_row` → `wasm_module` → `mcp_server` → `service` → `memory_extension` → `slash_command`）放进该插件的 scope，`unmount` 逆序 dispose、单条失败记日志不停、scope 被消费不存在半卸载。新 `lifecycle.rs` 四原语 `mount` / `unmount` / `reload_plugin` / `reload`（从 1,884 行的 `mod.rs` 抽出），`set_plugin_enabled` / watcher / `plugin.reload` / `hooks.reload` 只调它们；视图重算**唯一触发点** `after_transition()`。**收窄不是推翻**（附录 D.0.197）：DI 容器 / Proxy / 级联重启 / HMR / 依赖声明与版本闸仍不采，`src/harness/` 0 行；`projection.rs:14-24` 与 HARNESS_PHILOSOPHY §8 第五课同笔改写。
  - **② 空间可组合：`ScopeKey` + `visible_to`**（`scope.rs` → `visibility.rs`）。每条 registry 行带发现时派生的 key（`Project(root)` 只来自 `<project>/.claude/` 与 `.aleph/plugins{,.local}`，其余全 `Global`），五张脸（tool index · skills 索引 · agents 解析 · slash 列表 · MCP tool bridge）在请求构建时调同一谓词，hooks 的 `project_scope_allows` 改调它；`VisibilityCtx.project_root` 复用 hooks 那一份推导（`executor.rs:918` 上游），不造第二个「当前项目」（§12）。**行为变更：无 project 的会话只见 `Global`**（此前全部可见；fail-closed）。
  - **③ 终态、`Pending { waiting_on }`、activation gate**。`PluginStatus` 加 `Pending { waiting_on }`（mount 了但某声明的依赖未到终态：`mcp:manager` / `mcp:<server_id>`；**不因超时变 `Error`**，§8；计入 `is_active()`）；**变体名不改**（`Loaded / Disabled / Blocked / Error`，mount 失败写 `Error("<step>: <reason>")`，`write_failed_row` 一处）；`Overridden` **删除**（零生产者——`mod.rs:556-580` 只给赢家记 `shadowed`；PLUGIN_SYSTEM.md 那句「三者都有 registry 行」对它是假的）。新 `activation_gate.rs` + doctor 项 `extension/plugins-activated`：boot `load_all` 后列出全部非终态插件及其 `waiting_on`；终态集合从枚举派生（G6）。
  - **④ CC 兼容补齐（执行端）**，十二项，每项能指出接线那一行：exit code 2 = block（JSON 与 exit-code 决策同一函数派生，`hooks/executor.rs:697-713` `:941-952`）· `hookSpecificOutput.updatedInput` 与 `update_input:` 同路 · stdin 补 `transcript_path` / `permission_mode`（`tool_result` 字段名由 P0 真机抓包定，不猜）· 32 事件三分（alias / 加生产者 / 「无此时刻」，**不新增零生产者事件**，G4）；stdin 的 `hook_event_name` **= hook 注册时的拼法**（用户裁定 U-b：注册为 `PreToolUse` 收 `PreToolUse`，注册为 `before_tool_call` 收 `before_tool_call`，一个字段一份推导），`permission_mode` 含 `auto`（`ExecTier::Auto`）· matcher 的 CC 工具名走一张 `CC_TOOL_ALIASES`，只在派发时用 · 超时**保留 300 s**（DEVIATION）· agent `permissionMode` 解析不应用（DEVIATION）· **`commands/*.md` 正文注入**（`/cmd args` → `SkillRegistration{Command}` → 正文经零消费者的 `template.rs` `SkillTemplate` 展开 → 本轮用户内容；`allowed-tools` 作本轮静态 retain，R10 例外同类）· **agents 正文 → `AgentDef.system_prompt`**（插件与磁盘 `agents/*.md` 同一条路）· marketplace `source.source` 与 `source.type` 都认 · **只读发现 `~/.claude/plugins/`**（`PluginOrigin::ClaudeCache`，读 `installed_plugins.json` → `cache/<mk>/<plugin>/<ver>/`，**默认 disabled**，启用态只写 `plugins.toml`，永不写 `~/.claude/`）· `allowed-tools` 双语义 · skills 的 CC 专属字段宽容解析不兑现（DEVIATION）。70 项验收表在 `scan-cc-plugin-format.md` 末尾，每项标 IMPLEMENTED / CONNECT / DEVIATION。
  - **⑤ MCP server 面**：见 §5.27。首跑发现（附录 D.0.199）：「无 operator 界面 ⇒ 拒绝」不能靠 `OperatorApprovalRequester` 的零订阅者臂——boot 给事件总线挂了内部消费者，`publish_frame` 在真服务器上永远 `Ok(n>0)`；presence 要在**连接表**上探。
  - **⑥ CUT 清单（熵减）**，每条带零引用 grep：**OpenClaw 的真实大小 ≈ 50 行**（附录 D.0.198）——`markdown_skill/spec.rs:85-124` 两个从未被读的 serde DTO + 一个字段 + 十来行 attribution 注释 / 夹具 / 三个测试名；`rg -i 'openclaw\|clawhub' src` 从 245 行降到实测值（谓词与数字在 P5.1 提交消息），非注释命中只许 ACP 预设（`config/types/acp.rs:420-429`，R3 托管别人的 agent，**留**）与 `member_add.rs:102`，census `the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset` 钉住。零客户端 RPC **删前逐个多行 grep 复核**（census 脚本只匹配单行字面量）：`plugin.{list,installFromZip,enable,disable,config.get,config.set}`（`handlers/mod.rs:364` 那句「复数是遗留」**说反了**——复数是每个客户端调的）· `plugins.{load,unload,executeCommand}` + `execute_plugin_command` · `command.execute`（**证据纠错**：它在 `tool_catalog_init.rs:475-483` 是被覆盖成真 handler 的，不是永久桩；砍它的理由只有零客户端——每张脸都经 `chat.send` 用同一个 `CommandParser`）· `mcp.*` **十一个全砍**（用户裁定：`resources` / `prompts` 有工具面 `mcp_list_*`，`tools` 孪生 `tools.catalog`，`add/update/delete` 孪生 Panel 在调的 `mcp_config.*`，`logs` 是自报 `implemented:false` 的桩，`status/start/stop/restart` 无人调）——`mcp.list`（CLI doctor）与三个审批动词留；连带 `McpCommand::Aggregate{Tools,Resources,Prompts}` 三个无人说的词。`command.execute` 顺带砍掉只有它在用的 `ToolCatalog::{is_namespace,list_namespace_children}`。幻影 Node 运行时：`EXTENSION_SYSTEM.md:143-218` 整节、`packages/plugin-sdk/`（961 行 TS，零宿主）、`examples/plugins/media-video`（`kind="nodejs"` 是 unknown variant，从未加载过；私有 `plugin.call` 协议）全删；兄弟仓 `plugins/media-office/src/index.js:349` 同形状，记为 follow-up。`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 归档。
  - **验证**：最小可信验证集六条 + `just clippy` 全绿；六条守卫 G1–G6 各有一次记录在案的变异红（红名单在各任务提交消息里，**这里刻意不抄条数**——上一条 entry 写下的 16543 在写下那天就开始漂，真源是 `cargo test` 的输出）；真机 `qa/plugins/run.sh {scope,command,exit2,cc-cache,visibility}` + `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}` 十阶段 PASS，跑在本 worktree 自己 `target/` 的二进制上（共享 target 的第二棵树跑的是第一棵的二进制——附录 C）。
```

- [ ] **Step 2 (b): new §5.27** — insert before `## 6. UI / Panel` (exact text):

```markdown
### 5.27 MCP server 面 (MCP Server Face · 2026-09-20)
- **口语关键词**：把 Aleph 挂进 dsh / pi / Claude Code、Aleph 当 MCP server、`/mcp`、Streamable HTTP、`[mcp_server]`、expose 白名单、`list_changed`
- **代码锚点**：`src/gateway/mcp_face/{mod.rs, config.rs, …}`（接口脸）；wire 类型复用 `src/mcp/{jsonrpc,protocol,types}.rs`；进程级句柄 `try_mcp_face() -> Option<&'static Arc<McpFace>>`；lifecycle 触发点 `src/extension/lifecycle.rs::after_transition`；operator 探针 `GatewayServer::operator_presence_probe()`；QA `qa/mcp_face/run.sh`
- **职责**：`src/mcp/`（19k 行）与 `src/acp/` 都是**客户端**；这一面让 Aleph 自己成为一个 MCP **server**——dsh 一行 `cordis.yml`、pi 一行 `pi-mcp-adapter`、Claude Code 一行 `.mcp.json`（`type: http`）零专属代码挂载。把 `tools/call` 翻成 Aleph 的 scoped dispatch（审批门 + spend ledger 同一条路）。
- **状态**：✅ 已实现（2026-09-20 轮，spec §3.7）。**决定**：只做 Streamable HTTP（`POST/GET/DELETE /mcp` + `Mcp-Session-Id`；stdio 会让宿主 spawn 第二个 `aleph-server` 撞单例 flock；legacy SSE dsh 不支持）· 协议版本协商，支持集 `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]`（不支持的版本回 `2025-11-25`；`src/mcp/modern/` 的 `2026-07-28` 无握手无 session，**不说**，`server/discover` 答 `-32601` 让 adapter 的 `auto` 探测退回 `initialize`）· 只宣告 `tools: { listChanged: true }` + 短 `instructions`，prompts / resources / sampling / elicitation / roots / tasks 不做 · 认证与护栏逐请求复用 `connect` + `/ws` 那三道（loopback 免凭据；远程 `Authorization: Bearer` 否则 401；明文远程 426；Origin 403；`local` 位来自 `trusted_proxy::resolve_client` 不是 `is_loopback()`；远程 `POST /mcp` 私有限流桶 `MCP_REMOTE_POSTS_PER_MINUTE = 120` → 429 + `Retry-After`）· `[mcp_server] enabled / expose`，默认集合＝按谓词挑出的无副作用工具减去逐名点名的 `DEFAULT_EXPOSE_EXCLUDES`（`tool_usage` / `config_audit` / `node_list` / `user_profile`；36 个名字，P6.1 逐名钉住），启动校验 `expose ⊆ 目录`（G5），**`expose` 改动即时生效**（`mcp_server.expose` ∈ `LIVE_SUBSECTIONS` + 面上的 `ArcSwap<BTreeSet<String>>` + 重跑 G5 + `list_changed`；`enabled` 仍 `Restart`）· 每 MCP session ↔ 一个 Aleph 会话，principal `McpClient { client_name }` · **审批**：`McpFace` 每次调用问 `OperatorPresence` 探针（连接表），`unattended = !present` → 无 operator 界面时 confirm 门立刻 fail-closed、`isError: true` + `MCP_APPROVAL_HINT`；有 operator 时卡按现有流升起、无死线，MCP 客户端等自己的超时，放弃后卡仍挂在 Panel（U-d 接受；「handler future 掉落时收回卡」是 follow-up）· lifecycle 迁移（`after_transition` 里 `try_mcp_face()` 那一行，G3 钉住）与 `expose` 变更 → `notifications/tools/list_changed`。**错误形状**：执行错误 → `isError`（不是协议错误）；不在 expose → tool not found；非 JSON-RPC body → 400；非 `initialize` 缺 session 头 → 400；未知 session → 404。
- **打磨话术**：「Aleph 有 MCP **client**（`src/mcp/`）也有 MCP **server 面**（`src/gateway/mcp_face/`），两者共用 wire 类型、不共用状态机。要加一个 MCP 方法：server 侧只在 `mcp_face/`，缺的类型给 `src/mcp/` 加，**不在 `mcp_face/` 里再定义一份**（CLAUDE.md 禁用清单）。`pi` 不在 Aleph 内跑（U3）：它经这一面挂上来，或经 `aleph-cli` 的 bash 契约——后者的整改是 §8 DECIDE 2。」
- **真机**：`qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`，每阶段证明什么见 `qa/README.md`。
```

- [ ] **Step 2 (c): Appendix D.0.196–199** — append after `**附录 D.0.195**` paragraph (exact text):

```markdown

**附录 D.0.196** · **派生法只盖住它列举过的面——「唯一作者」守卫证明的是调用不是覆盖** —— 2026-08-16 那轮把「注册是可逆 effect」的理念落成「一个函数从 registry 派生整套投影 + 每条改激活态的路径都调它」，配守卫 `publishing_plugin_projections_has_exactly_one_author`。守卫是真的，也一直是绿的，而它证明的命题是**「改激活态的路径都调了那一个函数」**，不是**「那一个函数盖住了插件写进运行时的每一个面」**。派生列举了三个面（skill dirs / sub-agents / tool index）；三个月里在派生之外漏了四处，且每一处两端完整、中间没线（§7）：`[memory]` 扩展只有 `register` 没有 `unregister`（disable 后仍挂着）· 插件 slash 只在 boot 的 `tool_catalog_init.rs` 注册一次（enable / disable / reload / watcher 全不重跑）· MCP 插件 disable 走 `unload_runtime_plugin`、enable 什么都不做，只有 `reload()` 才 `sync_mcp_plugin_servers`（§14 闸的两个方向）· `reload_plugin(id)` 是 `reload()` 的窄孪生（§16）。四个都不是「派生函数写错了」，是**派生函数从没听说过它们**——列举法（§5）。守卫没说谎，它只是回答了一个比我们以为的更窄的问题。分辨法是一句所有权规则而不是更长的清单：**它有逆操作吗？有 → 效果，注册处返回 `Disposer`，由持有者逆序执行；没有但能从 registry 重算 → 视图，才归派生。** 判问句：**这条守卫变红的条件里，包不包括「多了一个它不认识的面」？** 不包括，它守的就是产地不是覆盖（§4 的第二形态）。→ 附录 E.0 · E.3 · §3.10 2026-09-20 轮

**附录 D.0.197** · **一条裁定被收窄的时候，要写下哪半句被推翻、哪半句还站着——否则两个方向的读者各拿走一半** —— 「对照 Cordis 但架构不移植」在 2026-08-15 / 08-16 / 08-19 裁了三次，代码里有 `projection.rs:14-24` 那段话，文档里有 HARNESS_PHILOSOPHY §8 第五课。2026-09-20 采了它的一条机制（所有权规则：`register() -> Disposer`），于是那三份表述同时半对半错：「不引 fiber 运行时」仍然成立（没有 DI 容器、Proxy 上下文、级联重启、HMR、依赖声明与版本闸，`src/harness/` 仍 0 行），「等价保证由单函数派生给出」不再成立（D.0.196）。**不改那三份就是三份说谎的拷贝（§1）；只改成「现在采 Cordis 了」是把收窄写成推翻，下一个人会顺着它去搬 fiber。** 所以收窄的写法是三段：① 推翻的那半句逐字引出来 · ② 站着的那半句逐条点名 · ③ 一句为什么这次采的东西不违反站着的那半句（Rust 里 `Disposer` 是一个返回值类型，不是运行时）。→ 附录 E.0 · E.3 · HARNESS_PHILOSOPHY §8 第五课补注

**附录 D.0.198** · **一个「要砍掉的东西」的真实大小，是它专属代码的行数，不是 grep 它名字的命中数——两者差一个数量级时，砍法就不一样** —— `rg -i 'openclaw|clawhub|claw' src` 命中 286 行 / 156 文件，读起来像一次大手术。逐条分类之后：**只服务那个格式的代码 ≈ 50 行**（`markdown_skill/spec.rs:85-124` 两个从未被读过的 serde DTO + 一个字段），其余是 parity 注释（"mirrors X's default"）、把它当 **ACP agent 托管**的预设（`config/types/acp.rs:420-429`，那是 R3 的核心定位，必须留）、以及三个撞名的别项目（`ClawTeam` / `clawshell` / `claw-code`）。grep 把三个「claw」和四种关系（实现 / 对照 / 托管 / 撞名）折成了一个数字。**先分类再数**：① 哪些行是这个格式的**实现**（删）· ② 哪些是**对照注释**（改标签或不动——注释不是代码）· ③ 哪些是把它当**外部 agent 托管**（留，且要能说出为什么）· ④ 哪些根本是**别的东西**。分完之后 CUT 是一个小提交 + 一次文档改标签，而不是一轮重构；而**每一类留下的都要有一个能变红的守卫**（本轮：census 只看非注释行、只许两个文件命中，`the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset`）。反向的错也存在：`ARCHITECTURE.md:261` 列着一个从不存在的 `src/clawhub/`——名字的 grep 会把它数进「足迹」，而它是一行文档谎言（§1）。→ 附录 E.0 · E.9 · §3.10 2026-09-20 轮

**附录 D.0.199** · **「有没有人能答」是连接表上的事实，不是事件总线上的订阅数——一条靠 `Ok(0)` 拒绝的臂，在有内部订阅者的进程里永远不触发** —— `OperatorApprovalRequester` 的设计是「发一帧审批卡，`publish_frame` 返回 `Ok(0)` 个订阅者就自动拒绝」，读起来像一道 fail-closed 的闸：没人在线就不挂起。P6 第一次把它当 MCP 面的「无 operator 界面 ⇒ 拒绝」来用时去数了一遍订阅者：boot 路径给事件总线挂了 **17 个内部消费者**（转录、指标、通道 relay……），于是每一台真服务器上 `publish_frame` 都返回 `Ok(n>0)`，那条拒绝臂**从未在生产上执行过一次**——单测里它是绿的，因为单测的总线是空的（判据 §2 恒真谓词的「恒绿」脸 + §7 的「传感器量的不是它以为的量」）。订阅者计数回答的是「有没有人**听这个 topic**」，不是「有没有一个**能点按钮的人**」；后者只住在 `GatewayServer.connections` 里（`ConnectionState { caller_role, .. }`），所以 presence 必须在**连接表**上探（`operator_presence_probe()`），派发时以 `unattended = !present` 走既有的 fail-closed 臂。判问句：**这个「没人」的判据，量的是哪一张表？** 量的表里如果有不是人的行，它就不是 presence。→ 附录 E.0 · E.4 · §5.27
```

- [ ] **Step 2 (d): Appendix E trigger lines** — E.0 (append after the last E.0 bullet, currently `:5656`):

```markdown
- **派生法只盖住它列举过的面——「唯一作者」守卫证明的是调用不是覆盖** —— 三个面派生得对，第四个面派生函数从没听说过（memory extension 无 unregister、slash 只在 boot 注册、MCP enable 不重挂、`reload_plugin` 窄孪生）；问一条绿守卫：**它变红的条件里包不包括「多了一个它不认识的面」**。分辨法是所有权规则：有逆操作 → 效果返回 `Disposer`；能从 registry 重算 → 视图 → 附录 D.0.196
- **一条裁定被收窄时，写下哪半句被推翻、哪半句还站着** —— 「不引 fiber」站着，「单函数派生等价」倒了；三份表述（`projection.rs:14-24`、HARNESS_PHILOSOPHY 第五课、FL §3.10）同笔改，否则是三份说谎的拷贝，而只写「现在采 Cordis 了」会让下一个人去搬 fiber → 附录 D.0.197
- **要砍的东西的真实大小是专属代码行数，不是 grep 名字的命中数** —— 286 行命中 vs ≈50 行实现：先分四类（实现 / 对照注释 / 当外部 agent 托管 / 撞名的别项目）再数；留下的每一类要有能变红的守卫；反向：文档里一个从不存在的目录也会被 grep 数进足迹 → 附录 D.0.198
- **「有没有人能答」是连接表上的事实，不是事件总线上的订阅数** —— `OperatorApprovalRequester` 的 `Ok(0)`-订阅者拒绝臂在真服务器上永远不触发（boot 挂了 17 个内部消费者）；presence 在 `GatewayServer.connections` 上探，派发时 `unattended = !present`。问：**这个「没人」量的是哪张表，表里有没有不是人的行** → 附录 D.0.199
```

E.3 (append before `### 附录 E.4`):

```markdown
- **插件写进运行时的每个面都要有逆操作，派生只负责能重算的** —— `src/extension/` 的守卫 `publishing_plugin_projections_has_exactly_one_author` 证的是调用不是覆盖；新增一个把东西放进运行时的 `pub fn` 而不返回 `Disposer`，G1 census 红；新增一个视图而不在 `after_transition` 里重算，G2 往返红 → 附录 D.0.196 · D.0.197 · §3.10 2026-09-20 轮
```

E.9 (append before `### 附录 E.10`):

```markdown
- **MCP server 面只有一张，wire 类型只有一份** —— server 侧只在 `src/gateway/mcp_face/`，缺的类型给 `src/mcp/` 加；`expose ⊆ 工具目录` 启动校验（G5）；需审批且无 operator → `isError` 而不是挂起。砍「某个集成的足迹」前先分四类再数——grep 的 286 行里实现只有 ≈50 行 → §5.27 · 附录 D.0.198
```

E.4 (append before `### 附录 E.5`, at `:5816`):

```markdown
- **一条靠订阅者计数拒绝的审批臂，在有内部订阅者的进程里是恒绿的** —— `OperatorApprovalRequester` 的 `Ok(0)` 拒绝从未在生产上跑过（boot 挂 17 个内部消费者）；「有没有 operator」只能问 `GatewayServer.connections`（`caller_role`），MCP 面就是这么做的（`operator_presence_probe()` → `unattended`）。给审批门加新的"没人在线"判据前，先说出它量的是哪张表 → 附录 D.0.199 · §5.27
```

- [ ] **Step 3: Post-condition**

```bash
grep -n '^### 5\.27' docs/reference/FEATURE_LOCATOR.md                  # 1
grep -o '附录 D\.0\.19[6-9]\*\*' docs/reference/FEATURE_LOCATOR.md | sort -u | wc -l   # 4
rg -c '→ 附录 D\.0\.19[6-9]' docs/reference/FEATURE_LOCATOR.md          # ≥ 7 (4 E.0 + 1 E.3 + 1 E.4 + 1 E.9)
rg -n 'ClaudeCache|EffectScope|mcp_face' docs/reference/FEATURE_LOCATOR.md | wc -l   # ≥ 6
```

- [ ] **Step 4: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs(feature-locator): §3.10 plugin-scope round, §5.27 MCP face, appendix D.0.196-199 + E triggers

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P8.8: CLAUDE.md — disallow-list line, two routing rows, one QA pointer

**Files:**
- Modify: `CLAUDE.md:70` (append one bullet after the CDP line), `:118-119` (insert a row after `src/gateway/runtime/`), `:124` (replace the `src/mcp/ · src/hub/` row; insert a `src/extension/` row before it).
- The `src/tools/ src/builtin_tools/` row (`:112`) does NOT change: its pointers (TOOL_SYSTEM.md · SECURITY.md · FL §3.2–§3.14) do not name PLUGIN_SYSTEM.md; the plugin host gets its own row instead. The spec §7 line "`src/extension/` 行的判据指针更新" assumes a row that does not exist at `3ddc1f2e7` (判据 §1 — the spec described a row the table never had); this task creates it.
- No new criteria row in the 形状名索引 (all three D entries are instances of §5 / §7 / §14 / §16 / §1).

- [ ] **Step 1: Pre-condition** — `rg -n 'mcp_face|src/extension/' CLAUDE.md` → 0; `rg -n '第二个 CDP' CLAUDE.md` → `:70`.

- [ ] **Step 2: Edits (exact text)**

After `:70` (the CDP bullet) insert:

```markdown
- **第二个 MCP server 实现**（在 `src/gateway/` 之外再挂一个 `/mcp`、或在 `src/mcp/` 里造一套 server 侧类型）—— 唯一真源是 **`src/gateway/mcp_face/`**（一张接口脸：Streamable HTTP `/mcp`、`tools/*` + `list_changed`，把 `tools/call` 翻成同一条 scoped dispatch），**wire 类型来自 `src/mcp/{jsonrpc,protocol,types}.rs`，不复制**；stdio 传输刻意不做（宿主 spawn 第二个 `aleph-server` 会撞单例 flock）（2026-09-20；判据 §1）→ [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面
```

After `:118` (the `src/gateway/runtime/` row) insert:

```markdown
| `src/gateway/mcp_face/` | [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27 | E.4 E.9 | `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`（每阶段证明什么见 [`qa/README.md`](qa/README.md)） |
```

Before `:124` (the `src/mcp/ · src/hub/` row) insert:

```markdown
| `src/extension/` | [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) · [PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) · FL §3.10 §5.27 | E.0 E.3 E.9 | `qa/plugins/run.sh {scope,visibility,command,exit2,cc-cache}`（前六个阶段与每阶段证明什么见 [`qa/README.md`](qa/README.md)） |
```

`:124` → `| \`src/mcp/\` · \`src/hub/\` | FL §5.20 §5.24 · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) FL §5.21 | E.9 | \`qa/plugins/run.sh\`（Hub 走 \`browse\` / \`marketplaces\`；阶段清单见 [\`qa/README.md\`](qa/README.md)） |`

- [ ] **Step 3: Post-condition** — `rg -n 'mcp_face' CLAUDE.md | wc -l` → 2 lines (the disallow bullet; the routing row, which names it twice); `rg -n 'E.4 E.9' CLAUDE.md` → the new row (E.4 because of D.0.199's approval trigger); `rg -c 'src/extension/' CLAUDE.md` → 1; the 形状名索引 (`:78-97`) is byte-identical (`git diff CLAUDE.md | grep -E '^[-+]\| \*\*[0-9]+\*\*'` → empty).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "CLAUDE.md: MCP-server single-source line; routing rows for src/extension and src/gateway/mcp_face

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.9: qa/README.md — the new `plugins` stages and the new `mcp_face` fixture

**Files:**
- Modify: `qa/README.md:618-625` (the `./qa/plugins/run.sh` command block) and the per-fixture list under `## 每个装置在证明什么` (`:1558-1695`) — **there is no `plugins` entry in that list today** (verified: `sed -n '1558,1695p' qa/README.md | grep -c 'plugins'` → 0), and the command block lists only 3 of the 6 stages `run.sh` has (`manifest` / `scaffold` / `trust`; missing `browse` / `marketplaces` / `panel` — drift found while writing this task, fixed here).

**Consumes:** stage names from spec §6 (P7 writes the scripts; names are fixed by the spec): `qa/plugins/run.sh {scope,command,exit2,cc-cache,visibility}`, `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`.

- [ ] **Step 1: Pre-condition** — `rg -n 'mcp_face|cc-cache' qa/README.md` → 0; `rg -n 'qa/plugins/run.sh' qa/README.md` → `:618, :621, :623` only.

- [ ] **Step 2: Command block** — replace `:618-625` with:

```text
./qa/plugins/run.sh manifest     # Claude Code manifest + marketplace unions through a real
                                 # load_all; ${CLAUDE_PLUGIN_ROOT}; per-plugin config across
                                 # a server restart
./qa/plugins/run.sh scaffold     # `aleph plugin init --type <rt>` output really installs and
                                 # loads — the CLI and the server are two authors
./qa/plugins/run.sh trust        # owner trust: default posture, enforce, vouch, restart,
                                 # withdraw. Three restarts, because the policy is a LOAD gate
./qa/plugins/run.sh browse       # marketplace contents are listable, and a name found that
                                 # way actually installs
./qa/plugins/run.sh marketplaces # the registration surface: list / add / remove, and the
                                 # removable bit the Panel draws its button from
./qa/plugins/run.sh panel        # BOOTS AND WAITS: the same surface through the browser
./qa/plugins/run.sh scope        # an MCP-kind plugin enable → disable → enable: tools.catalog
                                 # changes EVERY time (the disable→enable arm had no producer
                                 # until 2026-09-20; a unit test cannot see a transient server)
./qa/plugins/run.sh command      # `/cmd args` from a plugin's commands/*.md: the BODY appears
                                 # in the request the mock provider logs — the only oracle for
                                 # "the model received it"
./qa/plugins/run.sh exit2        # a PreToolUse hook that does `echo reason >&2; exit 2` blocks
                                 # the tool call AND the reason reaches the model; exit 1 does
                                 # not block (the asymmetry is the assertion)
./qa/plugins/run.sh cc-cache     # a fixture ~/.claude/plugins/installed_plugins.json is
                                 # discovered (origin=claude-cache), is DISABLED by default,
                                 # and its tools appear only after one enable verb — including
                                 # the server a CC plugin.json declares via mcpServers with no
                                 # aleph.runtime key (P4.15: inferred Mcp, shows in tools.catalog);
                                 # nothing under the fixture ~/.claude is written (mtime check)
./qa/plugins/run.sh visibility   # a <project>/.claude plugin is invisible to a session with no
                                 # project root and visible to a session in that project —
                                 # both arms, or the predicate could be constant

./qa/mcp_face/run.sh handshake   # `initialize` negotiates each of 2025-11-25 / 2025-06-18 /
                                 # 2025-03-26 to itself; an unsupported one (2024-11-05, the
                                 # sessionless 2026-07-28) is answered with 2025-11-25, not an
                                 # error; capabilities == tools.listChanged; a session id is
                                 # minted; string ids come back as strings
./qa/mcp_face/run.sh tools       # tools/list ⊆ [mcp_server].expose; tools/call on a read-only
                                 # tool returns a text result; a tool NOT in expose is
                                 # "not found" even though it exists in the catalog
./qa/mcp_face/run.sh auth        # a non-loopback request without a bearer → 401; with the
                                 # gateway's device token → 200 (same credential as connect)
./qa/mcp_face/run.sh list_changed # toggle a plugin over JSON-RPC while an SSE client is
                                 # attached: the client receives notifications/tools/list_changed
                                 # (the lifecycle → after_transition → face wire, end to end)
./qa/mcp_face/run.sh deny        # a tool that needs human approval, with no operator UI
                                 # attached → result isError:true whose text names where to
                                 # approve (fail-closed, not fail-dead); the call never hangs
```

- [ ] **Step 3: Per-fixture list** — append after the `terminal` entry (`:1600-1619` region) two entries:

```markdown
- **`plugins`** — 改 `src/extension/`（lifecycle / effects / visibility / hooks / discovery）前跑。十一阶段分两代：
  前六个（`manifest` `scaffold` `trust` `browse` `marketplaces` `panel`）是 2026-08-19/20 两轮的，证的是 manifest
  联合、脚手架、owner trust、marketplace 三张脸；后五个是 2026-09-20 作用域化轮的：
  - `scope` — **本轮存在的理由**。enable→disable→enable 一个 MCP-kind 插件，`tools.catalog` **每次都变**。此前
    disable 走 `unload_runtime_plugin` 而 enable 什么都不做，只有 `reload()` 才重挂 server——16k 单测全绿，因为没有
    一条单测持有一个真的 transient server（判据 §14 闸的两个方向）。
  - `command` — `/cmd args` 之后正文出现在 mock provider 记录的请求里。**那是「模型收到了」唯一的 oracle**；
    `commands.list` 里有条目证明的是广告，不是到达（判据 §4）。
  - `exit2` — `echo reason >&2; exit 2` 的 PreToolUse hook 阻断工具调用**且原因回到模型**；`exit 1` 不阻断。两臂都要：
    只有阻断臂时，一个把所有非零都当 block 的实现也绿。
  - `cc-cache` — 夹具 `~/.claude/plugins/installed_plugins.json` 被发现、origin 是 `claude-cache`、**默认 disabled**、
    一个启用动词之后工具可见；夹具 `~/.claude` 树的 mtime 不变（永不写）。
  - `visibility` — 项目插件对无 project 的会话不可见、对该项目的会话可见。**两臂**——少一臂，一个恒真或恒假的谓词都绿。
- **`mcp_face`** — 改 `src/gateway/mcp_face/` 或 `[mcp_server]` 配置前跑 `{handshake,tools,auth,list_changed,deny}`。
  `handshake` 的三个版本各自协商到自己、不支持的回 `2025-11-25`（脚本里那份 `SUPPORTED` 与 `mcp_face/protocol.rs` 的常量是两份——
  P6.5 的单测钉住常量，这里钉住 wire，两份漂开时先红的是这里）；`tools` 同时断言「在目录里但不在 expose 里」
  的工具是 not found，否则白名单可能是恒真的；`auth` 用与 `connect` 相同的 device token，证明是**一层**信任模型不是第二套；
  `list_changed` 是 lifecycle → `after_transition` → face 那条线的端到端证据（单测只能证两端）；`deny` 断言
  `isError:true` **且调用返回了**——挂起和拒绝在客户端上都是「没结果」，只有超时能分开它们，所以脚本给这一步一个上限。
```

- [ ] **Step 4: Post-condition** — `rg -n '^\./qa/plugins/run\.sh' qa/README.md | wc -l` → 11; `rg -n '^\./qa/mcp_face/run\.sh' qa/README.md | wc -l` → 5; `sed -n '/每个装置在证明什么/,$p' qa/README.md | grep -c -E '^- \*\*`(plugins|mcp_face)`\*\*'` → 2.

- [ ] **Step 5: Commit**

```bash
git add qa/README.md
git commit -m "qa/README: plugins stages scope/command/exit2/cc-cache/visibility and the mcp_face fixture — what each proves

Also lists browse/marketplaces/panel in the command block; they existed
in run.sh and not here.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P8.10: One archive at `docs/archive/` — the 2026-03-18 ClawHub spec + plan, and the voice-sidecar file from `docs/reference/archive/`

> **Reconciled (R5.2 Q7):** ONE archive location, the one CLAUDE.md names. `docs/reference/archive/` (one file, `SELF_BUILT_VOICE_SIDECAR.md`, commit `af2fe5a5e`) moves too and the directory is deleted.

**Files:**
- Move: `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md` → `docs/archive/2026-03-18-clawhub-integration-design.md`
- Move: `docs/superpowers/plans/2026-03-18-clawhub-integration.md` → `docs/archive/2026-03-18-clawhub-integration-plan.md`
- Move: `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` → `docs/archive/SELF_BUILT_VOICE_SIDECAR.md`; then `docs/reference/archive/` is empty and disappears.

**Naming convention (recorded, since the directory is new):** `docs/archive/` is the Tier-3 location CLAUDE.md names (`Tier 3（默认忽略）＝ docs/archive/、历史规格`) and **does not exist at `3ddc1f2e7`** (`ls docs/archive` → No such file). Files keep their basenames; the ClawHub plan gets a `-plan` suffix so the pair is distinguishable side by side. Inbound links at `3ddc1f2e7`: `rg -n '2026-03-18-clawhub-integration' .` outside the two files and the 2026-09-20 spec/evidence → 0; `rg -n 'SELF_BUILT_VOICE_SIDECAR|reference/archive' .` outside the file itself → 0 (verified). Nothing breaks.

- [ ] **Step 1: Pre-condition** — `ls docs/archive` → No such file; `git ls-files docs/superpowers/specs/2026-03-18-clawhub-integration-design.md docs/superpowers/plans/2026-03-18-clawhub-integration.md docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` → all three listed; re-run the two inbound-link greps above (expected 0 — if a link appeared since `3ddc1f2e7`, fix it in this commit).

- [ ] **Step 2: Move**

```bash
mkdir -p docs/archive
git mv docs/superpowers/specs/2026-03-18-clawhub-integration-design.md docs/archive/2026-03-18-clawhub-integration-design.md
git mv docs/superpowers/plans/2026-03-18-clawhub-integration.md docs/archive/2026-03-18-clawhub-integration-plan.md
git mv docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md docs/archive/SELF_BUILT_VOICE_SIDECAR.md
rmdir docs/reference/archive
```

Prepend to each of the two ClawHub files (first line, above the existing title):

```markdown
> **ARCHIVED 2026-09-20** — historical. The ClawHub integration this describes was never built as described (`src/clawhub/` never existed; the SKILL.md `metadata.openclaw.*` DTOs it specified were parsed, never read, and CUT 2026-09-20). Kept for provenance only. Do not implement from it.

```

Prepend to `SELF_BUILT_VOICE_SIDECAR.md` (first line):

```markdown
> **ARCHIVED** — moved from `docs/reference/archive/` on 2026-09-20 so the repo has one archive (`docs/archive/`, CLAUDE.md Tier 3). Content unchanged: a preserved design, kept for future revival (commit `af2fe5a5e`).

```

- [ ] **Step 3: Post-condition** — `git status --short docs/` shows three `R` lines; `rg -n 'ARCHIVED' docs/archive | wc -l` → 3; `ls docs/superpowers/specs docs/superpowers/plans | grep -c clawhub` → 0; `ls docs/reference/archive` → No such file; `rg -n 'reference/archive' . --glob '!node_modules'` → 0.

- [ ] **Step 4: Commit**

```bash
git add docs/archive docs/superpowers/specs docs/superpowers/plans docs/reference/archive
git commit -m "docs: one archive at docs/archive/ — clawhub spec+plan and the voice-sidecar design (Tier 3)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Contract deltas

- None in P5/P8 code (these phases consume contract names only in prose). Applied global rulings: G-1 (`PluginId = String`), G-2 (status names `Loaded / Disabled / Blocked / Error / Pending`; no `Active` / `Failed`), G-3 (`Disposer` → `BoxFuture<'static, DisposeOutcome>`), G-4 (`try_mcp_face() -> Option<&'static Arc<McpFace>>`), G-5 (`scopes: Mutex<HashMap<String, EffectScope>>`, `load_guard`, `after_transition` once per primitive), G-6 (no cherry-picks; the "**P<n> writer**" matrix rows are absorbed by R1.2 / R3.4 / R4.4 / R4.6 / R6.8), G-7 (U-a … U-d), G-8 (`auto`).
- Two facts the contract/spec stated that the code contradicts (kept as the record):
  - Spec §7: "`src/extension/` 行的判据指针更新" — CLAUDE.md's routing table has **no** `src/extension/` row at `3ddc1f2e7`; P8.8 creates one (R5.5 text).
  - Spec §3.9 / evidence row 19: `command.execute` "permanent error stub, never overridden" — it IS overridden at `tool_catalog_init.rs:475-483`; P5.4 cuts it on the zero-client ground (R5.2 Q1 confirmed).
- MCP versions: the face speaks `["2025-11-25", "2025-06-18", "2025-03-26"]` (P6.5); `2026-07-28` (`src/mcp/modern/`) is handshake-less/sessionless and is not spoken (R6.2). The contract's "newest version implemented by `src/mcp/protocol.rs`" resolves to `2025-11-25`, the newest **handshake** revision the client stack names.

## Open questions for the lead

All nine from the first draft are ruled (R5.2, G-6, G-7): Q1 CUT stands (P5.4) · Q2 all eleven `mcp.*` CUT (P5.5) · Q3 `load_runtime_plugin` → P1.11 (verified present in plan-P1 P1.11's table: "`plugin_ops.rs:187-220 load_runtime_plugin` → `mount`") · Q4 → P4.7d · Q5 CUT in P5.4 · Q6 `AggregateTools` in P5.6 · Q7 one archive (P8.10) · Q8 no cherry-picks · Q9 delete `media-video`.

Remaining, new:

1. **P1.10 / P1.11 ordering vs. P5.3's census** — P5.3 now only proves the class is gone. If P1.10 lands but P1.11 does not (P1.11 is the "delete the bodies" task), `load_runtime_plugin` survives and P5.3's second grep is non-empty; the task says to fix the one line — but a surviving `pub fn` is P1's to delete, not P5's. If that happens, P5.3 reports it instead of deleting (it is listed in plan-P1 P1.11's zero-reference grep, so P1.11's own Step 4 would already be red).
2. **`aggregate_from_healthy` after P5.6** — kept (one user: `aggregate_instructions`). Nothing to decide unless P6 removes `McpInstructionsLayer`.

## Coverage map

| Spec item | Task |
|---|---|
| §1.2 OpenClaw ≈50 lines; §3.9 row 1 (code) | P5.1 |
| §3.9 row 2 (docs: ARCHITECTURE `:261`, ALEPH_HUB §7, PLUGIN_SYSTEM / SKILL_MODEL_TAXONOMY parity labels, archive) | P8.1, P8.2, P8.3(f)(g)(h), P5.1 (taxonomy `:14,:65,:98-100`), P8.10 (one archive, incl. the voice-sidecar file — R5.2 Q7) |
| §3.9 row 3 (OpenClaw keep list) | P5.1 allow-list + census; no task deletes them |
| §3.9 row 4 零客户端 RPC — `plugin.*` singular + `:364` comment | P5.2 |
| §3.9 row 4 — `plugins.{load,unload,executeCommand}` | **P1.10** (`load`/`unload`) + **P4.7d** (`executeCommand` + WASM command chain); P5.3 is the post-condition census (R5.1) |
| §3.9 row 4 — `command.execute` + `handle_execute` (+ `ToolCatalog::{is_namespace,list_namespace_children}`, R5.2 Q5) | P5.4 |
| §3.9 row 4 — `mcp.*` eleven | P5.5 (all eleven deleted, U-a) + P5.6 cascade (three `Aggregate*` words) |
| §3.9 row 5 幻影 Node 运行时 (EXTENSION_SYSTEM `:143-218`, `packages/plugin-sdk/`, `media-video`) | P5.7 |
| §3.9 row 6 兄弟仓 follow-up | P5.7 (doc line with `path:line`) |
| §3.9 row 7 `Overridden` / `reload_plugin` | referenced only (P3 / P1); doc side P8.3(a), P8.3(i) |
| §3.9 row 8 说谎的注释 (`projection.rs:14-24`, HARNESS_PHILOSOPHY 第五课) | `projection.rs` comment → P1 (code file; matrix row below); 第五课 → P8.6 |
| §8 DECIDE 1 `AlephSkillSpec` deadline | P5.8 |
| §7 FEATURE_LOCATOR (§3.10 entry, §5.x MCP face, D, E.3/E.9) | P8.7 (§5.27; D.0.196–199 incl. R5.4's operator-presence entry; E.0/E.3/E.4/E.9 triggers) |
| §7 CLAUDE.md (disallow line, routing rows) | P8.8 |
| §7 PLUGIN_SYSTEM.md | P8.3 (+ P5.2 / P5.7 same-commit edits; `:481-483` carried by P1.10, R1.2) |
| §7 EXTENSION_SYSTEM.md | P8.4 (+ P5.7 same-commit edits; `:613-709` Direct Commands carried by P4.7d, R4.4; pi-aleph pointer by P6.8, R6.8) |
| §7 GATEWAY.md | P8.5 |
| §7 ALEPH_HUB / ARCHITECTURE / SKILL_MODEL_TAXONOMY / HARNESS_PHILOSOPHY | P8.2 / P8.1 / P5.1+P5.8 / P8.6 |
| §7 `qa/README.md` | P8.9 |
| §10 DEVIATION list written into docs | P8.3(d) (R5.3 list: timeout 300 s · skills CC-only fields (P4.12 sentence) · agent `permissionMode` · `settings.json` not read · `~/.aleph/hooks.json`; `hook_event_name` listed under CONNECT) |

## doc-code同笔 matrix (for the lead to enforce 判据 §1)

Each row: a code task in P1–P6 that changes a documented fact → the doc file:anchor that becomes false → which task carries the fix. "same commit" rows are already inside a P5 task; the "**P<n> writer**" rows were absorbed by the reconciliation (R1.2, R3.4, R4.4, R4.6, R6.8 — noted per row). All phases stay in the worktree until P9 (G-6), so P8's later commits are not a gap on `main`.

| Code task (phase) | Documented fact that changes | Doc anchor | Carried by |
|---|---|---|---|
| P1 `effects/` + `lifecycle.rs` (mount/unmount/reload_plugin/reload, `after_transition`) | "one function derives … every path calls it" as the whole guarantee; `reload()` calls `sync_mcp_plugin_servers`; `unload_runtime_plugin` captures server ids | `PLUGIN_SYSTEM.md:570-578` MCP Runtime Wiring; `:790-820` 投影单一咽喉; HARNESS_PHILOSOPHY `:350` 第五课; `src/extension/projection.rs:14-24` (a code comment — **P1 must rewrite it in the same commit**; text to use = P8.3(j) paragraph, English) | P8.3(e)(i)(j), P8.6, P8.7(a)(c D.0.196/197) |
| P1 `MemoryExtensionRegistry::unregister`, ToolCatalog `unregister_skills(&[String])` | (undocumented gaps become documented effects) | new section | P8.3(i), P8.4 |
| P1 deletes narrow `reload_plugin` (`mod.rs:1303-1341`) | `plugin.reload` semantics | `PLUGIN_SYSTEM.md:460-480` table row `plugin.reload`; EXTENSION_SYSTEM `:403-413` (`plugins.reload` never existed) | P8.4 (table), P8.3(i) |
| P2 `visibility.rs` (`ScopeKey`, `visible_to`, five faces, no-project ⇒ Global only) | "Scope 管理" describes discovery paths only; hooks-only `project_scope_allows` | `PLUGIN_SYSTEM.md:334-350`; FL §5.10 hooks — one sentence pointing at `visibility.rs` (**absorbed by P2.2, R3.4**) | P8.3(c), P8.4, P8.7(a ②) |
| P3 `PluginStatus::Pending`, `Overridden` removed, `activation_gate`, doctor `extension/plugins-activated` | status table + "现在三者都有 registry 行" | `PLUGIN_SYSTEM.md:149-171` (P3.1 makes the minimal truth fix in its own commit, R3.4; P8.3(a) reformats against that text); FL §5.9 Doctor line (**absorbed by P3.4, R3.4**) | P8.3(a), P8.7(a ③) |
| P4 #1/#2 exit-2 + `updatedInput`; #4 `hook_event_name` spelling (U-b); #5 `CC_TOOL_ALIASES`; #6 300 s | hooks contract | `PLUGIN_SYSTEM.md:353-383` DEVIATION / CONNECT lists; FL §5.10 exit-2 sentence (**absorbed by P4.1, R4.6**) | P8.3(d) |
| P4 #7 commands body via `SkillTemplate` (+ P4.7d cut of `plugins.executeCommand` + WASM command chain) | `commands/*.md` "注册到 dispatch registry" only; Direct Commands section | `PLUGIN_SYSTEM.md:372`; `EXTENSION_SYSTEM.md:613-709` (**carried by P4.7d, R4.4** — text in P5.3 Step 2) | P8.3(d) row |
| P4 #8 `AgentDef.system_prompt` | `agents/*.md` "自动发现" (body dropped); FL §3.10 "刻意仍不做 plugin agent content" | `PLUGIN_SYSTEM.md:371`; FL §3.10 old bullet (the round entry states the reversal — the old bullet stays as history) | P8.3(d) row, P8.7(a) |
| P4 #10 `PluginOrigin::ClaudeCache` | origin set is Config/Workspace/Global/Bundled | `PLUGIN_SYSTEM.md:173-179` (+ new paragraph), `:334-350` table | P8.3(b)(c), P8.4 |
| P4 #9 marketplace `source.source` | marketplace 判别键 | `PLUGIN_SYSTEM.md:499` "source 分类只有一个答案" — one sentence (**absorbed by P4.9, R4.6**) | — |
| P5.1 DTO cut | `OpenClawMetadata` named in taxonomy | `SKILL_MODEL_TAXONOMY.md:14,65,98-100` | **P5.1 same commit** |
| P5.2 `plugin.*` singular cut | RPC table, `plugin.config.get` row | `PLUGIN_SYSTEM.md:464-479, :662` | **P5.2 same commit** |
| P1.10 `plugins.{load,unload}` cut | namespace inequality paragraph | `PLUGIN_SYSTEM.md:481-483` (**carried by P1.10, R1.2** — text in P5.3 Step 2) | — (P5.3 census verifies) |
| P5.4 `command.execute` + namespace queries cut | §3.5 打磨话术 | `FEATURE_LOCATOR.md:1017` | **P5.4 same commit** |
| P5.5/P5.6 eleven `mcp.*` cut + three `Aggregate*` words | none documented for the eleven (PLUGIN_SYSTEM `:577` names `mcp.list`, which stays; `handlers/mcp.rs:1-7` module doc is a code comment, edited in P5.5) | — | — |
| P5.7 Node runtime cut | 14 Node.js mentions | `EXTENSION_SYSTEM.md:3,11,33-43,58,76,143-218,302,391-394,438-439,607`; `PLUGIN_SYSTEM.md:619-620` | **P5.7 same commit** |
| P5.8 deadline | Phase 2 date | `SKILL_MODEL_TAXONOMY.md:92,108-115` + `spec.rs:11-19` + `mod.rs:6-9` | **P5.8 same commit** |
| P6 `src/gateway/mcp_face/` (`/mcp`, `[mcp_server]` + live `expose` (P6.9), `try_mcp_face`, `notify_tools_list_changed`, operator-presence probe) | Gateway HTTP routes list; disallow list; routing table; FL §5; the approval-card shape (U-d) | `GATEWAY.md:1150-1168` HTTP Server bullet (**absorbed by P6.7, R6.8**) — P8.5 adds the section; CLAUDE.md `:70`, `:118`; FL §5.27 + D.0.199 + E.4 | P8.5, P8.8, P8.7(b)(c)(d) |
| P6 `packages/pi-aleph/` | `packages/` re-created after P5.7 emptied it; the client-side how-to lives in its README | EXTENSION_SYSTEM "Node plugins run as MCP stdio servers" (P5.7) — the pi-aleph pointer sentence (**absorbed by P6.8, R6.8**); GATEWAY.md MCP 面 first paragraph — the pointer is **carried by P8.5** (lead addendum: the section is P8.5's) | P8.5 |
| P7 `qa/plugins/run.sh` +5 stages, `qa/mcp_face/run.sh` | qa/README command block + per-fixture list | `qa/README.md:618-625`, `:1558-1695` | P8.9 |
| P7 QA needs fake API key | already documented (`qa/README.md`, memory) | — | — |

Every row that this file does not carry names the reconciliation ruling that assigned it (R1.2, R3.4, R4.4, R4.6, R6.8); none is unowned.

