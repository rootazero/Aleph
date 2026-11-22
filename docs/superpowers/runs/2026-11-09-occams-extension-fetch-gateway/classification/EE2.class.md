# Classification & routing — EE2 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| EE2-01 | FIX-NOW | multi-site | high | Do not detach dispatch or fake success; preserve journal recovery. |
| EE2-02 | FIX-NOW | local-edit | medium | Preserve successful cleanup, Drop fallback, and artifact/source lifetimes. |
