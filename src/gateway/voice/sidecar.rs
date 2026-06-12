//! aleph-voice sidecar supervisor: lazy spawn, READY handshake, crash-loop guard.
//!
//! Process-global singleton (OnceLock) mirroring the SwiftBridge precedent —
//! STT (media processor / inbound router) and TTS (reply emitter) paths share
//! one child process. No eager start: first voice demand spawns it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use crate::config::types::voice_local::VoiceLocalConfig;
use crate::sync_primitives::Arc;

/// Resolved connection info for one sidecar incarnation.
#[derive(Debug, Clone)]
pub struct SidecarEndpoint {
    /// e.g. "http://127.0.0.1:54321/v1" — joins the existing OpenAI-compat
    /// client code, which appends "/audio/transcriptions" etc.
    pub base_url: String,
    /// Per-spawn bearer token (used as the provider api_key).
    pub token: String,
}

/// Remote model state subset we care about (mirrors the sidecar status DTO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteModelState {
    Ready,
    Downloading { percent: u8 },
    Other(String),
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const CRASH_LIMIT: usize = 3;
const COOLDOWN: Duration = Duration::from_secs(300);

/// Pure decision: are we in a crash loop? (>= CRASH_LIMIT crashes inside CRASH_WINDOW)
pub fn crash_loop_active(crashes: &VecDeque<Instant>, now: Instant) -> bool {
    crashes
        .iter()
        .filter(|t| now.duration_since(**t) <= CRASH_WINDOW)
        .count()
        >= CRASH_LIMIT
}

struct Inner {
    child: Option<tokio::process::Child>,
    endpoint: Option<SidecarEndpoint>,
    crashes: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
}

pub struct VoiceSidecarSupervisor {
    cfg: VoiceLocalConfig,
    inner: tokio::sync::Mutex<Inner>,
    handshake_timeout: Duration,
}

static GLOBAL: std::sync::OnceLock<Arc<VoiceSidecarSupervisor>> = std::sync::OnceLock::new();

/// Install the global supervisor at boot (no-op if already installed).
pub fn init_global(cfg: VoiceLocalConfig) -> Arc<VoiceSidecarSupervisor> {
    GLOBAL.get_or_init(|| Arc::new(VoiceSidecarSupervisor::new(cfg))).clone()
}

/// The global supervisor, if local voice was enabled at boot.
pub fn global() -> Option<Arc<VoiceSidecarSupervisor>> {
    GLOBAL.get().cloned()
}

impl VoiceSidecarSupervisor {
    pub fn new(cfg: VoiceLocalConfig) -> Self {
        Self {
            cfg,
            inner: tokio::sync::Mutex::new(Inner {
                child: None,
                endpoint: None,
                crashes: VecDeque::new(),
                cooldown_until: None,
            }),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn with_handshake_timeout(mut self, t: Duration) -> Self {
        self.handshake_timeout = t;
        self
    }

    pub fn config(&self) -> &VoiceLocalConfig {
        &self.cfg
    }

    /// Endpoint if the sidecar is currently alive — never spawns.
    pub async fn peek_endpoint(&self) -> Option<SidecarEndpoint> {
        let mut inner = self.inner.lock().await;
        if Self::child_alive(&mut inner) {
            inner.endpoint.clone()
        } else {
            None
        }
    }

    /// Get a live endpoint, spawning the sidecar if needed.
    pub async fn ensure_endpoint(&self) -> anyhow::Result<SidecarEndpoint> {
        let mut inner = self.inner.lock().await;
        if Self::child_alive(&mut inner) {
            if let Some(ep) = inner.endpoint.clone() {
                return Ok(ep);
            }
        }
        let now = Instant::now();
        if let Some(until) = inner.cooldown_until {
            if now < until {
                bail!(
                    "local voice sidecar in crash-loop cooldown ({}s left)",
                    (until - now).as_secs()
                );
            }
            inner.cooldown_until = None;
        }
        if crash_loop_active(&inner.crashes, now) {
            inner.cooldown_until = Some(now + COOLDOWN);
            bail!("local voice sidecar crash loop detected — cooling down {}s", COOLDOWN.as_secs());
        }

        let bin = self.binary_path()?;
        tracing::info!(bin = %bin.display(), "spawning aleph-voice sidecar");
        let mut child = tokio::process::Command::new(&bin)
            .arg("--stt-model").arg(&self.cfg.stt_model)
            .arg("--tts-model").arg(&self.cfg.tts_model)
            .arg("--tts-voice").arg(&self.cfg.tts_voice)
            .arg("--idle-unload-stt-secs").arg(self.cfg.idle_unload_stt_secs.to_string())
            .arg("--idle-unload-tts-secs").arg(self.cfg.idle_unload_tts_secs.to_string())
            .arg("--idle-exit-secs").arg(self.cfg.idle_exit_secs.to_string())
            .arg("--download-source").arg(&self.cfg.download_source)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {}", bin.display()))?;

        let stdout = child.stdout.take().context("sidecar stdout unavailable")?;
        let mut reader = tokio::io::BufReader::new(stdout);
        // Bound the handshake read so a misbehaving binary cannot grow memory
        // unbounded: any single line is capped at 64KB and the cumulative
        // handshake output at 256KB — exceeding either is a handshake failure.
        const MAX_LINE_BYTES: usize = 64 * 1024;
        const MAX_HANDSHAKE_BYTES: usize = 256 * 1024;
        let ready = tokio::time::timeout(self.handshake_timeout, async {
            let mut total = 0usize;
            let mut buf: Vec<u8> = Vec::new();
            loop {
                buf.clear();
                let mut limited = (&mut reader).take((MAX_LINE_BYTES + 1) as u64);
                match limited.read_until(b'\n', &mut buf).await {
                    Ok(0) => return None, // EOF before READY
                    Ok(_) => {}
                    Err(_) => return None,
                }
                if !buf.ends_with(b"\n") && buf.len() > MAX_LINE_BYTES {
                    return None; // oversized line without newline
                }
                total += buf.len();
                if total > MAX_HANDSHAKE_BYTES {
                    return None; // too much pre-READY chatter
                }
                let line = String::from_utf8_lossy(&buf);
                if let Some(json) = line.trim_end().strip_prefix("READY ") {
                    return Some(json.to_string());
                }
            }
        })
        .await;

        let endpoint = match ready {
            Ok(Some(json)) => {
                #[derive(Deserialize)]
                struct Ready { port: u16, token: String }
                let r: Ready = serde_json::from_str(&json).context("parse READY line")?;
                SidecarEndpoint {
                    base_url: format!("http://127.0.0.1:{}/v1", r.port),
                    token: r.token,
                }
            }
            Ok(None) | Err(_) => {
                let _ = child.start_kill();
                // Reap the killed child off-task — dropping a Child after
                // start_kill never waits on it, leaving a zombie per failed
                // handshake (these accumulate in crash loops).
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                inner.crashes.push_back(now);
                while inner.crashes.len() > 8 {
                    inner.crashes.pop_front();
                }
                bail!("sidecar did not print READY within {:?}", self.handshake_timeout);
            }
        };

        // Drain remaining stdout so the pipe never blocks the child.
        // Post-handshake stdout is contractually empty; the drain only
        // discards, so an unbounded line reader is fine here.
        tokio::spawn(async move {
            let mut lines = reader.lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });

        inner.child = Some(child);
        inner.endpoint = Some(endpoint.clone());
        Ok(endpoint)
    }

    /// try_wait-based liveness; records non-zero exits as crashes.
    fn child_alive(inner: &mut Inner) -> bool {
        let Some(child) = inner.child.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    inner.crashes.push_back(Instant::now());
                    while inner.crashes.len() > 8 {
                        inner.crashes.pop_front();
                    }
                    tracing::warn!(%status, "aleph-voice exited abnormally");
                } else {
                    tracing::info!("aleph-voice exited cleanly (deep idle)");
                }
                inner.child = None;
                inner.endpoint = None;
                false
            }
            Err(_) => false,
        }
    }

    fn binary_path(&self) -> anyhow::Result<std::path::PathBuf> {
        if let Some(ref p) = self.cfg.binary_path {
            if p.exists() {
                return Ok(p.clone());
            }
            bail!("voice.local.binary_path does not exist: {}", p.display());
        }
        let exe = std::env::current_exe().context("current_exe")?;
        let dir = exe.parent().context("exe parent dir")?;
        let candidate = dir.join(format!("aleph-voice{}", std::env::consts::EXE_SUFFIX));
        if candidate.exists() {
            return Ok(candidate);
        }
        bail!(
            "aleph-voice binary not found next to aleph-server ({}); set voice.local.binary_path",
            candidate.display()
        )
    }

    /// Fire warmup: ensure running + POST /voice/warmup.
    pub async fn warmup(&self) -> anyhow::Result<()> {
        let ep = self.ensure_endpoint().await?;
        let client = reqwest::Client::new();
        client
            .post(format!("{}/voice/warmup", ep.base_url))
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(5))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// TTS model state via /voice/status (preflight for the downloading case).
    pub async fn tts_model_state(&self) -> anyhow::Result<RemoteModelState> {
        let ep = self.ensure_endpoint().await?;
        let client = reqwest::Client::new();
        let v: serde_json::Value = client
            .get(format!("{}/voice/status", ep.base_url))
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_model_state(&v["tts"]["model_state"]))
    }

    /// STT model state via /voice/status (downloading guard for inbound path).
    pub async fn stt_model_state(&self) -> anyhow::Result<RemoteModelState> {
        let ep = self.ensure_endpoint().await?;
        let client = reqwest::Client::new();
        let v: serde_json::Value = client
            .get(format!("{}/voice/status", ep.base_url))
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_model_state(&v["stt"]["model_state"]))
    }
}

/// Parse the sidecar's tagged ModelState JSON into our subset.
pub fn parse_model_state(v: &serde_json::Value) -> RemoteModelState {
    match v["state"].as_str() {
        Some("ready") => RemoteModelState::Ready,
        Some("downloading") => RemoteModelState::Downloading {
            percent: v["percent"].as_u64().unwrap_or(0).min(100) as u8,
        },
        Some(other) => RemoteModelState::Other(other.to_string()),
        None => RemoteModelState::Other("unknown".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fake_sidecar_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-voice.sh");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\n{body}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (dir, path)
    }

    fn cfg_with_bin(path: std::path::PathBuf) -> VoiceLocalConfig {
        VoiceLocalConfig { enabled: true, binary_path: Some(path), ..Default::default() }
    }

    #[test]
    fn crash_loop_decision() {
        let now = Instant::now();
        let recent: VecDeque<Instant> = (0..3).map(|_| now).collect();
        assert!(crash_loop_active(&recent, now));
        let stale: VecDeque<Instant> = (0..3).map(|_| now - Duration::from_secs(120)).collect();
        assert!(!crash_loop_active(&stale, now));
        let two: VecDeque<Instant> = (0..2).map(|_| now).collect();
        assert!(!crash_loop_active(&two, now));
    }

    #[test]
    fn parses_model_states() {
        let v: serde_json::Value = serde_json::json!({"state": "downloading", "percent": 42});
        assert_eq!(parse_model_state(&v), RemoteModelState::Downloading { percent: 42 });
        let v: serde_json::Value = serde_json::json!({"state": "ready"});
        assert_eq!(parse_model_state(&v), RemoteModelState::Ready);
        // Out-of-range percent clamps instead of wrapping through `as u8`.
        let v: serde_json::Value = serde_json::json!({"state": "downloading", "percent": 300});
        assert_eq!(parse_model_state(&v), RemoteModelState::Downloading { percent: 100 });
    }

    #[tokio::test]
    async fn handshake_parses_ready_and_reuses_endpoint() {
        let (_d, path) =
            fake_sidecar_script(r#"echo 'READY {"v":1,"port":59999,"token":"tok123"}'; sleep 30"#);
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path));
        let ep = sup.ensure_endpoint().await.unwrap();
        assert_eq!(ep.base_url, "http://127.0.0.1:59999/v1");
        assert_eq!(ep.token, "tok123");
        // Second call reuses the live child (no respawn → same endpoint).
        let ep2 = sup.ensure_endpoint().await.unwrap();
        assert_eq!(ep2.token, "tok123");
        assert!(sup.peek_endpoint().await.is_some());
    }

    #[tokio::test]
    async fn no_ready_line_times_out_and_records_crash() {
        let (_d, path) = fake_sidecar_script("sleep 30");
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path))
            .with_handshake_timeout(Duration::from_millis(200));
        let err = sup.ensure_endpoint().await.unwrap_err();
        assert!(format!("{err:#}").contains("READY"));
        assert!(sup.peek_endpoint().await.is_none());
    }

    #[tokio::test]
    async fn oversized_handshake_output_fails_fast() {
        // 100KB of garbage with no newline — the bounded reader must fail the
        // handshake immediately instead of buffering it or waiting out the
        // full timeout.
        let (_d, path) = fake_sidecar_script("head -c 102400 /dev/zero | tr '\\0' A; sleep 30");
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path))
            .with_handshake_timeout(Duration::from_secs(10));
        let start = Instant::now();
        let err = sup.ensure_endpoint().await.unwrap_err();
        assert!(format!("{err:#}").contains("READY"));
        assert!(start.elapsed() < Duration::from_secs(5), "should fail before the 10s timeout");
        assert!(sup.peek_endpoint().await.is_none());
    }

    #[tokio::test]
    async fn crash_loop_triggers_cooldown() {
        let (_d, path) = fake_sidecar_script("exit 1");
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path))
            .with_handshake_timeout(Duration::from_millis(150));
        for _ in 0..3 {
            let _ = sup.ensure_endpoint().await;
        }
        let err = sup.ensure_endpoint().await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("crash loop") || msg.contains("cooldown"), "got: {msg}");
    }

    #[tokio::test]
    async fn missing_binary_yields_actionable_error() {
        let cfg = VoiceLocalConfig {
            enabled: true,
            binary_path: Some("/nonexistent/aleph-voice".into()),
            ..Default::default()
        };
        let err = VoiceSidecarSupervisor::new(cfg).ensure_endpoint().await.unwrap_err();
        assert!(format!("{err:#}").contains("binary_path"));
    }
}
