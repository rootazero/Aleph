//! Debug-only observation bridge. Canonical UI and transport remain in the Panel.
use std::io::Write;
use std::sync::Mutex;
use tauri::Manager;

#[path = "../../../qa/phase9_clock.rs"]
mod clock;

pub(crate) fn start_clock() {
    clock::start();
}

static LAST_EVENT: Mutex<Option<serde_json::Value>> = Mutex::new(None);
static BACKGROUND: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static DRAINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn own_background(worker: std::thread::JoinHandle<()>) {
    *BACKGROUND
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(worker);
}

fn nonce() -> Option<String> {
    let root = crate::qa_root::try_root()?;
    let value = std::fs::read_to_string(root.join(".qa-owned")).ok()?;
    let value = value.strip_suffix('\n')?;
    (value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| value.to_owned())
}

fn boot_diagnostic() {
    let target = crate::connection::load_target();
    let run = std::env::var("ALEPH_PANEL_QA_RUN_ID").ok();
    let script_ready = run.is_some() && nonce().is_some();
    let line = format!(
        "ALEPH_PANEL_QA_BOOT target={target:?} root={} run_id={run:?} script_ready={script_ready}\n",
        crate::qa_root::root().display()
    );
    let mut paths = vec![crate::qa_root::root().join(".phase9-boot")];
    if let Ok(path) = std::env::var("ALEPH_PANEL_QA_CAPTURE") {
        paths.push(std::path::PathBuf::from(path));
    }
    for path in paths {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

pub(crate) fn install(app: &tauri::AppHandle) -> tauri::Result<()> {
    boot_diagnostic();
    let crate::connection::ConnectionTarget::Remote(url) = crate::connection::load_target() else {
        return Ok(());
    };
    let capability = tauri::ipc::CapabilityBuilder::new("phase9-observation")
        .remote(format!("{}/*", url.origin().ascii_serialization()))
        .window("main")
        .local(false)
        .permission("allow-phase9-observation");
    app.add_capability(capability)
}

pub(crate) fn script() -> Option<String> {
    let run = std::env::var("ALEPH_PANEL_QA_RUN_ID").ok()?;
    let nonce = nonce()?;
    let message = std::env::var("ALEPH_PANEL_QA_MESSAGE").unwrap_or_default();
    Some(format!(
        "{}\nlet phase9Delivery = Promise.resolve(); installAlephPanelQa({}, event => {{ phase9Delivery = phase9Delivery.then(async () => {{ const deadline = Date.now() + 10000; while (!window.__TAURI_INTERNALS__?.invoke) {{ if (Date.now() > deadline) throw new Error('QA IPC initialization timeout'); await new Promise(resolve => setTimeout(resolve, 5)); }} return window.__TAURI_INTERNALS__.invoke('phase9_observation', {{event}}); }}).catch(error => {{ console.error('Phase9 QA delivery', error); document.title = 'PHASE9_QA_ERROR ' + String(error); }}); }});",
        include_str!("../../../qa/nsh_panel_driver.js"),
        serde_json::json!({"nonce": nonce, "qa_run_id": run, "message": message})
    ))
}

fn emit(event: &serde_json::Value) {
    let mut event = event.clone();
    clock::annotate(&mut event);
    let line = format!("ALEPH_PANEL_QA {event}\n");
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    if let Ok(path) = std::env::var("ALEPH_PANEL_QA_CAPTURE") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

#[tauri::command]
pub(crate) async fn phase9_observation(
    window: tauri::WebviewWindow,
    event: serde_json::Value,
) -> Result<(), String> {
    let url = window.url().map_err(|_| "QA origin unavailable")?;
    if window.label() != "main" || !crate::connection::load_target().serves_origin(&url) {
        return Err("QA origin denied".into());
    }
    let nonce = nonce().ok_or("QA root unavailable")?;
    let run = std::env::var("ALEPH_PANEL_QA_RUN_ID").map_err(|_| "QA run unavailable")?;
    if event["nonce"].as_str() != Some(nonce.as_str())
        || event["qa_run_id"].as_str() != Some(run.as_str())
    {
        return Err("QA observation identity denied".into());
    }
    if event.to_string().len() > 16 * 1024 {
        return Err("QA observation exceeds limit".into());
    }
    emit(&event);
    let close = event["kind"].as_str() == Some("close-requested");
    *LAST_EVENT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(event);
    if close {
        // QA lite starts no resident workers after bring_target_online returns.
        let worker = BACKGROUND
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        let joined = worker.is_some_and(|worker| worker.join().is_ok());
        DRAINED.store(joined, std::sync::atomic::Ordering::Release);
        window.app_handle().exit(if joined { 0 } else { 1 });
    }
    Ok(())
}

pub(crate) fn exited() {
    let Some(mut event) = LAST_EVENT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
    else {
        return;
    };
    event["kind"] = serde_json::json!("closed");
    event["payload"] =
        serde_json::json!({"owned_drained": DRAINED.load(std::sync::atomic::Ordering::Acquire)});
    emit(&event);
}
