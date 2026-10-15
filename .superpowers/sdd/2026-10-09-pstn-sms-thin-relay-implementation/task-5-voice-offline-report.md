# Task 5：Voice-call thin relay offline seam / 电话薄 Relay 离线接缝报告

## Scope / 范围

本报告只记录 Task 5 的 **纯离线 Core-side voice-call reducer/transport seam**。实现位于：

- `src/gateway/interfaces/voice_call/protocol.rs`
- `src/gateway/interfaces/voice_call/transport.rs`
- `src/gateway/interfaces/voice_call/mod.rs`
- `tests/voice_call_protocol_test.rs`

它消费现有版本化电信 wire 类型 `shared/protocol/src/telecom_relay.rs`，但不把离线命令日志或 reducer 状态解释为供应商已交付、已接通、已播放或真实线路效果。

## Implemented seam / 已实现接缝

### 1. Versioned envelope identity and generation fences / 版本化信封身份与 generation 栅栏

`src/gateway/interfaces/voice_call/protocol.rs` 中的 `CallSession` 固定并校验：

- `provider_account_id`
- `phone_call_id`
- `provider_call_id`
- `binding_id`
- 非零 `connection_generation`

每个 `TelecomEnvelope` 在进入 reducer 前都必须通过 `check_version()`，并且 envelope 的 `binding_id` 与 `connection_generation` 必须分别匹配当前已认证会话。Call/provider/account 身份也会在 transcript、output 和 playback event 上重新校验；错误的 binding、call、provider call、account 或 generation fail closed。

`src/gateway/interfaces/voice_call/transport.rs` 中的 `OfflineCallTransport::start_call` 只允许一个 active call；结束后只接受严格递增的 `connection_generation`。旧 generation 不能重新取得活跃会话。这个 transport 只保存 Core 本地状态和待交给未来 adapter 的 `CallCommand`，没有网络或 provider 实现。

### 2. Transcript role and epoch sequencing / transcript role 与 epoch 顺序

- 只有 `CallTurnRole::UserTranscript` 才能作为入站 transcript；`AssistantText` 不能伪装成用户 turn。
- `TranscriptState::Interim` 只返回 `ApplyOutcome::InterimIgnored`，不会创建 turn，也不会触发 agent/tool turn。
- committed transcript 以 `turn_id`、`output_epoch` 和 transcript-side `output_seq` 去重；同一 turn 的更高 transcript sequence 可更新 committed transcript，重复或较旧 sequence 不会覆盖已提交内容。
- 低于当前 active turn 的 `output_epoch` 的迟到 transcript/playback event 返回 `Ignored(StaleOutputEpoch)`，不改变状态。
- 更高 epoch 才能开始新 turn；开始新 turn 会清空旧 turn 的 queued output。相同 epoch 不能借新 `turn_id` 启动另一个 turn，也不能抹掉 active turn 的 generated/queued output。
- assistant output 的 `output_seq` 独立于 committed transcript sequence；两者不会因共享数字字段而互相推进。

### 3. Generated vs queued vs playback facts / 生成、排队、播放事实分离

`TurnState` 明确保存三类不同事实：

1. `record_generated_output`：Core 已生成文本；只更新 `generated_text` 和按 `output_seq` 保存的 generated chunk，不声称 carrier 已接收。
2. `record_output_queued`：收到显式 carrier queue acknowledgement 后才更新 `queued_text`；未知或内容不一致的 output sequence 被拒绝。
3. `publish_playback_ack`：只在收到明确 playback event 后记录 `PlaybackState` 与 `PlayedRange`；函数调用成功、发送 command 或生成文本都不会伪造“用户听到”。

`send_text_output` 只接受当前 turn、当前 epoch 中已生成且内容精确匹配的 output，并向本地 `CallCommand::SendText` 记录一条待外部 adapter 处理的 command；它不会自行写入 queued 或 played 状态。

### 4. Per-output-seq playback ranges / 每个 output sequence 的播放范围

播放事实按 `output_seq` 保存在 `PlaybackFact` 中，而不是只挂在 turn 上。`played_text_for_output(output_seq)` / `heard_text_for_output(output_seq)` 只返回该 chunk 被 carrier 精确确认的 Unicode scalar-value 前缀；只有 `PlaybackState::Played` 且 `Exact.chars` 不超过该 generated chunk 的 Unicode scalar 数量时才变为 `PlayedRange::Exact`；`Queued`、`Started`、`Cleared`、`Failed`、`None`、`Uncertain` 及过长 `Exact` 均 fail closed 为 `PlayedRange::Unknown`。

一个 chunk 的 ACK 只释放或更新该 chunk：`Played`、`Cleared`、`Failed` 会从 queued 集合中移除对应 sequence，但不会错误地释放相邻 chunk。每个 output sequence 的 playback lifecycle 只允许 `Queued → Started → Played` 或 `Queued/Started → Cleared|Failed`；重复、回退和终态覆盖均被忽略。终态 `Played/Cleared/Failed` 也会阻止同 sequence 的迟到 send/queue acknowledgement。显式取消或首次 `Cleared` 后的晚到 playback 也只会被忽略，不会复活 heard/queue fact；即使 `Cleared` 指向未知 sequence，也会先建立整个 epoch 的 fence。多 chunk turn 在不能表达精确 chunk 范围时保持 turn-level `played_range == Unknown`；不会把某个 chunk 的局部 ACK 误报为整段 turn 已播放。新的不确定/缺失范围也不会沿用旧的 exact range。

### 5. Cancellation/no-replay and same-epoch turn fences / 取消、禁止重播与同 epoch turn 栅栏

`cancel_output(phone_call_id, output_epoch)` 先验证具体 call 和 active epoch，再永久记录该 epoch 的取消状态并产生一个 `CallCommand::CancelOutput`。取消后的 generated output 即使收到迟到 playback event，也不能再次通过 `send_text_output` 产生第二次发送 command。

`PlaybackState::Cleared` 同样永久 fence 当前 output epoch；取消或清空后，迟到的本地 generated/queued acknowledgement 也不会再写入 generated/queued facts。清空动作在 reducer 中不会被当作“可安全重播”；旧 epoch 的迟到 ACK 只会被忽略。新 turn 使用新的 epoch，因而不会复用已取消/已清空的 output。

### 6. Side-effect ledger and hangup / 副作用账本与挂断

`commit_tool_side_effect` 要求精确的 `turn_id` 与 `output_epoch`，并把非空 marker 写入 session-level ledger 与当前 turn snapshot。turn interruption/replacement 只清理旧输出，不回滚已发生的 tool side effect；新 active turn 会继承该 ledger snapshot。这是本地状态事实，不是对外部副作用的撤销声明。

`hangup` 是本地 unconditional off-switch：清理 active queued output、移除 active turn、将 session 标记为 `Unavailable`，并向 command log 写入 `CallCommand::Hangup`。挂断后任何后续事件或再次挂断都 fail closed 为 `SessionUnavailable`；不会在重连时自动恢复旧 turn 或旧输出。

## Verification evidence / 验证证据

本会话已有以下离线验证证据：

- `cargo test -p alephcore --test voice_call_protocol_test --features test-helpers -- --test-threads=1`：**22 passed, 0 failed**；完整日志：`/tmp/aleph-telecom-task14/voice-final-4.log`。此前 13/15/19-test 日志已被后续 22-test 验证 supersede。
- 日志中的 22 个测试覆盖 interim transcript、旧 epoch 丢弃与新 turn 清队列、精确/不确定/非 Played 或过长 playback range、per-output-seq ACK、单调 playback lifecycle、终态回退拒绝、终态后的 send/queue 拒绝、未知 Cleared 建立 epoch fence、transcript/assistant sequence 分离、generated/queued/playback 三事实、取消后的 generated/queued/playback no-mutation 与 no-replay、Cleared fence、same-epoch turn fence、副作用账本、hangup，以及 identity/generation/cancel scope 拒绝。
- 本会话记录的 changed-file `rustfmt` 检查通过。
- 本会话记录的 scoped `git diff --check` 通过。

另外，受同一 catalog byte ratchet 影响的精确单测 `executor::builtin_registry::definitions::tests::catalog_description_bytes_ratchet` 已通过 1/1（日志：`/tmp/aleph-telecom-task14/catalog-ratchet-final.log`）；这不是全库验证。以上是 targeted offline protocol test 和格式/diff hygiene 证据；不表示 workspace 或整个库的验证已通过。

## Explicit non-scope / 明确未完成事项

本报告**没有**实现或证明：

- provider I/O、供应商 webhook/验签、媒体编解码、VAD、ASR/TTS 或真实音频播放；
- Relay lease/factory、部署连接、Core 主动认证连接的生产 adapter；
- live PSTN/SIP/Twilio/其他运营商线路、真实手机效果或费用行为；
- call coordinator、工具注册、A/B 审批、公共上下文/历史隔离或 startup wiring；
- `src/gateway/voice/streaming/mod.rs`、`src/gateway/voice/inbound/mod.rs`、`src/gateway/voice/outbound.rs` 的生产接入；
- G1–G5 证据、双地区线路矩阵或 Task 6 的综合验收。

因此本文件不作 production claim，也不声称 Task 5 的生产 transport/lifecycle 已完成；它只证明上述纯 reducer/transport seam 在离线测试夹具中的行为。没有运行 cargo/stage/commit，也没有修改三个受保护的 dirty docs；本次交付只新增本报告文件。
