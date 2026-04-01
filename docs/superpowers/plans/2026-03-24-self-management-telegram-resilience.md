# Self-Management & Telegram Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Telegram polling stall auto-restart, move self-management from core to a skill, and separate official/user skills directories with auto-update.

**Architecture:** Three independent fixes. Fix 1 adds a retry loop inside the Telegram spawned task. Fix 2 removes self-management from `OperationalGuidelinesLayer` and creates a `/self` SKILL.md in the official skills repo. Fix 3 splits `~/.aleph/skills/` into `skills/` (user) + `skills-official/` (git-managed), adds `update_official_skills()` called on startup, and updates `DiscoveryManager` to scan both directories.

**Tech Stack:** Rust (tokio, teloxide), Markdown (SKILL.md)

**Spec:** `docs/superpowers/specs/2026-03-24-self-management-and-telegram-resilience-design.md`

---

## File Map

| File | Responsibility | Task |
|------|---------------|------|
| `src/gateway/interfaces/telegram/mod.rs` | Telegram polling retry loop + watchdog restart trigger | 1 |
| `src/thinker/layers/operational_guidelines.rs` | Remove Self-Management prompt section | 2 |
| `/Users/zouguojun/Workspace/Aleph-skills/self/SKILL.md` | NEW: `/self` skill with full workspace map + operation protocol | 3 |
| `src/skill/mod.rs` | Update `guess_source()` — return Bundled for skills-official/ | 4 |
| `src/extension/mod.rs` | Add skills-official/ to SkillSystem scan dirs | 4 |
| `src/skills/updater.rs` | NEW: `update_official_skills()` git pull logic | 5 |
| `src/skills/mod.rs` | Re-export updater module | 5 |
| `src/bin/aleph-server/commands/start/mod.rs` | Call `update_official_skills()` before extension load | 5 |
| `~/.aleph/guides/overview.md` | Update workspace tree (add `skills-official/`) | 6 |
| `~/.aleph/guides/generation.md` | Add video/audio examples + URL rules | 6 |

---

### Task 1: Telegram Polling Stall Auto-Restart

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs:560-784`

This is the largest task. The current code spawns a single task that builds handlers, creates a dispatcher, spawns a watchdog, and runs `select!` on dispatch/shutdown. We wrap lines 564-781 in a retry loop, move watchdog creation inside the loop, add a stall restart channel, and remove `.enable_ctrlc_handler()`.

- [ ] **Step 1: Add constants at the top of the spawned task block**

At `src/gateway/interfaces/telegram/mod.rs`, inside the `tokio::spawn(async move {` block (line 560), before `tracing::info!("Starting Telegram long-polling...");` (line 561), add:

```rust
const STALL_WARN_SECS: u64 = 90;
const STALL_RESTART_SECS: u64 = 300;

let mut attempt = 0u32;
let mut healthy_since: Option<Instant> = None;
```

Also add `use std::time::Instant;` if not already imported (check file head — `Instant` is already imported at line 36 via `use std::time::{Instant, SystemTime, UNIX_EPOCH};`).

- [ ] **Step 2: Wrap handler+dispatcher+watchdog+select in retry loop**

Replace lines 561-783 (from `tracing::info!("Starting Telegram long-polling...");` through `*status.write().await = ChannelStatus::Disconnected;`) with the retry loop structure. The key changes:

1. Open `loop {` after the constants, with `attempt += 1;` at top
2. Move **all** handler building (message_handler, callback_handler, handler combining, dispatcher building, watchdog, select!) inside the loop
3. The `inbound_tx`, `config`, `channel_id`, `pairing_codes_clone`, etc. variables declared before the spawn at lines 535-558 are still valid — they are `Clone` types captured by the outer `move` block. Inside the loop, re-clone them for each iteration's closures.

The full replacement for the spawned task body:

```rust
tracing::info!("Starting Telegram long-polling...");
*status.write().await = ChannelStatus::Connected;

const STALL_WARN_SECS: u64 = 90;
const STALL_RESTART_SECS: u64 = 300;

let mut attempt = 0u32;
let mut healthy_since: Option<Instant> = None;

loop {
    attempt += 1;

    // Clone senders for this iteration's handler closures
    let iter_inbound_tx = inbound_tx.clone();
    let iter_inbound_tx_cb = inbound_tx_for_cb.clone();
    let iter_callback_tx = callback_tx.clone();
    let iter_config = config.clone();
    let iter_config_cb = config_for_cb.clone();
    let iter_channel_id = channel_id.clone();
    let iter_channel_id_cb = channel_id_for_cb.clone();
    let iter_last_update_msg = last_update_at.clone();
    let iter_last_update_cb = last_update_at.clone();
    let iter_last_update_wd = last_update_at.clone();
    let iter_pairing_codes = pairing_codes_clone.clone();
    let iter_prompt_times = prompt_times_clone.clone();
    let iter_runtime_users = runtime_users_clone.clone();
    let iter_runtime_users_cb = runtime_users_for_cb.clone();

    // -- Build message handler (same logic as before, using iter_ clones) --
    let message_handler = Update::filter_message().endpoint(
        move |bot: Bot, msg: teloxide::types::Message| {
            let inbound_tx = iter_inbound_tx.clone();
            let config = iter_config.clone();
            let channel_id = iter_channel_id.clone();
            let last_update = iter_last_update_msg.clone();
            let pairing_codes = iter_pairing_codes.clone();
            let prompt_times = iter_prompt_times.clone();
            let runtime_users = iter_runtime_users.clone();
            async move {
                last_update.store(
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                    Ordering::Relaxed,
                );
                if let Some(inbound) = TelegramChannel::convert_message(
                    &msg, &bot, &config, &channel_id, &runtime_users,
                ).await {
                    if let Err(e) = inbound_tx.send(inbound).await {
                        tracing::error!("Failed to send inbound message: {}", e);
                    }
                } else if let Some(from) = &msg.from {
                    // ... (exact same pairing logic as current lines 586-636) ...
                    let user_id = from.id.0 as i64;
                    let is_dm = !msg.chat.is_group() && !msg.chat.is_supergroup();
                    let has_allowlist = !config.allowed_users.is_empty()
                        || !runtime_users.read().await.is_empty();
                    if is_dm && has_allowlist && !config.is_user_allowed(user_id) {
                        if let Some(text) = msg.text() {
                            let code = text.trim().to_uppercase();
                            let mut codes = pairing_codes.write().await;
                            if let Some(entry) = codes.get(&code) {
                                if !entry.is_expired() {
                                    codes.remove(&code);
                                    drop(codes);
                                    runtime_users.write().await.push(user_id);
                                    let _ = bot.send_message(msg.chat.id, "Paired successfully! You can now send messages.").await;
                                    tracing::info!("User {} paired via code {}", user_id, code);
                                } else {
                                    codes.remove(&code);
                                    let _ = bot.send_message(msg.chat.id, "Pairing code expired. Please request a new one.").await;
                                }
                            } else {
                                let mut times = prompt_times.write().await;
                                let should_prompt = times.get(&user_id).map(|t| t.elapsed().as_secs() > 300).unwrap_or(true);
                                if should_prompt {
                                    times.insert(user_id, Instant::now());
                                    let _ = bot.send_message(msg.chat.id, "Please enter your pairing code.").await;
                                }
                            }
                        }
                    }
                }
                Ok::<(), std::convert::Infallible>(())
            }
        },
    );

    // -- Build callback handler (same logic as before, using iter_ clones) --
    let callback_handler = Update::filter_callback_query().endpoint(
        move |bot: Bot, q: TgCallbackQuery| {
            let tx = iter_callback_tx.clone();
            let inbound_tx = iter_inbound_tx_cb.clone();
            let config = iter_config_cb.clone();
            let channel_id = iter_channel_id_cb.clone();
            let last_update = iter_last_update_cb.clone();
            let runtime_users = iter_runtime_users_cb.clone();
            async move {
                last_update.store(
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                    Ordering::Relaxed,
                );
                // ... (exact same callback logic as current lines 659-729) ...
                let (raw_chat_id, thread_id_val) = q.message.as_ref()
                    .map(|m| {
                        let chat = m.chat().id.0;
                        let tid = match m {
                            teloxide::types::MaybeInaccessibleMessage::Regular(msg) => msg.thread_id.map(|t| t.0.0),
                            _ => None,
                        };
                        (chat, tid)
                    })
                    .unwrap_or((0, None));
                let conv_id_str = if let Some(tid) = thread_id_val {
                    format!("{}:topic:{}", raw_chat_id, tid)
                } else {
                    raw_chat_id.to_string()
                };
                let msg_id_str = q.message.as_ref().map(|m| m.id().to_string()).unwrap_or_default();
                if let Some(data) = q.data.clone() {
                    let user_id_val = q.from.id.0 as i64;
                    let query = CallbackQuery {
                        id: q.id.clone(),
                        user_id: UserId::new(q.from.id.to_string()),
                        chat_id: ConversationId::new(conv_id_str.clone()),
                        message_id: MessageId::new(msg_id_str),
                        data: data.clone(),
                    };
                    if let Err(e) = tx.send(query).await {
                        tracing::error!("Failed to send callback query: {}", e);
                    }
                    let rt_allowed = runtime_users.read().await.contains(&user_id_val);
                    if config.is_user_allowed(user_id_val) || rt_allowed {
                        let inbound = InboundMessage {
                            id: MessageId::new(format!("cb_{}", q.id)),
                            channel_id: channel_id.clone(),
                            conversation_id: ConversationId::new(conv_id_str),
                            sender_id: UserId::new(q.from.id.to_string()),
                            sender_name: q.from.username.clone().or_else(|| Some(q.from.first_name.clone())),
                            text: data,
                            attachments: Vec::new(),
                            timestamp: Utc::now(),
                            reply_to: None,
                            is_group: false,
                            raw: None,
                        };
                        if let Err(e) = inbound_tx.send(inbound).await {
                            tracing::error!("Failed to re-inject callback as inbound message: {}", e);
                        }
                    }
                }
                if let Err(e) = bot.answer_callback_query(&q.id).await {
                    tracing::warn!("Failed to answer callback query: {}", e);
                }
                Ok::<(), std::convert::Infallible>(())
            }
        },
    );

    let handler = dptree::entry()
        .branch(message_handler)
        .branch(callback_handler);

    // NOTE: no .enable_ctrlc_handler() — shutdown managed via shutdown_tx
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler).build();

    // Stall restart channel for this iteration
    let (stall_tx, mut stall_rx) = tokio::sync::mpsc::channel::<()>(1);
    let watchdog_cancel = CancellationToken::new();
    let watchdog_token = watchdog_cancel.clone();

    // Watchdog task: warn at 90s, trigger restart at 300s
    let _watchdog = tokio::spawn({
        let last_update_wd = iter_last_update_wd;
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let last = last_update_wd.load(Ordering::Relaxed);
                        let gap = now.saturating_sub(last);
                        if gap > STALL_RESTART_SECS {
                            tracing::error!(
                                gap_secs = gap,
                                "Telegram polling stall — triggering auto-restart"
                            );
                            let _ = stall_tx.send(()).await;
                            break;
                        } else if gap > STALL_WARN_SECS {
                            tracing::warn!(
                                "Telegram polling stall detected ({}s since last update)",
                                gap
                            );
                        }
                    }
                    _ = watchdog_token.cancelled() => break,
                }
            }
        }
    });

    // Main select: dispatch, shutdown, or stall restart
    let which = tokio::select! {
        _ = dispatcher.dispatch() => "stopped",
        _ = &mut shutdown_rx => "shutdown",
        _ = stall_rx.recv() => "stall",
    };

    // Cancel watchdog before next iteration or exit
    watchdog_cancel.cancel();

    if which == "shutdown" {
        tracing::info!("Telegram channel shutdown requested");
        break;
    }

    // Restart path
    *status.write().await = ChannelStatus::Connecting;
    tracing::error!(
        attempt = attempt,
        reason = which,
        "Telegram polling {} — auto-restarting",
        which
    );

    // Reset attempt if healthy for >5 min
    if healthy_since.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(300)) {
        attempt = 1;
    }
    let delay = std::cmp::min(5 * 2u64.pow(attempt.saturating_sub(1).min(4)), 60);
    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

    // Reset stall timestamp and healthy tracker
    last_update_at.store(
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        Ordering::Relaxed,
    );
    healthy_since = Some(Instant::now());

    tracing::info!(
        attempt = attempt,
        "Telegram reconnected, queued messages will be delivered"
    );
    *status.write().await = ChannelStatus::Connected;
} // end retry loop

*status.write().await = ChannelStatus::Disconnected;
```

**Important notes for implementation:**
- The `inbound_tx` and `inbound_tx_for_cb` were originally created at lines 535-536 as `self.channel_state.sender()` clones. Inside the retry loop, clone from these outer variables each iteration.
- `bot` needs `.clone()` inside the loop since `Dispatcher::builder` takes ownership — `Bot` is `Clone`.
- The `shutdown_rx` is `&mut` borrowed in `select!` so it survives across loop iterations.
- Remove the `.enable_ctrlc_handler()` call that was at line 740.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: no errors (warnings OK)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add polling stall auto-restart with exponential backoff"
```

---

### Task 2: Remove Self-Management from OperationalGuidelinesLayer

**Files:**
- Modify: `src/thinker/layers/operational_guidelines.rs:46-53`

- [ ] **Step 1: Remove the Self-Management section**

In `src/thinker/layers/operational_guidelines.rs`, delete lines 46-53 (the `### Self-Management` section):

```rust
// DELETE these lines:
output.push_str("### Self-Management\n");
output.push_str("You can manage all Aleph configuration. When needed, call read_config_guide(topic) ");
output.push_str("to get the configuration manual for the relevant domain, then use file read/write ");
output.push_str("tools to make changes.\n");
output.push_str("- Always backup config files before modification (cp file file.bak)\n");
output.push_str("- Show planned changes to the user and confirm before writing\n");
output.push_str("- After writing, read the file back to verify the format is valid\n");
output.push_str("- API keys must be stored via vault_store tool, never written to config files\n\n");
```

Keep everything else (Diagnostic Capabilities, When You Detect Issues, What You Must NEVER Do).

- [ ] **Step 2: Run existing tests**

Run: `cargo test -p alephcore --lib operational_guidelines`
Expected: PASS (the existing tests don't assert on Self-Management content)

- [ ] **Step 3: Commit**

```bash
git add src/thinker/layers/operational_guidelines.rs
git commit -m "thinker: remove self-management from OperationalGuidelinesLayer (moved to /self skill)"
```

---

### Task 3: Create `/self` Skill in Official Skills Repo

**Files:**
- Create: `/Users/zouguojun/Workspace/Aleph-skills/self/SKILL.md`

- [ ] **Step 1: Create the SKILL.md file**

Create `/Users/zouguojun/Workspace/Aleph-skills/self/SKILL.md` with the full prompt content from the spec (section 2c). The file starts with YAML frontmatter:

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

Followed by the full prompt body from spec lines 157-279.

- [ ] **Step 2: Verify SKILL.md parses correctly**

Verify the YAML frontmatter matches existing skill format by comparing with `~/.aleph/skills/git/SKILL.md` or similar. Run the skill manifest parser tests:
```bash
cargo test -p alephcore --lib skill -- --nocapture 2>&1 | tail -10
```
Expected: existing manifest parsing tests pass.

- [ ] **Step 3: Commit and push to Aleph-skills repo**

```bash
cd ~/Workspace/Aleph-skills
git add self/SKILL.md
git commit -m "feat: add /self skill for self-management mode"
git push
```

---

### Task 4: Skills Directory Separation (Discovery + Priority)

**Files:**
- Modify: `src/skill/mod.rs:296-320` (`guess_source()` function)
- Modify: `src/extension/mod.rs:270-276`

The key insight: `SkillRegistry::register()` keeps the **first** entry at equal priority (`existing.priority() >= manifest.priority() => reject`). So we need official skills to have **lower** priority than user skills. The cleanest approach: make `guess_source()` return `SkillSource::Bundled` (priority 1) for `skills-official/` paths, vs `SkillSource::Global` (priority 2) for `skills/`. This way user skills always override official ones.

- [ ] **Step 1: Update `guess_source()` to recognize `skills-official/`**

In `src/skill/mod.rs`, modify the `guess_source()` function (lines 301-320):

```rust
fn guess_source(path: &Path) -> SkillSource {
    let path_str = path.to_string_lossy();

    // Official skills directory → Bundled (priority 1, overridable by Global/Workspace)
    if path_str.contains("skills-official") {
        return SkillSource::Bundled;
    }

    if path_str.contains(".aleph/skills") {
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph").join("skills");
            if path.starts_with(&home_skills) {
                return SkillSource::Global;
            }
        } else {
            tracing::warn!("dirs::home_dir() returned None, defaulting to Global source");
            return SkillSource::Global;
        }
        return SkillSource::Workspace;
    }

    SkillSource::Bundled
}
```

- [ ] **Step 2: Add `skills-official/` to the SkillSystem scan dirs**

In `src/extension/mod.rs`, modify lines 270-276:

```rust
// Before:
let skill_dirs: Vec<PathBuf> = self.discovery.discover_skill_dirs()
    .unwrap_or_default()
    .into_iter()
    .map(|d| d.path)
    .collect();

// After:
let mut skill_dirs: Vec<PathBuf> = Vec::new();
// Official skills dir (SkillSource::Bundled, priority 1)
let official_dir = dirs::home_dir()
    .unwrap_or_else(|| PathBuf::from("/tmp"))
    .join(".aleph")
    .join("skills-official");
if official_dir.exists() {
    skill_dirs.push(official_dir);
}
// User skills dirs (SkillSource::Global, priority 2 — overrides official)
for d in self.discovery.discover_skill_dirs().unwrap_or_default() {
    skill_dirs.push(d.path);
}
```

No need for `discover_skill_official_dirs()` — we just hardcode the known path and let `guess_source()` handle the priority.

- [ ] **Step 3: Update test for `guess_source`**

In `src/skill/mod.rs`, update the test section:

```rust
#[test]
fn guess_source_official() {
    let path = PathBuf::from("/Users/test/.aleph/skills-official/self/SKILL.md");
    assert_eq!(guess_source(&path), SkillSource::Bundled);
}
```

- [ ] **Step 4: Compile and test**

Run: `cargo check -p alephcore 2>&1 | head -30`
Run: `cargo test -p alephcore --lib skill::mod -- guess_source`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/skill/mod.rs src/extension/mod.rs
git commit -m "skills: add skills-official/ directory with Bundled priority"
```

---

### Task 5: Official Skills Auto-Update on Startup

**Files:**
- Create: `src/skills/updater.rs`
- Modify: `src/skills/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/mod.rs`

- [ ] **Step 1: Create `src/skills/updater.rs`**

```rust
//! Official skills repository auto-updater.
//!
//! Runs on server startup to keep ~/.aleph/skills-official/ in sync
//! with the official GitHub repository via git pull --ff-only.

use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

const DEFAULT_REPO_URL: &str = "https://github.com/rootazero/Aleph-skills.git";
const GIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Update the official skills repo via git.
///
/// - If the directory doesn't exist or has no `.git`: clone with `--depth 1`
/// - If `.git` exists: `git pull --ff-only`, fallback to `fetch + reset --hard`
/// - Errors are logged but never propagated (non-fatal)
pub async fn update_official_skills(skills_official_dir: &Path) {
    let dir = skills_official_dir;

    if !dir.join(".git").exists() {
        // First install: clone
        info!("Cloning official skills repository...");
        let result = tokio::time::timeout(
            GIT_TIMEOUT * 2, // Allow more time for initial clone
            tokio::process::Command::new("git")
                .args(["clone", "--depth", "1", DEFAULT_REPO_URL])
                .arg(dir)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {
                info!("Official skills cloned successfully");
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Official skills clone failed (non-fatal): {}", stderr.trim());
            }
            Ok(Err(e)) => {
                warn!("Official skills clone failed (non-fatal): {}", e);
            }
            Err(_) => {
                warn!("Official skills clone timed out (non-fatal)");
            }
        }
        return;
    }

    // Existing repo: fast-forward pull
    let result = tokio::time::timeout(
        GIT_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(dir)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("Already up to date") {
                // Silent — no log spam on every startup
            } else {
                info!("Official skills updated");
            }
        }
        Ok(Ok(_)) => {
            // ff-only failed — force reset to origin (safe: skills-official is read-only)
            warn!("Official skills ff-only pull failed, resetting to origin/main");
            let _ = tokio::process::Command::new("git")
                .args(["fetch", "origin"])
                .current_dir(dir)
                .output()
                .await;
            let _ = tokio::process::Command::new("git")
                .args(["reset", "--hard", "origin/main"])
                .current_dir(dir)
                .output()
                .await;
            info!("Official skills force-reset to origin/main");
        }
        Ok(Err(e)) => {
            warn!("Official skills update failed (non-fatal): {}", e);
        }
        Err(_) => {
            warn!("Official skills update timed out (non-fatal)");
        }
    }
}
```

- [ ] **Step 2: Add migration function in the same file**

Below `update_official_skills()`, add:

```rust
/// Migrate from single ~/.aleph/skills/ (git clone) to split layout.
///
/// If ~/.aleph/skills/.git exists with the official remote:
/// 1. Move ~/.aleph/skills/ → ~/.aleph/skills-official/
/// 2. Create new empty ~/.aleph/skills/
/// 3. Move any non-git-tracked files (user skills) to new ~/.aleph/skills/
pub async fn migrate_skills_directory(aleph_home: &Path) {
    let skills_dir = aleph_home.join("skills");
    let official_dir = aleph_home.join("skills-official");

    // Skip if already migrated or skills-official already exists
    if official_dir.exists() {
        return;
    }

    // Check if skills/ is a git repo with the official remote
    let git_dir = skills_dir.join(".git");
    if !git_dir.exists() {
        return;
    }

    let remote_check = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&skills_dir)
        .output()
        .await;

    let is_official = match remote_check {
        Ok(output) if output.status.success() => {
            let url = String::from_utf8_lossy(&output.stdout);
            url.trim().contains("Aleph-skills")
        }
        _ => false,
    };

    if !is_official {
        return;
    }

    info!("Migrating skills directory to split layout...");

    // Find user-added files (not tracked by git)
    let untracked = tokio::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(&skills_dir)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let user_files: Vec<&str> = untracked.lines()
        .filter(|l| !l.is_empty())
        .collect();

    // Move skills/ → skills-official/
    if let Err(e) = std::fs::rename(&skills_dir, &official_dir) {
        warn!("Failed to migrate skills directory: {}", e);
        return;
    }

    // Create new empty skills/
    if let Err(e) = std::fs::create_dir_all(&skills_dir) {
        warn!("Failed to create user skills directory: {}", e);
        return;
    }

    // Move user files back to skills/
    for file in user_files {
        let src = official_dir.join(file);
        let dst = skills_dir.join(file);
        if src.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::rename(&src, &dst) {
                warn!("Failed to move user file {}: {}", file, e);
            }
        }
    }

    info!("Skills directory migration complete");
}
```

- [ ] **Step 3: Register module in `src/skills/mod.rs`**

Add near the top of `src/skills/mod.rs`:

```rust
pub mod updater;
```

- [ ] **Step 4: Call migration + updater on startup**

In `src/bin/aleph-server/commands/start/mod.rs`, in the `initialize_extension_manager` function body, **before** the `alephcore::extension::ExtensionManager::with_defaults().await` call (line 174), add:

```rust
// Migrate old single-dir layout and update official skills
let aleph_home = dirs::home_dir()
    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
    .join(".aleph");
alephcore::skills::updater::migrate_skills_directory(&aleph_home).await;
alephcore::skills::updater::update_official_skills(&aleph_home.join("skills-official")).await;
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -30` and `cargo check --bin aleph-server 2>&1 | head -30`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/skills/updater.rs src/skills/mod.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "skills: add official skills repo auto-update on startup"
```

---

### Task 6: Update Config Guides

**Files:**
- Modify: `~/.aleph/guides/overview.md`
- Modify: `~/.aleph/guides/generation.md`

- [ ] **Step 1: Update overview.md**

Add the full workspace directory tree to `~/.aleph/guides/overview.md`, including the new `skills-official/` directory. Update the File Map table to include:

| File | Format | Hot-reload | Description |
|------|--------|------------|-------------|
| `~/.aleph/skills-official/` | Directories | On next skill discovery | Official skills (git-managed, auto-updated) |
| `~/.aleph/skills/` | Directories | On next skill discovery | User custom skills |
| `~/.aleph/plugins/installed/` | Directories | On load | Installed plugins |
| `~/.aleph/workspaces/{id}/` | Directories | N/A | Agent workspaces (file output) |
| `~/.aleph/backups/` | Files | N/A | Config backups (timestamped) |

- [ ] **Step 2: Update generation.md**

Add to `~/.aleph/guides/generation.md`:

1. A new "## URL Rules" section explaining standard vs full URL behavior
2. Video provider example (`t8star-video` with `type = "video"`)
3. Audio provider example (`suno` with `type = "audio"`)
4. "Add video provider" and "Add audio provider" operation sections

Content from spec section 2f.

- [ ] **Step 3: Commit (in Aleph main repo)**

```bash
git add -f docs/superpowers/plans/2026-03-24-self-management-telegram-resilience.md
git commit -m "docs: update config guides with skills-official dir and video/audio providers"
```

Note: The guide files in `~/.aleph/guides/` are outside the git repo. They need to be updated on the running system directly, or committed to a mechanism that deploys them (check if they're generated from the repo or manually managed).

---

## Execution Order

Tasks 1-3 are fully independent and can run in parallel.
Task 4 depends on nothing but should run before Task 5.
Task 5 depends on Task 4 (discovery must know about skills-official before startup calls update).
Task 6 is independent and can run anytime.

```
Task 1 (Telegram) ────────┐
Task 2 (Remove prompt) ───┤
Task 3 (/self skill) ─────┤── all independent
Task 4 (Discovery) ───────┤
                           ↓
Task 5 (Auto-update) ─── depends on Task 4
Task 6 (Guides) ────────── independent
```
