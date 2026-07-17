# Gateway 自签 TLS 证书 SAN 自动发现 — 设计文档

- 日期：2026-07-18
- 状态：设计已确认（待写实现计划）
- 关联：[[project-release-26-7-17-and-debian-tls-deploy]]、[[project-gateway-tls-hardening]]、`docs/reference/SECURITY.md#remote-tls`
- 锚点：`src/gateway/tls.rs`、`src/gateway/config.rs`

## 1. 问题（The Pit）

26.7.17 引入网关原生 TLS。自签模式（`[gateway.tls] enabled = true`、cert/key 路径留空）经
`rcgen` 生成并持久化到 `~/.aleph/data/tls/{cert,key}.pem`。但 `tls.rs:72` 把 SAN **硬编码**为
`["localhost", "127.0.0.1"]`：

```rust
rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
```

后果：当服务 `--bind 0.0.0.0` 暴露在公网 IP（生产机 ColoCrossing = `172.245.43.211`）上时，
远程客户端连 `wss://172.245.43.211:18790` 会因**证书 SAN 不含该 IP** 而在 TLS 主机名校验阶段失败
（`ERR_CERT_COMMON_NAME_INVALID` / hostname mismatch）——这不仅是"自签不受信"，而是"证书根本不是
发给这个地址的"。浏览器可勉强点过，严格客户端直接硬失败。

## 2. 目标与非目标

**目标**：远程客户端连 `wss://172.245.43.211:18790` 时，TLS **主机名校验通过**（SAN 命中该 IP），
一次性信任（浏览器接受一次 / 指纹 pin）后即可稳定连接。修复是**产品级**的：任何 `0.0.0.0` 部署都自动
让自签证书覆盖本机真实接口 IP，而非只修这一台机器。

**非目标（明确排除）**：
- ACME / Let's Encrypt 自动签发（违 R3——core 不搬砖，签发是 Caddy/certbot 的活）。
- 裸 IP 上的"零告警绿锁"（需公有 CA 的 IP 证书，用户已选裸 IP + R3 路线，超范围）。
- 改动 provided-cert（Tier ③ `cert_path`/`key_path`）、trusted-proxy、disabled 三条路径。

**用户澄清定案**：① 必须裸 IP `172.245.43.211`（无域名）；② 一次性信任可接受；③ 用户点明"证书应针对
`0.0.0.0` 绑定而非仅某个 IP"——技术上转译为"把该绑定暴露的全部具体接口 IP 放进 SAN"（`0.0.0.0` 本身
是通配绑定地址，不可作为 SAN 条目参与主机名匹配）。

## 3. 方案（Approach A：接口 IP 自动发现）

### 3.1 依赖与 API 事实（已核实）
- `if-addrs` **已在 Cargo.lock**（传递依赖）→ 提升为直接依赖，成本低，不违 R3。
- `rcgen 0.13.2` 的 `generate_simple_self_signed` 内部经 `CertificateParams::new` 对每个 SAN 字符串
  做 `parse::<IpAddr>()`：可解析为 IP → `SanType::IpAddress`，否则 → `SanType::DnsName`。**现有
  `"127.0.0.1"` 已是靠此变成 IP SAN**，故新增 `"172.245.43.211"` 天然成为 IP SAN，无需改调用方式。

### 3.2 配置变更（`src/gateway/config.rs`）
`GatewayTlsConfig` 增一个**纯附加**字段：

```rust
pub struct GatewayTlsConfig {
    pub enabled: bool,
    pub cert_path: String,
    pub key_path: String,
    /// 追加到自签证书的额外 SAN（主机名 / 自动发现看不到的 NAT 后 IP）。
    /// 自动发现始终对自签模式生效；本字段仅做补充。provided-cert 模式忽略。
    #[serde(default)]
    pub san: Vec<String>,
}
```

```toml
[gateway.tls]
enabled = true
san = ["vps.example.com"]   # 可选：主机名，或不在本机接口上的 NAT 公网 IP
```

**决策（已确认）**：自动发现**始终开启**、不加禁用开关（YAGNI）。`san` 只做补充，不替代、不关闭发现。

### 3.3 SAN 组装（`src/gateway/tls.rs`，纯/非纯拆分）

- **非纯**：`discover_interface_ips() -> Vec<IpAddr>`
  - `if_addrs::get_if_addrs()`；过滤掉 loopback 与 link-local；任何错误 → 返回空（非致命，P7）。
- **纯**：`self_signed_sans(configured: &[String], discovered: &[IpAddr]) -> Vec<String>`
  - 基底 `["localhost", "127.0.0.1", "::1"]`
  - `+` discovered 的 `ip.to_string()`
  - `+` 校验后的 `configured`（保留能 `parse::<IpAddr>()` 或匹配简单主机名 `^[A-Za-z0-9.-]+$` 的；
    其余 trim 后为空或畸形的**丢弃并 `warn!`**——一条坏配置不得让证书生成失败/启动 brick，P7）
  - 保序去重（`HashSet` 记录已见，`retain`）。

### 3.4 生成
`generate_self_signed(sans: &[String]) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)>`
把组装好的列表交给 `generate_simple_self_signed(sans.to_vec())`；rcgen 自行分类每条为 IP / DNS SAN。

### 3.5 SAN 漂移时重生（持久化证书陷阱）
现状：`cert.pem`+`key.pem` 存在即复用 → 升级后旧证书（仅 localhost）不会自动更新。

新增 sidecar `~/.aleph/data/tls/sans.json`，记录当前证书生成时用的 SAN 集合。启动时：

> **复用**已持久化证书，当且仅当 `cert.pem` + `key.pem` + `sans.json` 三者都在
> **且 `desired ⊆ recorded`**（desired 是本次组装出的 SAN 集，recorded 是 sidecar 记录的集）；
> 否则以 `desired` **重生**证书并重写 sidecar。

**用子集（非相等）判定以最小化 churn**：
- 首次升级后启动：无 sidecar → 重生（含公网 IP）。✓
- 稳态：desired == recorded → 子集成立 → 复用。✓
- 删掉一个 docker 网桥：desired 缩小，仍 ⊆ recorded → 复用，**不 churn**。✓
- 出现新地址（新公网 IP / 首次覆盖）：desired ⊄ recorded → 重生。✓（必要且正确）

重生会改指纹 → 客户端需**重新信任一次**（修复落地的必然代价，可接受）。

`should_reuse(san_file: &Path, desired: &[String]) -> bool`：读 sidecar（JSON 字符串数组）→ 与 desired
作集合子集比较；文件缺失/不可读/非子集 → false（重生）。可用 temp-dir 单测。

### 3.6 编排
`load_or_generate` 内：`discover_interface_ips()` → `self_signed_sans(&cfg.san, &discovered)`（desired）
→ `should_reuse(sans.json, &desired) ? 读现有 : 生成并持久化(cert.pem/key.pem/sans.json)`。
`resolve_mode` / Disabled / Provided 分支不变。

## 4. 错误处理与边界

- **发现失败**：`get_if_addrs` 出错 → 空发现集 → 证书仍含 loopback + `san`，服务照常带 TLS 启动。
- **Docker 网桥 IP**（172.17/18/20/21.0.1）会进入 SAN——无害（无人从外部经网桥 IP 连），且子集复用规则
  保证其增删不反复 churn 证书。
- **ColoCrossing 现存证书**（仅 localhost）：下次启动 `desired ⊄ recorded` → 自动重生含 `172.245.43.211`。
- **畸形 `san` 条目**：`self_signed_sans` 校验期丢弃 + warn，不 brick 启动。

## 5. 测试

- `self_signed_sans`（纯，快）：保序去重；基底恒在；畸形 configured 被丢弃；discovered 正确并入。
- reuse/regen 决策：temp-dir 预写 sidecar，覆盖子集命中 / 子集不命中 / 缺 sidecar 三态。
- 一个集成测试：生成 → 重新解析 PEM → 断言含形如 `172.245.43.211` 的 IP SAN（用 rcgen params 或轻量解析）。
- `discover_interface_ips` 本身依赖宿主、不做确定性断言（只保证非 panic、错误返空）。

## 6. 文档与部署

- 文档：`SECURITY.md` Tier ② 段 + `tls.rs` 模块头注释更新为"自签证书现自动覆盖本机接口 IP + `san` 补充"。
- **落到生产机（本次工作的收尾，成功标准所在）**：
  1. 重建 Linux `aleph-server`（CI 或 UbuntuDev 手动构建路径）。
  2. 重新部署到 ColoCrossing（按既有分步流程 + `pkill -x` 兜底，见
     [[feedback-ssh-deploy-pgrep-self-kill]]）→ 自签证书自动重生含 `172.245.43.211`。
  3. **验证**（服务端硬证明）：`curl --cacert ~/.aleph/data/tls/cert.pem https://172.245.43.211:18790/`
     返回 `200` 且**无主机名错误** = 信任锚 + SAN 双双通过——这正是浏览器"接受一次"之后能稳定连接的前提。
     另可 `openssl x509 -in cert.pem -noout -ext subjectAltName` 确认列出 `IP Address:172.245.43.211`。

## 7. 影响面

- 改动文件：`src/gateway/tls.rs`（主）、`src/gateway/config.rs`（+1 字段）、`Cargo.toml`（if-addrs 提直接依赖）、
  `docs/reference/SECURITY.md`（Tier ② 文案）。
- 兼容性：`san` 带 `#[serde(default)]`，旧 config 照常加载；行为变化仅限自签证书 SAN 集扩大 + 一次重生。
- 红线核对：R1（不碰平台 API，if-addrs 是跨平台库）、R3（复用既有依赖、单模块小改）、R7/R10（无认知逻辑、
  不进 harness）——均不触碰。
