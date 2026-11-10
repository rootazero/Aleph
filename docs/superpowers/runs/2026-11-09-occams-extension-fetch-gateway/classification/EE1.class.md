# Classification & routing — EE1 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | Note |
|---|---|---|---|---|
| EE1-01 | FIX-NOW | local-edit | low | Preserve timeline shape, tool/result correlation, and termination metadata semantics. |
| EE1-02 | FIX-NOW | multi-site | high | Do not alter permit ordering or create a second cancellation authority. |
| EE1-03 | DEFER | multi-site | high | Failed claims must retain a recoverable barrier and never clear replacement goals. |
| EE1-04 | FIX-CAREFULLY | local-edit | low | Reuse existing terminal settlement; do not introduce duplicate run-state cleanup. |
