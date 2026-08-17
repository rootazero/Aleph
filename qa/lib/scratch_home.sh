# shellcheck shell=bash
#
# The scratch-HOME discipline every `qa/*/run.sh` fixture shares. Sourced, never
# executed.
#
# ## Why this is one function and not two lines per fixture
#
# Redirecting `HOME` into a throwaway root is what isolates a QA run from the
# operator's real `~/.aleph`. It also, silently, points rustup and cargo at an
# empty toolchain store: the next rustup-shimmed command that runs under that
# HOME finds no toolchain for the pin in `rust-toolchain.toml` and installs a
# fresh one — ~1.3 GB per run, into a directory the fixture deletes on exit (or
# worse, keeps under `KEEP=1`). Nothing fails, nothing is logged; the only
# symptom is `$TMPDIR` growing. Three abandoned roots had quietly accumulated
# 4.0 GB by 2026-08-17, none of it QA data — three copies of
# `.rustup/toolchains/1.96.0-aarch64-apple-darwin`.
#
# The per-invocation `HOME="$REAL_HOME" cargo …` guards the fixtures already
# carry do NOT cover this, and it is worth being precise about why: each one
# fixes the single line it is written on, while `export HOME=` stays in force
# for the whole process. Every drive script, every `bash`-tool call a scenario
# makes the agent run, every command typed into a shell that inherited the
# export is unprotected. A rule that has to be repeated at each call site is
# the failure mode, not the fix — so the toolchain homes are pinned once, in
# the environment, and the redirect that creates the hazard lives in the same
# function as the pin that removes it. A caller cannot take the isolation
# without the protection.
#
# ## Usage
#
#     . "$HERE/../lib/scratch_home.sh"
#     qa_redirect_home "$QA_ROOT"
#
# Sets `REAL_HOME` (shell-local — see below) and exports `HOME`, `ALEPH_HOME`,
# `RUSTUP_HOME`, `CARGO_HOME`. Call it after `QA_ROOT` is chosen and before the
# first command that must see the throwaway home.
#
# Enforced by `tests/qa_fixture_hygiene.rs`, which derives the fixture list from
# the filesystem rather than from a list in the test — a seventh fixture that
# hand-rolls `export HOME=` is named by that guard on its first run.

qa_redirect_home() {
    local qa_root="$1"
    if [ -z "$qa_root" ]; then
        echo "qa_redirect_home: needs a scratch root" >&2
        return 2
    fi

    # Captured BEFORE the redirect: every line below needs the operator's real
    # home, and after `export HOME=` there is no way back to it.
    REAL_HOME="$HOME"

    # Deliberately NOT exported. The fixtures' promise is isolation from the
    # real home, so handing the QA server's children a path straight back to it
    # is a (small) hole in exactly the thing being tested. Fixtures whose child
    # processes genuinely need it export it themselves, right after this call.
    #
    # Honour a pre-existing RUSTUP_HOME/CARGO_HOME instead of deriving one: an
    # operator who relocated their toolchain already has the correct answer in
    # the environment, and re-deriving `$REAL_HOME/.rustup` would be a second
    # answer — wrong for precisely the people who set it. These two DO have to
    # be exported; being inherited by children is the entire point.
    RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
    CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
    export RUSTUP_HOME CARGO_HOME

    export HOME="$qa_root/home"
    # The `.aleph` dir ITSELF, not its parent.
    export ALEPH_HOME="$qa_root/home/.aleph"
}
