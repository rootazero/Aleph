# Phase8 / NSH Final QUALITY Review / Phase8 / NSH 最终 QUALITY 审阅

**Date / 日期:** 2026-11-15
**Repository / 仓库:** local `main`; planning-only review
**Verdict / 结论:** **PASS WITH RECORDED LIMITS**
**Reviewer / 审阅者:** independent read-only `reviewer` subagent, sequential after the SPEC review and repairs
**Scope / 范围:** final Phase8 Integration & Security and NSH v0.5 Panel parity planning package

## Review method / 审阅方法

The parent ran Graphify before the review with `graphify query "Phase8 Integration Security NSH 0.5 restart plan panel parity R2 evidence" --budget 1800`; it identified `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md` at L1. The reviewer used that orientation before source reads. It performed a read-only consistency and traceability review. It did not modify files or run Cargo, builds, tests, network commands, the future harness, NSH runtime, or performance measurements.

The first QUALITY pass found two P1 planning defects and one P2 portability defect: the nonexistent `bun run dev:live` launcher, an invalid/unavailable performance launcher and marker protocol, a receipt schema that could not represent valid threshold rejection, and the shell-unsafe `<fresh-task-target>` placeholder. Those findings were repaired before this final review.

## Accepted conclusions / 已接受结论

- The planning-only boundary is consistent. Tasks 1–4 prepare evidence and plans; Task 5 requires explicit human approval before production implementation. No dependency migration, lockfile/workspace change, Tauri removal, release or push is authorized.
- The repaired performance plan uses the supported debug-only lite profile: `--no-default-features --features aleph-desktop-shell/qa-owned-root`. It does not claim release performance and explicitly defers a release-profile comparison to a separately reviewed release-safe QA confinement design.
- The future harness matches the current Tauri QA root contract: an OS-temp direct child named with the `aleph-shell-qa-` prefix, no-follow-link creation, a 32-byte lowercase hexadecimal nonce, and `owned_root / ".qa-owned"` containing `nonce + "\n"`. The launcher passes root and nonce as separate argv values.
- Tauri and NSH launch descriptions use built binaries, explicit working directories, Python argv arrays and `shell=False`; they record cwd and fixture identity. `ALEPH_PANEL_APPVIEW_START` remains a future QA-only application marker immediately before the application launch. The current v0.4 NSH shell is explicitly excluded from v0.5 runtime proof.
- The receipt distinguishes valid threshold rejection from invalid measurements using `threshold_status`, `baseline`, `thresholds`, and `acceptance_reason`. Exit codes distinguish accepted results, invalid/missing receipts, valid `NOT_ACCEPTED` results and launcher/schema/protocol failures.
- F1–F5 retain real ingress, barrier or mutation, terminal assertion, test target and future receipt shape. F2 uses the crate-internal captured-session seam; F4 respects caller-cursor-ignored/current-snapshot semantics and does not make lag alone invalidate an unchanged lease.
- Current-source citations, Contract A versus the simulated transcript P2, renderer ACK versus PTY reply boundaries, NSH v0.5 NOT_PROVEN/DEFERRED statuses, Phase0–6 and ADR-003–009 reconciliation, R2 approval, detached daemon exclusion, Phase4 parity and two-week dual-run removal gates are internally consistent.
- The review found no stale absolute machine paths, contradictory release-profile claims, stale receipt schema, invalid launcher reference, or wording that upgrades planning artifacts into runtime proof.

## Final files / 最终文件

- `docs/superpowers/runs/2026-11-15-phase8-nsh-spec-review.md`
- `docs/superpowers/runs/2026-11-15-phase8-nsh-quality-review.md`
- `docs/superpowers/plans/2026-11-15-phase8-integration-security-implementation.md`
- `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md`
- `docs/superpowers/runs/2026-11-15-nsh05-migration-inventory.md`
- `docs/superpowers/runs/2026-11-15-nsh05-upstream-evidence.md`
- `docs/superpowers/specs/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/plans/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/specs/2026-11-15-nsh-r2-native-ui-decision.md`
- `docs/superpowers/runs/2026-11-15-phase8-entry-inventory.md`

## Recorded limits / 记录的限制

This is a QUALITY verdict for the planning package. It does not prove Cargo compilation, security behavior, NSH v0.5 compatibility, native Panel projection, measured performance, cross-platform support, R2 approval, Phase4 parity, the two-week dual run, or Tauri/WebChat removal. The proposed harness, schema, Phase8 integration target and execution receipts do not yet exist. The current shell remains the old v0.4/absolute-path/mock implementation recorded by the NSH inventory.

No Cargo, build, test, runtime, network or performance command was run. The final next action is the human Task5 approval decision; implementation remains blocked until that decision is supplied.
