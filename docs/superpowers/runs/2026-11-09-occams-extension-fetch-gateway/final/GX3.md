# Final verdicts — GX3

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX3-01 | UPHELD | P1 | Line 60-63 locks conn (Mutex), then line 66-72 runs blocking SQLite query inline on async fn. Violates stated contract. / `get_or_create` locks the SQLite mutex and performs synchronous queries inline at lines 60-72; Tokio runtime-thread blocking is reachable. |
| GX3-02 | UPHELD | P1 | Line 224-225 deletes messages without prior FTS cleanup; compare reset_session (cited 657-662) which does DELETE messages_fts first. / Lines 224-225 delete `messages` without first deleting matching `messages_fts` rows, unlike the cited cleanup path. |
| GX3-03 | REFUTED | P1 | tie-break(88apiclaude/claude-opus-5-5): Excerpt starts mid-function at line 74; no lock/transaction is visible, so the TOCTOU race cannot be confirmed. Without the race, every n==0 case returns meta.state matching the DB. |
| GX3-04 | UPHELD | P2 | Line 285 record_enqueued calls std::fs::create_dir_all (line 272) + write_atomic (275) on sync path before spawn. Blocking I/O on worker. / `record_enqueued` performs synchronous filesystem I/O at lines 271-275 before the delivery task can spawn. |
| GX3-05 | REFUTED | P2 | Lines 425-428 interpolate from compile-time constant slice (408-424). No runtime injection vector; maintainability concern only, severity inflated. / Lines 425-427 interpolate only compile-time constants from the fixed `needed` slice; no attacker-controlled or currently reachable injection path exists. |
| GX3-06 | REFUTED | P2 | Line 681 increments after spawn; docs say "re-queued," log says "re-delivered." Log wording mismatch, not a defect. Semantics match implementation. / Line 681 counts successfully registered/requeued messages; the documented return semantics are re-queued count, not completed deliveries. |
| GX3-07 | REFUTED | P3 | Lines 372-386 build SQL+params conditionally but both blocks mirror same conditionals (376-377, 379-380 match 384-386). Compiler can't verify but human-readable pairing is clear. / Lines 372-386 currently keep SQL placeholders and parameter order synchronized; this is hypothetical future-edit risk, not a reachable defect. |
