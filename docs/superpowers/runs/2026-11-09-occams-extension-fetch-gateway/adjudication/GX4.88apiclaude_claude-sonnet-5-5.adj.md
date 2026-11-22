# Adjudication — GX4 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX4-01 | UPHELD | Lines 94-95,108 mask each delta independently; no cross-call state exists to prevent split credential reassembly | P0 |
| GX4-02 | UPHELD | Line 310-311 blocking write_all under admission mutex (manager.rs:1009); close/stop must wait for same lock | P1 |
| GX4-03 | UPHELD | Lines 140-142 unconditionally evict oldest owner when FIFO exceeds 64, regardless of session liveness | P1 |
| GX4-04 | UPHELD | Lines 273-281 return None when rows empty; cursor position change alone triggers no dirty flag | P1 |
| GX4-05 | UPHELD | Line 185 prepend uses next_seq() after RunComplete already claimed seq; emits higher seq before lower | P1 |
| GX4-06 | UPHELD | Lines 782-789 remove session before kill; line 789 s.kill() has no Result check; kill failure invisible | P1 |
| GX4-07 | UPHELD | Line 268 clone_killer only sends SIGHUP (portable-pty confirmed); no verification child actually exited | P2 |
| GX4-08 | UPHELD | Lines 694-700 drop master but reader holds cloned fd (portable-pty try_clone confirmed); blocking read persists | P2 |
| GX4-09 | UPHELD | Lines 271-276 early-return after spawn_command leaves child without kill/wait; std::process::Child drop doesn't reap | P2 |
| GX4-10 | UPHELD | Lines 578-592 thread spawn errors silently dropped via .ok(); no reader/waiter means no output/reaping | P2 |
| GX4-11 | UPHELD | Lines 98-99 Grid::new uses default 1000 instead of inheriting current scrollback_limit from existing grid | P2 |
| GX4-12 | UPHELD | Lines 671-678 only pops one when >= cap before inserting one; live count never shrinks below old cap | P2 |
