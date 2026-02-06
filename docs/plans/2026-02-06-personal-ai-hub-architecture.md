# Personal AI Hub Architecture Design

> Aleph 从"通用工具库"到"个人私有智能中枢"的架构升华

**设计日期**: 2026-02-06
**设计版本**: v1.0
**状态**: Production-Ready

---

## 1. 核心定位

### 1.1 产品定位

**Aleph = 个人私有智能中枢 (Personal AI Hub)**

- **部署模式**: 个人/家庭部署,非 SaaS
- **架构理念**: Server-Client 分布式架构
- **数据主权**: 所有核心功能、数据持久化、配置归核在服务端
- **客户端角色**: 纯粹的"壳子" (对话窗口) + "感官与肢体" (本地动作执行器)

### 1.2 使用场景

```
┌─────────────────────────────────────────────────────────────────┐
│                    Aleph Personal Universe                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  [ 物理层 ]          [ 接入层 (Thin Clients) ]      [ 核心层 ]   │
│                                                                  │
│  自用电脑          ┌─ Tauri Desktop App ─┐                      │
│  (Mac/Win)    ←──→ │  Shared SDK inside  │                      │
│                    └─────────────────────┘                      │
│                              │                                   │
│  移动端            ┌─ Future Mobile App ─┐       ┌────────────┐ │
│  (iOS/Android) ←─→ │  Shared SDK inside  │  ←──→ │  Gateway   │ │
│                    └─────────────────────┘       │  (Brain)   │ │
│                              │                   │            │ │
│  社交软件          ┌─ Social Bot Bridge ─┐       │ • Intent   │ │
│  (TG/WA)      ←──→ │  Shared SDK inside  │       │ • Memory   │ │
│                    └─────────────────────┘       │ • Skills   │ │
│                              │                   │ • Config   │ │
│  终端              ┌───── Aleph CLI ──────┐       └────────────┘ │
│  (Remote SSH) ←──→ │  Shared SDK inside  │                      │
│                    └─────────────────────┘                      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**核心理念**: "以家为中心,全网可触达"

- **局域网 (重度交互)**: 极低延迟,本地工具执行 (截屏、文件操作)
- **远程接入 (指令/查询)**: 通过社交机器人、移动端访问家里的"大脑"

---

## 2. 发现机制 (Discovery)

### 2.1 发现阶梯

| 阶梯 | 机制 | 适用场景 |
|------|------|----------|
| **第一阶梯** | mDNS/Zeroconf | 局域网内自动发现 `aleph.local` |
| **第二阶梯** | 配对码/手动输入 | 远程接入 (DDNS/内网穿透) |
| **第三阶梯** | 多实例管理 | 记录多个实例 `(Name, URL, Token)` |

### 2.2 家电化体验

**设计目标**: 像打印机或 HomeKit 设备一样,"打开即发现"

**实现路径**:
1. `clients/shared` SDK 集成 mDNS 扫描器 (基于 `mdns-sd` 库)
2. 客户端启动后自动扫描,显示"发现附近的 Aleph 实例"
3. 点击实例 → 进入配对流程 → 保存到实例列表

### 2.3 远程接入策略

**场景**: 外出时通过手机访问家里的 Aleph

**方案**:
- DDNS (动态域名): `home.example.com:18789`
- 内网穿透: Tailscale / ZeroTier / frp
- 配对码快速接入: 输入 6 位配对码 → SDK 查询实例地址

---

## 3. 配置同步 (Configuration Sync)

### 3.1 同步策略

**核心原则**: Server as Source of Truth

**理由**:
- 一致性 > 零延迟 (50-200ms 延迟在设置类低频操作中可接受)
- 多端联动体验 (一端修改,所有端同步更新)
- 简化客户端逻辑 (无需乐观更新回滚)

### 3.2 配置分层

| 层级 | 同步策略 | 示例 |
|------|----------|------|
| **Tier 1 (Critical)** | Server 强制同步 | API Keys, 工具权限白名单, 安全策略 |
| **Tier 2 (Preferences)** | Server 广播同步 | 主题色、启用的技能列表、快捷键 |
| **Tier 3 (Ephemeral)** | 客户端本地存储 | 窗口位置、日志级别、缓存路径 |

### 3.3 同步流程

```
┌─────────────┐                    ┌─────────────┐
│   Client A  │                    │   Server    │
├─────────────┤                    ├─────────────┤
│             │ ── config.patch ──→│             │
│             │                    │  [验证]     │
│             │                    │  [保存]     │
│             │                    │  [广播]     │
│             │ ←─ config.changed ─│             │
└─────────────┘                    └─────────────┘
       ↑                                  │
       └───────── config.changed ─────────┤
                                          ↓
                                   ┌─────────────┐
                                   │   Client B  │
                                   ├─────────────┤
                                   │ [更新 UI]   │
                                   └─────────────┘
```

### 3.4 离线策略

**原则**: 禁止修改配置,仅允许受限查看

**理由**:
- 瘦客户端在没有"大脑"的情况下不应修改核心配置
- 避免安全隐患和状态不一致风险

**UI 表现**:
- 连接断开时进入"只读/断开模式"
- 配置页面置灰,显示重连提示
- 可查看已缓存的聊天记录

### 3.5 SDK 实现

**ConfigManager** (`clients/shared/config/`):

```rust
pub struct ConfigManager {
    gateway: Arc<Gateway>,
    local_config: RwLock<ConfigTree>,
    subscribers: Vec<Box<dyn Fn(ConfigChangedEvent)>>,
}

impl ConfigManager {
    /// 连接成功后加载完整配置
    pub async fn load_full_config(&self) -> Result<Config>;

    /// 申请配置变更
    pub async fn apply_patch(&self, patch: JsonValue) -> Result<()>;

    /// 订阅配置变更事件
    pub fn on_config_changed(&mut self, callback: impl Fn(ConfigChangedEvent) + 'static);
}
```

---

## 4. 用户模型 (User Model)

### 4.1 核心策略

**选择**: Owner + Limited Guests (混合模式)

**理由**:
- 选项 A (绝对私有) 过于封闭,无法利用社交连接优势
- 选项 B (家庭共享) 过于臃肿,需要完整的多用户系统
- **选项 C** 平衡了绝对隐私与社交便利性

### 4.2 角色定义

| 角色 | 认证方式 | 权限 | 数据访问 |
|------|----------|------|----------|
| **Owner** | 设备级信任 (Public Key) | 完全控制 | 所有数据 |
| **Guest** | JWT 风格 Token (TTL + Scope) | 受限权限 | 仅自己的 Session |
| **Anonymous** | 无认证 | 拒绝访问 | - |

### 4.3 数据隔离边界

#### Memory Facts (核心记忆)
- **Owner**: 私有记忆库,完全控制
- **Guest**: 默认无法访问 Owner 记忆
- **Shared Facts**: Owner 可标记特定 Fact 为 `Shared`,允许特定 Guest 检索

#### Session 历史
- **按 Token 隔离**: 每个 Guest 只能看到自己的对话历史
- **Owner 后台权限**: 可查看所有 Session 元数据,可选查看详细内容 (配置项控制)

#### 工具执行权限
- **高危工具** (ServerOnly): `shell:exec`, `file:write` → 仅限 Owner
- **逻辑工具** (可授权): `translate`, `web_search`, `summarize` → 可授予 Guest

### 4.4 AuthToken 生命周期

#### Owner 设备
- **类型**: 设备级信任 (Permanent Pairing)
- **认证**: Public Key + Device Fingerprint
- **管理**: 支持"踢出设备" (Revoke Device)

#### Guest 访客
- **类型**: JWT 风格 Token
- **内容**: `{ user_id, role, scope, exp, iat }`
- **TTL**: 可配置 (1h / 24h / 30d)
- **Scope**: 权限列表 `["translate", "weather"]`

### 4.5 社交身份映射

**IdentityMap** (`core/src/gateway/identity_map.rs`):

```rust
pub struct IdentityMap {
    // Platform:UserId -> InternalUserId
    mappings: HashMap<String, UserId>,
}

// 示例映射
"telegram:123456" -> UserId::Owner
"telegram:789012" -> UserId::Guest("Mom")
"whatsapp:+86..." -> UserId::Guest("Dad")
```

**未识别用户处理**:
- 陌生人私聊 Bot → 触发配对请求
- Owner 多端收到通知 → 授权/拒绝
- 授权后自动添加到 IdentityMap

---

## 5. 访客管理 (Guest Management)

### 5.1 交互策略

**核心理念**: Conversation-First, Visual-Fallback

| 场景 | 推荐方式 | 理由 |
|------|----------|------|
| 快速邀请 | 对话式 (`/invite 1h translate`) | 符合 Ghost 美学,跨平台一致 |
| 查看列表 | Desktop 侧边栏 / CLI | 视觉化管理,一目了然 |
| 批量操作 | Desktop 面板 | 需要可视化确认 |

### 5.2 对话式管理

**示例交互**:

```
Owner: "给我妈妈生成一个访客邀请,只能用翻译和天气查询,有效期 1 个月"
Aleph: "已生成邀请链接: https://aleph.local/invite/abc123
       权限: translate, weather_query
       过期时间: 2026-03-06"

Owner: "列出所有访客"
Aleph: "当前有 2 个活跃访客:
       1. 妈妈 (TG: +86...) - translate, weather - 28天后过期
       2. 临时演示 (Web) - chat_only - 2小时后过期"

Owner: "撤销临时演示的访问"
Aleph: "已撤销访客 '临时演示' 的 Token"
```

### 5.3 视觉辅助管理

#### Desktop App (macOS/Tauri)
- 菜单栏图标右键 → "访客管理"
- 弹出轻量级侧边栏面板 (符合 Ghost 美学)
- 显示活跃访客列表:
  - 姓名、来源 (TG/Web)、权限、剩余时间
  - 快速操作按钮: 延期、修改权限、撤销

#### CLI
```bash
aleph guests list
aleph guests invite --scope translate,weather --ttl 30d --name "Mom"
aleph guests revoke <guest_id>
aleph guests update <guest_id> --scope full_guest
```

### 5.4 邀请链接生成

**方案**: 加密参数 URL + QR Code

**流程**:
1. Owner 发起邀请 (对话/UI)
2. SDK 生成加密 Token: `HMAC(guest_config, server_secret)`
3. 生成短链接: `https://aleph.local/join?t=<encrypted_token>`
4. 生成 QR Code (移动端扫码)

**安全性**:
- **一次性使用**: Token 激活后立即失效
- **短 TTL**: 邀请链接本身 15 分钟有效 (激活后 Guest Token 按设定 TTL)
- **签名验证**: 防止伪造

### 5.5 社交机器人配对流程

**场景**: 陌生人私聊 Telegram Bot

**流程**:

```
┌──────────────┐                  ┌──────────────┐                  ┌──────────────┐
│  Stranger    │                  │  Aleph Bot   │                  │    Owner     │
├──────────────┤                  ├──────────────┤                  ├──────────────┤
│              │ ── "Hello" ────→ │              │                  │              │
│              │                  │ [查询 Map]   │                  │              │
│              │                  │ [未找到]     │                  │              │
│              │                  │              │ ── 通知请求 ───→ │              │
│              │ ←─ 礼貌回复 ──── │              │   "User @xxx     │              │
│              │   "已向 Owner    │              │    请求访问"     │              │
│              │    发送请求"     │              │                  │ [批准: 家庭]  │
│              │                  │              │ ←── 授权决定 ──── │              │
│              │                  │ [生成 Token] │                  │              │
│              │                  │ [更新 Map]   │                  │              │
│              │ ←─ "已授权" ──── │              │                  │              │
│              │ ── "翻译..." ──→ │              │                  │              │
│              │ ←─ "Translation" │              │                  │              │
└──────────────┘                  └──────────────┘                  └──────────────┘
```

**Owner 多端通知**:
- Desktop: 系统通知 + 侧边栏提示
- CLI: 终端弹出提示 (如果在运行)
- Telegram: Bot 私信 Owner 账户

### 5.6 权限模板 (Permission Presets)

**预设角色**:

```json
{
  "presets": {
    "family": {
      "name": "家庭成员",
      "scope": ["chat", "memory_search", "photo_analysis", "web_search"],
      "default_ttl": "30d"
    },
    "guest": {
      "name": "临时访客",
      "scope": ["chat", "translate"],
      "default_ttl": "1h"
    },
    "collaborator": {
      "name": "协作者",
      "scope": ["chat", "file_read", "shared_memory", "web_search"],
      "default_ttl": "7d"
    }
  }
}
```

**动态调整**:
```
Owner: "把妈妈的权限改成协作者模式"
Aleph: "已将访客 '妈妈' 的权限更新为 Collaborator (chat, file_read, shared_memory, web_search)"
```

---

## 6. 架构实现要点

### 6.1 SDK 层 (`clients/shared/`)

#### InvitationManager
```rust
pub struct InvitationManager {
    gateway: Arc<Gateway>,
}

impl InvitationManager {
    /// 生成邀请链接
    pub async fn create_invitation(
        &self,
        guest_name: String,
        scope: Vec<String>,
        ttl: Duration,
    ) -> Result<Invitation>;

    /// 激活邀请码
    pub async fn activate_invitation(&self, token: &str) -> Result<GuestToken>;

    /// 获取活跃访客列表
    pub async fn list_active_guests(&self) -> Result<Vec<GuestInfo>>;

    /// 撤销访客
    pub async fn revoke_guest(&self, guest_id: &str) -> Result<()>;
}
```

#### ConfigManager
```rust
pub struct ConfigManager {
    gateway: Arc<Gateway>,
    local_config: RwLock<ConfigTree>,
}

impl ConfigManager {
    /// 加载完整配置
    pub async fn load_full_config(&self) -> Result<Config>;

    /// 应用配置补丁
    pub async fn apply_patch(&self, patch: JsonValue) -> Result<()>;

    /// 订阅配置变更
    pub fn on_config_changed(&mut self, callback: impl Fn(ConfigChangedEvent));
}
```

### 6.2 核心层 (`core/src/`)

#### PolicyEngine (`gateway/security/policy_engine.rs`)
```rust
pub struct PolicyEngine {
    role_policies: HashMap<Role, Policy>,
}

impl PolicyEngine {
    /// 检查工具执行权限
    pub fn check_tool_permission(
        &self,
        user_role: &Role,
        tool_name: &str,
    ) -> PermissionResult;

    /// 动态更新访客权限
    pub fn update_guest_scope(&mut self, guest_id: &str, scope: Vec<String>);
}
```

#### IdentityMap (`gateway/identity_map.rs`)
```rust
pub struct IdentityMap {
    // "platform:user_id" -> UserId
    mappings: DashMap<String, UserId>,
}

impl IdentityMap {
    /// 查询用户身份
    pub fn resolve(&self, platform: &str, platform_user_id: &str) -> Option<UserId>;

    /// 添加映射
    pub fn add_mapping(&self, platform: &str, platform_user_id: &str, user_id: UserId);

    /// 触发配对请求
    pub async fn trigger_pairing_request(&self, platform: &str, user_info: PlatformUser);
}
```

#### Memory Namespacing (`memory/store.rs`)
```sql
-- Facts 表增加 namespace 字段
CREATE TABLE facts (
    id TEXT PRIMARY KEY,
    namespace TEXT NOT NULL,  -- "owner" / "guest:<guest_id>" / "shared"
    content TEXT,
    embedding BLOB,
    metadata TEXT,
    created_at INTEGER
);

CREATE INDEX idx_facts_namespace ON facts(namespace);
```

### 6.3 客户端 UI

#### macOS App
- 侧边栏管理面板: `AlephApp/Views/GuestManagementPanel.swift`
- 轻量级悬浮卡片设计,符合 Ghost 美学
- 实时订阅 `guests.updated` 事件更新列表

#### Tauri App
- React 组件: `src/components/GuestManager.tsx`
- 使用 `invoke` 调用 Rust SDK
- 订阅 WebSocket 事件流

#### CLI
- 子命令: `aleph guests <command>`
- 表格输出: 使用 `tabled` 库
- 交互式确认: 使用 `inquire` 库

---

## 7. 实施路线图

### Phase 1: SDK 基础 (Week 1-2)
- [ ] `clients/shared/` 目录结构
- [ ] mDNS 发现器实现
- [ ] ConfigManager 实现
- [ ] InvitationManager 骨架

### Phase 2: 核心认证 (Week 3-4)
- [ ] `core/src/gateway/security/` 扩展
- [ ] PolicyEngine 实现
- [ ] IdentityMap 实现
- [ ] JWT Token 生成/验证

### Phase 3: 访客管理 (Week 5-6)
- [ ] 邀请链接生成与激活
- [ ] 社交机器人配对流程
- [ ] Owner 多端通知机制
- [ ] 权限模板系统

### Phase 4: 客户端集成 (Week 7-8)
- [ ] macOS App 访客管理面板
- [ ] Tauri App 访客管理 UI
- [ ] CLI `guests` 子命令
- [ ] 对话式管理集成

### Phase 5: 数据隔离 (Week 9-10)
- [ ] Memory Facts 命名空间
- [ ] Session 历史隔离
- [ ] Shared Facts 机制
- [ ] 工具权限过滤

---

## 8. 验证测试

### 8.1 功能测试

**场景 1: 局域网发现**
1. 启动 Gateway Server
2. 启动 CLI Client
3. Client 自动发现 `aleph.local`
4. 一键配对成功

**场景 2: Guest 邀请**
1. Owner 通过对话生成邀请: `/invite 1h translate`
2. 访客点击链接激活 Token
3. 访客使用翻译工具成功
4. 访客尝试执行 shell 命令被拒绝

**场景 3: 配置同步**
1. Owner 在 Desktop 修改主题色
2. 所有其他在线客户端 (CLI, Web) 实时更新
3. 断开网络后配置页面置灰

**场景 4: 社交机器人配对**
1. 陌生人私聊 Telegram Bot
2. Owner Desktop 收到通知
3. Owner 批准授权 (家庭模板)
4. 陌生人与 Bot 正常对话

### 8.2 安全测试

- [ ] Guest Token 过期后自动拒绝访问
- [ ] Guest 无法访问 Owner 的私有记忆
- [ ] Guest 无法执行高危工具
- [ ] 邀请链接一次性使用验证
- [ ] Owner 撤销 Guest 后立即生效

---

## 9. 设计决策记录

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 发现机制 | mDNS + 配对码 | 局域网无缝 + 远程灵活 |
| 配置同步 | Server as Source of Truth | 一致性 > 延迟,简化逻辑 |
| 用户模型 | Owner + Guests | 私有主权 + 社交便利 |
| 访客管理 | 对话 + 视觉 | 符合 Ghost 美学,多场景适配 |
| 离线策略 | 禁止修改配置 | 瘦客户端哲学,保证数据一致性 |

---

## 10. 参考文档

- [Architecture](../ARCHITECTURE.md) - 系统架构总览
- [Gateway System](../GATEWAY.md) - WebSocket 控制面
- [Agent System](../AGENT_SYSTEM.md) - Agent Loop 设计
- [Security](../SECURITY.md) - 安全系统
- [Server-Client Architecture](2026-02-06-server-client-architecture-design.md) - 工具路由设计
