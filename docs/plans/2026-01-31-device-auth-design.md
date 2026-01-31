# Device Authentication Protocol Design

> 设备认证协议设计 — 完整对齐 Moltbot 安全模型

**Date:** 2026-01-31
**Status:** Approved
**Scope:** 设备配对 + 渠道配对，统一设计

---

## 1. 设计决策

| 决策点 | 选择 |
|--------|------|
| 侧重 | 均衡（安全 + 体验） |
| 范围 | 设备配对 + 渠道配对，统一设计 |
| 特性 | 全部 7 项对齐 Moltbot |
| 存储 | 保持 SQLite |
| 兼容 | 强制重新配对（无向后兼容） |
| 公钥算法 | Ed25519 |

### 实现特性清单

| 特性 | 安全收益 | 体验收益 |
|------|----------|----------|
| Token HMAC 签名 | 防伪造 | — |
| 配对码升级 8 位 Base-32 | 防爆破 | 更易读（无 0/1/I/O） |
| 本地请求自动豁免 | — | 开发时无需配对 |
| 待批准请求容量限制 | 防 DoS | — |
| 设备公钥 + 签名验证 | 防 clone | — |
| Token 轮换/刷新 | 限制泄露影响 | 无需重新配对 |
| 文件锁（并发安全） | 数据完整性 | — |

---

## 2. 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                     SecurityManager                          │
│         (统一入口，协调所有认证子系统)                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ TokenManager │  │ PairingManager│ │ DeviceRegistry   │   │
│  │  (签名Token) │  │ (8位Base32码) │ │ (公钥+指纹存储)  │   │
│  └──────────────┘  └──────────────┘  └──────────────────┘   │
│         │                 │                   │              │
│         └─────────────────┼───────────────────┘              │
│                           ▼                                  │
│                  ┌─────────────────┐                         │
│                  │  SecurityStore  │                         │
│                  │    (SQLite)     │                         │
│                  └─────────────────┘                         │
└─────────────────────────────────────────────────────────────┘
```

### 文件结构

```
core/src/gateway/security/
├── mod.rs              # SecurityManager 统一入口
├── token.rs            # TokenManager (添加 HMAC 签名)
├── pairing.rs          # PairingManager (升级 8 位 Base32)
├── device.rs           # DeviceRegistry (新增：公钥 + 指纹)
├── store.rs            # SecurityStore (新增：统一 SQLite)
└── crypto.rs           # 加密工具 (新增：Ed25519 + HMAC)
```

---

## 3. 数据模型

### 设备表 (devices)

```sql
CREATE TABLE devices (
    device_id       TEXT PRIMARY KEY,           -- UUID
    device_name     TEXT NOT NULL,              -- 用户友好名称
    device_type     TEXT,                       -- "macos", "ios", "android", "cli"

    -- 公钥认证
    public_key      BLOB NOT NULL,              -- Ed25519 公钥 (32 bytes)
    fingerprint     TEXT NOT NULL,              -- SHA256(public_key) 前 16 字符

    -- 权限
    role            TEXT NOT NULL DEFAULT 'operator',  -- "operator" | "node"
    scopes          TEXT NOT NULL DEFAULT '["*"]',     -- JSON 数组

    -- 生命周期
    created_at      INTEGER NOT NULL,           -- Unix ms
    approved_at     INTEGER NOT NULL,           -- Unix ms
    last_seen_at    INTEGER,                    -- Unix ms
    revoked_at      INTEGER,                    -- NULL = 有效

    UNIQUE(fingerprint)
);
```

### Token 表 (tokens)

```sql
CREATE TABLE tokens (
    token_id        TEXT PRIMARY KEY,           -- UUID
    device_id       TEXT NOT NULL,              -- 关联设备

    -- 签名 Token
    token_hash      TEXT NOT NULL,              -- HMAC-SHA256(token, secret)

    -- 元数据
    role            TEXT NOT NULL,
    scopes          TEXT NOT NULL,              -- JSON 数组

    -- 生命周期
    issued_at       INTEGER NOT NULL,           -- Unix ms
    expires_at      INTEGER NOT NULL,           -- Unix ms
    last_used_at    INTEGER,
    rotated_at      INTEGER,                    -- 轮换时间
    revoked_at      INTEGER,                    -- NULL = 有效

    FOREIGN KEY (device_id) REFERENCES devices(device_id)
);

CREATE INDEX idx_tokens_device ON tokens(device_id);
CREATE INDEX idx_tokens_expires ON tokens(expires_at);
```

### 配对请求表 (pairing_requests)

```sql
CREATE TABLE pairing_requests (
    request_id      TEXT PRIMARY KEY,           -- UUID
    code            TEXT NOT NULL UNIQUE,       -- 8 位 Base32

    -- 请求类型
    pairing_type    TEXT NOT NULL,              -- "device" | "channel"

    -- 设备配对字段
    device_name     TEXT,
    device_type     TEXT,
    public_key      BLOB,                       -- 设备提交的公钥

    -- 渠道配对字段
    channel         TEXT,                       -- "telegram", "imessage"
    sender_id       TEXT,

    -- 通用字段
    remote_addr     TEXT,
    metadata        TEXT,                       -- JSON

    -- 生命周期
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,

    CHECK (pairing_type IN ('device', 'channel'))
);

CREATE INDEX idx_pairing_code ON pairing_requests(code);
CREATE INDEX idx_pairing_expires ON pairing_requests(expires_at);
```

### 已批准发送者表 (approved_senders)

```sql
CREATE TABLE approved_senders (
    channel         TEXT NOT NULL,
    sender_id       TEXT NOT NULL,
    approved_at     INTEGER NOT NULL,
    revoked_at      INTEGER,

    PRIMARY KEY (channel, sender_id)
);
```

---

## 4. Rust 类型定义

### 核心类型 (crypto.rs)

```rust
use ed25519_dalek::{SigningKey, VerifyingKey, Signature};

/// 设备密钥对（客户端生成并持有私钥）
pub struct DeviceKeyPair {
    pub signing_key: SigningKey,      // 私钥，客户端保存
    pub verifying_key: VerifyingKey,  // 公钥，发送给服务端
}

/// 设备指纹（公钥的 SHA256 前 16 字符）
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceFingerprint(pub String);  // e.g., "a1b2c3d4e5f6g7h8"

/// HMAC 签名的 Token
pub struct SignedToken {
    pub token: String,      // 原始 token (UUID)
    pub signature: String,  // HMAC-SHA256 签名
}
```

### 设备类型 (device.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub device_name: String,
    pub device_type: Option<DeviceType>,
    pub public_key: Vec<u8>,          // Ed25519 公钥
    pub fingerprint: DeviceFingerprint,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    MacOS,
    IOS,
    Android,
    CLI,
    Web,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    Operator,  // 完全控制（CLI、macOS App）
    Node,      // 受限执行（iOS/Android 节点）
}
```

### Token 类型 (token.rs)

```rust
#[derive(Debug, Clone)]
pub struct Token {
    pub token_id: String,
    pub device_id: String,
    pub token_hash: String,           // 不存原文，只存哈希
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    pub rotated_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Token 验证结果
pub struct TokenValidation {
    pub device_id: String,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub remaining_secs: u64,
}
```

### 配对类型 (pairing.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingRequest {
    Device {
        request_id: String,
        code: String,               // 8 位 Base32
        device_name: String,
        device_type: Option<DeviceType>,
        public_key: Vec<u8>,
        remote_addr: Option<String>,
        created_at: i64,
        expires_at: i64,
    },
    Channel {
        request_id: String,
        code: String,
        channel: String,
        sender_id: String,
        metadata: Option<serde_json::Value>,
        created_at: i64,
        expires_at: i64,
    },
}

/// 配对码常量
pub const PAIRING_CODE_LENGTH: usize = 8;
pub const PAIRING_CODE_CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const PAIRING_CODE_EXPIRY_SECS: u64 = 300;  // 5 分钟
pub const MAX_PENDING_REQUESTS: usize = 10;     // 容量限制
```

---

## 5. SecurityManager API

### 统一入口 (mod.rs)

```rust
pub struct SecurityManager {
    store: SecurityStore,           // SQLite 存储
    token_secret: [u8; 32],         // HMAC 密钥（启动时生成/加载）
    config: SecurityConfig,
}

pub struct SecurityConfig {
    pub require_auth: bool,              // 是否强制认证
    pub allow_loopback: bool,            // 本地请求自动豁免
    pub token_expiry_secs: u64,          // Token 过期时间 (默认 86400)
    pub pairing_expiry_secs: u64,        // 配对码过期时间 (默认 300)
    pub max_pending_requests: usize,     // 待批准请求上限 (默认 10)
}
```

### 设备配对流程

```rust
impl SecurityManager {
    /// 1. 客户端发起配对请求（提交公钥）
    pub async fn request_device_pairing(
        &self,
        device_name: String,
        device_type: Option<DeviceType>,
        public_key: Vec<u8>,
        remote_addr: Option<String>,
    ) -> Result<PairingRequest, SecurityError>;

    /// 2. 操作员批准配对（返回设备信息）
    pub async fn approve_device_pairing(
        &self,
        code: &str,
    ) -> Result<Device, SecurityError>;

    /// 3. 操作员拒绝配对
    pub async fn reject_device_pairing(
        &self,
        code: &str,
    ) -> Result<(), SecurityError>;

    /// 4. 列出待批准请求
    pub async fn list_pending_requests(
        &self,
    ) -> Result<Vec<PairingRequest>, SecurityError>;
}
```

### Token 管理

```rust
impl SecurityManager {
    /// 为已批准设备生成签名 Token
    pub async fn issue_token(
        &self,
        device_id: &str,
        role: DeviceRole,
        scopes: Vec<String>,
    ) -> Result<SignedToken, SecurityError>;

    /// 验证 Token（检查签名 + 过期 + 撤销状态）
    pub async fn validate_token(
        &self,
        token: &str,
        signature: &str,
    ) -> Result<TokenValidation, SecurityError>;

    /// 轮换 Token（旧 Token 失效，返回新 Token）
    pub async fn rotate_token(
        &self,
        old_token: &str,
        old_signature: &str,
    ) -> Result<SignedToken, SecurityError>;

    /// 撤销 Token
    pub async fn revoke_token(
        &self,
        token_id: &str,
    ) -> Result<(), SecurityError>;

    /// 撤销设备所有 Token
    pub async fn revoke_device_tokens(
        &self,
        device_id: &str,
    ) -> Result<u64, SecurityError>;  // 返回撤销数量
}
```

### 设备验证

```rust
impl SecurityManager {
    /// 验证设备签名（握手时调用）
    pub async fn verify_device_signature(
        &self,
        device_id: &str,
        payload: &[u8],
        signature: &Signature,
    ) -> Result<Device, SecurityError>;

    /// 检查是否为本地请求（自动豁免）
    pub fn is_loopback(&self, remote_addr: &str) -> bool;

    /// 更新设备最后活跃时间
    pub async fn touch_device(
        &self,
        device_id: &str,
    ) -> Result<(), SecurityError>;
}
```

### 渠道配对

```rust
impl SecurityManager {
    /// 发起渠道配对
    pub async fn request_channel_pairing(
        &self,
        channel: &str,
        sender_id: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<PairingRequest, SecurityError>;

    /// 批准渠道发送者
    pub async fn approve_sender(
        &self,
        code: &str,
    ) -> Result<(String, String), SecurityError>;  // (channel, sender_id)

    /// 检查发送者是否已批准
    pub async fn is_sender_approved(
        &self,
        channel: &str,
        sender_id: &str,
    ) -> Result<bool, SecurityError>;

    /// 撤销发送者
    pub async fn revoke_sender(
        &self,
        channel: &str,
        sender_id: &str,
    ) -> Result<(), SecurityError>;
}
```

---

## 6. 连接握手协议

### 首次连接流程

```
┌─────────────────────────────────────────────────────────────────────┐
│                         客户端首次连接                               │
└─────────────────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────┐     ┌──────────────────────────────────────────────┐
│ 客户端生成   │ ──► │ Ed25519 密钥对                                │
│ 密钥对       │     │ - signing_key (私钥) → 本地安全存储           │
│             │     │ - verifying_key (公钥) → 发送给 Gateway       │
└─────────────┘     └──────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Client → Gateway: connect (首次)                                   │
├─────────────────────────────────────────────────────────────────────┤
│  {                                                                   │
│    "method": "connect",                                              │
│    "params": {                                                       │
│      "protocol": 2,                      // 新协议版本               │
│      "device_name": "MacBook Pro",                                  │
│      "device_type": "macos",                                        │
│      "public_key": "base64...",          // Ed25519 公钥            │
│      "client": { "id": "aether-macos", "version": "0.2.0" }         │
│    }                                                                 │
│  }                                                                   │
└─────────────────────────────────────────────────────────────────────┘
     │
     ▼ (is_loopback && allow_loopback?)
     │
   ┌─┴─┐
   │Yes│ ──► 自动批准，跳过配对流程
   └───┘
     │No
     ▼
┌─────────────────────────────────────────────────────┐
│  Gateway → Client: pairing_required                 │
├─────────────────────────────────────────────────────┤
│  {                                                   │
│    "error": {                                        │
│      "code": 4001,                                   │
│      "message": "Pairing required",                  │
│      "data": {                                       │
│        "code": "A3B7K9M2",        // 8 位 Base32     │
│        "expires_in": 300,                            │
│        "fingerprint": "a1b2c3d4..."  // 公钥指纹    │
│      }                                               │
│    }                                                 │
│  }                                                   │
└─────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────┐
│  操作员批准: aether pairing approve A3B7K9M2        │
└─────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Gateway → Client: connect_ok                                       │
├─────────────────────────────────────────────────────────────────────┤
│  {                                                                   │
│    "result": {                                                       │
│      "protocol": 2,                                                  │
│      "device_id": "uuid...",                                        │
│      "token": "uuid...",                 // 原始 Token               │
│      "token_signature": "hex...",        // HMAC 签名               │
│      "expires_at": 1738400000000,        // Unix ms                 │
│      "role": "operator",                                            │
│      "scopes": ["*"]                                                │
│    }                                                                 │
│  }                                                                   │
└─────────────────────────────────────────────────────────────────────┘
```

### 重连流程

```
┌─────────────────────────────────────────────────────────────────────┐
│  Client → Gateway: connect (重连)                                   │
├─────────────────────────────────────────────────────────────────────┤
│  {                                                                   │
│    "method": "connect",                                              │
│    "params": {                                                       │
│      "protocol": 2,                                                  │
│      "device_id": "uuid...",                                        │
│      "token": "uuid...",                                            │
│      "token_signature": "hex...",                                   │
│      "timestamp": 1738400000000,         // 防重放                  │
│      "nonce": "random...",               // 防重放                  │
│      "payload_signature": "base64..."    // Ed25519 签名            │
│    }                                                                 │
│  }                                                                   │
└─────────────────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────┐
│  Gateway 验证:                                       │
│  1. 验证 Token HMAC 签名                            │
│  2. 检查 Token 未过期/未撤销                         │
│  3. 验证 payload_signature (Ed25519)                │
│  4. 检查 timestamp 在 ±5 分钟内                      │
│  5. 检查 nonce 未使用过                              │
└─────────────────────────────────────────────────────┘
```

### Payload 签名构造

```rust
/// 客户端构造签名 payload
fn build_connect_payload(
    device_id: &str,
    token: &str,
    timestamp: i64,
    nonce: &str,
) -> String {
    format!("v2|connect|{}|{}|{}|{}", device_id, token, timestamp, nonce)
}
```

---

## 7. RPC 方法与错误码

### 认证相关 RPC 方法

| 方法 | 权限 | 说明 |
|------|------|------|
| `connect` | 无需认证 | 连接握手，首次配对或重连 |
| `pairing.list` | operator | 列出待批准的配对请求 |
| `pairing.approve` | operator | 批准配对请求 |
| `pairing.reject` | operator | 拒绝配对请求 |
| `devices.list` | operator | 列出已批准设备 |
| `devices.get` | operator | 获取设备详情 |
| `devices.revoke` | operator | 撤销设备（所有 Token 失效） |
| `tokens.rotate` | 已认证 | 轮换当前 Token |
| `tokens.revoke` | operator | 撤销指定 Token |
| `senders.list` | operator | 列出已批准的渠道发送者 |
| `senders.revoke` | operator | 撤销渠道发送者 |

### 错误码定义

```rust
pub enum SecurityErrorCode {
    // 认证错误 (4xxx)
    AuthRequired          = 4001,  // 需要认证
    PairingRequired       = 4002,  // 需要配对
    InvalidToken          = 4003,  // Token 无效
    TokenExpired          = 4004,  // Token 已过期
    TokenRevoked          = 4005,  // Token 已撤销
    InvalidSignature      = 4006,  // 签名验证失败
    DeviceRevoked         = 4007,  // 设备已撤销

    // 配对错误 (41xx)
    PairingCodeInvalid    = 4101,  // 配对码无效
    PairingCodeExpired    = 4102,  // 配对码已过期
    PairingLimitExceeded  = 4103,  // 待批准请求超过上限

    // 权限错误 (42xx)
    PermissionDenied      = 4201,  // 无权限
    ScopeInsufficient     = 4202,  // scope 不足

    // 防重放错误 (43xx)
    TimestampExpired      = 4301,  // 时间戳过期
    NonceReused           = 4302,  // Nonce 重复使用
}
```

---

## 8. 本地豁免与自动清理

### 本地请求检测

```rust
pub fn is_loopback(&self, remote_addr: &str) -> bool {
    // IPv4 loopback
    if remote_addr == "127.0.0.1" || remote_addr.starts_with("127.") {
        return true;
    }
    // IPv6 loopback
    if remote_addr == "::1" {
        return true;
    }
    // Unix socket
    if remote_addr.is_empty() {
        return true;
    }
    false
}
```

### 自动清理机制

```rust
impl SecurityManager {
    /// 启动后台清理任务（每 60 秒）
    pub fn start_cleanup_task(self: Arc<Self>) -> JoinHandle<()>;

    /// 执行清理
    async fn cleanup(&self) -> Result<CleanupStats, SecurityError> {
        // 1. 清理过期配对请求
        // 2. 清理过期 Token
        // 3. 清理过期 Nonce（防重放缓存）
    }
}
```

### Nonce 缓存

```rust
pub struct NonceCache {
    cache: RwLock<LruCache<String, i64>>,  // nonce -> timestamp
    max_age_ms: i64,
}

impl NonceCache {
    /// 检查并记录 nonce（原子操作）
    pub fn check_and_insert(&self, nonce: &str, timestamp: i64) -> bool;

    /// 清理过期 nonce
    pub fn cleanup(&self, min_timestamp: i64) -> u64;
}
```

---

## 9. 实现计划

### 文件变更清单

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `security/crypto.rs` | Ed25519 + HMAC 工具函数 |
| 新增 | `security/store.rs` | SecurityStore (SQLite 统一存储) |
| 新增 | `security/device.rs` | Device 类型 + DeviceRegistry |
| 重写 | `security/token.rs` | 添加 HMAC 签名，移除内存存储 |
| 重写 | `security/pairing.rs` | 8 位 Base32，统一设备/渠道配对 |
| 重写 | `security/mod.rs` | SecurityManager 统一入口 |
| 重写 | `handlers/auth.rs` | 适配新协议 v2 |
| 重写 | `handlers/pairing.rs` | 适配统一配对 API |
| 删除 | `device_store.rs` | 合并到 SecurityStore |
| 删除 | `pairing_store.rs` | 合并到 SecurityStore |
| 修改 | `server.rs` | 集成 SecurityManager |
| 修改 | `protocol.rs` | 新增错误码 |

### 依赖新增

```toml
[dependencies]
ed25519-dalek = { version = "2.1", features = ["rand_core"] }
hmac = "0.12"
sha2 = "0.10"
lru = "0.12"
base32 = "0.5"
```

### 实现步骤

```
Step 1: 基础设施 (无破坏性)
├── 新增 crypto.rs (Ed25519 + HMAC)
├── 新增 store.rs (SQLite schema + 迁移)
└── 单元测试

Step 2: 核心类型 (无破坏性)
├── 新增 device.rs (Device 类型)
├── 重写 token.rs (SignedToken)
├── 重写 pairing.rs (8 位 Base32)
└── 单元测试

Step 3: SecurityManager (无破坏性)
├── 重写 mod.rs (统一入口)
├── 实现所有 API 方法
├── 实现 NonceCache
├── 实现 cleanup 任务
└── 集成测试

Step 4: RPC Handlers (破坏性变更)
├── 重写 handlers/auth.rs
├── 重写 handlers/pairing.rs
├── 新增 handlers/devices.rs
├── 新增 handlers/tokens.rs
└── 集成测试

Step 5: 集成 & 迁移
├── 修改 server.rs
├── 删除旧文件 (device_store.rs, pairing_store.rs)
├── 数据库迁移脚本
└── 端到端测试
```

### 数据库迁移

```rust
impl SecurityStore {
    pub async fn migrate(&self) -> Result<(), SecurityError> {
        let version = self.get_schema_version().await?;

        if version < 2 {
            // 删除旧表（强制重新配对）
            self.conn.execute_batch(r#"
                DROP TABLE IF EXISTS approved_devices;
                DROP TABLE IF EXISTS pairing_requests;
                DROP TABLE IF EXISTS approved_senders;
            "#).await?;

            // 创建新表
            self.conn.execute_batch(SCHEMA_V2).await?;
            self.set_schema_version(2).await?;
        }

        Ok(())
    }
}
```

---

## 10. 参考

- Moltbot 设备认证: `/Users/zouguojun/Workspace/moltbot/src/infra/device-pairing.ts`
- Moltbot Gateway Auth: `/Users/zouguojun/Workspace/moltbot/src/gateway/auth.ts`
- Moltbot 安全文档: `https://docs.molt.bot/gateway/security`
