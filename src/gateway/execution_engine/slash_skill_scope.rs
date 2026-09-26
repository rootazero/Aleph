//! The two tool facts a `/<name>` run carries: the scope a plugin COMMAND's
//! `allowed-tools:` RESTRICTS the turn to, and the names a SKILL's
//! `allowed-tools:` PRE-GRANTS for it.
//!
//! Claude Code gives the one frontmatter key two meanings. On a command it
//! narrows the turn's tools; on a skill it "does NOT restrict tool access;
//! only pre-grants permission for the listed tools". The two ride two keys
//! here and are never allowed to share one: a restriction read as a
//! pre-grant is an approval bypass, a pre-grant read as a restriction takes
//! the model's tools away.
//!
//! **The restriction.** A command's declaration is validated at registration
//! (`tool_metadata::registry::registration::ToolRegistrar::register_skills`),
//! rides the slash-command envelope as `mode["allowed_tools"]`, and has to
//! reach the run loop, which builds the tool surface. Since the envelope is
//! re-parsed on entry and the run loop iterates Think→Act many times, the
//! scope is lifted into request metadata once and read back from there.
//! Which registrations restrict, and which pre-grant, is decided by
//! `slash_skill_pregrant::split` — the one caller of both encoders.
//!
//! **The pre-grant** ([`SLASH_SKILL_PREGRANT_TOOLS_KEY`]) is per turn: never
//! replayed on resume (`resume_coordinator` replays only the restriction),
//! stripped by the steering rescue ([`strip`]), and removed from every
//! incoming request before `split` derives it ([`forget_pregrant`]).
//! `turn_permissions::apply_pregrant` is its one reader.
//!
//! This module owns all three halves of that wire — the keys' spelling, the
//! encoding, and the decoding — because they were previously spread across
//! three files, with the decode written out twice (once for builtins, once
//! for MCP) and a third tool source that nobody remembered to filter at all.
//! One derivation, several consumers.
//!
//! # The restriction's tri-state is the whole point
//!
//! * key absent → `None` → the command declared nothing; the run keeps the
//!   agent's full tool surface. This is what most commands ship with.
//! * `[]` → `Some(empty)` → the author wrote `allowed-tools: []`; deny all.
//! * `["grep", …]` → `Some(names)` → narrow to exactly these.
//!
//! The encoding is JSON, not a comma-joined string, for exactly the middle
//! case: `""` cannot say "the author wrote an empty list" — it reads
//! identically to a key that was never written, and an empty allow-set means
//! *allow-all* by the time it reaches `ScopedToolService`. A string encoding
//! would silently turn "deny everything" into "allow everything".

use std::collections::{HashMap, HashSet};

use crate::tool_metadata::UnifiedTool;

/// Request-metadata key carrying this run's tool RESTRICTION (a plugin
/// command's `allowed-tools:`).
///
/// Written by [`stamp_list`] (for [`stamp_from_mode`] and the resume replay),
/// read by [`from_metadata`], removed by [`strip`]. Nothing outside this
/// module should spell it.
pub(crate) const SLASH_SKILL_ALLOWED_TOOLS_KEY: &str = "slash_skill_allowed_tools";

/// Request-metadata key: the tools a `/<skill>` PRE-GRANTS for this turn —
/// Claude Code's reading of a skill's `allowed-tools:`: the listed names run
/// without the tier's confirmation, and the tool SURFACE is untouched.
///
/// Distinct from [`SLASH_SKILL_ALLOWED_TOOLS_KEY`], which narrows the
/// surface. Written only by [`stamp_pregrant_from_names`], read only by
/// [`pregrant_from_metadata`], removed by [`forget_pregrant`] and [`strip`].
/// Nothing outside this module should spell it.
pub(crate) const SLASH_SKILL_PREGRANT_TOOLS_KEY: &str = "slash_skill_pregrant_tools";

/// Lift a plugin command's restriction out of a parsed slash-command envelope
/// into request metadata (`slash_skill_pregrant::split` decides it is one).
///
/// `mode` is the deserialized `SLASH_COMMAND_MODE_KEY` JSON. A `null` or
/// absent `allowed_tools` writes nothing (allow-all); an array — **including
/// an empty one** — is always written, because an empty list is a
/// declaration, not the absence of one.
pub(crate) fn stamp_from_mode(metadata: &mut HashMap<String, String>, mode: &serde_json::Value) {
    let Some(allowed) = mode.get("allowed_tools").and_then(|v| v.as_array()) else {
        return;
    };
    let tools: Vec<String> = allowed
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    stamp_list(metadata, &tools);
}

/// Write an explicit declaration (an empty one included) under the key.
///
/// The one encoder: [`stamp_from_mode`] feeds it the mode JSON's list, and
/// `resume_coordinator::plan_resume` feeds it the list frozen on the crashed
/// run's `RunStarted` envelope, so a replayed scope is byte-for-byte the wire
/// the run loop decodes.
pub(crate) fn stamp_list(metadata: &mut HashMap<String, String>, tools: &[String]) {
    // `if let` is the shape serde hands back, not a swallow: a `&[String]`
    // cannot fail to serialise.
    if let Ok(encoded) = serde_json::to_string(tools) {
        metadata.insert(SLASH_SKILL_ALLOWED_TOOLS_KEY.to_string(), encoded);
    }
}

/// Read this run's skill tool scope back out of request metadata.
///
/// `None` means "no declaration" — do not narrow anything.
///
/// A malformed value resolves to `Some(empty)`, i.e. deny-all, not to `None`.
/// [`stamp_from_mode`] in this same process is the only writer, so an
/// unreadable value means something is wrong, and "I cannot read the
/// restriction" must never be read back as "there is no restriction".
pub(crate) fn from_metadata(metadata: &HashMap<String, String>) -> Option<HashSet<String>> {
    metadata.get(SLASH_SKILL_ALLOWED_TOOLS_KEY).map(|raw| {
        match serde_json::from_str::<Vec<String>>(raw) {
            Ok(names) => names.into_iter().collect(),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "slash-skill tool scope is unreadable; denying all tools for this run"
                );
                HashSet::new()
            }
        }
    })
}

/// Drop both tool facts from a metadata map that is being reused for a
/// different run (steering rescue), so a command's narrowing does not leak
/// into a plain loop continuation, and a skill's pre-grant does not outlive
/// the turn it was granted for.
pub(crate) fn strip(metadata: &mut HashMap<String, String>) {
    metadata.remove(SLASH_SKILL_ALLOWED_TOOLS_KEY);
    forget_pregrant(metadata);
}

/// Write a skill's pre-grant from its `allowed-tools:` names as the author
/// wrote them. Claude Code names map to Aleph names
/// (`extension::hooks::normalize_cc_tool_entry`); a scoped `Bash(git *)` is
/// DROPPED rather than folded into `bash` — pre-granting the whole tool for
/// a grant scoped to one command widens an approval skip — and so is `*`,
/// which would pre-grant every tool. Nothing left ⇒ nothing is written: an
/// empty pre-grant is no grant, not deny-all.
pub(crate) fn stamp_pregrant_from_names(metadata: &mut HashMap<String, String>, names: &[String]) {
    let mut tools: Vec<String> = Vec::new();
    for name in names {
        let Some(tool) = crate::extension::hooks::normalize_cc_tool_entry(name, false) else {
            continue;
        };
        if tool != "*" && !tools.contains(&tool) {
            tools.push(tool);
        }
    }
    if tools.is_empty() {
        return;
    }
    // `if let` is the shape serde hands back, not a swallow: a `&[String]`
    // cannot fail to serialise.
    if let Ok(encoded) = serde_json::to_string(&tools) {
        metadata.insert(SLASH_SKILL_PREGRANT_TOOLS_KEY.to_string(), encoded);
    }
}

/// This turn's pre-granted names. Absent or unreadable ⇒ empty: a grant that
/// cannot be read is not granted — the fail-closed direction for a key whose
/// only effect is to skip a confirmation.
pub(crate) fn pregrant_from_metadata(metadata: &HashMap<String, String>) -> Vec<String> {
    let Some(raw) = metadata.get(SLASH_SKILL_PREGRANT_TOOLS_KEY) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|e| {
        tracing::error!(
            error = %e,
            "slash-skill pre-grant is unreadable; nothing is pre-granted for this run"
        );
        Vec::new()
    })
}

/// Remove a pre-grant the request arrived with. `slash_skill_pregrant::split`
/// calls this for every request before it derives the turn's own, so no
/// producer — a client, a channel, a queue replay, a continuation — can hand
/// a run a pre-grant it did not earn by invoking a skill.
pub(crate) fn forget_pregrant(metadata: &mut HashMap<String, String>) {
    metadata.remove(SLASH_SKILL_PREGRANT_TOOLS_KEY);
}

/// Narrow a candidate tool list to `scope`, returning how many were dropped.
///
/// `None` leaves the list untouched. `Some(empty)` empties it — that is the
/// explicit deny-all, and it is enforced by the resulting `LoopToolRegistry`
/// being empty rather than by a second refusal path: `build_registry_from_tools`
/// builds the request's registry out of exactly this list, so a tool that is
/// not here is not dispatchable, listable, or describable.
pub(crate) fn narrow(tools: &mut Vec<UnifiedTool>, scope: Option<&HashSet<String>>) -> usize {
    let Some(scope) = scope else {
        return 0;
    };
    let before = tools.len();
    tools.retain(|t| scope.contains(t.name.as_str()));
    before - tools.len()
}

/// Whether a tool joined *after* [`narrow`] ran may still enter this run's
/// surface. Sources joined later (MCP, markdown CLI skills) must consult the
/// same set rather than re-deriving it — the second derivation is how the
/// third source ends up unfiltered.
pub(crate) fn admits(scope: Option<&HashSet<String>>, name: &str) -> bool {
    scope.is_none_or(|s| s.contains(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_metadata::ToolSource;

    fn tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(format!("builtin:{name}"), name, "desc", ToolSource::Builtin)
    }

    fn mode(allowed: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "type": "skill", "allowed_tools": allowed })
    }

    #[test]
    fn an_absent_declaration_round_trips_as_allow_all() {
        let mut meta = HashMap::new();
        stamp_from_mode(&mut meta, &mode(serde_json::Value::Null));
        assert!(
            !meta.contains_key(SLASH_SKILL_ALLOWED_TOOLS_KEY),
            "a null declaration must write no key at all"
        );
        assert!(from_metadata(&meta).is_none());

        let mut tools = vec![tool("grep"), tool("bash")];
        assert_eq!(narrow(&mut tools, from_metadata(&meta).as_ref()), 0);
        assert_eq!(tools.len(), 2, "allow-all must not drop anything");
        assert!(admits(from_metadata(&meta).as_ref(), "anything_at_all"));
    }

    #[test]
    fn an_explicit_empty_declaration_round_trips_as_deny_all() {
        let mut meta = HashMap::new();
        stamp_from_mode(&mut meta, &mode(serde_json::json!([])));
        // The distinction the comma-joined encoding could not carry: this key
        // IS present, and it decodes to a set, not to "no declaration".
        assert!(meta.contains_key(SLASH_SKILL_ALLOWED_TOOLS_KEY));
        let scope = from_metadata(&meta);
        assert_eq!(scope, Some(HashSet::new()));

        let mut tools = vec![tool("grep"), tool("bash")];
        assert_eq!(narrow(&mut tools, scope.as_ref()), 2);
        assert!(tools.is_empty(), "`allowed-tools: []` must deny everything");
        assert!(!admits(scope.as_ref(), "grep"));
    }

    #[test]
    fn a_named_declaration_round_trips_as_that_set() {
        let mut meta = HashMap::new();
        stamp_from_mode(&mut meta, &mode(serde_json::json!(["grep", "file_read"])));
        let scope = from_metadata(&meta);

        let mut tools = vec![tool("grep"), tool("bash"), tool("file_read")];
        assert_eq!(narrow(&mut tools, scope.as_ref()), 1);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["grep", "file_read"]);

        assert!(admits(scope.as_ref(), "grep"));
        assert!(!admits(scope.as_ref(), "bash"));
    }

    #[test]
    fn an_unreadable_value_denies_rather_than_allows() {
        let mut meta = HashMap::new();
        meta.insert(
            SLASH_SKILL_ALLOWED_TOOLS_KEY.to_string(),
            "grep,bash".to_string(), // the old comma encoding — not JSON
        );
        let scope = from_metadata(&meta);
        assert_eq!(
            scope,
            Some(HashSet::new()),
            "an unparseable restriction must not read back as `no restriction`"
        );
    }

    #[test]
    fn strip_removes_the_declaration() {
        let mut meta = HashMap::new();
        stamp_from_mode(&mut meta, &mode(serde_json::json!(["grep"])));
        stamp_pregrant_from_names(&mut meta, &["bash".to_string()]);
        strip(&mut meta);
        assert!(from_metadata(&meta).is_none());
        assert!(
            pregrant_from_metadata(&meta).is_empty(),
            "the steering rescue's strip must drop the pre-grant too"
        );
    }

    #[test]
    fn pregrant_is_a_separate_key_from_the_restrict_scope() {
        let mut md = HashMap::new();
        stamp_pregrant_from_names(&mut md, &["grep".to_string(), "bash".to_string()]);
        assert!(md.contains_key(SLASH_SKILL_PREGRANT_TOOLS_KEY));
        assert!(
            !md.contains_key(SLASH_SKILL_ALLOWED_TOOLS_KEY),
            "pre-grant must not narrow"
        );
        assert!(
            from_metadata(&md).is_none(),
            "and must not read back as a scope"
        );
        assert_eq!(
            pregrant_from_metadata(&md),
            vec!["grep".to_string(), "bash".to_string()]
        );
        // Absent / empty → nothing pre-granted (an empty pre-grant is a
        // no-op, not deny-all), and nothing is written for an empty list.
        assert!(pregrant_from_metadata(&HashMap::new()).is_empty());
        let mut empty = HashMap::new();
        stamp_pregrant_from_names(&mut empty, &[]);
        assert!(empty.is_empty());
        forget_pregrant(&mut md);
        assert!(pregrant_from_metadata(&md).is_empty());
    }

    #[test]
    fn pregrant_entries_are_filtered_to_bare_aleph_names() {
        // `Bash(gh:*)` would pre-grant ALL of bash for a scoped grant, and `*`
        // every tool: both dropped. CC names map to Aleph names.
        let mut md = HashMap::new();
        stamp_pregrant_from_names(
            &mut md,
            &[
                "Bash(gh pr view:*)".to_string(),
                "Read".to_string(),
                "*".to_string(),
                "grep".to_string(),
                "file_read".to_string(),
            ],
        );
        assert_eq!(
            pregrant_from_metadata(&md),
            vec!["file_read".to_string(), "grep".to_string()]
        );
        // Nothing grantable left ⇒ nothing written.
        let mut only_wide = HashMap::new();
        stamp_pregrant_from_names(
            &mut only_wide,
            &["*".to_string(), "Bash(git *)".to_string()],
        );
        assert!(only_wide.is_empty(), "{only_wide:?}");
    }

    #[test]
    fn an_unreadable_pregrant_grants_nothing() {
        let mut md = HashMap::new();
        md.insert(
            SLASH_SKILL_PREGRANT_TOOLS_KEY.to_string(),
            "bash,file_write".to_string(),
        );
        assert!(pregrant_from_metadata(&md).is_empty());
    }

    /// The list encoder a resume replays a frozen scope through writes the
    /// same wire `stamp_from_mode` does — the empty declaration included,
    /// which is the case a second encoder is most likely to drop.
    #[test]
    fn stamp_list_round_trips_through_from_metadata_including_the_empty_declaration() {
        for tools in [vec![], vec!["grep".to_string(), "file_read".to_string()]] {
            let mut meta = HashMap::new();
            stamp_list(&mut meta, &tools);
            assert_eq!(
                from_metadata(&meta),
                Some(tools.iter().cloned().collect::<HashSet<_>>())
            );
        }
    }
}

/// End-to-end tests over the whole RESTRICT wire — a plugin command's
/// `allowed-tools:` (a skill's pre-grants instead: `slash_skill_pregrant`'s
/// tests) — and over registration's validation of any declaration.
///
/// Every hop is the production function — registration, the command parser,
/// the slash-command envelope, the split `execute.rs` runs, the scope decode,
/// the narrowing, the request registry build, and finally the real
/// `ScopedToolService`. The assertion is on **the tool list the model is
/// handed**, not on any intermediate value: throwing away the narrowing step's
/// effect turns these red.
///
/// The only stub is the leaf executor a tool would eventually dispatch into,
/// which no assertion here depends on.
#[cfg(test)]
mod wire_tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use serde_json::Value;

    use crate::command::CommandParser;
    use crate::executor::ToolRegistry;
    use crate::gateway::inbound_router::{serialize_parsed_command, SLASH_COMMAND_MODE_KEY};
    use crate::skill::SkillInfo;
    use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

    /// Leaf executor. `build_registry_from_tools` needs one to delegate to; no
    /// assertion in this module reaches it.
    struct DeadEndRegistry;

    impl ToolRegistry for DeadEndRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
        fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::error::Result<Value>> + Send + '_>,
        > {
            let name = name.to_string();
            Box::pin(async move { Err(crate::error::AlephError::tool_not_found(&name)) })
        }
    }

    /// The agent's tool surface before any skill narrowing — three real
    /// builtin names so the declarations under test can be honest ones.
    fn agent_surface() -> Vec<UnifiedTool> {
        ["grep", "file_read", "bash"]
            .into_iter()
            .map(|n| UnifiedTool::new(format!("builtin:{n}"), n, "desc", ToolSource::Builtin))
            .collect()
    }

    /// The plugin command every wire test declares on: its catalog row is the
    /// shape `extension::slash_effect::plugin_command_skill_info` registers.
    const COMMAND: &str = "plug:scoped";

    /// Run the real wire for a plugin command declaring `declared`, and
    /// return the tool names the model would see. `Err(rejected)` when
    /// registration refused the command.
    async fn surface_for(declared: Option<Vec<String>>) -> Result<Vec<String>, Vec<String>> {
        // --- hop 1: registration validates the declaration and puts it on
        // the UnifiedTool.
        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        let rejected = catalog
            .register_skills(&[SkillInfo {
                id: COMMAND.to_string(),
                name: "scoped".to_string(),
                description: "narrows its own toolbelt".to_string(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: declared,
                argument_hint: None,
                plugin_id: Some("plug".to_string()),
            }])
            .await;
        if !rejected.is_empty() {
            return Err(rejected);
        }

        // --- hop 2: the command parser derives CommandContext::Skill.
        let parsed = CommandParser::new(Arc::clone(&catalog))
            .parse_async(&format!("/{COMMAND} do a thing"))
            .await
            .expect("the command must resolve as a slash command");

        // --- hop 3: the slash-command envelope.
        let mode_json = serialize_parsed_command(&parsed).expect("commands serialize");
        let mode: Value = serde_json::from_str(&mode_json).expect("envelope is JSON");
        assert_eq!(mode.get("type").and_then(Value::as_str), Some("skill"));
        let session = crate::gateway::router::SessionKey::main("wire");
        let mut request = super::super::tests::gate_test_request(&session, "wire-run");
        request
            .metadata
            .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode_json);

        // --- hop 4: `execute.rs`'s split lifts a COMMAND's scope into
        // request metadata (no skill of that id is registered, so it is not a
        // skill's pre-grant).
        super::super::slash_skill_pregrant::split_with(
            &mut request,
            "wire-agent",
            &crate::skill::SkillSystem::new(),
        )
        .await;
        let metadata = request.metadata;

        // --- hop 5: the run loop decodes it once and narrows.
        let scope = super::from_metadata(&metadata);
        let mut tools = agent_surface();
        super::narrow(&mut tools, scope.as_ref());

        // --- hop 6: the request's tool registry is built from exactly that
        // list, and the real ScopedToolService lists it for the model.
        let registry = Arc::new(crate::tools::adapters::build_registry_from_tools(
            Arc::new(DeadEndRegistry),
            &tools,
        ));
        let allowed: BTreeSet<String> = tools.iter().map(|t| t.name.clone()).collect();
        let svc = super::super::tool_service_builder::build_request_tool_service(
            registry,
            allowed,
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );

        let mut names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        names.sort();
        Ok(names)
    }

    /// The one this whole change exists for. Before the wiring was complete
    /// this returned all three names — `with_routing_capabilities` had zero
    /// callers, so the declaration never left the manifest.
    #[tokio::test]
    async fn a_declared_tool_list_narrows_the_surface_the_model_sees() {
        let names = surface_for(Some(vec!["grep".to_string(), "file_read".to_string()]))
            .await
            .expect("a declaration of real tool names must register");
        assert_eq!(
            names,
            vec!["file_read".to_string(), "grep".to_string()],
            "the model must see exactly the tools the command declared"
        );
    }

    /// `allowed-tools: []` is a declaration, not an absence. It must not fall
    /// through to allow-all — which is what the previous comma-joined
    /// encoding plus the `if !tools.is_empty()` short-circuit produced.
    #[tokio::test]
    async fn an_explicitly_empty_list_denies_every_tool() {
        let names = surface_for(Some(Vec::new()))
            .await
            .expect("an empty declaration is well-formed");
        assert!(
            names.is_empty(),
            "`allowed-tools: []` must leave no tools on the surface, got {names:?}"
        );
    }

    /// Most commands declare nothing. None of them may lose a tool because
    /// of this wire.
    #[tokio::test]
    async fn no_declaration_preserves_the_full_surface() {
        let names = surface_for(None).await.expect("no declaration is fine");
        assert_eq!(
            names,
            vec![
                "bash".to_string(),
                "file_read".to_string(),
                "grep".to_string()
            ],
            "a command that declares nothing must keep the agent's whole toolbelt"
        );
    }

    /// Upstream Claude Code writes `Read` / `Bash` / `Grep`. Aleph has no such
    /// tools. A command's parse translates them (`manifest/parsers.rs`); a
    /// declaration that still names one here would, matched literally, retain
    /// zero tools while reporting success — a report-success no-op strictly
    /// worse than the silent drop this replaces — so the row is refused
    /// outright and the author is named.
    #[tokio::test]
    async fn an_unknown_tool_name_refuses_the_skill_outright() {
        let rejected = surface_for(Some(vec!["grep".to_string(), "Read".to_string()]))
            .await
            .expect_err("a declaration naming a nonexistent tool must be refused");
        assert_eq!(rejected, vec![COMMAND.to_string()]);
    }

    /// The refusal has to be visible in the *catalog*, not only in the return
    /// value: a skill that registers with its declaration quietly dropped is
    /// exactly the failure mode being fixed, and it would still satisfy an
    /// assertion about the returned list.
    #[tokio::test]
    async fn a_refused_skill_gets_no_slash_command_at_all() {
        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        let rejected = catalog
            .register_skills(&[SkillInfo {
                id: "bad-skill".to_string(),
                name: "Bad Skill".to_string(),
                description: "names a tool that does not exist".to_string(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: Some(vec!["Bash".to_string()]),
                argument_hint: None,
                plugin_id: None,
            }])
            .await;

        assert_eq!(rejected, vec!["bad-skill".to_string()]);
        assert!(
            catalog.check_conflict("bad-skill").await.is_none(),
            "a refused skill must not be registered as a slash command"
        );
        assert!(
            CommandParser::new(catalog)
                .parse_async("/bad-skill")
                .await
                .is_none(),
            "and it must not resolve"
        );
    }

    /// Nobody under `execution_engine/` may spell the metadata key except
    /// this module.
    ///
    /// The rule is "no second reader", not "the two known readers call the
    /// helper": pinning the known ones only pins the known ones, and the whole
    /// defect being repaired here was a *third* tool source (markdown CLI
    /// skills) that joined the surface after the narrowing and re-widened it,
    /// because the predicate lived at each consumer instead of at the wire.
    ///
    /// Literals are kept (`code_keeping_literals`) and comments dropped, so a
    /// prose mention of the key is fine and a `metadata.get("…")` is not —
    /// `code_text` would delete the string payload and go blind to exactly the
    /// bypass this is watching for.
    #[test]
    fn only_this_module_spells_the_scope_metadata_key() {
        use crate::utils::source_scan::{code_keeping_literals, production_prefix};

        const OWNER: &str = "src/gateway/execution_engine/slash_skill_scope.rs";

        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gateway/execution_engine");
        let mut files = Vec::new();
        walk(&root, &mut files);

        let mut offenders: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for file in files {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if rel == OWNER || rel.ends_with("/tests.rs") || rel.contains("_tests.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            scanned += 1;
            let body = code_keeping_literals(&production_prefix(&text));
            for (n, line) in body.lines().enumerate() {
                if line.contains("\"slash_skill_allowed_tools\"") {
                    offenders.push(format!("{rel}:{}", n + 1));
                }
            }
        }

        // Self-defence: a walk that found nothing would pass for the wrong
        // reason, and `execute.rs` alone is well over a dozen files' worth of
        // the tree this is supposed to cover.
        assert!(
            scanned > 10,
            "the census scanned only {scanned} files — it is not looking where it thinks"
        );
        assert!(
            offenders.is_empty(),
            "the scope metadata key is spelled outside `slash_skill_scope`: {offenders:?}"
        );
    }

    /// A skill may not borrow another *slash command's* name. The catalog is
    /// a slash-command index; `Skill` and `Custom` entries live only there and
    /// are never in the run loop's candidate tool list. Admitting one would
    /// pass validation and then match nothing — a silent deny-all, which is
    /// the failure this whole change removes.
    #[tokio::test]
    async fn a_sibling_skills_slash_name_is_not_a_tool_name() {
        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        let rejected = catalog
            .register_skills(&[
                SkillInfo {
                    id: "sibling".to_string(),
                    name: "Sibling".to_string(),
                    description: "just exists".to_string(),
                    scope: crate::domain::skill::PromptScope::System,
                    version: None,
                    allowed_tools: None,
                    argument_hint: None,
                    plugin_id: None,
                },
                SkillInfo {
                    id: "borrower".to_string(),
                    name: "Borrower".to_string(),
                    description: "names a sibling skill".to_string(),
                    scope: crate::domain::skill::PromptScope::System,
                    version: None,
                    allowed_tools: Some(vec!["sibling".to_string()]),
                    argument_hint: None,
                    plugin_id: None,
                },
            ])
            .await;

        assert_eq!(rejected, vec!["borrower".to_string()]);
        assert!(
            catalog.check_conflict("sibling").await.is_some(),
            "the sibling itself must still register"
        );
    }

    /// A skill naming only real tools still registers — the guard has to be
    /// able to say yes, or it is a guard that rejects everything.
    #[tokio::test]
    async fn a_skill_naming_real_tools_still_registers() {
        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        let rejected = catalog
            .register_skills(&[SkillInfo {
                id: "good-skill".to_string(),
                name: "Good Skill".to_string(),
                description: "names real tools".to_string(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: Some(vec!["grep".to_string(), "bash".to_string()]),
                argument_hint: None,
                plugin_id: None,
            }])
            .await;
        assert!(rejected.is_empty(), "unexpectedly refused: {rejected:?}");
        assert!(catalog.check_conflict("good-skill").await.is_some());
    }
}
