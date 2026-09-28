//! The CLI ships under two names — `al` (primary) and `aleph` — built from one
//! library entry. These tests spawn the real binaries, because the thing under
//! test is what a user sees after typing one name or the other: a unit test of
//! `Cli::command()` cannot tell which name the process was invoked as.

use std::process::{Command, Output};

/// Run a built binary with every home-like directory pointed into a
/// throwaway dir. `run()` initializes file logging and loads the config file
/// before it dispatches, so a test that reaches dispatch must not touch the
/// developer's real `~/.aleph` or config dir.
fn run(bin: &str, args: &[&str]) -> Output {
    let home = tempfile::tempdir().expect("create temp home");
    let output = Command::new(bin)
        .args(args)
        .env("HOME", home.path())
        .env("ALEPH_HOME", home.path().join(".aleph"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .output()
        .unwrap_or_else(|e| panic!("spawn {bin}: {e}"));
    assert!(
        output.status.success(),
        "{bin} {args:?} exited with {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(bin: &str, args: &[&str]) -> String {
    String::from_utf8(run(bin, args).stdout).expect("stdout is UTF-8")
}

/// The `complete -F _<fn> ... <name>` lines of a bash completion script.
fn bash_registrations(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("complete "))
        .collect()
}

/// Asserts the script registers its completion function for exactly `name`.
///
/// Checked on the registration line's last token rather than by substring:
/// `_aleph` contains `_al`, so "contains `al`" would pass for either name.
fn assert_registers_only(script: &str, name: &str) {
    let registrations = bash_registrations(script);
    assert!(
        !registrations.is_empty(),
        "no `complete` registration in:\n{script}"
    );
    for line in registrations {
        assert_eq!(
            line.split_whitespace().last(),
            Some(name),
            "completion registered for the wrong command: {line}"
        );
        assert!(
            line.contains(&format!("-F _{name} ")),
            "completion function is not `_{name}`: {line}"
        );
    }
}

#[test]
fn help_usage_names_the_invoked_binary() {
    let al = stdout(env!("CARGO_BIN_EXE_al"), &["--help"]);
    assert!(al.contains("Usage: al"), "al --help:\n{al}");
    assert!(!al.contains("Usage: aleph"), "al --help:\n{al}");

    let aleph = stdout(env!("CARGO_BIN_EXE_aleph"), &["--help"]);
    assert!(aleph.contains("Usage: aleph"), "aleph --help:\n{aleph}");
}

/// `--version` prints the product name, not the invoked name: both
/// binaries are the same program and must say so identically.
#[test]
fn both_names_report_the_same_version() {
    let al = stdout(env!("CARGO_BIN_EXE_al"), &["--version"]);
    let aleph = stdout(env!("CARGO_BIN_EXE_aleph"), &["--version"]);
    assert_eq!(al, aleph);
    assert!(al.starts_with("aleph "), "unexpected version line: {al}");
}

#[test]
fn completion_registers_the_invoked_name() {
    let al = stdout(env!("CARGO_BIN_EXE_al"), &["completion", "bash"]);
    assert_registers_only(&al, "al");
    assert!(
        !al.contains("_aleph"),
        "`al completion bash` still emits `aleph` functions:\n{al}"
    );

    let aleph = stdout(env!("CARGO_BIN_EXE_aleph"), &["completion", "bash"]);
    assert_registers_only(&aleph, "aleph");
}
