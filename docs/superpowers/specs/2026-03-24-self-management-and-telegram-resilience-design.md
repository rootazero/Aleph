# Self-Management System & Telegram Resilience

Date: 2026-03-24 (v3 — full skill-ization + skills repo separation)

## Problem Statement

Four related issues prevent Aleph from reliably handling self-configuration requests:

1. **Telegram polling stall with no recovery**: The watchdog detects long-polling stalls (>90s) but only logs warnings. A stall on 2026-03-22 16:55 persisted for 22000+ seconds with no auto-restart. All Telegram messages were silently dropped.

2. **Self-management embedded in core**: Self-management logic lives in `OperationalGuidelinesLayer` (prompt layer) and is paradigm-restricted (only Background/CLI). This violates R3 (Core Minimalism) and R10 (Intelligence Lives in the Prompt) — it should be a skill.

3. **No explicit self-management entry point**: Users need a `/self` command to explicitly tell the LLM "you're managing yourself now" with full workspace context, available on all channels.

4. **Official skills repo lacks update mechanism**: `~/.aleph/skills/` is git-cloned from the official repo on first install, but never updated. User-added skills in the same directory create git conflicts on pull.

## Design

### Fix 1: Telegram Polling Stall Auto-Restart

**File**: `src/gateway/interfaces/telegram/mod.rs`

#### Mechanism

Wrap the dispatcher lifecycle inside a retry loop within the existing spawned task. When the watchdog detects a stall exceeding `STALL_RESTART_THRESHOLD` (300s), it signals the loop to restart the dispatcher.

```rust
tokio::spawn {
  let mut attempt = 0u32;
  let mut healthy_since: Option<Instant> = None;
  loop {
    attempt += 1;
    // Rebuild handlers, dispatcher, watchdog each iteration
    // Clone inbound_tx from the base sender (still connected to forwarder)
    // pairing_codes, runtime_allowed_users etc are Arc<RwLock> — clone survives

    let (stall_tx, stall_rx) = mpsc::channel::<()>(1);
    let watchdog_cancel = CancellationToken::new();
    // Watchdog: checks every 60s, sends to stall_tx when gap > 300s
    // Uses watchdog_cancel for clean shutdown

    // NOTE: remove .enable_ctrlc_handler() — shutdown managed via shutdown_tx
    let mut dispatcher = Dispatcher::builder(bot, handler).build();

    let which = select! {
      _ = dispatcher.dispatch() => "stopped",
      _ = &mut shutdown_rx => "shutdown",  // &mut borrow — survives across iterations
      _ = stall_rx.recv() => "stall",
    };

    // Cancel watchdog before next iteration or exit
    watchdog_cancel.cancel();

    if which == "shutdown" { break; }

    // Set status to Connecting (reuse existing variant)
    *status.write().await = ChannelStatus::Connecting;
    tracing::error!(attempt, "Telegram polling {} — auto-restarting", which);

    // Exponential backoff: 5s → 10s → 20s → 40s → 60s cap
    // Reset attempt counter if last healthy run lasted > 5 min
    if healthy_since.is_some_and(|t| t.elapsed() > Duration::from_secs(300)) {
      attempt = 1; // Reset to 1 (not 0) so backoff exponent is valid
    }
    let delay = std::cmp::min(5 * 2u64.pow(attempt.saturating_sub(1).min(4)), 60);
    tokio::time::sleep(Duration::from_secs(delay)).await;

    // Reset last_update timestamp and healthy tracker
    last_update_at.store(now_secs(), Relaxed);
    healthy_since = Some(Instant::now());

    tracing::info!(attempt, "Telegram reconnected, queued messages will be delivered");
    *status.write().await = ChannelStatus::Connected;
  }
}
```

#### Key Design Decisions

- **`&mut shutdown_rx` borrow**: `tokio::select!` borrows the oneshot receiver, so it survives across loop iterations. Only consumed when the sender fires.
- **Watchdog lifecycle**: Each iteration creates a fresh `CancellationToken` + watchdog task. On loop restart (stall or unexpected stop), `watchdog_cancel.cancel()` kills the old watchdog before spawning a new one. No duplicate watchers.
- **No `.enable_ctrlc_handler()`**: Removed because teloxide's Ctrl+C handler would cause `dispatch()` to return normally, triggering an unwanted restart. Shutdown is managed exclusively via `shutdown_tx` from `stop()`.
- **Attempt counter reset**: If the dispatcher ran healthy for >5 minutes before the next stall, `attempt` resets to 1 (not 0, to avoid unsigned underflow in exponent). `saturating_sub(1)` provides additional safety.
- **Arc state survives**: `pairing_codes`, `runtime_allowed_users`, `pairing_prompt_times` are all `Arc<RwLock<...>>` — cloned before the spawn, valid across all loop iterations.

#### Queue Coordination

- **No message loss**: Telegram server queues undelivered updates for 24h. On reconnect, teloxide resumes from last acknowledged offset — burst delivery is automatic and ordered.
- **Backpressure**: The existing bounded `mpsc` channel between Telegram handler and inbound forwarder provides natural backpressure during burst catch-up.
- **Status visibility**: `ChannelStatus::Connecting` during backoff sleep, `Connected` after successful restart. Log messages distinguish initial connect from reconnect.
- **Reconnect log**: `INFO "Telegram reconnected, queued messages will be delivered"` after each successful restart.

#### Parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| `STALL_WARN_THRESHOLD` | 90s | Existing WARN log, unchanged |
| `STALL_RESTART_THRESHOLD` | 300s | Triggers auto-restart |
| Backoff | 5s → 10s → 20s → 40s → 60s cap | Resets if healthy >5min |
| Max attempts | Unlimited | Retry forever until shutdown |

#### Reconnection Status

Reuse existing `ChannelStatus::Connecting` during reconnection backoff — no new enum variant needed. Log messages distinguish initial connect from reconnect (`"auto-restarting (attempt N)"` vs `"Starting Telegram channel..."`). This avoids breaking exhaustive `match` sites in `channel_registry.rs` and `whatsapp/pairing.rs`.

---

### Fix 2: Self-Management Full Skill-ization

Self-management moves entirely out of core into a skill. This aligns with R3 (Core Minimalism), R8 (LLM Sovereignty), and R10 (Intelligence Lives in the Prompt).

#### 2a. Remove self-management from OperationalGuidelinesLayer

**File**: `src/thinker/layers/operational_guidelines.rs`

Remove the "Self-Management" section (lines 46-53) that mentions `read_config_guide`. Keep the "Diagnostic Capabilities" section (read-only monitoring) and "When You Detect Issues" / "What You Must NEVER Do Autonomously" sections — these are safety guidelines, not self-management.

Before:
```rust
output.push_str("### Self-Management\n");
output.push_str("You can manage all Aleph configuration. When needed, call read_config_guide(topic) ");
// ...
```

After: Section removed entirely. The `/self` skill now owns all self-management prompt content.

**Paradigm filter unchanged**: The remaining diagnostic/safety content stays limited to Background/CLI as before — these are operational awareness features that don't need Messaging paradigm support. When users want self-management on any channel, they use `/self`.

#### 2b. `/self` Skill in Official Repo

**New file**: `~/Workspace/Aleph-skills/self/SKILL.md`

Deployed to `~/.aleph/skills-official/self/SKILL.md` via the skills repo mechanism (see Fix 4).

Frontmatter:
```yaml
---
name: Self-Management
description: "Enter self-management mode — configure providers, agents, channels, skills, generation, and other system settings"
scope: system
invocation:
  user_invocable: true
  disable_model_invocation: true
---
```

User flow: `/self <request>` → skill fallthrough → LLM receives self-management prompt + user's original request → LLM calls `read_config_guide(topic)` → follows guide to configure.

#### 2c. Skill Prompt Content

The prompt provides three layers of knowledge:

1. **Workspace map** (`~/.aleph/` directory tree with purpose annotations)
2. **Operation protocol** (backup → read → plan → confirm → write → verify → reload)
3. **Domain routing** (table mapping topics to `read_config_guide(topic)` calls)

```markdown
# Aleph Self-Management Mode

You are now in self-management mode. You have full access to read, modify,
and manage your own configuration and workspace.

## Workspace Structure: ~/.aleph/

~/.aleph/
├── config.toml              # Main config (hot-reload via fswatch)
├── soul.md                  # Global persona definition
├── user_profile.md          # User profile (loaded per session)
├── mcp_config.json          # MCP server definitions (mcp_manage to reload)
│
├── agents/{id}/             # Agent data directory
│   ├── SOUL.md              # Persona (SoulManifest, YAML frontmatter)
│   ├── IDENTITY.md          # Name, role, description
│   ├── MEMORY.md            # Long-term memory (≤20K chars)
│   ├── AGENTS.md            # Operating manual
│   ├── TOOLS.md             # Tool configuration
│   ├── HEARTBEAT.md         # Heartbeat state
│   └── sessions/            # Session history
│
├── workspaces/{id}/         # Agent workspace (file output)
│   ├── output/              # Generated files
│   └── .tool_output/        # Tool temporary output
│
├── guides/                  # Config guides (read by read_config_guide tool)
│   ├── overview.md          # File map, operation model, all sections
│   ├── providers.md         # LLM providers (OpenAI, Claude, Gemini, Ollama)
│   ├── generation.md        # Media generation (image/speech/video/audio)
│   ├── channels.md          # Telegram, Discord, etc.
│   ├── agents.md            # Agent workspace, SOUL.md, identity
│   ├── skills.md            # Skill install, format, discovery
│   ├── mcp.md               # MCP server configuration
│   ├── general.md           # Default provider, language, policies
│   └── cron.md              # Scheduled tasks
│
├── skills/                  # User custom skills (not git-managed by Aleph)
│   └── {name}/SKILL.md
│
├── skills-official/         # Official skills repo (git clone, auto-updated)
│   └── {name}/SKILL.md
│
├── plugins/                 # Plugin system
│   ├── installed/{name}/    # Installed plugins
│   └── cache/               # Marketplace cache
│
├── data/                    # Persistent data (LanceDB, vault, sessions DB)
├── logs/                    # Log files (aleph-server.log.YYYY-MM-DD)
├── backups/                 # Config backups (timestamped)
├── browser/                 # Headless browser profile
├── templates/               # Team templates
├── output/                  # Global default output directory
└── .venv/                   # Python virtual environment (for skills)

## Operation Protocol

1. **Backup first**: `bash(cp ~/.aleph/config.toml ~/.aleph/config.toml.bak)`
2. **Read current state**: Read the target file before any modification
3. **Show plan**: Tell the user what you intend to change and why
4. **Confirm**: Wait for user confirmation before writing
5. **Write**: Make the change
6. **Verify**: Read back and validate format
7. **Reload**: config.toml auto-reloads; MCP needs `mcp_manage`; channels need restart

## Secret Management (CRITICAL)

API keys and credentials MUST be stored in the encrypted vault.
NEVER write secrets to config files.

- Store: vault_store(action="store", key="<convention>", secret="<value>")
- Delete: vault_store(action="delete", key="<convention>")
- List: vault_store(action="list")

Key naming conventions:
- LLM providers: provider:{name} (e.g., provider:openai)
- Generation providers: gen:{name} (e.g., gen:stability)
- Channels: channel:{type}:{id} (e.g., channel:telegram:bot1)

## Detailed Guides

For specific configuration domains, call read_config_guide(topic):

| Topic | Covers |
|-------|--------|
| overview | File map, operation model, all sections |
| providers | LLM providers (OpenAI, Claude, Gemini, Ollama) |
| generation | Image/speech/video/audio generation providers |
| channels | Telegram, Discord, channel-agent bindings |
| agents | Agent workspace, SOUL.md, identity, model override |
| skills | Skill install, format, discovery |
| mcp | MCP server configuration |
| general | Default provider, language, memory, policies |
| cron | Scheduled tasks |

**Always call the relevant guide before making changes** — guides contain
structure templates, field definitions, and caveats you need.

## Common Workflows

### Add a generation provider (image/video/speech/audio)
1. read_config_guide(topic="generation") for structure and URL rules
2. Add [generation.providers.<name>] to config.toml
   - base_url: use full URL for non-standard APIs (no auto-completion),
     or standard base URL for OpenAI-compatible APIs (system auto-appends path)
3. vault_store(action="store", key="gen:<name>", secret="<api_key>")
4. Optionally set default_<type> = "<name>" in [generation]

### Add an LLM provider
1. read_config_guide(topic="providers") for structure
2. Add [providers.<name>] with protocol, models, enabled=true
3. vault_store(action="store", key="provider:<name>", secret="<api_key>")

### Modify agent personality
1. Read ~/.aleph/agents/{id}/SOUL.md
2. Edit content, preserve YAML frontmatter structure
3. Write back — takes effect on next agent resolution

### Install a plugin
1. aleph plugin marketplace update
2. aleph plugin install <name>
```

#### 2d. `read_config_guide` Tool and Guides — Preserved

`ReadConfigGuideTool` (`src/builtin_tools/config_guide.rs`) and `~/.aleph/guides/*.md` are **kept as-is**. They serve as the data layer that the `/self` skill directs the LLM to use. The skill provides routing knowledge; the tool + guides provide domain knowledge.

#### 2e. Update `~/.aleph/guides/overview.md`

Add the workspace directory tree (including new `skills-official/` directory) and missing entries (plugins/, workspaces/, backups/, browser/, templates/, output/).

#### 2f. Update `~/.aleph/guides/generation.md`

Add video and audio provider examples, plus URL auto-completion rules:

**URL auto-completion rules** (new section):

```markdown
## URL Rules

- **Standard URL** (base only): System auto-appends OpenAI-compatible path segments.
  Examples: `https://api.openai.com` or `https://api.openai.com/v1`
  → System appends `/images/generations`, `/audio/speech`, etc. as needed.

- **Full URL** (complete path): System uses the URL as-is, no auto-completion.
  Example: `https://ai.t8star.cn/v2/videos/generations`
  → Used exactly as written.

Rule of thumb: if your URL ends with a resource path (e.g., `/generations`, `/speech`),
it's treated as a full URL. If it ends with a version prefix or domain root, the system
appends the standard path.
```

**Video/audio provider examples** (new):

```toml
[generation.providers.t8star-video]
type = "video"
provider = "t8star_veo"
base_url = "https://ai.t8star.cn/v2/videos/generations"  # Full URL — no auto-completion
model = "veo3.1-pro-4k"
# api_key — vault_store with key "gen:t8star-video"

[generation.providers.suno]
type = "audio"
provider = "suno"
model = "v4"
# api_key — vault_store with key "gen:suno"
```

---

### Fix 3: Skills Directory Separation + Auto-Update

#### 3a. Directory Separation

Split the current single `~/.aleph/skills/` into two directories:

```
~/.aleph/skills/            # User custom skills (user-managed, no Aleph git ops)
~/.aleph/skills-official/   # Official repo (git clone from GitHub, auto-updated)
```

**Migration**: On first startup after upgrade, if `~/.aleph/skills/.git` exists and remote matches the official repo URL:
1. Move `~/.aleph/skills/` → `~/.aleph/skills-official/`
2. Create empty `~/.aleph/skills/`
3. Any non-git-tracked files in the old dir (user skills) are moved to new `~/.aleph/skills/`

If `.git` doesn't exist or remote doesn't match, leave as-is and create `skills-official/` fresh via clone.

#### 3b. SkillSystem::init() — Scan Both Directories

**File**: `src/skill/mod.rs` and startup code

Update `SkillSystem::init()` to receive both directories:

```rust
// Startup provides both paths:
let dirs = vec![
    skills_official_dir,  // ~/.aleph/skills-official/ (SkillSource::Global)
    skills_user_dir,      // ~/.aleph/skills/ (SkillSource::Global)
];
skill_system.init(dirs).await?;
```

Scan order matters: user dir is scanned **after** official dir. Since `SkillRegistry::register()` replaces lower-priority entries with equal-or-higher-priority ones, and both are `SkillSource::Global` (priority 2), the **later registration wins**. This means user skills with the same name override official skills.

#### 3c. Auto-Update on Startup

**File**: New function in `src/skills/mod.rs` (or a new `src/skills/updater.rs`)

On server startup, before `SkillSystem::init()`:

```rust
/// Update the official skills repo via git pull.
/// Non-blocking: errors are logged, never prevent startup.
pub async fn update_official_skills(skills_official_dir: &Path) -> Result<()> {
    if !skills_official_dir.join(".git").exists() {
        // First install: clone
        let url = "https://github.com/rootazero/Aleph-skills.git";
        Command::new("git")
            .args(["clone", "--depth", "1", url])
            .arg(skills_official_dir)
            .output().await?;
        return Ok(());
    }

    // Existing repo: fast-forward pull
    let output = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(skills_official_dir)
        .output().await?;

    if output.status.success() {
        info!("Official skills updated");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Official skills update failed (non-fatal): {}", stderr);
        // If ff-only fails (diverged), reset to remote
        // This is safe because skills-official is read-only
        let _ = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(skills_official_dir)
            .output().await;
        let _ = Command::new("git")
            .args(["reset", "--hard", "origin/main"])
            .current_dir(skills_official_dir)
            .output().await;
        info!("Official skills force-reset to origin/main");
    }
    Ok(())
}
```

**Key decisions**:
- `--ff-only` first: safe, no merge conflicts possible
- If ff-only fails (shouldn't happen for a read-only dir, but defensive): `fetch` + `reset --hard origin/main`. This is safe because `skills-official/` is Aleph-managed, not user-editable
- Runs on every server startup (not cron): startup frequency is reasonable, and it's the simplest reliable trigger
- Async with timeout (10s): if network is unavailable, log WARN and continue with existing skills
- Never blocks startup

#### 3d. Config for Official Repo URL

**File**: `~/.aleph/config.toml`

```toml
[skills]
official_repo = "https://github.com/rootazero/Aleph-skills.git"  # default
auto_update = true  # default true
```

This allows users to disable auto-update or point to a fork.

---

## Files Changed

| File | Change |
|------|--------|
| `src/gateway/interfaces/telegram/mod.rs` | Retry loop + stall restart |
| `src/thinker/layers/operational_guidelines.rs` | Remove Self-Management section (keep diagnostics/safety) |
| `src/skill/mod.rs` | Scan `skills-official/` in addition to `skills/` |
| `src/skills/mod.rs` (or new `updater.rs`) | `update_official_skills()` function |
| Startup code (`server_init.rs` or `coordinator.rs`) | Call `update_official_skills()` before `SkillSystem::init()`, migration logic |
| `~/Workspace/Aleph-skills/self/SKILL.md` | **New**: `/self` skill in official repo |
| `~/.aleph/guides/overview.md` | Add workspace tree with `skills-official/` |
| `~/.aleph/guides/generation.md` | Add video/audio examples + URL rules |

## Testing

- **Telegram stall**: Unit test with mock dispatcher that exits immediately — verify retry loop increments attempt, resets timestamp, and cancels old watchdog. Test backoff calculation edge cases (attempt=1, healthy reset). Integration: manually kill network → verify reconnect log.
- **Self-management removal**: Verify `OperationalGuidelinesLayer` output no longer contains "Self-Management" or "read_config_guide". Existing diagnostic/safety tests still pass.
- **`/self` skill**: Verify SKILL.md parses correctly. End-to-end: `/self list providers` via Panel, confirm LLM calls `read_config_guide`.
- **Skills separation**: Test migration logic: (a) existing git repo migrates correctly, (b) user files preserved, (c) fresh install clones successfully.
- **Auto-update**: Test `update_official_skills()`: (a) fresh clone, (b) ff-only success, (c) network failure → graceful degradation, (d) diverged repo → force reset.
