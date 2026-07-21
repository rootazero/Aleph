# Module: src/domain

- Path: `src/domain/`
- Files scanned: 2
- Total LOC: 1103
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 1 |
| medium   | 8 |
| low      | 8 |
| **Total**| **17** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/domain/skill.rs:23-25|medium|SkillId::new accepts any string (incl. empty, ":foo", "a:b:c"); no constructor validation, is_well_formed is dead.
ISSUE|src/domain/skill.rs:101-103|medium|PluginId::new accepts any string incl. empty; Plugin("") collides on equality and silently misroutes plugin provenance.
ISSUE|src/domain/skill.rs:183-191|medium|Os serde aliases ("macos","win") are case-sensitive but docstring (lines 177-182) promises parity with Os::from_str which is case-insensitive via eq_ignore_ascii_case; "Win"/"MACOS" JSON inputs fail to deserialize despite passing from_str.
ISSUE|src/domain/skill.rs:349,518|medium|homepage: Option<String> and InstallSpec::url: Option<String> accept arbitrary strings with no scheme/format validation; downstream consumer may issue network requests, exposing SSRF if consumer lacks its own validation.
ISSUE|src/domain/skill.rs:264-273|low|EligibilitySpec required_bins/required_env/required_config/any_bins are Vec<String> with no empty-string or whitespace rejection; empty env name in required_env silently passes if consumer treats empty as "set".
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/domain/skill.rs:381-402,420|high|DispatchSpec and ArgMode are pub but have zero production consumers (verified via repo-wide grep); InvocationPolicy::command_dispatch is set to None everywhere and never read — dead code that suggests an unfinished dispatch feature.
ISSUE|src/domain/skill.rs:38-51|medium|SkillId::plugin_prefix returns Some("") for ":foo" and SkillId::skill_name returns "" for "foo:"; callers cannot distinguish "no prefix" from "empty prefix" via the Option, contradicting the spirit of is_well_formed.
ISSUE|src/domain/skill.rs:159-166|medium|SkillSource::Plugin(_).priority() = 3 but no documented tiebreaker when two SkillSource::Plugin values share a skill id; resolution relies on ordering that isn't a property of SkillSource itself.
```

### Perspective 3 — Architecture Compliance
```
ISSUE|src/domain/skill.rs:159-166|medium|SkillSource::priority() encodes override-resolution policy into the domain value object; per R10, harness should resolve policy from data, not the domain type carrying it.
ISSUE|src/domain/skill.rs:661-663|medium|SkillManifest::is_model_visible() encodes the model-visibility policy ("not Disabled AND not disable_model_invocation") inside the aggregate root; per R10 this is harness-level logic that should live in the prompt/loader.
```

### Perspective 4 — Code Quality
```
ISSUE|src/domain/skill.rs:1-1045|low|src/domain/skill.rs is 1045 lines, exceeds 500-LOC guideline; consider splitting into id.rs, eligibility.rs, install.rs, manifest.rs submodules.
ISSUE|src/domain/skill.rs:38-64|low|SkillId::plugin_prefix, skill_name, is_well_formed all re-parse the same string independently; share a single helper.
ISSUE|src/domain/skill.rs:316-330|low|InstallKind::as_str duplicates the serde rename_all = "lowercase" strings; three places to update on a new variant.
ISSUE|src/domain/skill.rs:59-70|low|SkillId::is_well_formed and SkillId::is_empty are pub but only used in tests; either document as API surface or drop pub.
ISSUE|src/domain/skill.rs:112-115|low|PluginId::is_empty is pub but only used in tests; no production caller.
ISSUE|src/domain/skill.rs:199-205|low|ParseOsError::input() accessor and the struct itself have no external caller (only constructed by Os::from_str); standard thiserror would be more idiomatic than hand-rolled Display + Error impls.
ISSUE|src/domain/skill.rs:23-25,79-89|low|SkillId has both SkillId::new(impl Into<String>) and From<&str>/From<String>; three construction paths for the same operation.
```