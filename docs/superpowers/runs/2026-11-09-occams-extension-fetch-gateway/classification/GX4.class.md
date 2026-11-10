# Classification & routing — GX4 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| GX4-01 | FIX-NOW | multi-site | medium | Preserve attended streaming, event shape, and terminal-event lifecycle. |
| GX4-02 | FIX-NOW | multi-site | high | Never block admission; preserve ordering, generation checks, and stop rejection. |
| GX4-03 | FIX-NOW | local-edit | low | Live sessions must retain creator authorization; unknown exited owners must still fail closed. |
| GX4-04 | FIX-NOW | local-edit | low | Track cursor coordinates without emitting frames for a truly static terminal. |
| GX4-05 | FIX-NOW | multi-site | medium | Keep sequence numbers monotonic and RunComplete exactly once. |
| GX4-06 | FIX-NOW | multi-site | high | Retain failed targets for retry and surface kill errors. |
| GX4-07 | DEFER | multi-site | high | Escalation must preserve Windows behavior, bounded waits, and reap ownership. |
| GX4-08 | DEFER | multi-site | high | Cancellation must drain tail output before releasing reader resources. |
| GX4-09 | FIX-CAREFULLY | multi-site | medium | Every post-spawn error must terminate and reap the child. |
| GX4-10 | FIX-CAREFULLY | needs-new-test | medium | Start both workers or roll back; never report success without reaping. |
| GX4-11 | FIX-CAREFULLY | local-edit | low | Inherit scrollback limits while preserving alternate-grid content semantics. |
| GX4-12 | FIX-CAREFULLY | multi-site | medium | Enforce the cap before insertion and preserve FIFO and kill-failure ownership. |
