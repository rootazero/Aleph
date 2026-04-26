//! 通用 Channel Trait 契约测试框架
//!
//! 为所有 Channel 实现提供统一的契约验证，无需连接真实平台实例。

use alephcore::gateway::channel::{
    Channel, ChannelError, ChannelStatus, ConversationId, HealthStatus, MessageId, OutboundMessage,
};

/// 运行完整的 Channel 契约测试套件
///
/// 测试内容：
/// 1. 初始状态 = Disconnected
/// 2. health 初始状态 = Healthy，失败计数 = 0
/// 3. start() 后状态变为 Connected（或 Error）
/// 4. capabilities 与实现一致性（typing_indicator、read_receipts、reactions）
/// 5. send() 返回的 SendResult 包含有效 message_id（仅在 Connected 状态）
/// 6. stop() 后状态恢复为 Disconnected
pub async fn test_channel_contract<C: Channel>(mut channel: C) {
    // 1. 初始状态检查
    assert_eq!(
        channel.status(),
        ChannelStatus::Disconnected,
        "Channel 初始状态必须是 Disconnected"
    );

    // 2. health 初始状态
    let health = channel.health().await;
    assert_eq!(
        health.status,
        HealthStatus::Healthy,
        "Channel 初始 health 必须是 Healthy"
    );
    assert_eq!(
        health.failure_count, 0,
        "Channel 初始失败计数必须是 0"
    );

    // 3. start() 状态转换
    let start_result = channel.start().await;
    match start_result {
        Ok(()) => {
            assert_eq!(
                channel.status(),
                ChannelStatus::Connected,
                "start() 成功后状态必须是 Connected"
            );
        }
        Err(_) => {
            assert!(
                matches!(
                    channel.status(),
                    ChannelStatus::Error | ChannelStatus::Disabled
                ),
                "start() 失败后状态必须是 Error 或 Disabled"
            );
        }
    }

    // 4. capabilities 一致性检查
    let caps = channel.capabilities();

    // typing_indicator
    if caps.typing_indicator {
        let result = channel
            .send_typing(&ConversationId::new("test"))
            .await;
        assert!(
            !matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明支持 typing_indicator，但调用返回 UnsupportedFeature"
        );
    } else {
        let result = channel
            .send_typing(&ConversationId::new("test"))
            .await;
        assert!(
            matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明不支持 typing_indicator，但调用未返回 UnsupportedFeature"
        );
    }

    // read_receipts
    if caps.read_receipts {
        let result = channel
            .mark_read(&MessageId::new("test"))
            .await;
        assert!(
            !matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明支持 read_receipts，但调用返回 UnsupportedFeature"
        );
    } else {
        let result = channel
            .mark_read(&MessageId::new("test"))
            .await;
        assert!(
            matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明不支持 read_receipts，但调用未返回 UnsupportedFeature"
        );
    }

    // reactions
    if caps.reactions {
        let result = channel
            .react(
                &ConversationId::new("test"),
                &MessageId::new("test"),
                "👍",
            )
            .await;
        assert!(
            !matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明支持 reactions，但调用返回 UnsupportedFeature"
        );
    } else {
        let result = channel
            .react(
                &ConversationId::new("test"),
                &MessageId::new("test"),
                "👍",
            )
            .await;
        assert!(
            matches!(result, Err(ChannelError::UnsupportedFeature(_))),
            "capabilities 声明不支持 reactions，但调用未返回 UnsupportedFeature"
        );
    }

    // 5. send() 返回格式检查（仅在 Connected 状态下）
    if channel.status() == ChannelStatus::Connected {
        let result = channel
            .send(OutboundMessage::text("test-conv", "hello"))
            .await;
        if let Ok(send_result) = result {
            assert!(
                !send_result.message_id.as_str().is_empty(),
                "send() 返回的 message_id 不能为空"
            );
        }
    }

    // 6. stop() 后状态转换
    let stop_result = channel.stop().await;
    stop_result.ok(); // stop 允许失败
    assert_eq!(
        channel.status(),
        ChannelStatus::Disconnected,
        "stop() 后状态必须是 Disconnected"
    );
}

/// 运行简化的契约测试（不调用 start/stop，仅验证静态属性）
pub fn test_channel_properties<C: Channel>(channel: &C) {
    // info 非空检查
    let info = channel.info();
    assert!(!info.id.as_str().is_empty(), "Channel ID 不能为空");
    assert!(!info.name.is_empty(), "Channel name 不能为空");
    assert!(!info.channel_type.is_empty(), "Channel type 不能为空");

    // id() 和 channel_type() 与 info 一致
    assert_eq!(channel.id(), &info.id);
    assert_eq!(channel.channel_type(), info.channel_type.as_str());

    // capabilities 非空
    let _caps = channel.capabilities();

    // inbound_subscribe 可用
    let _rx = channel.inbound_subscribe();
}
