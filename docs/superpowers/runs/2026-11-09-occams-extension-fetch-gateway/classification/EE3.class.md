# Classification & routing — EE3 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| EE3-01 | FIX-NOW | local-edit | medium | Reject escaping symlinks; never fall through to a different candidate. |
| EE3-02 | FIX-NOW | local-edit | medium | Preserve agent and project candidate context inside spawn_blocking. |
| EE3-03 | FIX-CAREFULLY | local-edit | low | Preserve install-once global semantics and unique session keys. |
| EE3-04 | FIX-CAREFULLY | needs-new-test | low | Exercise the real request gate, not immutable metadata snapshots. |
