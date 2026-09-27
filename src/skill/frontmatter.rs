//! The one splitter — and the one `allowed-tools:` spelling — for SKILL.md.
//!
//! Three ingestion paths read a file called `SKILL.md`:
//!
//! | path | root | produces |
//! |---|---|---|
//! | [`crate::skill::manifest`] | `~/.aleph/skills`, `~/.claude/skills`, project dirs, **and every active plugin's `<root>/skills`** | `SkillManifest` |
//! | [`crate::extension::manifest::parsers`] | `{plugin_dir}/skills/*/SKILL.md`, `{plugin_dir}/commands/*.md` | `SkillRegistration` |
//! | [`crate::tools::markdown_skill::parser`] | whatever `skills.install` names | `AlephSkillSpec` |
//!
//! Their *destinations* are three different concepts and are deliberately NOT
//! merged (see the module docs on each). What they were doing three times, and
//! getting differently wrong three times, is the mechanical part: deciding
//! where the `---` fence ends.
//!
//! * `extension::manifest::parsers` cut at the first `\n---` **substring**, so
//!   a `---` inside a YAML block scalar (or the first horizontal rule in the
//!   body) truncated the frontmatter.
//! * `tools::markdown_skill::parser` cut at the first `\n---\n`, same class.
//! * `skill::manifest` iterated *lines* and required the delimiter to be alone
//!   on its line. That one is correct, so it is the one that survives here.
//!
//! [`split`] is that implementation, extracted so there is one answer. The
//! YAML call after it is shared the same way: [`parse_frontmatter_yaml`] is
//! what `skill::manifest` and `extension::manifest::parsers` both parse the
//! block with, so neither drops a file over YAML syntax the other retries.
//! (Their field types still differ: a typed field one of them has can still
//! cost that one the file.)
//!
//! # `allowed-tools:`
//!
//! [`read_allowed_tools`] is the single reader of the [`ALLOWED_TOOLS_KEY`]
//! frontmatter block's shapes, and [`normalize_allowed_tools`] is the skill
//! policy on top of it. They live here rather than in `manifest.rs` so the
//! key's spelling and the lenient shape-handling that spelling requires cannot
//! drift apart, and so a fourth ingestion path finds them together instead of
//! writing a fourth `Option<Vec<String>>`.
//!
//! The census in this module's tests is the guard that keeps that true.

/// The frontmatter key naming a skill's declared tool scope.
///
/// One spelling, one place. `manifest::RawFrontmatter` renames its field to
/// exactly this string; the census test below fails by name if a second file
/// grows its own copy.
pub const ALLOWED_TOOLS_KEY: &str = "allowed-tools";

/// The only way [`split`] can fail: there is no `---` fenced block to split.
///
/// Deliberately not an error *enum*: a caller that wants "no frontmatter is
/// fine" (the plugin manifest parsers) maps this to its default, and a caller
/// that wants it fatal (the skill manifest, the markdown-CLI spec) maps it to
/// its own error type. A richer type here would have to be translated by both
/// anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoFrontmatter;

impl std::fmt::Display for NoFrontmatter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no YAML frontmatter found")
    }
}

impl std::error::Error for NoFrontmatter {}

/// Split markdown content into (`yaml_frontmatter`, body).
///
/// Expects the content to start with `---\n` and contain a closing `---` **on
/// a line of its own**; a `---` embedded in a longer line (`--- not a fence`,
/// `foo: ---`) is not a terminator. CRLF is normalised to LF in both halves.
/// The body is returned untrimmed — callers that want it trimmed do that
/// themselves, because they disagree about whether a leading blank line is
/// content.
///
/// # The one boundary this does not cross
///
/// Leading whitespace on the fence line is tolerated (`line.trim() == "---"`),
/// so a line inside a YAML block scalar whose entire content is `---` — an
/// indented markdown horizontal rule in a `description: |` block, say — still
/// reads as the closing fence and truncates the frontmatter.
///
/// That is inherited `skill::manifest` behaviour and it is kept deliberately.
/// Tightening the rule to "column 0 only" would make every SKILL.md that
/// indents its closing fence stop parsing, i.e. it would silently delete those
/// skills — the exact failure class this module exists to remove. Trading a
/// rare truncation for a new way to lose a whole skill is not an improvement,
/// so the limitation is stated (and pinned by a test) rather than fixed by
/// guess. Changing it is a decision about `skill::manifest`'s semantics and
/// belongs in its own change.
///
/// # Errors
///
/// [`NoFrontmatter`] when the content does not open with `---`, or when no
/// closing delimiter line is found.
pub fn split(content: &str) -> Result<(String, String), NoFrontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(NoFrontmatter);
    }

    // Find the end of the opening `---` line
    let after_opening = match trimmed[3..].find('\n') {
        Some(pos) => 3 + pos + 1,
        None => return Err(NoFrontmatter),
    };

    // Find the closing `---` that appears on its own line (allowing \r for CRLF).
    // We iterate lines so that a `---` inside a YAML value does not falsely
    // terminate the frontmatter.
    let rest = &trimmed[after_opening..];
    let closing_pos = rest
        .lines()
        .enumerate()
        .skip(1) // first line is part of the YAML, not a delimiter
        .find(|(_, line)| line.trim() == "---")
        .map(|(idx, _)| rest.split_inclusive('\n').take(idx).map(str::len).sum())
        .or_else(|| {
            // Handle case where --- is at very start of rest (empty frontmatter)
            if rest.starts_with("---") {
                Some(0)
            } else {
                None
            }
        })
        .ok_or(NoFrontmatter)?;

    let yaml_str = &rest[..closing_pos];
    // The closing line may carry leading whitespace (matched via `line.trim()`),
    // so skip to the end of the whole delimiter line rather than a fixed `+3`.
    let closing_line = &rest[closing_pos..];
    let body = match closing_line.find('\n') {
        Some(nl) => &closing_line[nl + 1..],
        None => "", // closing `---` is the final line; no body follows
    };

    let yaml_normalized = yaml_str.replace("\r\n", "\n").replace('\r', "\n");
    let body_normalized = body.replace("\r\n", "\n").replace('\r', "\n");

    Ok((yaml_normalized, body_normalized))
}

/// Deserialise a frontmatter block: the one YAML call of both readers of a
/// SKILL.md / command file (`extension::manifest::parsers` and
/// `skill::manifest`), so they cannot answer the same file two ways.
///
/// One narrow retry. Claude Code's own authoring guidance gives
/// `argument-hint: [arg1] [arg2] [optional-arg]` as the format, which is not
/// YAML (a flow sequence followed by more text), and a YAML error drops the
/// whole file. So on an error — and only then — a bare bracketed
/// `argument-hint:` line is quoted as the literal text it is
/// ([`quote_bare_argument_hint`]) and the same parser runs once more; nothing
/// else in the block is touched. `origin` names the file in the warn that
/// says so. If the retry fails too, the error returned is the ORIGINAL one:
/// it points at what the author wrote, not at the rewrite.
pub(crate) fn parse_frontmatter_yaml<T: serde::de::DeserializeOwned>(
    yaml: &str,
    origin: &dyn std::fmt::Display,
) -> Result<T, crate::yaml::Error> {
    let original = match crate::yaml::from_str(yaml) {
        Ok(parsed) => return Ok(parsed),
        Err(e) => e,
    };
    let Some(quoted) = quote_bare_argument_hint(yaml) else {
        return Err(original);
    };
    match crate::yaml::from_str(&quoted) {
        Ok(parsed) => {
            tracing::warn!(
                origin = %origin,
                "`argument-hint:` is not valid YAML; read as literal text"
            );
            Ok(parsed)
        }
        Err(_) => Err(original),
    }
}

/// `yaml` with each bare bracketed `argument-hint:` line's value quoted as a
/// YAML double-quoted string, or `None` if no line qualifies. A line
/// qualifies only if it starts at column 0 with the key, its value is on that
/// one line and starts with `[` (so never `"`, `'`, `|`, `>` or empty), and
/// no indented line continues it. Everything after the key is taken as
/// written, a trailing `# comment` included.
fn quote_bare_argument_hint(yaml: &str) -> Option<String> {
    const KEY: &str = "argument-hint:";
    let lines: Vec<&str> = yaml.split_inclusive('\n').collect();
    let rewritten: String = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let Some(rest) = line.strip_prefix(KEY) else {
                return (*line).to_string();
            };
            let value = rest.trim_end_matches(['\n', '\r']);
            let eol = rest.get(value.len()..).unwrap_or_default();
            let value = value.trim();
            let continued = lines
                .get(i + 1)
                .is_some_and(|next| next.starts_with([' ', '\t']) && !next.trim().is_empty());
            if !value.starts_with('[') || continued {
                return (*line).to_string();
            }
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{KEY} \"{escaped}\"{eol}")
        })
        .collect();
    (rewritten != yaml).then_some(rewritten)
}

/// Normalise the [`ALLOWED_TOOLS_KEY`] frontmatter block into a name list.
///
/// Two shapes exist in the wild and both must work. Aleph's own convention is
/// a YAML sequence, but every skill authored for upstream Claude Code writes a
/// single comma-separated scalar (`allowed-tools: Read, Grep, Bash(cargo *)`).
/// Deserialising into a strict `Vec<String>` would make the YAML parser
/// reject the frontmatter outright, and that does **not** degrade to "the declaration was
/// ignored": it fails the whole parse, and the directory scan then drops the
/// SKILL.md. A skill would vanish because of a key it does not even need. So
/// the field is taken as raw YAML — the same leniency `automation:` uses, and
/// for the same reason.
///
/// The *shape* is lenient, and so, for a skill, are the *names*: registration,
/// where a real tool registry can say which names exist, maps each entry
/// through [`pregrant_tool_name`] and drops, with a warn, what grants nothing
/// (`tool_metadata::registry::registration`). The skill keeps its slash
/// command; only a plugin command's list is refused over an unknown name.
///
/// Returns `None` when the key is absent or null (no declaration), and
/// `Some(names)` otherwise, possibly empty. For a skill both pre-grant nothing;
/// the two are kept apart because the same shapes on a plugin command mean
/// allow-all and deny-all.
///
/// A shape that is neither a sequence nor a scalar (a mapping, say) warns and
/// resolves to `None`. That is the one fail-open in this chain and it is
/// deliberate: at parse time there is no registry to refuse against, the
/// alternative punishes a YAML typo by silently disarming a skill, and `None`
/// is byte-for-byte the behaviour every skill has today. The warn is the
/// visibility the decision rests on.
///
/// # Deny-all has exactly one spelling, and it is a sequence
///
/// A **sequence** that comes out empty — `allowed-tools: []` — is a
/// declaration: `Some(vec![])`. On a plugin command
/// [`crate::gateway::execution_engine::slash_skill_scope`] enforces it as
/// deny-all: the slash command can call none of its declared tools, and only
/// what the run attaches itself remains (`subagent`, and the disclosure /
/// deferral meta-tools `get_tool_schema` / `tool_search` when those are on).
/// On a skill it pre-grants nothing (and restricts only on the stale-row arm
/// `slash_skill_pregrant::split` describes).
///
/// A **scalar** that names no tool is not. `allowed-tools: ,` /
/// `allowed-tools: ""` / `allowed-tools: "   "` warn and resolve to `None`,
/// the same fail-open as the unusable shape above and for the same reasons,
/// one of which is specific to this arm: the harshest outcome on the whole
/// chain would arrive **silently**, because registration warns per entry it
/// drops and an empty set has no entries to warn about. Dropping a trailing
/// comma (the leniency below) and then letting
/// a comma with nothing on either side of it cost the author every tool would
/// be the same typo answered two opposite ways.
///
/// `""` is deliberately on the typo side of that line. It is the shape a
/// half-deleted value or an unsubstituted template leaves behind, and
/// `slash_skill_scope`'s own module doc records why an empty string may not
/// carry this meaning: `""` "reads identically to a key that was never
/// written". An author who means deny-all has `[]`, which is unambiguous in
/// every reader.
///
/// # This is the *skill* policy
///
/// The shapes are read by [`read_allowed_tools`]; this function is what a
/// skill makes of the two it cannot use. A plugin command's `allowed-tools`
/// and a plugin agent's `tools` read the same shapes and answer those two
/// with deny-all instead (`extension::manifest::parsers::restrict_tool_list`):
/// their lists only narrow, and they have no second reader to fall back on.
#[must_use]
pub fn normalize_allowed_tools(
    raw: Option<&crate::yaml::Value>,
    skill_name: &str,
) -> Option<Vec<String>> {
    match read_allowed_tools(raw) {
        Ok(names) => names,
        // Same typo as a trailing comma, one step further. A skill's list
        // pre-grants (it never narrows the surface), so `None` and
        // `Some(vec![])` grant the same nothing; the warn names the typo.
        Err(UnusableAllowedTools::NamesNothing) => {
            tracing::warn!(
                skill = %skill_name,
                value = ?raw,
                key = ALLOWED_TOOLS_KEY,
                "skill declares `allowed-tools:` as a scalar that names no tool — read as \
                 no declaration: `/<skill>` pre-grants nothing"
            );
            None
        }
        Err(UnusableAllowedTools::Shape) => {
            tracing::warn!(
                skill = %skill_name,
                shape = ?raw,
                key = ALLOWED_TOOLS_KEY,
                "skill declares `allowed-tools:` in a shape that is neither a list nor a \
                 comma-separated string — ignored: `/<skill>` pre-grants nothing"
            );
            None
        }
    }
}

/// The Aleph tool one entry of a SKILL's `allowed-tools` may pre-grant, or
/// `None` when the entry can grant nothing whatever tools exist.
///
/// Claude Code names map to Aleph names
/// (`extension::hooks::normalize_cc_tool_entry`: `Read` → `file_read`,
/// `mcp__s__t` → `s__t`); `None` for a CC tool with no counterpart, a bare
/// `mcp__<server>`, a scoped `Bash(git *)` (pre-granting all of `bash` for a
/// grant scoped to one command widens an approval skip), and anything
/// [`is_glob_pattern`](crate::config::types::policies::is_glob_pattern)
/// accepts — a pre-grant folds exact entries, and the policy reads such a key
/// back as a glob. An unknown name passes through; whether it exists is the
/// caller's question.
///
/// The ONE mapping of a skill's list: registration validates its result
/// against the tools that exist, and the turn's pre-grant
/// (`slash_skill_scope::stamp_pregrant_from_names`) maps the loaded file with
/// it before intersecting with that validated list — two spellings of one
/// name must meet in the intersection.
pub(crate) fn pregrant_tool_name(entry: &str) -> Option<String> {
    crate::extension::hooks::normalize_cc_tool_entry(entry, false)
        .filter(|tool| !crate::config::types::policies::is_glob_pattern(tool))
}

/// Why a present [`ALLOWED_TOOLS_KEY`] value gives no list to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnusableAllowedTools {
    /// A scalar with no tool in it: `,`, `""`, `"   "`.
    NamesNothing,
    /// Neither a sequence nor a string: a mapping, a number, a bool.
    Shape,
}

/// The shapes of the [`ALLOWED_TOOLS_KEY`] block, with no policy and no log:
/// the one reader both a skill ([`normalize_allowed_tools`]) and a plugin
/// command build on, each deciding for itself what an unusable value means.
///
/// `Ok(None)` — absent or null, no declaration. `Ok(Some(names))` — a YAML
/// sequence or a comma-separated string; a sequence that comes out empty is
/// `allowed-tools: []`, the explicit deny-all. `Err(why)` — present, and
/// unusable.
///
/// Blank entries are dropped rather than forwarded as unknown names: a
/// trailing comma is a typo, and costing the author their slash command over
/// one would be a worse answer than they asked for.
pub(crate) fn read_allowed_tools(
    raw: Option<&crate::yaml::Value>,
) -> Result<Option<Vec<String>>, UnusableAllowedTools> {
    let Some(value) = raw else {
        return Ok(None);
    };
    match value {
        crate::yaml::Value::Null => Ok(None),
        crate::yaml::Value::Sequence(items) => Ok(Some(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
        )),
        crate::yaml::Value::String(s) => {
            let names: Vec<String> = s
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            if names.is_empty() {
                Err(UnusableAllowedTools::NamesNothing)
            } else {
                Ok(Some(names))
            }
        }
        _ => Err(UnusableAllowedTools::Shape),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Code's authoring guidance gives `argument-hint: [arg1] [arg2]
    /// [optional-arg]` as the format. That is not YAML; retried once with the
    /// line quoted, it is the text the author wrote and the rest of the block
    /// is untouched.
    #[test]
    fn a_multi_bracket_argument_hint_is_read_as_text() {
        let yaml = "name: n\nargument-hint: [arg1] [arg2] [optional-arg]\ndescription: d\n";
        assert!(
            crate::yaml::from_str::<crate::yaml::Value>(yaml).is_err(),
            "premise: this is not YAML"
        );
        let v: crate::yaml::Value = parse_frontmatter_yaml(yaml, &"t").unwrap();
        assert_eq!(
            v["argument-hint"].as_str(),
            Some("[arg1] [arg2] [optional-arg]")
        );
        assert_eq!(v["description"].as_str(), Some("d"));
    }

    /// The retry is for that one line. A block broken elsewhere still fails,
    /// and with the error the author's own text produced — not the rewrite's.
    #[test]
    fn an_unrelated_yaml_error_reports_the_original_error() {
        let yaml = "argument-hint: [a] [b]\ndescription: a: b\n";
        let original = crate::yaml::from_str::<crate::yaml::Value>(yaml)
            .unwrap_err()
            .to_string();
        let rewritten = quote_bare_argument_hint(yaml).expect("premise: the hint line qualifies");
        let retried = crate::yaml::from_str::<crate::yaml::Value>(&rewritten)
            .unwrap_err()
            .to_string();
        assert_ne!(
            retried, original,
            "premise: the two errors are distinguishable"
        );
        let err = parse_frontmatter_yaml::<crate::yaml::Value>(yaml, &"t")
            .unwrap_err()
            .to_string();
        assert_eq!(err, original);
    }

    #[test]
    fn only_a_bare_bracketed_hint_line_is_rewritten() {
        for untouched in [
            "argument-hint: \"[a] [b]\"\n",
            "argument-hint: '[a] [b]'\n",
            "argument-hint: pr-number\n",
            "argument-hint:\n",
            "description: [a] [b]\n",
            "  argument-hint: [a] [b]\n",
            "argument-hint: [a,\n  b]\n",
        ] {
            assert_eq!(quote_bare_argument_hint(untouched), None, "{untouched:?}");
        }
        assert_eq!(
            quote_bare_argument_hint("argument-hint: [a] [\"b\\c\"]\nx: 1\n").as_deref(),
            Some("argument-hint: \"[a] [\\\"b\\\\c\\\"]\"\nx: 1\n")
        );
    }

    #[test]
    fn splits_a_plain_document() {
        let (yaml, body) = split("---\nname: a\n---\nBody here.").unwrap();
        assert_eq!(yaml, "name: a\n");
        assert_eq!(body, "Body here.");
    }

    /// The bug the two other splitters had: a `---` that is not alone on its
    /// line (here, inside a block scalar) must not terminate the frontmatter.
    /// `find("\n---")` cut here; the line-wise scan does not.
    #[test]
    fn a_dashed_line_inside_a_yaml_value_is_not_a_terminator() {
        let content =
            "---\nname: a\ndescription: |\n  intro\n  --- not a fence\n  outro\n---\nBody.";
        let (yaml, body) = split(content).unwrap();
        assert!(
            yaml.contains("--- not a fence"),
            "the block scalar must stay inside the frontmatter, got {yaml:?}"
        );
        assert_eq!(body, "Body.");
    }

    /// The stated boundary, pinned so it is a recorded decision and not a
    /// surprise: an *indented bare* `---` inside a block scalar DOES terminate.
    /// If this test ever starts failing, someone tightened the fence rule —
    /// which is allowed, but it changes `skill::manifest`'s semantics for every
    /// SKILL.md that indents its closing fence, so it must be a deliberate
    /// change with that consequence weighed, not a drive-by.
    #[test]
    fn an_indented_bare_fence_still_terminates_documents_a_known_boundary() {
        let content = "---\nname: a\ndescription: |\n  intro\n  ---\n  outro\n---\nBody.";
        let (yaml, _) = split(content).unwrap();
        assert_eq!(
            yaml, "name: a\ndescription: |\n  intro\n",
            "documented limitation: an indented line that is exactly `---` reads as the fence"
        );
    }

    /// A horizontal rule in the *body* must not be mistaken for the closing
    /// fence either — the closing fence is the first one, and everything after
    /// it is body verbatim.
    #[test]
    fn a_horizontal_rule_in_the_body_survives() {
        let (_, body) = split("---\nname: a\n---\nIntro\n\n---\n\nOutro").unwrap();
        assert!(body.contains("Intro") && body.contains("Outro"));
        assert!(body.contains("---"), "body rule must survive: {body:?}");
    }

    #[test]
    fn crlf_is_normalised_in_both_halves() {
        let (yaml, body) = split("---\r\nname: a\r\n---\r\nBody\r\n").unwrap();
        assert_eq!(yaml, "name: a\n");
        assert_eq!(body, "Body\n");
    }

    #[test]
    fn a_closing_fence_at_end_of_file_yields_an_empty_body() {
        let (yaml, body) = split("---\nname: a\n---").unwrap();
        assert_eq!(yaml, "name: a\n");
        assert_eq!(body, "");
    }

    #[test]
    fn missing_or_unclosed_frontmatter_is_an_error() {
        assert_eq!(split("no fence at all").unwrap_err(), NoFrontmatter);
        assert_eq!(split("---\nname: a\nno close").unwrap_err(), NoFrontmatter);
    }

    #[test]
    fn allowed_tools_accepts_a_sequence() {
        let v: crate::yaml::Value = crate::yaml::from_str("[grep, file_read]").unwrap();
        assert_eq!(
            normalize_allowed_tools(Some(&v), "s"),
            Some(vec!["grep".to_string(), "file_read".to_string()])
        );
    }

    #[test]
    fn allowed_tools_accepts_the_upstream_comma_scalar() {
        let v = crate::yaml::Value::String("Read, Grep, Bash(cargo run -- *)".to_string());
        assert_eq!(
            normalize_allowed_tools(Some(&v), "s"),
            Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Bash(cargo run -- *)".to_string()
            ])
        );
    }

    #[test]
    fn allowed_tools_distinguishes_absent_from_empty() {
        assert!(normalize_allowed_tools(None, "s").is_none());
        assert!(normalize_allowed_tools(Some(&crate::yaml::Value::Null), "s").is_none());
        let empty: crate::yaml::Value = crate::yaml::from_str("[]").unwrap();
        assert_eq!(normalize_allowed_tools(Some(&empty), "s"), Some(vec![]));
    }

    /// The documented leniency, pinned on its own so the boundary below has
    /// something to be a boundary *of*: a trailing comma is a typo and must
    /// not cost the author the tool it precedes.
    #[test]
    fn allowed_tools_tolerates_a_trailing_comma() {
        let v = crate::yaml::Value::String("Read,".to_string());
        assert_eq!(
            normalize_allowed_tools(Some(&v), "s"),
            Some(vec!["Read".to_string()]),
            "a trailing comma is a typo; the named tool must survive it"
        );
    }

    /// A scalar that names no tool at all is the same typo one step further —
    /// and resolving it to `Some(empty)` would hand the author the *harshest*
    /// outcome on the whole chain (`slash_skill_scope`'s explicit deny-all, so
    /// the slash command can call none of its declared tools) for a
    /// pure-punctuation slip,
    /// silently: registration warns about each entry it drops, and an empty
    /// set has no entries to warn about.
    ///
    /// So it reads as no declaration. The contrast case is asserted in the
    /// same test so the two can never be conflated: an empty **sequence** is
    /// still the explicit deny-all, and that is the only spelling of it.
    #[test]
    fn allowed_tools_scalar_naming_no_tool_is_a_typo_not_a_deny_all() {
        for raw in [",", ",,", " , ", "   ", ""] {
            let v = crate::yaml::Value::String(raw.to_string());
            assert_eq!(
                normalize_allowed_tools(Some(&v), "s"),
                None,
                "scalar {raw:?} names no tool, so it is a typo, not a declaration — it must \
                 read as allow-all, not as the explicit deny-all"
            );
        }

        // Contrast, deliberately in the same test: the empty *sequence* is a
        // declaration and keeps meaning deny-all.
        let empty_seq: crate::yaml::Value = crate::yaml::from_str("[]").unwrap();
        assert_eq!(
            normalize_allowed_tools(Some(&empty_seq), "s"),
            Some(vec![]),
            "`allowed-tools: []` is the explicit deny-all and must stay one"
        );
    }

    #[test]
    fn allowed_tools_unusable_shape_reads_as_no_declaration() {
        let v: crate::yaml::Value = crate::yaml::from_str("{read: yes}").unwrap();
        assert!(normalize_allowed_tools(Some(&v), "s").is_none());
    }

    // ---------------------------------------------------------------------
    // Census: the key is spelled in exactly one place
    // ---------------------------------------------------------------------

    /// Production code only: everything from the first `#[cfg(test)]` onward
    /// is dropped (a test may name the key freely in a fixture or a
    /// test-function name), and so are comment lines (a doc comment that
    /// *names* the key — this module's prose does, repeatedly — is
    /// documentation, not a spelling).
    ///
    /// Both cuts are delegated, not re-written. The first version hand-rolled
    /// `take_while(|l| l.trim() != "#[cfg(test)]")`, which
    /// `utils::source_scan::tests::no_module_hand_rolls_the_cfg_test_prefix_cut`
    /// refuses by name and which is wrong twice over: it under-scans a file
    /// that gates an item mid-file, and it reads a whole-file test module
    /// (`agents/tests.rs` and 100-odd others, whose `#[cfg(test)]` lives on
    /// the PARENT's `mod` line) as 100% production.
    /// [`crate::utils::source_scan::production_text`] answers both, because it
    /// is handed the path as well as the text.
    ///
    /// The positive control in the caller — the enumerated files must still be
    /// *found* — is what keeps a narrowing here from silently turning the
    /// census green.
    fn code_lines(path: &std::path::Path, src: &str) -> String {
        crate::utils::source_scan::strip_comment_lines(&crate::utils::source_scan::production_text(
            path, src,
        ))
    }

    fn walk_rs(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Does the part-1 matcher below actually go red for anything?
    ///
    /// A source census that cannot be shown to fail is [判据 §2] — it looks
    /// identical to one that passes because nothing is wrong. This states the
    /// specific line shape it catches, and the specific near-misses it does
    /// not (a doc comment naming the key; a YAML fixture inside a test).
    #[test]
    fn the_serde_rename_matcher_flags_exactly_the_shape_it_claims() {
        assert!(spells_kebab_key_in_a_serde_attr(
            "    #[serde(rename = \"allowed-tools\", default)]"
        ));
        assert!(!spells_kebab_key_in_a_serde_attr(
            "    /// The `allowed-tools:` frontmatter block."
        ));
        assert!(!spells_kebab_key_in_a_serde_attr(
            "        let content = \"---\\nallowed-tools: grep\\n---\";"
        ));
        assert!(!spells_kebab_key_in_a_serde_attr(
            "    #[serde(rename = \"mcpServers\", default)]"
        ));
    }

    /// One line's worth of the part-1 predicate: a serde attribute that spells
    /// the kebab key by hand.
    fn spells_kebab_key_in_a_serde_attr(line: &str) -> bool {
        let code = line.trim_start();
        !code.starts_with("//") && code.starts_with("#[serde(") && code.contains(ALLOWED_TOOLS_KEY)
    }

    /// Self-defence, part 1 — the hand-written serde rename.
    ///
    /// A fourth ingestion path that writes `#[serde(rename = "allowed-tools")]`
    /// of its own is exactly how the third one happened: `extension::manifest::
    /// parsers::SkillFm` had a strict `Option<Vec<String>>` under that rename,
    /// with zero readers and the skill-deleting shape
    /// [`normalize_allowed_tools`] exists to avoid.
    ///
    /// The sanctioned way to reach this wire key is `rename_all =
    /// "kebab-case"` on the container plus a field named `allowed_tools`, with
    /// [`ALLOWED_TOOLS_KEY`] as the name anything else refers to it by — so
    /// the expected count of hand-written renames is **zero**, here included.
    /// `walk_rs` finding a plausible number of files is the positive control
    /// that keeps "zero" from meaning "the walk broke".
    ///
    /// This is a source scan, not a runtime assertion, for the same reason
    /// `extension::projection`'s census is: at runtime a second speller looks
    /// exactly like the first one being read twice.
    #[test]
    fn no_file_hand_writes_a_serde_rename_for_the_kebab_key() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk_rs(&src, &mut files);
        assert!(
            files.len() > 100,
            "census scanned only {} files — the walk is broken, not the tree",
            files.len()
        );

        let mut offenders: Vec<String> = Vec::new();
        for path in files {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                if spells_kebab_key_in_a_serde_attr(line) {
                    offenders.push(format!("{}:{}", path.display(), idx + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "`{ALLOWED_TOOLS_KEY}` is hand-written in a serde attribute at {offenders:?}. \
             Use `rename_all = \"kebab-case\"` + a field named `allowed_tools`, route the \
             value through `skill::frontmatter::read_allowed_tools` (a skill: \
             `normalize_allowed_tools`), and make sure the \
             new path actually reaches an enforcement point before honouring the key at all."
        );
    }

    /// Self-defence, part 2 — the `rename_all = "kebab-case"` back door.
    ///
    /// Part 1 cannot see a struct that spells the key implicitly: a serde
    /// container with `rename_all = "kebab-case"` and a field named
    /// `allowed_tools` produces the same wire key with no literal anywhere.
    /// So: any file that both deserialises YAML *and* names `allowed_tools`
    /// is a frontmatter reader of this key and must be on this list.
    ///
    /// The entries are not "the files that happen to match today" — each is
    /// here for a stated reason, and a new one is a decision, not a merge
    /// conflict:
    /// * `skill/manifest.rs` — the SKILL.md reader; it delegates the shape
    ///   handling to [`normalize_allowed_tools`] above.
    /// * `agents/loader.rs` — a **different concept**: agent `.md`
    ///   frontmatter, snake-cased `allowed_tools`, feeding `AgentDef`. It is
    ///   listed so the scan's blindness to it is a recorded decision rather
    ///   than an accident.
    /// * `extension/manifest/parsers.rs` — a plugin's `commands/*.md`; it
    ///   delegates the shape to [`read_allowed_tools`] and maps the names
    ///   through the CC alias table. It reaches an enforcement point for
    ///   commands only (`slash_effect::plugin_command_skill_info` →
    ///   `register_plugin_commands` → `slash_skill_scope`; the file → catalog-row half
    ///   is pinned by
    ///   `lifecycle::tests::a_mounted_commands_frontmatter_reaches_its_catalog_row`,
    ///   the row → run-loop half by `slash_skill_scope`'s wire tests), and
    ///   leaves a skill's declaration uncarried — `skill/manifest.rs` reads the
    ///   same SKILL.md.
    #[test]
    fn yaml_frontmatter_readers_of_allowed_tools_are_an_enumerated_set() {
        const ALLOWED: [&str; 3] = [
            "skill/manifest.rs",
            "agents/loader.rs",
            "extension/manifest/parsers.rs",
        ];

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk_rs(&src, &mut files);

        let this_file = src.join("skill").join("frontmatter.rs");
        let mut found: Vec<String> = Vec::new();
        for path in files {
            if path == this_file {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let code = code_lines(&path, &content);
            // Every way into the YAML parser — including this module's own
            // frontmatter entry point, which is how the two SKILL.md readers
            // reach it; a census blind to it would miss the next reader too.
            if !(code.contains("crate::yaml::from_str")
                || code.contains("crate::yaml::from_value")
                || code.contains("parse_frontmatter_yaml"))
            {
                continue;
            }
            if !code.contains("allowed_tools") {
                continue;
            }
            found.push(
                path.strip_prefix(&src)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }

        // Positive control first: if the predicate stopped matching the
        // files it is *supposed* to match, an empty offender list means the
        // scan broke, not that the tree is clean.
        for expected in ALLOWED {
            assert!(
                found.iter().any(|f| f == expected),
                "census did not find `{expected}`, which is known to read `allowed_tools` \
                 from YAML — the predicate is broken, so its silence proves nothing. \
                 Found: {found:?}"
            );
        }

        let offenders: Vec<&String> = found
            .iter()
            .filter(|f| !ALLOWED.contains(&f.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "a YAML frontmatter parser outside the enumerated set reads `allowed_tools`: \
             {offenders:?}. Either route it through skill::frontmatter, or add it here with \
             a reason — and answer first whether that path reaches an enforcement point at \
             all, because a parsed-but-unenforced declaration is a no-op that reports success."
        );
    }
}
