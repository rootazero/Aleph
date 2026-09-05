#!/usr/bin/env bash
# Real-machine contract probe for Aleph's Windows shell (`utils::shell`).
#
#   ./qa/winshell/run.sh all       # ~40 s  every stage below, in one process
#   ./qa/winshell/run.sh resolve   #        which `pwsh`, and its version
#   ./qa/winshell/run.sh encoding  #        the child's code page, 4 invocation forms
#   ./qa/winshell/run.sh exit      #        a native child's exit code
#   ./qa/winshell/run.sh comment   #        a trailing `#` vs. the epilogue
#   ./qa/winshell/run.sh length    #        the `-Command` command-line ceiling
#   ./qa/winshell/run.sh profile   #        what `-NoProfile` saves (the slow one)
#   ./qa/winshell/run.sh env       #        what an env_clear()ed child is missing
#
# ## What this is, and what it is not
#
# It is NOT an integration test of Aleph. No server is booted, no config is
# written, `~/.aleph` is never touched, no port is bound. It spawns the HOST's
# PowerShell and reports whether each fact the Windows shell design rests on
# actually holds here.
#
# It exists because those facts were measured once, in a chat window, and a
# number measured in a chat window is a number nobody else can check. This
# repo's discipline says a number carries the predicate it measured and the
# commit it measured at, and that another person must be able to re-derive it
# (判据 §18). This is the re-derivation.
#
# ## The seven contracts
#
#   1  `pwsh` resolves on PATH, at an absolute path, and reports a version.
#   2  A child is NOT reliably UTF-8 without the prologue, which is why
#      `PS_PROLOGUE` states the encoding rather than assuming it.
#      ⚠️ Deliberately NOT phrased as "the encoding is a property of the
#      invocation form". That is what shell.rs's comment says, and stage 2's
#      `2d` line measures it and does not reproduce it here: BOTH forms answer
#      the console's code page. The conclusion survives — the prologue is more
#      necessary, not less — but repeating the reason in a second place would be
#      repeating something the fixture below contradicts (判据 §1).
#   3  `pwsh -Command 'cmd /c exit 3'` does not exit 3. `PS_EPILOGUE` is what
#      makes it.
#   4  The epilogue must be joined with a NEWLINE. A `;` join is swallowed by a
#      script whose last line is a comment, and a succeeding script then reports
#      failure — in silence.
#   5  What this host's `-Command` ceiling actually IS — the largest script it
#      will carry, and the failure mode above it.
#      ⚠️ Deliberately says nothing about which arm the product takes. Whether
#      PowerShell stays on `-Command` at every size or gains a stdin route above
#      a threshold is a live decision, and a fixture that described either of
#      those two worlds would be wrong the day the other landed — while still
#      passing, because nothing in it would notice. The ceiling is the fact that
#      survives both, and it is the number any such threshold has to stay UNDER;
#      `5c` reads that threshold out of shell.rs and compares the two.
#   6  `-NoProfile` is in the argv to save time. Measured, not assumed.
#   7  The sandbox `env_clear()`s, so `WINDOWS_PASS_ENV` IS the child's
#      environment. A child with only `PATH` is crippled in ways that read as
#      "commands fail for no reason".
#
# ## Two things that keep it honest
#
# * **Nothing is copied.** The prologue, the epilogue, the argv flags, the
#   separators they are joined with and the environment allowlist are all
#   DERIVED from `src/utils/shell.rs` and `src/builtin_tools/code_exec.rs`
#   (`derive_ps_contract.mjs`). A hand copy would be a second statement of one
#   fact and would drift the first time either side moved (判据 §1). Run the
#   deriver alone to see what it read:
#
#       node qa/winshell/derive_ps_contract.mjs .
#
#   The ONE thing deliberately not derived is where `pwsh` lives — stage 1 walks
#   PATH with PATHEXT itself. Reading the path out of the product would make the
#   fixture agree with it by construction.
#
# * **Every stage can be made to say NO.** A stage that cannot go red is not a
#   probe (判据 §2). `QA_WINSHELL_FALSIFY=<name>` breaks exactly one input and
#   the named stage must turn red:
#
#       QA_WINSHELL_FALSIFY=prologue ./qa/winshell/run.sh encoding   # 2a/2b red
#       QA_WINSHELL_FALSIFY=epilogue ./qa/winshell/run.sh exit       # 3b red
#       QA_WINSHELL_FALSIFY=join     ./qa/winshell/run.sh comment    # 4a red
#       QA_WINSHELL_FALSIFY=length   ./qa/winshell/run.sh length     # 5a red
#       QA_WINSHELL_FALSIFY=threshold ./qa/winshell/run.sh length    # 5c red
#       QA_WINSHELL_FALSIFY=profile  ./qa/winshell/run.sh profile    # 6a red
#       QA_WINSHELL_FALSIFY=env      ./qa/winshell/run.sh env        # 7b/7c red
#       QA_WINSHELL_FALSIFY=resolve  ./qa/winshell/run.sh all        # 1a red, rest SKIP
#
# ## Where it runs
#
# Windows only, and on anything else it SKIPS LOUDLY rather than passing: every
# contract above is about a Windows host. `pwsh` does exist on Linux and macOS,
# but it is not the shell `utils::shell::resolve` picks there, and the code
# page / native-exit-code / 32767-character-command-line / `env_clear()`
# questions are not even askable.
#
#   QA_WINSHELL_N=3 ./qa/winshell/run.sh profile   # fewer timing samples
set -uo pipefail

STAGE="${1:-all}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

case "$STAGE" in
  resolve|encoding|exit|comment|length|profile|env|all) ;;
  *)
    echo "unknown stage: $STAGE (resolve|encoding|exit|comment|length|profile|env|all)" >&2
    exit 64
    ;;
esac

# Node, not Python: this host's `python`/`python3` are WindowsApps AppExecLink
# stubs that open the Microsoft Store and exit 49 without running anything, and
# `qa/terminal` was ported off Python for exactly that reason on 2026-09-05.
# Same reasoning, so the same language.
command -v node >/dev/null 2>&1 || {
  echo "no node on PATH — this fixture is a Node driver (see qa/terminal/run.sh's" >&2
  echo "header for why these fixtures are not Python on this host)" >&2
  exit 1
}

# The commit the numbers below belong to. A measurement without one is a
# measurement nobody can place (判据 §18).
COMMIT="$(cd "$REPO" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
DIRTY="$(cd "$REPO" && git status --porcelain 2>/dev/null | head -1)"
echo "repo:   $REPO"
echo "commit: $COMMIT${DIRTY:+  (working tree DIRTY — these numbers are about the tree, not the commit)}"

node "$HERE/probe_pwsh.mjs" "$REPO" "$STAGE"
RC=$?

echo
echo "=== verdict: winshell/$STAGE rc=$RC ==="
exit "$RC"
