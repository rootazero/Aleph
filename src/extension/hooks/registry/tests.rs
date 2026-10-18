//! Source-only regressions for the Hook shell; Main runs these after handoff.
use super::*;
use crate::extension::hooks::{HookContext, HookMemoSink, ShellHookConsent};
use crate::extension::types::{HookAction, HookEvent, HookKind, PluginKind, PluginOrigin};
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

fn prompt(label: &str, priority: HookPriority) -> HookConfig {
    HookConfig {
        event: HookEvent::BeforeAgentStart,
        kind: HookKind::Interceptor,
        priority,
        matcher: None,
        actions: vec![HookAction::Prompt {
            prompt: label.into(),
        }],
        plugin_name: label.into(),
        plugin_root: PathBuf::new(),
        handler: None,
        timeout_secs: None,
        declared_event: Some("BeforeAgentStart".into()),
        scope_key: ScopeKey::Global,
    }
}

fn user(label: &str, priority: HookPriority) -> HookInput {
    HookInput::user(
        prompt(label, priority),
        Path::new("/task11/hooks.json"),
        &ScopeKey::Global,
    )
}

fn plugin(label: &str, priority: HookPriority) -> HookInput {
    let mut row = PluginRecord::new(
        "owning-row".into(),
        "display".into(),
        PluginKind::Static,
        PluginOrigin::Global,
    );
    row.root_dir = PathBuf::from("/task11/plugin");
    HookInput::plugin(prompt(label, priority), &row)
}

async fn outputs(executor: &HookExecutor) -> Vec<String> {
    executor
        .execute_interceptors(HookEvent::BeforeAgentStart, HookContext::new("shell-test"))
        .await
        .unwrap()
        .1
        .additional_contexts
}

#[tokio::test]
async fn a_transition_retains_an_intervening_user_commit_and_latest_plugins() {
    let registry = Arc::new(HookRegistry::new(HookExecutor::empty()));
    registry
        .replace_users(vec![user("old-user", HookPriority::Normal)])
        .await
        .unwrap();
    let mut events = registry.subscribe();
    let captured = registry.snapshot();
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let transition = Arc::clone(&registry);
    let task = tokio::spawn(async move {
        let cursor = transition.begin_transition();
        observed_tx.send(()).unwrap();
        resume_rx.await.unwrap();
        transition
            .publish_transition(
                cursor,
                vec![plugin("new-plugin-A", HookPriority::Normal)],
                vec![user("stale-user-A", HookPriority::Normal)],
            )
            .await
            .unwrap();
    });
    observed_rx.await.unwrap();
    registry
        .replace_users(vec![user("new-user-B", HookPriority::Normal)])
        .await
        .unwrap();
    resume_tx.send(()).unwrap();
    task.await.unwrap();
    let snapshot = registry.snapshot();
    assert_eq!(snapshot.revision, 3);
    assert_eq!(events.try_recv().unwrap().revision, 2);
    assert_eq!(events.try_recv().unwrap().revision, 3);
    assert!(events.try_recv().is_err());
    let inventory = snapshot.value.executor.inventory();
    assert_eq!(
        inventory
            .iter()
            .map(|r| r.source.as_str())
            .collect::<Vec<_>>(),
        ["new-plugin-A", "new-user-B"]
    );
    assert_eq!(
        outputs(&snapshot.value.executor).await,
        ["new-plugin-A", "new-user-B"]
    );
    assert_eq!(outputs(&captured.value.executor).await, ["old-user"]);
    registry
        .replace_users(vec![user("latest-user", HookPriority::Normal)])
        .await
        .unwrap();
    assert_eq!(
        outputs(&*registry.read().await).await,
        ["new-plugin-A", "latest-user"]
    );
    registry
        .replace_users(vec![user("latest-user", HookPriority::Normal)])
        .await
        .unwrap();
    assert_eq!(
        registry.read().await.hook_count(),
        2,
        "sync replaces only the user layer"
    );
}

#[tokio::test]
async fn unchanged_cursor_uses_fresh_users_and_fair_reads_see_whole_generations() {
    let registry = Arc::new(HookRegistry::new(HookExecutor::empty()));
    let read = registry.read().await;
    let observed = registry.begin_transition();
    let mut publication = Box::pin(registry.publish_transition(
        observed,
        vec![plugin("plugin", HookPriority::Normal)],
        vec![user("user", HookPriority::Normal)],
    ));
    // Poll once while the read guard holds the gate: the writer must queue.
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(
            publication.as_mut().poll(cx).is_pending()
        ))
        .await
    );
    assert!(registry.try_read().is_err());
    assert_eq!(read.hook_count(), 0);
    drop(read);
    publication.await.unwrap();
    let read = registry.try_read().unwrap();
    assert_eq!(read.hook_count(), 2);
    assert_eq!(outputs(&read).await, ["plugin", "user"]);
    assert_eq!(registry.begin_transition(), observed + 1);
    assert_eq!(registry.revision(), 1);
}

#[tokio::test]
async fn trusted_admission_is_visible_and_rejected_actors_never_execute() {
    let registry = HookRegistry::new(HookExecutor::empty());
    let trusted_builtin = HookInput {
        config: prompt("core", HookPriority::System),
        origin: HookOrigin::Builtin {
            name: "core-test".into(),
        },
    };
    registry
        .publish_transition(
            registry.begin_transition(),
            vec![
                plugin("plugin-system", HookPriority::System),
                plugin("plugin-high", HookPriority::High),
                trusted_builtin,
            ],
            vec![
                user("builtin", HookPriority::System),
                user("user-high", HookPriority::High),
            ],
        )
        .await
        .unwrap();
    let snapshot = registry.snapshot();
    let rows = snapshot.value.executor.inventory();
    for source in ["plugin-system", "builtin"] {
        let row = rows.iter().find(|r| r.source == source).unwrap();
        assert!(!row.reachable);
        assert_eq!(row.admission.as_deref(), Some("rejected"));
        assert_eq!(row.requested_priority.as_deref(), Some("system"));
        assert!(row.effective_priority.is_none());
        assert!(row.admission_reason.as_ref().is_some_and(|r| !r.is_empty()));
        assert!(row.issue.as_ref().is_some_and(|r| !r.is_empty()));
    }
    let downgraded = rows.iter().find(|r| r.source == "plugin-high").unwrap();
    assert_eq!(downgraded.priority, "normal");
    assert_eq!(downgraded.requested_priority.as_deref(), Some("high"));
    assert_eq!(downgraded.effective_priority.as_deref(), Some("normal"));
    assert_eq!(downgraded.admission.as_deref(), Some("downgraded"));
    assert!(downgraded.admission_reason.is_some());
    assert_eq!(
        downgraded.source_identity.as_ref().unwrap()["plugin_id"],
        "owning-row"
    );
    let high = rows.iter().find(|r| r.source == "user-high").unwrap();
    assert_eq!(high.effective_priority.as_deref(), Some("high"));
    assert_eq!(
        outputs(&snapshot.value.executor).await,
        ["core", "user-high", "plugin-high"]
    );
}

#[tokio::test]
async fn content_identity_survives_reorder_and_duplicates_keep_their_effects() {
    let registry = HookRegistry::new(HookExecutor::empty());
    let one = user("one", HookPriority::Normal);
    let two = user("two", HookPriority::Normal);
    registry
        .replace_users(vec![one.clone(), two.clone(), one.clone()])
        .await
        .unwrap();
    let first = registry.snapshot();
    let ids: Vec<_> = first
        .value
        .executor
        .inventory()
        .into_iter()
        .map(|r| r.hook_id.unwrap())
        .collect();
    assert_eq!(ids[0], ids[2]);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(outputs(&first.value.executor).await, ["one", "two", "one"]);
    registry
        .replace_users(vec![two, one.clone(), one.clone()])
        .await
        .unwrap();
    let ids2: Vec<_> = registry
        .read()
        .await
        .inventory()
        .into_iter()
        .map(|r| r.hook_id.unwrap())
        .collect();
    assert_eq!(ids2, [ids[1].clone(), ids[0].clone(), ids[0].clone()]);
    let mut renamed = one.clone();
    renamed.config.plugin_name = "pure-log-label".into();
    assert_eq!(admit(renamed).unwrap().metadata.hook_id, ids[0]);
    let mut changed = one.clone();
    changed.config.timeout_secs = Some(42);
    assert_ne!(admit(changed).unwrap().metadata.hook_id, ids[0]);
    let project_a = HookInput::user(
        prompt("same-label", HookPriority::Normal),
        Path::new("/project-a/.aleph/hooks.json"),
        &ScopeKey::project(Path::new("/project-a")),
    );
    let project_b = HookInput::user(
        prompt("same-label", HookPriority::Normal),
        Path::new("/project-b/.aleph/hooks.json"),
        &ScopeKey::project(Path::new("/project-b")),
    );
    assert_ne!(
        admit(project_a).unwrap().metadata.hook_id,
        admit(project_b).unwrap().metadata.hook_id
    );
    let mut changed_origin = one;
    changed_origin.origin = HookOrigin::User {
        config_path: PathBuf::from("/other/hooks.json"),
        scope: ScopeKey::Global,
    };
    assert_ne!(admit(changed_origin).unwrap().metadata.hook_id, ids[0]);
}

#[tokio::test]
async fn close_refuses_both_layer_writes_without_revision_or_events() {
    let registry = HookRegistry::new(HookExecutor::empty());
    registry
        .replace_users(vec![user("still-readable", HookPriority::Normal)])
        .await
        .unwrap();
    let captured = registry.snapshot();
    let mut events = registry.subscribe();
    let signal = registry.close_signal();
    let notified = signal.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    registry.close();
    notified.await;
    assert!(registry.is_closed());
    assert!(matches!(
        registry
            .replace_users(vec![user("never", HookPriority::Normal)])
            .await,
        Err(HookRegistryError::Closed)
    ));
    assert!(matches!(
        registry
            .publish_transition(0, vec![plugin("never", HookPriority::Normal)], Vec::new())
            .await,
        Err(HookRegistryError::Closed)
    ));
    assert_eq!(registry.revision(), 1);
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert!(registry.snapshot().closed);
    assert!(!captured.closed);
    assert_eq!(outputs(&captured.value.executor).await, ["still-readable"]);
    assert_eq!(outputs(&*registry.read().await).await, ["still-readable"]);
    registry.close();
    assert_eq!(registry.revision(), 1);
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn unrepresentable_identity_fails_closed_without_publication() {
    use std::os::unix::ffi::OsStringExt;
    let registry = HookRegistry::new(HookExecutor::empty());
    let mut events = registry.subscribe();
    let mut input = user("never", HookPriority::Normal);
    input.origin = HookOrigin::User {
        config_path: PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])),
        scope: ScopeKey::Global,
    };
    assert!(matches!(
        registry.replace_users(vec![input]).await,
        Err(HookRegistryError::Identity(_))
    ));
    assert_eq!(registry.revision(), 0);
    assert!(events.try_recv().is_err());
    assert_eq!(registry.read().await.hook_count(), 0);
}

struct Memo(Mutex<Vec<String>>);
#[async_trait::async_trait]
impl HookMemoSink for Memo {
    async fn record(&self, _event: HookEvent, _capability: Option<&str>, outcome: &str) {
        self.0.lock().unwrap().push(outcome.into());
    }
}

#[tokio::test]
async fn rebuild_preserves_template_resources_and_live_consent() {
    let dir = tempfile::tempdir().unwrap();
    let consent = Arc::new(ShellHookConsent::with_path(dir.path().join("consent.json")));
    let memo = Arc::new(Memo(Mutex::new(Vec::new())));
    let template = HookExecutor::new(vec![prompt(
        "template-data-must-not-register",
        HookPriority::Normal,
    )])
    .with_timeout(Duration::from_secs(17))
    .with_consent(Arc::clone(&consent))
    .with_memo_sink(memo.clone());
    let registry = HookRegistry::new(template);
    assert_eq!(registry.read().await.hook_count(), 0);
    let mut command = user("command", HookPriority::Normal);
    command.config.plugin_root = canonical_root(dir.path());
    command.config.actions = vec![HookAction::Command {
        command: "echo must-not-run-without-consent".into(),
    }];
    registry
        .replace_users(vec![user("memo-effect", HookPriority::Normal), command])
        .await
        .unwrap();
    let snapshot = registry.snapshot();
    assert_eq!(
        snapshot.value.executor.command_timeout,
        Duration::from_secs(17)
    );
    assert_eq!(outputs(&snapshot.value.executor).await, ["memo-effect"]);
    assert!(!memo.0.lock().unwrap().is_empty());
    let entry = consent
        .entries()
        .into_iter()
        .find(|e| e.plugin_name == "command")
        .unwrap();
    assert_eq!(
        snapshot
            .value
            .executor
            .inventory()
            .iter()
            .find(|r| r.source == "command")
            .unwrap()
            .consent
            .as_deref(),
        Some("pending")
    );
    let reviewed_root = canonical_root(dir.path());
    assert!(consent
        .approve(&entry.fingerprint, Some(&reviewed_root))
        .unwrap()
        .is_some());
    assert_eq!(
        snapshot
            .value
            .executor
            .inventory()
            .iter()
            .find(|r| r.source == "command")
            .unwrap()
            .consent
            .as_deref(),
        Some("approved")
    );
    let recorded = memo.0.lock().unwrap().len();
    registry
        .replace_users(vec![user("next-memo-effect", HookPriority::Normal)])
        .await
        .unwrap();
    let next = registry.snapshot();
    assert_eq!(next.value.executor.command_timeout, Duration::from_secs(17));
    assert_eq!(outputs(&next.value.executor).await, ["next-memo-effect"]);
    assert_eq!(memo.0.lock().unwrap().len(), recorded + 1);
    assert_eq!(
        snapshot.value.executor.hook_count(),
        2,
        "captured data stays immutable"
    );
}

#[tokio::test]
async fn legacy_mutations_never_misalign_registered_metadata() {
    let registry = HookRegistry::new(HookExecutor::empty());
    registry
        .replace_users(vec![user("registered", HookPriority::Normal)])
        .await
        .unwrap();
    let mut executor = registry.read().await.clone();
    executor.add_hook(prompt("legacy", HookPriority::Normal));
    executor.remove_by_plugin_prefix("registered");
    let row = executor.inventory().pop().unwrap();
    assert_eq!(row.source, "legacy");
    assert!(row.hook_id.is_none());
    assert!(row.source_identity.is_none());
    assert!(row.requested_priority.is_none());
    assert!(row.effective_priority.is_none());
    assert!(row.admission.is_none());
    assert!(row.admission_reason.is_none());
}

#[tokio::test]
async fn registered_loader_captures_each_real_file_and_preserves_public_configs() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let dir = tempfile::tempdir().unwrap();
    let global = crate::utils::paths::get_config_dir()
        .unwrap()
        .join("hooks.json");
    let project_a = dir.path().join("project-a");
    let project_b = dir.path().join("project-b");
    let files = [
        global,
        project_a.join(".aleph/hooks.json"),
        project_a.join(".aleph/hooks.local.json"),
        project_b.join(".aleph/hooks.json"),
        project_b.join(".aleph/hooks.local.json"),
    ];
    let document = r#"{"hooks":{"before_agent_start":[{"priority":"high","hooks":[{"type":"prompt","prompt":"same-content"}]}]}}"#;
    for file in &files {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, document).unwrap();
    }
    // Repeating the discovery root must not load the same file twice.
    let roots = [project_a.clone(), project_b.clone(), project_a.clone()];
    let inputs = crate::extension::hooks::load_registered_user_hooks(None, &roots);
    let public = crate::extension::hooks::load_user_hooks(None, &roots);
    assert_eq!(inputs.len(), files.len());
    assert_eq!(public.len(), inputs.len());
    for ((input, config), file) in inputs.iter().zip(&public).zip(&files) {
        assert_eq!(
            serde_json::to_value(&input.config).unwrap(),
            serde_json::to_value(config).unwrap()
        );
        assert_eq!(input.config.plugin_root, config.plugin_root);
        let expected_scope = if file == &files[0] {
            ScopeKey::Global
        } else if file.starts_with(&project_a) {
            ScopeKey::project(&project_a)
        } else {
            ScopeKey::project(&project_b)
        };
        match &input.origin {
            HookOrigin::User { config_path, scope } => {
                assert_eq!(config_path, &canonical_root(file));
                assert_eq!(scope, &expected_scope);
                assert_eq!(input.config.scope_key, expected_scope);
            }
            _ => panic!("a disk user file cannot become a plugin or builtin producer"),
        }
    }
    assert_eq!(inputs[1].config.plugin_name, inputs[3].config.plugin_name);
    let registry = HookRegistry::new(HookExecutor::empty());
    registry.replace_users(inputs).await.unwrap();
    let rows = registry.read().await.inventory();
    assert_eq!(rows.len(), files.len());
    let ids: std::collections::HashSet<_> = rows
        .iter()
        .map(|row| row.hook_id.as_ref().unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        files.len(),
        "equal labels/content in distinct real files remain distinct"
    );
    for row in &rows {
        assert_eq!(row.source_identity.as_ref().unwrap()["kind"], "user");
        assert_eq!(row.admission.as_deref(), Some("accepted"));
        assert_eq!(row.effective_priority.as_deref(), Some("high"));
    }
}

#[tokio::test]
async fn plugin_collection_uses_active_owning_row_identity_despite_builtin_spelling() {
    use crate::extension::{ExtensionConfig, ExtensionManager, HookRegistration};
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: crate::discovery::DiscoveryConfig {
            working_dir: dir.path().to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
        plugins_config_path: Some(dir.path().join("plugins.toml")),
        extra_plugin_parents: Vec::new(),
    })
    .await
    .unwrap();
    let mut owner = PluginRecord::new(
        "builtin".into(),
        "user:global".into(),
        PluginKind::Static,
        PluginOrigin::Global,
    );
    owner.root_dir = dir.path().to_path_buf();
    owner.scope_key = ScopeKey::project(dir.path());
    let registration = |priority, plugin_id: &str| HookRegistration {
        event: HookEvent::BeforeAgentStart,
        priority,
        handler: "retained-handler".into(),
        name: Some("display-only".into()),
        description: None,
        plugin_id: plugin_id.into(),
        kind: Some(HookKind::Interceptor),
        matcher: None,
        actions: vec![HookAction::Prompt {
            prompt: "plugin-effect".into(),
        }],
        plugin_root: Some(dir.path().join("execution-root")),
        timeout_secs: Some(29),
        declared_event: Some("before_agent_start".into()),
    };
    {
        let mut registry = manager.plugin_registry.write().await;
        registry.register_plugin(owner.clone());
        registry.register_hook(registration(HookPriority::High.as_i32(), &owner.id));
        registry.register_hook(registration(HookPriority::System.as_i32(), &owner.id));
        registry.register_hook(registration(HookPriority::Normal.as_i32(), "no-owning-row"));
    }
    let inputs = manager.views().sync_hooks_from_registry().await;
    assert_eq!(
        inputs.len(),
        2,
        "an orphan registration supplies no trusted origin"
    );
    for input in &inputs {
        assert_eq!(input.config.handler.as_deref(), Some("retained-handler"));
        assert_eq!(input.config.timeout_secs, Some(29));
        assert_eq!(
            input.config.declared_event.as_deref(),
            Some("before_agent_start")
        );
        assert_eq!(input.config.plugin_root, dir.path().join("execution-root"));
        match &input.origin {
            HookOrigin::Plugin {
                plugin_id,
                scope,
                plugin_root,
            } => {
                assert_eq!(plugin_id, &owner.id);
                assert_eq!(scope, &owner.scope_key);
                assert_eq!(plugin_root, &canonical_root(&owner.root_dir));
            }
            _ => panic!("plugin id/display spelling must never produce builtin or user trust"),
        }
    }
    let registry = HookRegistry::new(HookExecutor::empty());
    registry
        .publish_transition(0, inputs, Vec::new())
        .await
        .unwrap();
    let rows = registry.read().await.inventory();
    let high = rows
        .iter()
        .find(|row| row.requested_priority.as_deref() == Some("high"))
        .unwrap();
    assert_eq!(high.admission.as_deref(), Some("downgraded"));
    assert_eq!(high.effective_priority.as_deref(), Some("normal"));
    let system = rows
        .iter()
        .find(|row| row.requested_priority.as_deref() == Some("system"))
        .unwrap();
    assert_eq!(system.admission.as_deref(), Some("rejected"));
    assert!(!system.reachable);
    assert!(system.effective_priority.is_none());
}

#[test]
fn identity_canonicalizes_action_maps_and_execution_roots() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = user("same-content", HookPriority::Normal);
    first.config.plugin_root = dir.path().to_path_buf();
    first.config.actions = vec![HookAction::Http {
        url: "https://example.invalid/hook".into(),
        headers: [("z".into(), "last".into()), ("a".into(), "first".into())]
            .into_iter()
            .collect(),
    }];
    let mut reordered = first.clone();
    reordered.config.plugin_root = dir.path().join(".");
    reordered.config.actions = vec![HookAction::Http {
        url: "https://example.invalid/hook".into(),
        headers: [("a".into(), "first".into()), ("z".into(), "last".into())]
            .into_iter()
            .collect(),
    }];
    let id = admit(first.clone()).unwrap().metadata.hook_id;
    assert_eq!(admit(reordered).unwrap().metadata.hook_id, id);
    let mut moved = first;
    moved.config.plugin_root = dir.path().join("different-root");
    assert_ne!(
        admit(moved).unwrap().metadata.hook_id,
        id,
        "serde-skipped execution root participates in identity"
    );
}

#[tokio::test]
async fn inventory_sort_and_legacy_removal_keep_metadata_attached_to_its_declaration() {
    let registry = HookRegistry::new(HookExecutor::empty());
    registry
        .replace_users(vec![
            user("low", HookPriority::Low),
            user("high", HookPriority::High),
        ])
        .await
        .unwrap();
    let captured = registry.snapshot();
    let rows = captured.value.executor.inventory();
    for row in &rows {
        assert_eq!(row.source, row.requested_priority.as_deref().unwrap());
        assert_eq!(
            row.hook_id.as_ref().unwrap(),
            &admit(user(
                &row.source,
                if row.source == "high" {
                    HookPriority::High
                } else {
                    HookPriority::Low
                }
            ))
            .unwrap()
            .metadata
            .hook_id
        );
    }
    let high_id = rows
        .iter()
        .find(|row| row.source == "high")
        .unwrap()
        .hook_id
        .clone();
    let mut legacy = captured.value.executor.clone();
    assert_eq!(legacy.remove_by_plugin_prefix("low"), 1);
    legacy.add_hook(prompt("manual", HookPriority::Normal));
    let remaining = legacy.inventory();
    assert_eq!(
        remaining
            .iter()
            .find(|row| row.source == "high")
            .unwrap()
            .hook_id,
        high_id
    );
    assert!(remaining
        .iter()
        .find(|row| row.source == "manual")
        .unwrap()
        .hook_id
        .is_none());
    assert_eq!(captured.value.executor.hook_count(), 2);
    assert_eq!(outputs(&captured.value.executor).await, ["high", "low"]);
}
