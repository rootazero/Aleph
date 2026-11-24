# Classification & routing — IF4 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF4-01 | FIX-NOW | multi-site | high | 认证前必须建立并验证 TLS，绝不明文发送密码 |
| IF4-02 | FIX-NOW | local-edit | medium | 只读取根 message 标签属性，绝不信任正文 |
| IF4-03 | FIX-NOW | multi-site | low | Debug 必须遮蔽密码、令牌和签名密钥 |
| IF4-04 | FIX-NOW | local-edit | medium | 参与者匹配只能命中一对一聊天 |
| IF4-05 | FIX-NOW | multi-site | medium | 批次失败时绝不提交已前移水位 |
| IF4-06 | FIX-NOW | multi-site | medium | 推进水位前必须排空同毫秒分页 |
| IF4-07 | FIX-NOW | multi-site | high | LINE 禁用策略和名单必须由 Core 统一授权 |
| IF4-08 | FIX-NOW | local-edit | low | 多片编辑必须明确拒绝，不能静默截断 |
| IF4-09 | FIX-CAREFULLY | multi-site | medium | 群回复必须使用 chat_guid 路由到群聊 |
| IF4-10 | FIX-CAREFULLY | multi-site | medium | 发送副作用前必须拒绝不支持的附件 |
| IF4-11 | DEFER | multi-site | high | 阻塞数据库批次不得运行在 Tokio worker |
| IF4-12 | FIX-CAREFULLY | multi-site | medium | 监听就绪或注册失败必须向启动方传播 |
| IF4-13 | FIX-CAREFULLY | multi-site | medium | 远程模式绝不能猜测 localhost 回调地址 |
| IF4-14 | FIX-CAREFULLY | local-edit | low | 目标解析必须分页直至命中或耗尽 |
| IF4-15 | FIX-CAREFULLY | multi-site | low | 同步清扫必须移入 spawn_blocking |
| IF4-16 | FIX-CAREFULLY | local-edit | medium | 下载过程中必须限制实际累计字节 |
| IF4-17 | FIX-CAREFULLY | local-edit | low | 专用目录失败时不得落盘到无所有权目录 |
| IF4-18 | FIX-CAREFULLY | local-edit | low | 默认路径必须匹配带前导斜杠的请求 |
| IF4-19 | FIX-CAREFULLY | local-edit | low | wire 字段必须使用实际 camelCase 名称 |
| IF4-20 | REJECT | no-code-change | none | refuted by adjudication |
| IF4-21 | DEFER | multi-site | high | 媒体消息不能被报告为成功空消息 |
| IF4-22 | FIX-CAREFULLY | local-edit | medium | 未发送附件不能返回成功结果 |
| IF4-23 | FIX-CAREFULLY | multi-site | medium | 连接必须受并发上限和读取截止时间约束 |
| IF4-24 | DEFER | multi-site | high | 必须使用同步层 direct-room 元数据判定私聊 |
| IF4-25 | DEFER | multi-site | high | 平台 mention 事实必须交给 Core 单一决策 |
| IF4-26 | DEFER | multi-site | medium | SQLite 去重操作不得阻塞运行时 worker |
| IF4-27 | FIX-CAREFULLY | local-edit | low | 内存缓存必须有界且持久层定期清理 |
| IF4-28 | FIX-CAREFULLY | multi-site | medium | 媒体限制必须在读取流时执行 |
| IF4-29 | FIX-CAREFULLY | local-edit | low | M
