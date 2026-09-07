//! The skill tool scope carried by a `/<skill>` run.
//!
//! A skill may declare `allowed-tools:` in its SKILL.md frontmatter. That
//! declaration is validated at registration
//! (`tool_metadata::registry::registration::ToolRegistrar::register_skills`),
//! rides the slash-command envelope as `mode["allowed_tools"]`, and has to
//! reach the run loop, which builds the tool surface. Since the envelope is
//! re-parsed on entry and the run loop iterates Think→Act many times, the
//! scope is lifted into request metadata once and read back from there.
//!
//! This module owns all three halves of that wire — the key's spelling, the
//! encoding, and the decoding — because they were previously spread across
//! three files, with the decode written out twice (once for builtins, once
//! for MCP) and a third tool source that nobody remembered to filter at all.
//! One derivation, several consumers.
//!
//! # The tri-state is the whole point
//!
//! * key absent → `None` → the skill declared nothing; the run keeps the
//!   agent's full tool surface. This is what every skill shipped with.
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

/// Request-metadata key carrying this run's skill tool scope.
///
/// Written by [`stamp_from_mode`], read by [`from_metadata`], removed by
/// [`strip`]. Nothing outside this module should spell it.
pub(crate) const SLASH_SKILL_ALLOWED_TOOLS_KEY: &str = "slash_skill_allowed_tools";

/// Lift the skill scope out of a parsed slash-command envelope into request
/// metadata.
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
    if let Ok(encoded) = serde_json::to_string(&tools) {
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

/// Drop the scope from a metadata map that is being reused for a different
/// run (steering rescue), so a skill's narrowing does not leak into a plain
/// loop continuation.
pub(crate) fn strip(metadata: &mut HashMap<String, String>) {
    metadata.remove(SLASH_SKILL_ALLOWED_TOOLS_KEY);
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
        strip(&mut meta);
        assert!(from_metadata(&meta).is_none());
    }
}

/// End-to-end tests over the whole `allowed-tools:` wire.
///
/// Every hop is the production function — skill registration, the command
/// parser, the slash-command envelope, the metadata lift, the scope decode,
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

    /// Run the real wire for a skill declaring `declared`, and return the tool
    /// names the model would see. `Err(rejected)` when registration refused
    /// the skill.
    async fn surface_for(declared: Option<Vec<String>>) -> Result<Vec<String>, Vec<String>> {
        // --- hop 1: registration validates the declaration and puts it on
        // the UnifiedTool.
        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        let rejected = catalog
            .register_skills(&[SkillInfo {
                id: "scoped-skill".to_string(),
                name: "Scoped Skill".to_string(),
                description: "narrows its own toolbelt".to_string(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: declared,
            }])
            .await;
        if !rejected.is_empty() {
            return Err(rejected);
        }

        // --- hop 2: the command parser derives CommandContext::Skill.
        let parsed = CommandParser::new(Arc::clone(&catalog))
            .parse_async("/scoped-skill do a thing")
            .await
            .expect("the skill must resolve as a slash command");

        // --- hop 3: the slash-command envelope.
        let mode_json = serialize_parsed_command(&parsed).expect("skill commands serialize");
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), mode_json.clone());

        // --- hop 4: `execute.rs` lifts the scope into request metadata.
        let mode: Value = serde_json::from_str(&mode_json).expect("envelope is JSON");
        assert_eq!(mode.get("type").and_then(Value::as_str), Some("skill"));
        super::stamp_from_mode(&mut metadata, &mode);

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
            "the model must see exactly the tools the skill declared"
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

    /// Every skill in existence declares nothing. None of them may lose a
    /// tool because of this change.
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
            "a skill that declares nothing must keep the agent's whole toolbelt"
        );
    }

    /// Upstream Claude Code skills write `Read` / `Bash` / `Grep`. Aleph has
    /// no such tools. Matching them literally would retain zero tools while
    /// reporting success — a report-success no-op strictly worse than the
    /// silent drop this replaces — so the skill is refused outright and the
    /// author is named.
    #[tokio::test]
    async fn an_unknown_tool_name_refuses_the_skill_outright() {
        let rejected = surface_for(Some(vec!["grep".to_string(), "Read".to_string()]))
            .await
            .expect_err("a declaration naming a nonexistent tool must be refused");
        assert_eq!(rejected, vec!["scoped-skill".to_string()]);
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
                },
                SkillInfo {
                    id: "borrower".to_string(),
                    name: "Borrower".to_string(),
                    description: "names a sibling skill".to_string(),
                    scope: crate::domain::skill::PromptScope::System,
                    version: None,
                    allowed_tools: Some(vec!["sibling".to_string()]),
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
            }])
            .await;
        assert!(rejected.is_empty(), "unexpectedly refused: {rejected:?}");
        assert!(catalog.check_conflict("good-skill").await.is_some());
    }
}
