# Functional QA Report: Aleph Phase9 Runtime Proof & Dependency Closure

| Field | Value |
|---|---|
| Date / branch / revision | 2026-11-17 / `runtime-proof` / `3892804c112fd6e574166a4c03a7c40a77d0b239` plus authorized working-tree changes |
| Caller / authority / depth | gstack QA functional-only; permitted writes: QA report and existing task evidence; no product release, commit, push, root, private API or policy bypass |
| Surfaces / scope | `qa/nsh_panel_launcher.py`, LaunchServices coalition parser/sampler, provider/Gateway helpers, Tauri/NSH runtime boundary, Phase9 integrity and reducer boundary |
| Runtime / native tools | gstack `1.100.0.0`; Python 3.14.2 via uv; `jsonschema==4.25.1`; Clang; Swift; repository-native unittest, Cargo and QA wrappers |
| Fixture ownership / destinations | Synthetic test fixtures and prior owned Gateway evidence; report directory `.gstack/qa-reports/run-20261117T231302Z/`; no external production destination |
| Probe budget / guarded command time / stop reason | Functional tests and static checks completed; live 10+30 stopped before execution because approved per-instance Tauri ownership/runtime is unavailable |

## Contract outcomes

| Contract and source | Exact probe / evidence | Expected → observed | Outcome |
|---|---|---|---|
| Coalition parser assigns target WebKit roles only through target coalition membership, `qa/test_nsh_panel_launcher.py` | `uv run --offline --no-project --with jsonschema==4.25.1 python -m unittest discover -s qa -p 'test_nsh_panel_*.py'` | Parser, overlap, missing-role, unrelated-app, sampler and provenance cases → 30 tests passed in 1.135s | pass |
| Saved marker provenance is bound to the owned raw capture, `qa/nsh_panel_launcher.py::verified_capture` | Included in launcher suite above | Mutated or foreign nonce-bound raw markers → fail closed; valid synthetic ownership evidence → accepted by verifier | pass |
| Phase9 runner cancellation/cleanup, `qa/test_phase9_test_closure.py` | `uv run --offline --no-project --with jsonschema==4.25.1 python -m unittest discover -s qa -p 'test_phase9_test_closure.py'` | Runner process groups and nested cancellation cleanup → 13 tests passed in 0.552s | pass |
| Cargo lock identity delta, `qa/test_cargo_lock_delta.py` | `uv run --offline --no-project --with jsonschema==4.25.1 python -m unittest discover -s qa -p 'test_cargo_lock_delta.py'` | Version/source/checksum identity comparison → 3 tests passed | pass |
| QA helper syntax and diff hygiene | `python3 -m py_compile qa/nsh_panel_launcher.py qa/nsh_panel_gateway.py qa/nsh_panel_provider.py`; `clang -fsyntax-only -Wall -Wextra -Werror qa/phase9_app_launch_wrapper.c`; `swiftc -typecheck qa/phase9_nsworkspace_launcher.swift`; `git diff --check` | All checks passed | pass |
| Default Panel artifact restoration | SHA-256 of `interfaces/webchat/dist/aleph_panel_bg.wasm` | Expected default artifact → `2a208e2aa4f5de4f13d6f250442d7a1b0535a2398a15a119cd8f957f26ac2719` | pass |
| Tauri per-instance WebKit ownership | Existing direct and bundle probes in `full-closure/direct-tauri-coalition-probe/receipt.json` and `full-closure/real-tauri-app-probe/receipt.json` | Direct binary completed but inherited Terminal/Pi coalition; official bundle had independent coalition but WebView construction blocked before Panel markers | blocked |
| NSH per-instance coalition feasibility | `full-closure/nsworkspace-coalition-isolation/receipt.json` and 100-sample resource receipt | Two NSH instances had disjoint coalitions and complete WebKit role membership; 100 samples had 90.109–109.964ms gaps | pass as feasibility evidence; not full D acceptance |
| Exact Tauri-first/NSH-second 10 warmups + 30 measured reducer input | Existing `qa/nsh_panel_parity.py` contract | Requires complete marker-bound ownership for 40 runs per surface; Tauri prerequisite is unavailable | not run |
| Phase9/Phase10 final disposition | `docs/superpowers/runs/2026-11-16-phase9-final-integrity.json` | D OPEN/BLOCKED; `phase10_readiness=false` | blocked |

## Findings

### ISSUE-001: Incorrect direct unittest module invocation

- Classification / severity: QA command setup issue; low severity.
- Intended contract and source: tests under `qa/` must be imported with their directory on the module path or run through the repository discover entry.
- Reproduction: direct module invocation from the repository root produced `ModuleNotFoundError: No module named 'nsh_panel_launcher'` and `ModuleNotFoundError: No module named 'nsh_panel_gateway'`.
- Observed: 19 tests loaded before the two import errors. The repository-native discover commands then passed the affected suites: 13/13 runner tests and 3/3 lock tests.
- Evidence: current terminal output in this QA session; successful commands are recorded above.
- Diagnosis / next action: use `python -m unittest discover -s qa -p ...`; no product fix required.

### ISSUE-002: Tauri ownership/runtime prerequisite remains unavailable

- Classification / severity: setup/platform blocker for Phase9 D acceptance.
- Intended contract and source: D requires a complete, per-instance ownership-bound resource sample for every measured run before `qa/nsh_panel_launcher.py::metric_document` can feed the unchanged reducer.
- Reproduction: prior direct Tauri probe and official `ai.aleph.desktop` bundle probes use public LaunchServices paths and retained raw `lsappinfo` evidence.
- Observed: direct Tauri produced real lifecycle markers but its PID was inside Terminal/Pi coalition `11437`; the official bundle received an independent coalition but stopped during WebView construction after `RunEvent::Ready`/`webview2`, with no Panel markers or complete ownership samples.
- Evidence: `docs/superpowers/runs/2026-11-16-phase9-resource-ownership-investigation.md`, `full-closure/direct-tauri-coalition-probe/receipt.json`, and `full-closure/real-tauri-app-probe/receipt.json`.
- Diagnosis / next action: obtain an approved public Tauri launch/runtime path that yields complete marker-bound ownership. Do not relax the coalition rule, substitute global RSS/name matching, or run the 10+30 matrix with incomplete evidence.

## Discoveries and permanent tests

- [ownership investigation](../../../docs/superpowers/runs/2026-11-16-phase9-resource-ownership-investigation.md) — public NSH coalition attribution is feasible; direct Tauri ownership is not established; official bundle WebView construction blocks.
- [final integrity](../../../docs/superpowers/runs/2026-11-16-phase9-final-integrity.json) — current structured disposition remains `INTEGRITY_PASS_WITH_OPEN_D_ACCEPTANCE_GATE`.

| Hypothesis / discovery | Native test or proposed case | Red evidence before repair | Green + original + adjacent evidence | Parent disposition |
|---|---|---|---|---|
| Target WebKit must be assigned through coalition membership, not global process names | Coalition parser tests for disjoint, overlapping, missing and unrelated-app records | Missing parser and overlapping synthetic membership were rejected | 30 launcher/provider/Gateway tests pass; unrelated WebKit is ignored outside target scope | authorized QA contract |
| Marker arrays must match the owned raw stdout/file capture | `verified_capture` provenance test | Mutated event data previously escaped hash-only validation | Nonce-bound marker reconstruction and hash checks pass | authorized QA contract |
| A complete metric document requires all 40 samples per surface | `metric_document` boundary tests and existing reducer | Protocol-only or incomplete resource scope is rejected | Existing tests preserve the rejection; no live matrix was fabricated | preserve D gate |
| Tauri LaunchServices app identity supplies usable ownership | Public direct and bundled app probes | Direct process inherited Terminal/Pi coalition; real bundle blocked in WebView construction | No compliant Tauri evidence yet | deferred platform blocker |

## Coverage limits and cleanup

- The exact 10 warmup + 30 measured comparison was not run. The existing reducer and thresholds were unchanged.
- No Phase10 readiness claim was made. Final integrity remains `PARTIAL / BLOCKED; D OPEN / BLOCKED; not PASS` with `phase10_readiness=false`.
- NSH feasibility evidence is not a complete dual-surface acceptance result because the Tauri prerequisite is missing.
- Browser QA was not applicable to this functional-only target.
- The gstack upgrade from `1.99.5.0` to `1.100.0.0` completed with `FF_OK`; no migrations newer than the old version were present. gstack telemetry was enabled after explicit user approval.
- The gstack build had warnings for codesign of `browse/dist/find-browse` and `bin/gstack-global-discover`; those tools were outside this repository QA target and were not used.
- Owned smoke/Gateway/native processes were checked after probes; no Cargo, Rust, Gateway, Tauri, NSH or helper process remains. Temporary diagnostic evidence and prior receipts remain under the task workspace by design.
