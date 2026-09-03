# Build a cargo target for a QA fixture, with cargo's exit code intact.
#
# Sourced by every fixture `run.sh` that runs cargo — source it ABOVE that
# fixture's build block, which is NOT necessarily next to `scratch_home.sh`:
# several fixtures hoist the build ahead of the HOME redirect (Windows; the
# reasoning is in `qa/teamchat_rooms/run.sh`), and a `qa_build` above its own
# `. build.sh` dies as `qa_build: command not found` before one assertion runs.
#
# It exists because of one measurement.
#
# Every fixture here built its binaries as
#
#     if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build … 2>&1 | tail -5); then
#
# and a pipeline's status is its LAST command's — `tail`'s — which is always 0.
# So a build that failed reported success, and the fixture went on to run
# whatever binary was already sitting in the shared target dir (`.cargo/config`
# pins one absolute path for every worktree, so that binary can be arbitrarily
# old and from an arbitrarily different tree).
#
# It was not hypothetical. 2026-08-29, `qa/run_halt/run.sh receipt`:
# `cargo build --bin aleph` fails with `no bin target named aleph in
# default-run packages` — that bin belongs to `aleph-cli`, and the invocation
# was missing `-p`. The failure was swallowed, the fixture ran a `target/debug/
# aleph` from sixteen days earlier, and the round's headline finding — "the CLI
# receives no stream frames from a real gateway" — was measured against it.
# (The finding turned out to be true for an unrelated reason, which is the
# unnerving part: a broken instrument that happens to agree with reality is
# still a broken instrument.)
#
# `${PIPESTATUS[0]}` is the whole point, and it is why this is a function
# rather than something a caller can inline back into an `if !`.
#
# Requires `$REPO`; uses `$REAL_HOME` when `qa_redirect_home` has already run
# (cargo's registry, git cache and rustup toolchain live under the developer's
# real HOME, and a build launched with the scratch one silently degrades into a
# full network fetch).
qa_build() {
  (cd "$REPO" && HOME="${REAL_HOME:-$HOME}" cargo build "$@" 2>&1 | tail -5)
  local rc=${PIPESTATUS[0]}
  if [ "$rc" != "0" ]; then
    echo "cargo build $* failed (exit $rc)" >&2
  fi
  return "$rc"
}
