# Moltbot 架构对齐更新

**日期**: 2025-01-28

## 概述

本次更新将 Aleph 的 CLAUDE.md 与 Moltbot 的最新架构进行了全面对齐，补充了缺失的关键设计模式。

## 更新内容

### 1. 进度状态更新

将"目标功能清单"更新为"实现进度"，标记了已完成（✅）和待完成（🔲）的功能：

- **Phase 1 Gateway**: 全部完成
- **Phase 2 Channels**: Telegram、Discord、iMessage、WebChat、CLI 完成，Slack、WhatsApp 待实现
- **Phase 3 Agent Runtime**: RPC Mode、Tool Streaming、Thinking Levels、Model Failover 完成
- **Phase 4 Tools**: Browser、Cron 完成，Canvas、Webhooks、Sessions Tools 待实现
- **Phase 5 Nodes**: macOS App 完成，iOS/Android 待实现
- **Phase 6 Voice & Media**: 全部待实现

### 2. 新增章节

#### Gateway RPC 方法体系

借鉴 Moltbot 的 200+ 方法按域分组模式，文档化了 HandlerRegistry 设计：
- health、auth、agent、session、channel、events、config、browser、cron 等域
- RPC 方法注册模式说明

#### WebSocket 连接握手协议

对齐 Moltbot 的连接协议：
- 第一帧必须是 connect 请求
- 设备身份验证
- 客户端角色（operator / node）
- 消息类型（req / res / event / stream）

#### Session Key 层级

文档化已实现的 6 种 session key 变体：
- Main、DirectMessage、Group、Task、Subagent、Ephemeral
- DM Scope 策略（Main / PerPeer / PerChannelPeer）

#### Plugin & Extension 系统

定义插件架构目标：
- Channels as Plugins
- Hook System
- Extension Registry
- Schema Registration

#### 多 Agent 编排

文档化 AgentInstance 隔离和 Sub-Agent 委托机制

#### CLI 命令体系

定义完整的命令行接口

### 3. 更新章节

- **项目结构**: 反映实际代码结构而非"目标结构"
- **技术栈**: 添加 axum、sqlite-vec、schemars 等已用依赖
- **配置系统**: 升级为 JSON5 格式，添加配置热更新说明
- **Quick Commands**: 更新为 feature-gated 构建命令
- **Reference**: 添加 Moltbot src/config/ 和 src/routing/ 路径

## 关键差距（待实现）

1. **Slack Channel** - 待实现
2. **WhatsApp Channel** - 待实现（Baileys-style）
3. **Block Streaming** - `<think>` 流式输出
4. **Canvas (A2UI)** - Agent 驱动的可视化工作区
5. **Webhooks** - 外部触发器
6. **Sessions Tools** - Agent 间通信
7. **Node Protocol** - iOS/Android 节点协议
8. **Voice & Media** - 语音唤醒、转录

## 下一步

根据 CLAUDE.md 末尾的 Key Context，当前聚焦：
- **Session Key 实现完成**
- **下一步: Agent 间通信 (Part B)**
