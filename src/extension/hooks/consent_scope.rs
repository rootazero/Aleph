//! Project-hook consent is keyed by the project (fire-site tests, 判据 §4).
//!
//! Every project's `.aleph/hooks.json` loads under the one label
//! `user:project`, and a byte-identical template in another repo resolves
//! `${CLAUDE_PLUGIN_ROOT}` to that repo. These tests drive the real loader
//! (or, for a plugin, the hook `sync_hooks_from_registry` would stamp), the
//! real executor and a real consent registry: one template, checked into two
//! projects, approved in one.

use super::ShellHookConsent;
use super::{load_user_hooks, ConsentEntry, ConsentStatus, HookContext, HookExecutor};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::visibility::{canonical_root, ScopeKey};
use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

/// The template both repos ship — CC's own idiom, a script beside the hook
/// file reached through the plugin root. The script leaves `ran` in the
/// directory it runs in: the hook's root.
const TEMPLATE: &str = r#"sh "${CLAUDE_PLUGIN_ROOT}/hooks/lint.sh""#;

/// `hook_root/hooks/lint.sh`, the code `TEMPLATE` runs from `hook_root`.
fn write_script(hook_root: &Path) {
    std::fs::create_dir_all(hook_root.join("hooks")).unwrap();
    std::fs::write(hook_root.join("hooks/lint.sh"), "touch ran\n").unwrap();
}

/// A project that checks in `TEMPLATE` as a `PreToolUse` hook.
fn write_project(root: &Path) {
    write_script(&root.join(".aleph"));
    let file = serde_json::json!({ "hooks": { "PreToolUse": [
        { "hooks": [{ "type": "command", "command": TEMPLATE }] }
    ] } });
    std::fs::write(root.join(".aleph/hooks.json"), file.to_string()).unwrap();
}

/// The project hooks the loader produces for `roots`. The user-global layer
/// reads this machine's real config dir, so it is left out, as the loader's
/// own tests do.
fn project_hooks(roots: &[PathBuf]) -> Vec<HookConfig> {
    load_user_hooks(None, roots)
        .into_iter()
        .filter(|h| h.scope_key != ScopeKey::Global)
        .collect()
}

/// A hook from a plugin found under `project`, as `sync_hooks_from_registry`
/// stamps it: the plugin's id, its directory inside the project, and the
/// owning row's `Project(root)` key.
fn project_plugin_hook(project: &Path) -> HookConfig {
    plugin_hook(
        project.join(".aleph/plugins/fmt"),
        ScopeKey::project(project),
    )
}

/// The plugin `fmt`'s `TEMPLATE` hook, installed at `plugin_root` and
/// visible to `scope`.
fn plugin_hook(plugin_root: PathBuf, scope_key: ScopeKey) -> HookConfig {
    write_script(&plugin_root);
    HookConfig {
        event: HookEvent::BeforeToolCall,
        kind: HookKind::Interceptor,
        priority: Default::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: TEMPLATE.into(),
        }],
        plugin_name: "fmt".into(),
        plugin_root,
        handler: None,
        timeout_secs: None,
        declared_event: Some("PreToolUse".into()),
        scope_key,
    }
}

fn consent_in(dir: &Path) -> Arc<ShellHookConsent> {
    Arc::new(ShellHookConsent::with_path(dir.join("allowlist.json")))
}

/// Fire `PreToolUse` while `project` is the session's project.
async fn fire_in(exec: &HookExecutor, project: &Path) {
    crate::projects::with_project_root(Some(project.to_path_buf()), async {
        exec.execute_interceptors(
            HookEvent::BeforeToolCall,
            HookContext::new("s").with_tool_name("bash"),
        )
        .await
        .expect("a skipped hook is not an error");
    })
    .await;
}

/// The entry recorded for `project`'s copy, if any.
fn entry_for(consent: &ShellHookConsent, project: &Path) -> Option<ConsentEntry> {
    let want = canonical_root(project);
    consent
        .entries()
        .into_iter()
        .find(|e| e.project_root.as_deref() == Some(want.as_path()))
}

/// What the two copies did: fire in `a`, approve the entry that fire
/// recorded — whatever it is keyed by — fire in `a` again, then in `b`.
/// Returns `(a ran, b ran)` and leaves the registry for the caller.
async fn approve_in_a_then_fire_in_b(
    exec: &HookExecutor,
    consent: &ShellHookConsent,
    (a, a_hook_root): (&Path, &Path),
    (b, b_hook_root): (&Path, &Path),
) -> (bool, bool) {
    fire_in(exec, a).await;
    assert!(
        !a_hook_root.join("ran").exists(),
        "unapproved: must not run"
    );
    let recorded = consent.entries();
    assert_eq!(recorded.len(), 1, "the first fire records one entry");
    consent
        .approve(&recorded[0].fingerprint, recorded[0].plugin_root.as_deref())
        .unwrap();

    fire_in(exec, a).await;
    fire_in(exec, b).await;
    (
        a_hook_root.join("ran").exists(),
        b_hook_root.join("ran").exists(),
    )
}

#[tokio::test]
async fn approving_a_project_hook_in_one_repo_leaves_the_same_template_in_another_pending() {
    let (a, b, state) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    write_project(a.path());
    write_project(b.path());
    let hooks = project_hooks(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    assert_eq!(hooks.len(), 2, "one hook per repo: {hooks:?}");
    assert!(
        hooks
            .iter()
            .all(|h| h.plugin_name == "user:project" && h.actions.len() == 1),
        "both copies load under the one shared label"
    );
    let consent = consent_in(state.path());
    let exec = HookExecutor::new(hooks).with_consent(consent.clone());

    let (ran_a, ran_b) = approve_in_a_then_fire_in_b(
        &exec,
        &consent,
        (a.path(), &a.path().join(".aleph")),
        (b.path(), &b.path().join(".aleph")),
    )
    .await;

    assert!(ran_a, "approved under a: runs under a");
    assert!(!ran_b, "a's approval ran b's lint.sh");
    // Each copy is its own entry, named by its project, so `aleph-server hooks
    // test` can say which repo it is approving.
    let a_entry = entry_for(&consent, a.path()).expect("a's copy is recorded under a");
    let b_entry = entry_for(&consent, b.path()).expect("b's copy is recorded under b");
    assert_eq!(a_entry.status, ConsentStatus::Approved);
    assert_eq!(b_entry.status, ConsentStatus::Pending);
    assert_eq!(b_entry.plugin_name, "user:project");
}

/// The same bypass through a plugin: another repo can ship a plugin with the
/// same id, and its hook's `${CLAUDE_PLUGIN_ROOT}` is inside that repo.
#[tokio::test]
async fn a_project_scoped_plugins_hook_is_keyed_by_its_project_too() {
    let (a, b, state) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (hook_a, hook_b) = (project_plugin_hook(a.path()), project_plugin_hook(b.path()));
    let (root_a, root_b) = (hook_a.plugin_root.clone(), hook_b.plugin_root.clone());
    let consent = consent_in(state.path());
    let exec = HookExecutor::new(vec![hook_a, hook_b]).with_consent(consent.clone());

    let (ran_a, ran_b) =
        approve_in_a_then_fire_in_b(&exec, &consent, (a.path(), &root_a), (b.path(), &root_b))
            .await;

    assert!(ran_a, "approved under a: runs under a");
    assert!(!ran_b, "a's approval ran b's copy of the plugin");
    // What tells project keying apart from root binding: the two plugin
    // roots differ, so binding the approval to its root alone would also
    // keep b from running. Only the project key gives each copy its own
    // entry — the thing a root-less approval cannot follow across projects.
    let a_entry = entry_for(&consent, a.path()).expect("a's copy is recorded under a");
    let b_entry = entry_for(&consent, b.path()).expect("b's copy is recorded under b");
    assert_eq!(a_entry.status, ConsentStatus::Approved);
    assert_eq!(b_entry.status, ConsentStatus::Pending);
}

/// The Http twin of the command gate: approving a project's URL template
/// does not let another project's copy POST its events. Nothing listens on
/// the port, so an approved hook fails its request instead of being skipped.
#[tokio::test]
async fn a_project_http_hooks_approval_is_its_projects_alone() {
    let (a, b, state) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let http_hook = |project: &Path| HookConfig {
        plugin_name: "user:project".into(),
        plugin_root: project.join(".aleph"),
        actions: vec![HookAction::Http {
            url: "http://127.0.0.1:9/lint".into(),
            headers: Default::default(),
        }],
        ..project_plugin_hook(project)
    };
    let consent = consent_in(state.path());
    let exec = HookExecutor::new(vec![http_hook(a.path()), http_hook(b.path())])
        .with_consent(consent.clone());

    fire_in(&exec, a.path()).await;
    let recorded = consent.entries();
    assert_eq!(recorded.len(), 1, "the first fire records one entry");
    consent
        .approve(&recorded[0].fingerprint, recorded[0].plugin_root.as_deref())
        .unwrap();
    let in_b = crate::projects::with_project_root(Some(b.path().to_path_buf()), async {
        exec.execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("a skipped hook is not an error")
            .1
    })
    .await;

    let error = in_b.action_results[0].error.as_deref().unwrap_or_default();
    assert!(
        error.contains("is not approved"),
        "a's approval let b's copy POST: {error}"
    );
    let b_entry = entry_for(&consent, b.path()).expect("b's copy is recorded under b");
    assert_eq!(b_entry.status, ConsentStatus::Pending);
}

/// An approval recorded before approvals were bound to a project — keyed by
/// the shared label alone, in the registry's old shape — authorises the hook
/// in no project. It stays on disk, untouched.
#[tokio::test]
async fn a_shared_label_approval_from_before_the_binding_authorises_no_project() {
    let (a, b, state) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    write_project(a.path());
    write_project(b.path());
    let registry = state.path().join("allowlist.json");
    let legacy_fp = ShellHookConsent::fingerprint("user:project", None, TEMPLATE);
    let legacy = serde_json::json!({ "version": 1, "entries": [{
        "fingerprint": legacy_fp, "plugin_name": "user:project",
        "command": TEMPLATE, "event": "PreToolUse",
        "status": "approved", "first_seen": 1, "approved_at": 2
    }] });
    std::fs::write(&registry, legacy.to_string()).unwrap();
    let consent = consent_in(state.path());
    let hooks = project_hooks(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let exec = HookExecutor::new(hooks).with_consent(consent.clone());

    fire_in(&exec, a.path()).await;
    fire_in(&exec, b.path()).await;

    assert!(
        !a.path().join(".aleph/ran").exists(),
        "legacy approval ran a"
    );
    assert!(
        !b.path().join(".aleph/ran").exists(),
        "legacy approval ran b"
    );
    for project in [a.path(), b.path()] {
        let entry = entry_for(&consent, project).expect("recorded under its project");
        assert_eq!(entry.status, ConsentStatus::Pending);
    }
    let kept = consent
        .entries()
        .into_iter()
        .find(|e| e.fingerprint == legacy_fp)
        .expect("the old entry is kept");
    assert_eq!(kept.status, ConsentStatus::Approved);
    assert!(kept.predates_project_binding());
}

/// Within one key an approval attests to the root it was reviewed from. The
/// same plugin id installed somewhere else — same owner, same command, same
/// key — is refused until the operator revokes the approval and reviews the
/// new root, which the next fire records.
#[tokio::test]
async fn an_approval_is_bound_to_the_root_it_was_reviewed_from() {
    let (first, second, state) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let consent = consent_in(state.path());
    let installed_at = |root: &Path| {
        HookExecutor::new(vec![plugin_hook(root.to_path_buf(), ScopeKey::Global)])
            .with_consent(consent.clone())
    };
    let (at_first, at_second) = (installed_at(first.path()), installed_at(second.path()));

    fire_in(&at_first, first.path()).await;
    let recorded = consent.entries().remove(0);
    consent
        .approve(&recorded.fingerprint, recorded.plugin_root.as_deref())
        .unwrap();
    fire_in(&at_first, first.path()).await;
    assert!(
        first.path().join("ran").exists(),
        "approved: runs from its root"
    );

    fire_in(&at_second, second.path()).await;
    assert!(
        !second.path().join("ran").exists(),
        "an approval of the first root ran the second"
    );

    // The way back: revoke, let it fire, review what that recorded, approve.
    consent.revoke(&recorded.fingerprint).unwrap();
    fire_in(&at_second, second.path()).await;
    let refreshed = consent.entries().remove(0);
    assert_eq!(
        refreshed.plugin_root.as_deref(),
        Some(second.path()),
        "the next fire records the root it ran from"
    );
    consent
        .approve(&refreshed.fingerprint, refreshed.plugin_root.as_deref())
        .unwrap();
    fire_in(&at_second, second.path()).await;
    assert!(second.path().join("ran").exists());
}

// ── The script's content (review Q1) ──────────────────────────────────────
//
// `TEMPLATE` names its script through `${CLAUDE_PLUGIN_ROOT}`, the way a
// Claude Code hook does. Until the consent resolver learned the path
// variables, such an approval bound no content at all.

/// Approve, then rewrite the script — a `git pull` — and fire again: the
/// approval was for the old content, so the new script does not run and the
/// hook reads as pending.
#[tokio::test]
async fn an_approved_hook_whose_script_is_rewritten_does_not_run() {
    let (repo, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_project(repo.path());
    let hook_root = repo.path().join(".aleph");
    let consent = consent_in(state.path());
    let exec = HookExecutor::new(project_hooks(&[repo.path().to_path_buf()]))
        .with_consent(consent.clone());

    fire_in(&exec, repo.path()).await;
    let recorded = consent.entries().remove(0);
    consent
        .approve(&recorded.fingerprint, recorded.plugin_root.as_deref())
        .unwrap();
    fire_in(&exec, repo.path()).await;
    assert!(hook_root.join("ran").exists(), "approved: runs");

    std::fs::remove_file(hook_root.join("ran")).unwrap();
    std::fs::write(
        hook_root.join("hooks/lint.sh"),
        "touch ran; touch rewritten\n",
    )
    .unwrap();
    fire_in(&exec, repo.path()).await;

    assert!(
        !hook_root.join("rewritten").exists(),
        "the rewritten script ran under the old approval"
    );
    assert!(!hook_root.join("ran").exists());
    assert_eq!(exec.inventory()[0].consent.as_deref(), Some("pending"));
}

/// An approval given while the script could not be hashed — every approval
/// of a `${…}` script before the resolver learned the path variables —
/// attests to no content. Once the script can be hashed it is refused, and
/// the fire withdraws it to pending, so `aleph-server hooks test` reviews and binds
/// what runs now.
#[tokio::test]
async fn an_approval_that_attests_to_no_script_is_withdrawn_once_the_script_can_be_hashed() {
    let (repo, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_project(repo.path());
    let hook_root = repo.path().join(".aleph");
    let project = canonical_root(repo.path());
    // The entry a P4.4d-era approval of this hook left behind: bound to the
    // project and the root, with no script fingerprint.
    let fp = ShellHookConsent::fingerprint("user:project", Some(&project), TEMPLATE);
    let unbound = serde_json::json!({ "version": 1, "entries": [{
        "fingerprint": fp, "plugin_name": "user:project",
        "project_root": project, "command": TEMPLATE, "event": "PreToolUse",
        "plugin_root": hook_root, "status": "approved",
        "first_seen": 1, "approved_at": 2
    }] });
    std::fs::write(state.path().join("allowlist.json"), unbound.to_string()).unwrap();
    let consent = consent_in(state.path());
    let exec = HookExecutor::new(project_hooks(&[repo.path().to_path_buf()]))
        .with_consent(consent.clone());

    fire_in(&exec, repo.path()).await;
    assert!(
        !hook_root.join("ran").exists(),
        "an approval that attests to no content ran the script"
    );
    // Found by its key, not by position: the entry under test is the seeded
    // one, whatever else a fire recorded.
    let entry = consent
        .entries()
        .into_iter()
        .find(|e| e.fingerprint == fp)
        .expect("the seeded entry is kept");
    assert_eq!(entry.status, ConsentStatus::Pending, "withdrawn for review");
    assert!(entry.script_fingerprint.is_some());

    consent
        .approve(&entry.fingerprint, entry.plugin_root.as_deref())
        .unwrap();
    fire_in(&exec, repo.path()).await;
    assert!(hook_root.join("ran").exists(), "re-approved: runs");
}
