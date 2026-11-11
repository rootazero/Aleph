# Classification & routing — IF5 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF5-01 | FIX-NOW | local-edit | medium | 无效签名必须拒绝，哈希一致性与 allowlist 检查仍须保留。 |
| IF5-02 | FIX-NOW | multi-site | medium | 未支持加密前必须拒绝私信，绝不能明文降级。 |
| IF5-03 | FIX-NOW | multi-site | low | Debug 必须脱敏，但 serde wire 格式不得变化。 |
| IF5-04 | FIX-NOW | multi-site | medium | 私聊使用 sender，群聊使用非空 room，保持会话隔离。 |
| IF5-05 | FIX-NOW | multi-site | high | 必须按 event ID 关联结果，并区分拒绝与不确定失败。 |
| IF5-06 | FIX-NOW | multi-site | low | Identify 前读取共享 manager 的当前 token，不得另建缓存。 |
| IF5-07 | FIX-NOW | multi-site | medium | 媒体能力关闭时必须显式拒绝附件，不能静默丢正文。
