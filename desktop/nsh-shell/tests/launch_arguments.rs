//! Exercise the real launch entry without opening a window or touching a store.
use std::process::{Command, Output};

fn launch(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"))
        .args(args)
        .env_remove("ALEPH_NSH_QA_URL")
        .env_remove("ALEPH_NSH_QA_NONCE")
        .env_remove("ALEPH_PANEL_QA_STATUS")
        .output()
        .expect("launch owned shell process")
}

#[test]
fn status_is_read_only_and_unbundled_context_is_not_disabled() {
    let output = launch(&["--autostart-status"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({"status":"unavailable", "reason":if cfg!(target_os = "macos") {"not_app_bundle"} else {"unsupported_platform"}})
    );
}

#[test]
fn status_rejects_qa_url_and_malformed_identity_without_echoing_input() {
    for (url, nonce) in [
        (Some("http://synthetic-secret.invalid"), "a1".repeat(32)),
        (None, "synthetic-secret".into()),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"));
        command
            .arg("--autostart-status")
            .env("ALEPH_NSH_QA_NONCE", nonce)
            .env_remove("ALEPH_NSH_QA_URL");
        if let Some(url) = url {
            command.env("ALEPH_NSH_QA_URL", url);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr)
            .unwrap()
            .contains("synthetic-secret"));
    }
    assert_eq!(
        launch(&["--autostart-status", "extra"]).status.code(),
        Some(1)
    );
}

#[test]
fn status_observer_refuses_invalid_or_missing_qa_context_before_launch() {
    for value in ["1", "synthetic-secret"] {
        // The old entry also rejects this target; a RED cannot open a window.
        let output = Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"))
            .args(["--connect", "http://box.invalid"])
            .env("ALEPH_PANEL_QA_STATUS", value)
            .env_remove("ALEPH_NSH_QA_URL")
            .env_remove("ALEPH_NSH_QA_NONCE")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("refused Panel status QA"));
        assert!(!error.contains("synthetic-secret"));
    }
}

#[test]
fn help_returns_before_the_native_event_loop() {
    let output = launch(&["--help"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--connect <origin>"));
    assert!(text.contains("does not start, stop or reconfigure a service"));
    assert!(output.stderr.is_empty());
}

#[test]
fn invalid_arguments_exit_without_echoing_secret_material() {
    for args in [
        vec!["--connect"],
        vec!["--connect", "local", "extra"],
        vec!["--synthetic-secret"],
        vec!["--connect", "https://u:synthetic-secret@box.invalid"],
        vec!["--connect", "https://box.invalid/?bt=synthetic-secret"],
        vec!["--connect", "http://box.invalid"],
    ] {
        let output = launch(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("refused configuration"));
        assert!(!error.contains("synthetic-secret"));
        assert!(!error.contains("box.invalid"));
    }
}

#[test]
fn production_target_cannot_redirect_a_qa_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"))
        .args(["--connect", "https://box.invalid"])
        .env("ALEPH_NSH_QA_URL", "http://127.0.0.1:29871/")
        .env("ALEPH_NSH_QA_NONCE", "a1".repeat(32))
        .output()
        .expect("launch owned shell process");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot be combined"));
    assert!(!error.contains("box.invalid"));
}

#[cfg(unix)]
#[test]
fn malformed_qa_environment_is_not_silently_downgraded_to_production() {
    use std::os::unix::ffi::OsStringExt;
    // This target is rejected even by the old entry, so a RED run cannot
    // accidentally create a window or open a production credential store.
    let output = Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"))
        .args(["--connect", "http://box.invalid"])
        .env("ALEPH_NSH_QA_URL", std::ffi::OsString::from_vec(vec![0xff]))
        .env(
            "ALEPH_NSH_QA_NONCE",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .output()
        .expect("launch owned shell process");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("non-Unicode QA"));
}

#[cfg(unix)]
#[test]
fn malformed_unicode_is_rejected_before_native_startup() {
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_aleph-nsh-shell"))
        .arg(std::ffi::OsString::from_vec(vec![0xff]))
        .env_remove("ALEPH_NSH_QA_URL")
        .env_remove("ALEPH_NSH_QA_NONCE")
        .output()
        .expect("launch owned shell process");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("non-Unicode"));
}
