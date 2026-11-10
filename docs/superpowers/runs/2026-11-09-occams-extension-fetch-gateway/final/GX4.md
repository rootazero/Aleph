# Final verdicts — GX4

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX4-01 | UPHELD | P0 | Lines 94-95,108 mask each delta independently; no cross-call state exists to prevent split credential reassembly / Lines 94–97 and 106 mask `delta`/`content` independently, so a credential split across chunks is never recognized. |
| GX4-02 | DOWNGRADE | P1 | Line 310-311 blocking write_all under admission mutex (manager.rs:1009); close/stop must wait for same lock / Lines 310–313 perform blocking `write_all`, but this excerpt does not prove it executes under the manager’s global admission lock. |
| GX4-03 | UPHELD | P1 | Lines 140-142 unconditionally evict oldest owner when FIFO exceeds 64, regardless of session liveness / Lines 140–143 unconditionally evict the oldest owner stamp, including one belonging to a still-live session. |
| GX4-04 | UPHELD | P1 | Lines 273-281 return None when rows empty; cursor position change alone triggers no dirty flag / Lines 273–281 omit cursor-position changes from the news test, so cursor-only movement produces no patch. |
| GX4-05 | UPHELD | P1 | Line 185 prepend uses next_seq() after RunComplete already claimed seq; emits higher seq before lower / Line 185 allocates a new sequence for the prepended final chunk; forwarding the earlier terminal event afterward reverses sequence order. |
| GX4-06 | UPHELD | P1 | Lines 782-789 remove session before kill; line 789 s.kill() has no Result check; kill failure invisible / Lines 784–790 remove the session, ignore `s.kill()`’s result, and return `Ok(())`, falsely reporting successful termination. |
| GX4-07 | UPHELD | P2 | Line 268 clone_killer only sends SIGHUP (portable-pty confirmed); no verification child actually exited / Lines 268 and 789 treat a successful SIGHUP delivery as conclusive termination without verifying or escalating process exit. |
| GX4-08 | UPHELD | P2 | Lines 694-700 drop master but reader holds cloned fd (portable-pty try_clone confirmed); blocking read persists / Lines 694–700 drop only the original master handle; cloned reader/writer descriptors may keep the blocking read alive. |
| GX4-09 | UPHELD | P2 | Lines 271-276 early-return after spawn_command leaves child without kill/wait; std::process::Child drop doesn't reap / Lines 257–276 start the child before fallible reader/writer acquisition; subsequent `?` exits without explicit kill or reap. |
| GX4-10 | UPHELD | P2 | Lines 578-592 thread spawn errors silently dropped via .ok(); no reader/waiter means no output/reaping / Lines 578–580 show reader thread creation can fail, but the excerpt does not show whether its `Err` is discarded. |
| GX4-11 | UPHELD | P2 | Lines 98-99 Grid::new uses default 1000 instead of inheriting current scrollback_limit from existing grid / Lines 98–99 create alternate grids with `Grid::new`, which does not inherit the configured scrollback limit. |
| GX4-12 | UPHELD | P2 | Lines 671-678 only pops one when >= cap before inserting one; live count never shrinks below old cap / Lines 671–677 evict exactly one session before inserting one, so lowering the cap cannot shrink an oversized live set. |
