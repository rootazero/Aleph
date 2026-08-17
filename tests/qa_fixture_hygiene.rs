//! Guards the one rule every `qa/` fixture has to obey about the scratch HOME.
//!
//! A fixture redirects `HOME` into a throwaway root so the run cannot touch the
//! operator's real `~/.aleph`. That redirect also, silently, points rustup and
//! cargo at an empty toolchain store: the next rustup-shimmed command under
//! that HOME finds no toolchain for `rust-toolchain.toml`'s pin and installs a
//! fresh one — ~1.3 GB, into a directory that is about to be deleted. Nothing
//! fails and nothing is logged, so the only symptom is `$TMPDIR` growing; three
//! abandoned roots had accumulated 4.0 GB of duplicated toolchain by
//! 2026-08-17.
//!
//! `qa/lib/scratch_home.sh::qa_redirect_home` performs the redirect and the pin
//! together, so a caller cannot take the isolation without the protection. The
//! rules below exist because a fix that must be *remembered* at N sites is the
//! same defect one level up — which is precisely how the leak happened: all
//! eleven `HOME="$REAL_HOME" cargo …` guards in these fixtures are individually
//! correct, and they cover only the lines they are written on.
//!
//! Every rule derives its subject list by walking `qa/`, never from a list kept
//! here — a seventh fixture is judged on its first run rather than whenever
//! someone remembers to add it. For the same reason there is no allowlist: the
//! rule costs nothing to obey, and an allowlist would be a second source of
//! truth about who may hand-roll a scratch home.

use std::path::{Path, PathBuf};

/// `qa/`, from the crate root.
fn qa_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("qa")
}

/// The shared helper — the only file allowed to spell the redirect.
fn helper_path() -> PathBuf {
    qa_root().join("lib").join("scratch_home.sh")
}

/// Every `*.sh` under `qa/`, excluding the shared `lib/` that implements the
/// rule. Returned as `(repo-relative path, contents)`.
fn fixture_shell_sources() -> Vec<(String, String)> {
    let root = qa_root();
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `lib/` owns the redirect; everything else is a caller.
                if path.file_name().and_then(|n| n.to_str()) != Some("lib") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("sh") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, text));
        }
    }
    out.sort();
    out
}

/// Shell code with comment lines removed.
///
/// The scanner judges code; a comment is documentation. Without this, prose
/// *describing* the rule would satisfy it and prose *quoting* the old spelling
/// would violate it — both directions have bitten source-level guards in this
/// repo before.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
}

fn code_of(text: &str) -> String {
    code_lines(text)
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rule 1 — nobody outside `qa/lib/` redirects HOME by hand.
///
/// This is the rule that actually closes the leak: a hand-rolled `export HOME=`
/// creates the hazard without the pin that neutralises it.
#[test]
fn no_qa_fixture_hand_rolls_the_scratch_home() {
    let sources = fixture_shell_sources();
    assert!(
        !sources.is_empty(),
        "the scan found no shell fixtures under {} — it is measuring nothing, \
         which would make every assertion below vacuously true",
        qa_root().display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for (rel, text) in &sources {
        // A per-command `HOME=… cmd` prefix is safe once the pins are in the
        // process environment, because the child inherits them; only the
        // process-wide `export HOME=` creates an unprotected window. So the
        // predicate is "does this file already carry the pins", derived from the
        // file itself — not an allowlist of blessed line numbers, which would
        // rot into a licence the moment one of those lines changed meaning.
        // (`qa/browser_managed/run.sh` legitimately runs playwright-cli under
        // the scratch HOME twice: that CLI's session store is HOME-scoped and
        // the developer's own browser sessions are not the fixture's to kill.)
        let carries_pins = code_of(text).contains("qa_redirect_home");
        for (n, line) in code_lines(text) {
            let t = line.trim_start();
            let process_wide = t.starts_with("export HOME=");
            let per_command = t.starts_with("HOME=\"$QA_ROOT") && !carries_pins;
            if process_wide || per_command {
                offenders.push(format!("{rel}:{n}: {}", line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these redirect HOME by hand instead of calling \
         `qa_redirect_home` from qa/lib/scratch_home.sh, so the run gets the \
         isolation without the RUSTUP_HOME/CARGO_HOME pin that stops rustup \
         from installing a ~1.3 GB toolchain into the throwaway root — silently, \
         once per run:\n  {}",
        offenders.join("\n  ")
    );
}

/// Rule 2 — a fixture that mints a scratch root must go through the helper.
///
/// The mirror of rule 1: rule 1 catches a redirect without the pin, this catches
/// a fixture that stopped redirecting at all — which would point the run at the
/// operator's real `~/.aleph`, the thing the scratch root exists to prevent.
#[test]
fn every_fixture_with_a_scratch_root_calls_the_shared_helper() {
    let mut offenders: Vec<String> = Vec::new();
    for (rel, text) in fixture_shell_sources() {
        let code = code_of(&text);
        // "Mints a scratch root" is derived from the content, not from a name
        // list: this is the `mktemp -d .../aleph-qa-*` line every fixture uses.
        if !code.contains("aleph-qa-") {
            continue;
        }
        if !code.contains("qa_redirect_home") {
            offenders.push(rel);
        }
    }

    assert!(
        offenders.is_empty(),
        "these mint a scratch QA root but never call `qa_redirect_home`, so \
         HOME still points at the operator's real home and the run reads and \
         writes the real ~/.aleph:\n  {}",
        offenders.join("\n  ")
    );
}

/// Rule 3 — the single source still does both halves.
///
/// Rules 1 and 2 funnel every fixture into one function; this asserts that
/// function has not been gutted. Without it, the funnel could be perfect and
/// the protection absent.
#[test]
fn the_shared_helper_pins_both_toolchain_homes() {
    let path = helper_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let code = code_of(&text);

    for var in ["RUSTUP_HOME", "CARGO_HOME"] {
        assert!(
            code.contains(&format!("{var}=")),
            "{} no longer assigns {var}; every qa fixture routes its scratch-HOME \
             redirect through this file, so dropping the pin re-opens the \
             ~1.3 GB-per-run toolchain install in every one of them at once",
            path.display()
        );
    }
    assert!(
        code.contains("export RUSTUP_HOME CARGO_HOME")
            || (code.contains("export RUSTUP_HOME") && code.contains("export CARGO_HOME")),
        "{} assigns the toolchain homes but does not export them — an \
         unexported value is invisible to the child processes that are the \
         entire point (the server, drive scripts, and any shell a scenario spawns)",
        path.display()
    );
    assert!(
        code.contains("REAL_HOME=\"$HOME\""),
        "{} must capture the real HOME before redirecting it; after the \
         redirect there is no way back to it",
        path.display()
    );
}
