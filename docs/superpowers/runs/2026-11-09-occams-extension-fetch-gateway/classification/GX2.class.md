# Classification & routing — GX2 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| GX2-01 | FIX-NOW | local-edit | medium | Create the new epoch before closing or acknowledging the old session. |
| GX2-02 | REJECT | no-code-change | none | refuted by adjudication |
| GX2-03 | REJECT | no-code-change | none | refuted by adjudication |
| GX2-04 | REJECT | no-code-change | none | refuted by adjudication |
| GX2-05 | FIX-CAREFULLY | local-edit | medium | Bind background topic generation to the newly created epoch. |
| GX2-06 | REJECT | no-code-change | none | refuted by adjudication |
| GX2-07 | REJECT | no-code-change | none | refuted by adjudication |
| GX2-08 | FIX-CAREFULLY | local-edit | low | Keep identity failures separate from timestamp failures. |
