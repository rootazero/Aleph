# Gateway 自签 TLS 证书 SAN 自动发现 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 gateway 自签 TLS 证书的 SAN 自动覆盖本机非环回接口 IP（含公网 `172.245.43.211`）加可选配置补充，使远程客户端连 `wss://172.245.43.211:18790` 时 TLS 主机名校验通过。

**Architecture:** 单模块 `src/gateway/tls.rs` 把写死的 `[localhost,127.0.0.1]` SAN 改为「base loopback + `if-addrs` 自动发现的接口 IP + `[gateway.tls] san` 配置补充」的组装；新增 `sans.txt` sidecar 记录证书的 SAN 集，启动时以「desired ⊆ recorded」子集判定决定复用还是重生（压 docker 网桥 churn）。纯逻辑（组装 / 校验 / 子集）与非纯 I/O（接口发现 / 文件读写）拆开，便于单测。

**Tech Stack:** Rust、`rcgen 0.13`（自签生成，字符串 SAN 自动分类 IP/DNS）、`if-addrs 0.13`（跨平台接口枚举，已在依赖树）、`tokio::fs`、`sha2`。

## Global Constraints

- **MSRV = 1.95**：禁用 nightly-only API。IPv6 link-local 判定用手写 `fe80::/10`（`Ipv6Addr::is_unicast_link_local` 未稳定）；`Ipv4Addr::is_link_local` / `is_loopback` 稳定可用。
- **依赖纪律（R3）**：只把已在 `Cargo.lock` 的 `if-addrs` 提为直接依赖；不引入证书解析库、不引入 serde_json 依赖（sidecar 用换行分隔纯文本）。
- **红线不碰**：R1（`if-addrs` 是跨平台库，非平台 API crate）、R7/R10（无认知逻辑、不进 `src/harness/`）。改的是 `src/gateway/`，非 harness。
- **cargo 节制（用户工作风格）**：测试步骤一律**定向**到本模块（`cargo test -p alephcore gateway::tls`），绝不跑全量；能批则批。
- **提交规范**：英文，`<scope>: <description>`，如 `gateway: ...`。单分支 main 直接开发。
- **provided-cert / trusted-proxy / disabled 三条 TLS 路径不得改动**；只动 `TlsMode::SelfSigned` 分支。

---

### Task 1: 配置 `san` 字段 + 纯 SAN 组装

**Files:**
- Modify: `src/gateway/config.rs`（`GatewayTlsConfig`，约 95–103 行）
- Modify: `src/gateway/tls.rs`（新增纯函数 + import）
- Test: `src/gateway/tls.rs`（`#[cfg(test)] mod tests`，约 87 行起）

**Interfaces:**
- Produces:
  - `GatewayTlsConfig.san: Vec<String>`（新字段，`#[serde(default)]`）
  - `fn self_signed_sans(configured: &[String], discovered: &[std::net::IpAddr]) -> Vec<String>`（纯，crate 内可见）
  - `fn is_plausible_dns_name(s: &str) -> bool`（纯，私有）
  - 常量 `const BASE_SANS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];`

- [ ] **Step 1: 给 `GatewayTlsConfig` 加 `san` 字段**

`src/gateway/config.rs`，把结构体替换为：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayTlsConfig {
    /// Terminate TLS in-process. Default false.
    pub enabled: bool,
    /// PEM certificate chain path. Empty + `enabled` ⇒ auto self-signed.
    pub cert_path: String,
    /// PEM private-key path. Empty + `enabled` ⇒ auto self-signed.
    pub key_path: String,
    /// Extra SAN entries (hostnames / IPs) added to the auto self-signed cert,
    /// on top of loopback + auto-discovered interface IPs. Ignored for a
    /// provided cert. Default empty.
    pub san: Vec<String>,
}
```

- [ ] **Step 2: 在 `tls.rs` 顶部补 import**

`src/gateway/tls.rs`，把现有 `use std::path::Path;` 段替换为：

```rust
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::gateway::config::GatewayTlsConfig;
```

- [ ] **Step 3: 写失败测试（纯组装）**

`src/gateway/tls.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn sans_include_base_discovered_and_config() {
        use std::net::{IpAddr, Ipv4Addr};
        let discovered = vec![IpAddr::V4(Ipv4Addr::new(172, 245, 43, 211))];
        let configured = vec!["vps.example.com".to_string(), "10.0.0.5".to_string()];
        let sans = self_signed_sans(&configured, &discovered);
        for expect in ["localhost", "127.0.0.1", "::1", "172.245.43.211", "vps.example.com", "10.0.0.5"] {
            assert!(sans.contains(&expect.to_string()), "missing {expect}");
        }
    }

    #[test]
    fn sans_dedup_and_drop_malformed() {
        use std::net::{IpAddr, Ipv4Addr};
        let discovered = vec![
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        ];
        let configured = vec!["127.0.0.1".to_string(), "bad name!".to_string(), "   ".to_string()];
        let sans = self_signed_sans(&configured, &discovered);
        assert_eq!(sans.iter().filter(|s| *s == "203.0.113.7").count(), 1);
        assert_eq!(sans.iter().filter(|s| *s == "127.0.0.1").count(), 1);
        assert!(!sans.iter().any(|s| s.contains('!')));
    }
```

- [ ] **Step 4: 运行测试确认失败**

Run: `cargo test -p alephcore gateway::tls::tests::sans_ -- --nocapture`
Expected: FAIL（`self_signed_sans` 未定义 / 编译错误）

- [ ] **Step 5: 实现纯组装函数**

`src/gateway/tls.rs`，在 `fingerprint` 函数上方插入：

```rust
/// Base SANs every self-signed cert always carries.
const BASE_SANS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// True if `s` is a plausible DNS name. rcgen rejects garbage and would fail
/// cert generation, which must never brick startup, so we pre-filter.
fn is_plausible_dns_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Assemble the SAN list for a self-signed cert (pure): base loopback set +
/// discovered interface IPs + validated operator extras, order-stable deduped.
pub(crate) fn self_signed_sans(configured: &[String], discovered: &[IpAddr]) -> Vec<String> {
    let mut sans: Vec<String> = BASE_SANS.iter().map(|s| (*s).to_string()).collect();
    for ip in discovered {
        sans.push(ip.to_string());
    }
    for raw in configured {
        let s = raw.trim();
        if s.parse::<IpAddr>().is_ok() || is_plausible_dns_name(s) {
            sans.push(s.to_string());
        } else if !s.is_empty() {
            tracing::warn!(san = %s, "gateway.tls.san: dropping malformed SAN entry");
        }
    }
    let mut seen = HashSet::new();
    sans.retain(|s| seen.insert(s.clone()));
    sans
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p alephcore gateway::tls::tests::sans_ -- --nocapture`
Expected: PASS（两个 `sans_*` 测试通过）

- [ ] **Step 7: 提交**

```bash
git add src/gateway/config.rs src/gateway/tls.rs
git commit -m "gateway: add [gateway.tls] san field + pure self-signed SAN assembly"
```

---

### Task 2: 接口 IP 自动发现（`if-addrs`）

**Files:**
- Modify: `Cargo.toml`（`[dependencies]`，`rcgen = "0.13"` 附近，约 233 行）
- Modify: `src/gateway/tls.rs`（新增两函数）
- Test: `src/gateway/tls.rs`（`mod tests`）

**Interfaces:**
- Consumes: `IpAddr`（Task 1 已 import）
- Produces:
  - `fn is_usable_san_ip(ip: &IpAddr) -> bool`（纯，私有）
  - `fn discover_interface_ips() -> Vec<IpAddr>`（非纯，crate 内可见）

- [ ] **Step 1: 把 `if-addrs` 提为直接依赖**

`Cargo.toml` 的 `[dependencies]` 段，在 `rcgen = "0.13"` 一行下方新增：

```toml
if-addrs = "0.13"
```

- [ ] **Step 2: 写失败测试（过滤谓词）**

`src/gateway/tls.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn usable_san_ip_filters_loopback_and_link_local() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        assert!(!is_usable_san_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_usable_san_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!is_usable_san_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_usable_san_ip(&IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
        assert!(is_usable_san_ip(&IpAddr::V4(Ipv4Addr::new(172, 245, 43, 211))));
        assert!(is_usable_san_ip(&IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn discover_interface_ips_has_no_loopback_and_no_panic() {
        for ip in discover_interface_ips() {
            assert!(!ip.is_loopback());
        }
    }
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p alephcore gateway::tls::tests::usable_san_ip_filters_loopback_and_link_local -- --nocapture`
Expected: FAIL（`is_usable_san_ip` / `discover_interface_ips` 未定义）

- [ ] **Step 4: 实现过滤谓词 + 发现函数**

`src/gateway/tls.rs`，在 `self_signed_sans` 上方插入：

```rust
/// True if `ip` is a usable SAN target (not loopback, not link-local).
fn is_usable_san_ip(ip: &IpAddr) -> bool {
    if ip.is_loopback() {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => !v4.is_link_local(),
        // fe80::/10 link-local (is_unicast_link_local is unstable on MSRV 1.95).
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

/// Enumerate this host's usable non-loopback interface IPs. Best-effort: any
/// failure yields an empty vec (the cert still gets loopback + configured SANs).
pub(crate) fn discover_interface_ips() -> Vec<IpAddr> {
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces.into_iter().map(|i| i.ip()).filter(is_usable_san_ip).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "gateway.tls: interface discovery failed; SAN limited to loopback + config");
            Vec::new()
        }
    }
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore gateway::tls::tests::usable_san_ip_filters_loopback_and_link_local gateway::tls::tests::discover_interface_ips_has_no_loopback_and_no_panic -- --nocapture`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml Cargo.lock src/gateway/tls.rs
git commit -m "gateway: auto-discover non-loopback interface IPs for self-signed SAN"
```

---

### Task 3: sidecar 子集重生 + 接线生成

**Files:**
- Modify: `src/gateway/tls.rs`（改 `generate_self_signed` 签名、`load_or_generate` 的 SelfSigned 分支、新增 sidecar 纯函数、更新模块头注释）
- Test: `src/gateway/tls.rs`（`mod tests`）

**Interfaces:**
- Consumes: `self_signed_sans`（Task 1）、`discover_interface_ips`（Task 2）
- Produces:
  - `fn parse_recorded_sans(content: &str) -> HashSet<String>`（纯，私有）
  - `fn desired_covered(recorded: &HashSet<String>, desired: &[String]) -> bool`（纯，私有）
  - `fn generate_self_signed(sans: &[String]) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)>`（签名变更）
  - `load_or_generate` 行为：SelfSigned 分支写 `sans.txt` sidecar，子集命中才复用

- [ ] **Step 1: 写失败测试（sidecar 纯逻辑 + 重生/复用）**

`src/gateway/tls.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn parse_recorded_sans_ignores_blanks() {
        let set = parse_recorded_sans("localhost\n127.0.0.1\n\n  \n203.0.113.7\n");
        assert_eq!(set.len(), 3);
        assert!(set.contains("203.0.113.7"));
    }

    #[test]
    fn desired_covered_subset_logic() {
        let recorded: HashSet<String> =
            ["localhost", "127.0.0.1", "::1", "203.0.113.7"].iter().map(|s| s.to_string()).collect();
        assert!(desired_covered(&recorded, &["127.0.0.1".to_string(), "203.0.113.7".to_string()]));
        assert!(!desired_covered(&recorded, &["203.0.113.7".to_string(), "198.51.100.9".to_string()]));
    }

    #[tokio::test]
    async fn regenerates_when_desired_not_covered() {
        let dir = tempfile::tempdir().unwrap();
        let cfg0 = GatewayTlsConfig { enabled: true, ..Default::default() };
        let (_c0, _k0, fp0) = load_or_generate(&cfg0, dir.path()).await.unwrap();
        assert!(dir.path().join("sans.txt").exists());

        let cfg1 = GatewayTlsConfig {
            enabled: true,
            san: vec!["203.0.113.77".to_string()],
            ..Default::default()
        };
        let (_c1, _k1, fp1) = load_or_generate(&cfg1, dir.path()).await.unwrap();
        assert_ne!(fp0, fp1, "adding an uncovered SAN must regenerate the cert");

        let recorded =
            parse_recorded_sans(&tokio::fs::read_to_string(dir.path().join("sans.txt")).await.unwrap());
        assert!(recorded.contains("203.0.113.77"));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore gateway::tls::tests::parse_recorded_sans_ignores_blanks gateway::tls::tests::desired_covered_subset_logic gateway::tls::tests::regenerates_when_desired_not_covered -- --nocapture`
Expected: FAIL（`parse_recorded_sans` / `desired_covered` 未定义；`generate_self_signed` 签名不符）

- [ ] **Step 3: 新增 sidecar 纯函数**

`src/gateway/tls.rs`，在 `is_usable_san_ip` 上方插入：

```rust
/// Parse the newline-delimited SAN sidecar into a set (blank lines ignored).
fn parse_recorded_sans(content: &str) -> HashSet<String> {
    content.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Reuse the persisted cert iff every desired SAN is already recorded (subset,
/// not equality — removing an interface IP must not thrash the cert).
fn desired_covered(recorded: &HashSet<String>, desired: &[String]) -> bool {
    desired.iter().all(|s| recorded.contains(s))
}
```

- [ ] **Step 4: 改 `generate_self_signed` 接受 SAN 列表**

把现有 `generate_self_signed` 整个函数替换为：

```rust
/// rcgen 0.13 self-signed for the given SANs (each string classified as IP or
/// DNS by rcgen). Returns PEM cert, PEM key, and the SHA-256 fingerprint hex.
fn generate_self_signed(sans: &[String]) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(sans.to_vec())?;
    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fp = fingerprint(&cert_pem);
    Ok((cert_pem, key_pem, fp))
}
```

- [ ] **Step 5: 重写 `load_or_generate` 的 SelfSigned 分支**

把 `load_or_generate` 里 `TlsMode::SelfSigned => { ... }` 整块替换为：

```rust
        TlsMode::SelfSigned => {
            let cert_file = dir.join("cert.pem");
            let key_file = dir.join("key.pem");
            let san_file = dir.join("sans.txt");

            let discovered = discover_interface_ips();
            let desired = self_signed_sans(&cfg.san, &discovered);

            if cert_file.exists() && key_file.exists() && san_file.exists() {
                let recorded = parse_recorded_sans(&tokio::fs::read_to_string(&san_file).await?);
                if desired_covered(&recorded, &desired) {
                    let cert = tokio::fs::read(&cert_file).await?;
                    let key = tokio::fs::read(&key_file).await?;
                    let fp = fingerprint(&cert);
                    return Ok((cert, key, fp));
                }
            }

            let (cert, key, fp) = generate_self_signed(&desired)?;
            tokio::fs::create_dir_all(dir).await?;
            tokio::fs::write(&cert_file, &cert).await?;
            tokio::fs::write(&key_file, &key).await?;
            tokio::fs::write(&san_file, desired.join("\n")).await?;
            Ok((cert, key, fp))
        }
```

- [ ] **Step 6: 更新模块头注释**

把 `src/gateway/tls.rs` 顶部 `//! ... auto self-signed (generated once via rcgen ...` 段的说明更新为反映接口 IP 覆盖。将现有第 3–6 行替换为：

```rust
//! Three modes off `[gateway.tls]`: disabled (plaintext), operator-provided
//! cert/key files, or auto self-signed. The self-signed cert's SAN covers
//! loopback, every non-loopback interface IP (auto-discovered), and any
//! `[gateway.tls] san` extras; it is persisted to `~/.aleph/data/tls/` with a
//! `sans.txt` sidecar, and regenerated when a newly-desired SAN is not yet
//! covered. No ACME here — auto-issuance is Caddy's / certbot's job (R3).
```

- [ ] **Step 7: 运行测试确认通过（含既有复用测试未回归）**

Run: `cargo test -p alephcore gateway::tls -- --nocapture`
Expected: PASS（新增 3 测试 + 既有 `mode_resolution` / `self_signed_generates_persists_and_reuses` 全通过）

- [ ] **Step 8: 提交**

```bash
git add src/gateway/tls.rs
git commit -m "gateway: SAN-drift sidecar — subset-reuse or regenerate self-signed cert"
```

---

### Task 4: 文档

**Files:**
- Modify: `docs/reference/SECURITY.md`（Tier ② 段，约 911–927 行）

**Interfaces:** 无代码接口。

- [ ] **Step 1: 更新 SECURITY.md Tier ② 文案**

把 `#### Tier ② — Native self-signed TLS (no domain; weaker)` 到其 TOML 块之间的正文段替换为：

```markdown
No domain, no proxy — Aleph generates and persists a self-signed cert to
`~/.aleph/data/tls/` on first boot and logs its SHA-256 fingerprint. Its SAN
**auto-covers loopback plus every non-loopback interface IP of the box**
(e.g. a VPS public IP on `eth0`), so connecting by that IP passes TLS hostname
validation; add hostnames or a NAT'd public IP via `[gateway.tls] san = [...]`.
Clients still get a browser cert warning (accept-once, or pin the fingerprint) —
encryption is real, the trust anchor is manual. A newly-appearing address
regenerates the cert (new fingerprint ⇒ re-trust once); a `sans.txt` sidecar
tracks coverage so churn stays minimal.
```

并把该 Tier 的 TOML 块替换为（加上 `san` 示例注释）：

```markdown
```toml
[gateway]
host = "0.0.0.0"

[gateway.tls]
enabled = true          # empty cert/key paths ⇒ auto self-signed
# san = ["vps.example.com"]   # optional: hostnames or a NAT'd IP not on any local interface
```
```

- [ ] **Step 2: 提交**

```bash
git add docs/reference/SECURITY.md
git commit -m "docs: SECURITY.md Tier 2 — self-signed SAN auto-covers interface IPs + san extras"
```

---

### Task 5: 构建 Linux server + 部署 ColoCrossing + 验证

> 非 TDD，属交付/验收任务。成功标准：远程按 IP 连接时 TLS 主机名校验通过。

**Files:** 无源码改动（构建 + 部署 + 验证）。

- [ ] **Step 1: 本地定向编译校验（低成本，非全量）**

Run: `cargo check -p alephcore`
Expected: 编译通过，无 error（`if-addrs` 直接依赖解析、`tls.rs` 改动编译干净）

- [ ] **Step 2: 构建 Linux `aleph-server` 二进制**

用既有 LAN Linux 构建机（见记忆 `project-ubuntudev-linux-build-machine` / `project-debian-server-manual-build-deploy`）在 x86_64-unknown-linux-gnu 上 `cargo build --release --bin aleph-server`，产出二进制；或若走发版，`just release <YY.M.D>` 后从 Release 拉 `aleph-server-x86_64-unknown-linux-gnu`。产出记为本地路径 `<linux-binary>`。
Expected: `<linux-binary> --version` 打印目标版本。

- [ ] **Step 3: 传到生产机并原子替换（分步、零中间态）**

```bash
scp <linux-binary> ColoCrossing:~/.local/bin/aleph-server.new
ssh ColoCrossing 'set -e; cd ~/.local/bin
  chmod +x aleph-server.new
  ./aleph-server.new --version                     # 确认新版本
  cp -p aleph-server aleph-server.bak-pre-san      # 备份
  ./aleph-server stop                              # 优雅停（IPC SIGTERM）
  for i in $(seq 1 12); do pgrep -x aleph-server >/dev/null || break; sleep 0.5; done
  pgrep -x aleph-server >/dev/null && pkill -x aleph-server || true   # 兜底：精确名不自杀
  mv aleph-server.new aleph-server                 # 原子替换
  ./aleph-server -d --bind 0.0.0.0 --port 18790 --log-file ~/.aleph/server.log start'
```
Expected: `Gateway stopped successfully` → 新进程启动，无 "Refusing to start"。

- [ ] **Step 4: 验证证书 SAN 已含公网 IP，且按 IP 连接主机名校验通过**

```bash
ssh ColoCrossing 'set -e
  echo "=== SAN ==="; openssl x509 -in ~/.aleph/data/tls/cert.pem -noout -ext subjectAltName
  echo "=== curl by IP, cert as trust anchor (no -k) ==="
  curl --cacert ~/.aleph/data/tls/cert.pem -o /dev/null -w "HTTPS %{http_code}\n" --max-time 8 https://172.245.43.211:18790/
  echo "=== new fingerprint ==="; openssl x509 -in ~/.aleph/data/tls/cert.pem -noout -fingerprint -sha256'
```
Expected:
- `subjectAltName` 一行含 `IP Address:172.245.43.211`（及 `DNS:localhost, IP Address:127.0.0.1` 等）。
- `curl --cacert ... https://172.245.43.211:18790/` 返回 `HTTPS 200` 且**无** `SSL: no alternative certificate subject name matches` 错误 = 信任锚 + SAN 双通过。

- [ ] **Step 5: 记录新指纹并收尾**

把 Step 4 打印的新 SHA-256 指纹告知用户（客户端首次告警时可比对 / pin）。更新记忆 `project-release-26-7-17-and-debian-tls-deploy` 的 SAN 遗留坑段为「已修复：自签 SAN 现自动含公网 IP，指纹 = <新值>」。

---

## Self-Review

**1. Spec coverage（逐节对照 spec）：**
- §3.2 config `san` 字段 → Task 1 Step 1 ✓
- §3.3 纯/非纯拆分（`discover_interface_ips` / `self_signed_sans` / 校验 / 去重）→ Task 1（纯）+ Task 2（非纯）✓
- §3.4 生成用组装 SAN → Task 3 Step 4 ✓
- §3.5 sidecar 子集重生 → Task 3 Step 1/3/5（`sans.txt`、`desired_covered` 子集、重生写 sidecar）✓
- §3.6 编排 → Task 3 Step 5 ✓
- §4 错误/边界（发现失败返空、docker 网桥无害、现存证书重生、畸形 san 丢弃）→ Task 2 Step 4（返空）、Task 1 Step 5（丢弃）、Task 3 测试（重生）✓
- §5 测试（组装 / reuse-regen / 生成含 IP SAN）→ Task 1/2/3 测试 + Task 5 Step 4 openssl 断言 IP SAN ✓
- §6 文档 + 部署 → Task 4 + Task 5 ✓
- §7 影响面（if-addrs 提直接依赖、`#[serde(default)]` 兼容）→ Task 2 Step 1、Task 1 Step 1 ✓

> 注：spec §3.5 写的 sidecar 名为 `sans.json`；计划统一用 **`sans.txt`（换行分隔）** 以零额外依赖（不引 serde_json），格式为实现细节，行为一致。

**2. Placeholder scan：** 无 TBD/TODO；所有代码步骤含完整代码；Task 5 构建步骤给出两条既有确定路径（LAN 构建机 / 发版拉取），非占位。

**3. Type consistency：** `self_signed_sans(&[String], &[IpAddr]) -> Vec<String>`、`discover_interface_ips() -> Vec<IpAddr>`、`is_usable_san_ip(&IpAddr) -> bool`、`parse_recorded_sans(&str) -> HashSet<String>`、`desired_covered(&HashSet<String>, &[String]) -> bool`、`generate_self_signed(&[String]) -> anyhow::Result<(Vec<u8>,Vec<u8>,String)>` — 各 Task 引用一致；`GatewayTlsConfig.san: Vec<String>` 在 Task 1 定义、Task 3 测试消费一致。
