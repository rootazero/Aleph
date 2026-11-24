# FIX-IF3 — region-splice track (raw API)

First-wave provenance / 首轮来源: model `88apiclaude/claude-sonnet-5-5`, one API call per finding. Second-wave changes below use branch `review/occams-ext-fx-fixes`; other first-wave statuses are preserved, not re-audited. / 下述为第二轮修复；其他首轮状态仅保留，未重新审计。

- IF3-05: FIXED
- IF3-01: FIXED
- IF3-08: FIXED
- IF3-09: FIXED
- IF3-02: FIXED
- IF3-14: FIXED
- IF3-17: FIXED
- IF3-18: FIXED
- IF3-10: FIXED
- IF3-06: FIXED
- IF3-07: FIXED
- IF3-16: FIXED
- IF3-21: FIXED

## IF3-03 — FIXED — Versioned `hex-v1:` encoding before encryption and matching decoding after decryption; legacy/unversioned/corrupt entries fail closed and require re-authentication; `test_auth_roundtrip` covers all byte values. / 加密边界使用版本化无损编码，旧数据不回退明文，二进制往返测试已扩充。
## IF3-04 — FIXED — Both fake and production inbound loops intersect `allowed_chats` before the existing access policy; `allowed_chats_intersects_access_policy` covers restricted, empty-list, and disabled-DM cases. / 聊天限制与既有授权取交集，不另建授权表。
## IF3-11 — FIXED — Failed Feishu captions return terminal `Internal` with the delivered image ID and failed text leg, never a retryable whole-message error; `caption_failure_reports_sent_image_without_retrying_it` checks HTTP 429/403 and one image send. / 附文失败显式报告部分送达，保留图片 ID，避免重复发图。
## IF3-12 — FIXED — WhatsApp channel and direct real-runtime send reject attachments before sending text; media capabilities are disabled; `unsupported_attachments_fail_before_any_text_is_sent` covers four MIME types with/without captions plus text-only success. / 未实现媒体明确拒绝，不再成功丢弃附件。
## IF3-13 — FIXED — Inbound image/document/video captions survive alongside explicit unsupported-media text; audio/voice and unknown bodies no longer become silent empty turns; `unsupported_media_preserves_captions_and_identity_without_empty_turns` covers identity and self-message filtering. / 保留说明文字并显式暴露媒体不支持，保持身份和自身消息过滤。
## IF3-15 — FIXED — Channel send preserves runtime `NotConnected` and `SendFailed`; ambiguous errors stay `Internal`, queue policy unchanged; `runtime_not_connected_stays_retryable_at_channel_boundary` checks the real disconnected runtime. / 保留断线错误类型，不扩大队列重试权限。
## IF3-19 — FIXED — Removed the mapper's write into the unconsumed history cache, keeping the existing argument/API and dormant utility; `mapping_group_messages_does_not_fill_unconsumed_history` checks distinct group JIDs. / 切除未接线历史缓存的生产写入，不新增上下文机制。
## IF3-20 — FIXED — Runtime retains a serialized supervisor through Starting/Running and awaits real `BotHandle::shutdown`; channel stop cancels and joins its event task before reuse; `stop_joins_starting_runtime_and_suspended_channel_loop` checks owned-start shutdown and a suspended old loop. / 持有并等待生命周期任务，禁止旧代在新代启动后回写。

Validation / 验证: `rustfmt --check --edition 2021 --config skip_children=true` passed for the five touched Rust files; scoped `git diff --check` passed. Focused tests are written but NOT executed: no Cargo check/test/clippy/build was run, as requested. / 五个修改文件格式检查及局部差异空白检查通过；遵照要求未运行 Cargo，未确认类型检查或测试通过。

Limits / 未做: no WhatsApp upload/download or transcription, no live WhatsApp/Feishu integration, no automatic migration of unversioned auth entries, no history injection, no commits/stash/history changes. WhatsApp auth entries without `hex-v1:` require re-authentication; stopping cancels any pending channel acknowledgement. / 未实现媒体上传下载、转录、真实平台验证、旧凭据自动迁移或历史注入；停止时会取消尚未结束的确认动作，旧格式凭据需重新认证。
