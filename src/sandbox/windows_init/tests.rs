//! Cross-platform unit tests for the `sandbox-init-windows` policy,
//! argv parser, capability translation, and protected-metadata
//! classifier. None of these touch Win32, so they run on macOS / Linux
//! dev boxes as well as Windows.

use super::args::parse_init_args;
use super::policy::{
    capability_names_for_network, classify_protected_metadata, WindowsInitPolicy,
    DACL_INHERIT_FLAGS_FOR_APPCONTAINER, DACL_SERIALIZATION_MUTEX_NAME,
};

#[test]
fn policy_round_trips_through_json() {
    let original = WindowsInitPolicy {
        require_restricted_token: true,
        use_app_container: true,
        require_app_container: false,
        app_container_capabilities: vec!["internetClient".to_string()],
        workspace_path: Some("C:\\workspace\\session-abc".to_string()),
        deny_read_globs: vec!["**/.env".to_string(), "**/*.pem".to_string()],
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: WindowsInitPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn policy_default_disables_all_strict_flags() {
    let p = WindowsInitPolicy::default();
    assert!(!p.require_restricted_token);
    assert!(!p.use_app_container);
    assert!(!p.require_app_container);
    assert!(p.app_container_capabilities.is_empty());
    assert!(p.workspace_path.is_none());
    assert!(p.deny_read_globs.is_empty());
}

#[test]
fn policy_accepts_missing_deny_read_globs_via_serde_default() {
    // Forward-compat: an older driver that omits the field still
    // deserializes — deny_read_globs defaults to empty (no floor).
    let parsed: WindowsInitPolicy = serde_json::from_str(r#"{"workspace_path":"C:\\ws"}"#).unwrap();
    assert!(parsed.deny_read_globs.is_empty());
}

#[test]
fn policy_accepts_missing_require_flag_via_serde_default() {
    // Spec § 2.5 promises the policy struct is forward-compatible:
    // a JSON like `{}` deserializes with default values.
    let parsed: WindowsInitPolicy = serde_json::from_str("{}").unwrap();
    assert!(!parsed.require_restricted_token);
}

#[test]
fn parse_init_args_extracts_policy_and_target() {
    let policy = WindowsInitPolicy {
        require_restricted_token: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&policy).unwrap();
    let argv = vec![
        "--policy".to_string(),
        json,
        "--".to_string(),
        "C:\\Windows\\System32\\cmd.exe".to_string(),
        "/c".to_string(),
        "echo hi".to_string(),
    ];
    let parsed = parse_init_args(&argv).unwrap();
    assert_eq!(parsed.policy, policy);
    assert_eq!(parsed.target, "C:\\Windows\\System32\\cmd.exe");
    assert_eq!(parsed.target_args, vec!["/c", "echo hi"]);
}

#[test]
fn parse_init_args_rejects_missing_policy() {
    let argv = vec!["--".to_string(), "cmd.exe".to_string()];
    let err = parse_init_args(&argv).unwrap_err();
    assert!(err.contains("missing --policy"), "got: {err}");
}

#[test]
fn parse_init_args_rejects_missing_target() {
    let argv = vec!["--policy".to_string(), "{}".to_string(), "--".to_string()];
    let err = parse_init_args(&argv).unwrap_err();
    assert!(err.contains("missing target"), "got: {err}");
}

#[test]
fn capability_names_for_allow_all() {
    let names =
        capability_names_for_network(&crate::sandbox::capabilities::NetworkPolicy::AllowAll);
    assert_eq!(
        names,
        vec![
            "internetClient".to_string(),
            "privateNetworkClientServer".to_string(),
        ]
    );
}

#[test]
fn capability_names_for_none_returns_empty() {
    let names = capability_names_for_network(&crate::sandbox::capabilities::NetworkPolicy::None);
    assert!(names.is_empty());
}

#[test]
fn capability_names_for_allow_hosts_returns_empty() {
    // AllowHosts is rejected at profile_for; if we somehow reach here
    // we're conservative and grant nothing.
    let names =
        capability_names_for_network(&crate::sandbox::capabilities::NetworkPolicy::AllowHosts {
            hosts: vec!["github.com".to_string()],
        });
    assert!(names.is_empty());
}

#[test]
fn parse_init_args_rejects_bad_json() {
    let argv = vec![
        "--policy".to_string(),
        "not json".to_string(),
        "--".to_string(),
        "cmd.exe".to_string(),
    ];
    let err = parse_init_args(&argv).unwrap_err();
    assert!(err.contains("JSON parse error"), "got: {err}");
}

#[test]
fn dacl_serialization_mutex_name_is_session_local() {
    // `Local\` (not `Global\`) so a standard user without
    // SeCreateGlobalPrivilege can still create the mutex; the leading
    // scope and the stable suffix are what every concurrent init must
    // agree on, so pin both. Drift here silently de-serializes the DACL
    // read-modify-write and reopens the multi-agent lost-update race
    // that can drop a per-execution deny ACE on `.git`.
    assert!(
        DACL_SERIALIZATION_MUTEX_NAME.starts_with("Local\\"),
        "mutex must be session-local, got: {DACL_SERIALIZATION_MUTEX_NAME}"
    );
    assert!(
        DACL_SERIALIZATION_MUTEX_NAME.ends_with("Sandbox.WorkspaceDacl"),
        "mutex suffix drifted, got: {DACL_SERIALIZATION_MUTEX_NAME}"
    );
    assert!(
        !DACL_SERIALIZATION_MUTEX_NAME.starts_with("Global\\"),
        "Global\\ scope would require SeCreateGlobalPrivilege a standard user lacks"
    );
}

#[test]
fn dacl_inherit_flags_matches_msdn_documented_bits() {
    // OBJECT_INHERIT_ACE = 0x1, CONTAINER_INHERIT_ACE = 0x2 per
    // Microsoft Windows SDK winnt.h. If this fires, the constant
    // drifted and SP-6 v2 workspace DACL grant is no longer
    // inheritable, which means AppContainer targets cannot read
    // or write subdirectories of their workspace.
    assert_eq!(DACL_INHERIT_FLAGS_FOR_APPCONTAINER, 0x3);
}

#[test]
fn classify_marks_existing_and_absent_metadata() {
    // The Windows stamper stamps a deny ACE on every entry and
    // pre-creates a stub for each `absent` one. If the `absent`
    // flag ever drifts, an absent `.git` would silently lose
    // protection. File-system semantics are identical on macOS /
    // Linux dev boxes, so the test runs everywhere.
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    // Create two of the four protected subpaths.
    std::fs::create_dir(ws.join(".git")).unwrap();
    std::fs::create_dir(ws.join(".aleph")).unwrap();
    // .codex and .agents intentionally absent.

    let targets = classify_protected_metadata(ws);
    assert_eq!(
        targets.len(),
        crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS.len(),
        "one entry per protected subpath"
    );
    for t in &targets {
        let name = t.path.file_name().unwrap().to_str().unwrap();
        let expect_absent = name == ".codex" || name == ".agents";
        assert_eq!(t.absent, expect_absent, "wrong absent flag for {name}");
    }
}

#[test]
fn classify_marks_all_absent_when_workspace_missing() {
    // Non-existent workspace root → every subpath is absent. The
    // Windows stamper's `create_dir` then fails (missing parent)
    // and logs — no panic. Confirms classification never walks a
    // missing directory.
    let bogus = std::path::PathBuf::from("/this/does/not/exist/aleph/test/abcdef");
    let targets = classify_protected_metadata(&bogus);
    assert_eq!(
        targets.len(),
        crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS.len()
    );
    assert!(
        targets.iter().all(|t| t.absent),
        "every entry should be absent for a missing workspace"
    );
}

#[test]
fn classify_treats_file_named_dot_git_as_existing() {
    // `.exists()` is true for files too, so a stray *file* named
    // `.git` counts as existing — the stamper DACLs the file in
    // place rather than trying to create a stub directory over it.
    // Pin the behavior so a future "directory-only" refactor is
    // intentional.
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    std::fs::write(ws.join(".git"), b"weird but legal\n").unwrap();
    let targets = classify_protected_metadata(ws);
    let git = targets
        .iter()
        .find(|t| t.path == ws.join(".git"))
        .expect(".git entry present");
    assert!(!git.absent, "a file named .git counts as existing");
}
