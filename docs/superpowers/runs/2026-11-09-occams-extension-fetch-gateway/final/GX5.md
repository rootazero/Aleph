# Final verdicts — GX5

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX5-01 | REFUTED | P1 | `std::env::var()` reads process memory (not I/O), no syscall block; tokio runtime unaffected / Line 103 performs an in-process environment lookup, not blocking filesystem or network I/O; no runtime-thread stall is demonstrated. |
| GX5-02 | REFUTED | P2 | Link configs are operator-written YAML (line 32 comment), not attacker input; env var size is operator-controlled / Lines 89–94 define rejection of oversized expansions, while operator-controlled environment and configs are not attacker-controlled as claimed. |
| GX5-03 | DOWNGRADE | P3 | TOCTOU is real (lines 142-151) but race window is narrow, ticket consumption prevents double-spend; robustness defect not logic bug / Lines 142–151 can misreport a concurrent collision as invalid-ticket, but the single-use ticket prevents duplicate issuance; impact is error clarity only. |
