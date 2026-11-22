# Adjudication — GX4 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX4-01 | UPHELD | Lines 94–97 and 106 mask `delta`/`content` independently, so a credential split across chunks is never recognized. | P0 |
| GX4-02 | DOWNGRADE | Lines 310–313 perform blocking `write_all`, but this excerpt does not prove it executes under the manager’s global admission lock. | P2 |
| GX4-03 | UPHELD | Lines 140–143 unconditionally evict the oldest owner stamp, including one belonging to a still-live session. | P1 |
| GX4-04 | UPHELD | Lines 273–281 omit cursor-position changes from the news test, so cursor-only movement produces no patch. | P1 |
| GX4-05 | UPHELD | Line 185 allocates a new sequence for the prepended final chunk; forwarding the earlier terminal event afterward reverses sequence order. | P1 |
| GX4-06 | UPHELD | Lines 784–790 remove the session, ignore `s.kill()`’s result, and return `Ok(())`, falsely reporting successful termination. | P1 |
| GX4-07 | UPHELD | Lines 268 and 789 treat a successful SIGHUP delivery as conclusive termination without verifying or escalating process exit. | P2 |
| GX4-08 | UPHELD | Lines 694–700 drop only the original master handle; cloned reader/writer descriptors may keep the blocking read alive. | P2 |
| GX4-09 | UPHELD | Lines 257–276 start the child before fallible reader/writer acquisition; subsequent `?` exits without explicit kill or reap. | P2 |
| GX4-10 | UPHELD | Lines 578–580 show reader thread creation can fail, but the excerpt does not show whether its `Err` is discarded. | P2 |
| GX4-11 | UPHELD | Lines 98–99 create alternate grids with `Grid::new`, which does not inherit the configured scrollback limit. | P2 |
| GX4-12 | UPHELD | Lines 671–677 evict exactly one session before inserting one, so lowering the cap cannot shrink an oversized live set. | P2 |
