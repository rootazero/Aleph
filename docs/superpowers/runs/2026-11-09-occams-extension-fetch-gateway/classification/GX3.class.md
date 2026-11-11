# Classification & routing — GX3 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| GX3-01 | FIX-NOW | multi-site | medium | Preserve mutex scoping and avoid awaiting with a non-Send guard held. |
| GX3-02 | FIX-NOW | local-edit | low | Delete matching FTS rows before source messages, preserving cleanup ordering. |
| GX3-03 | REJECT | no-code-change | none | refuted by adjudication |
| GX3-04 | FIX-CAREFULLY | local-edit | medium | Keep journal ordering ahead of delivery without blocking the Tokio worker. |
| GX3-05 | REJECT | no-code-change | none | refuted by adjudication |
| GX3-06 | REJECT | no-code-change | none | refuted by adjudication |
| GX3-07 | REJECT | no-code-change | none | refuted by adjudication |
