#!/usr/bin/env python3
"""Run the TSX or Classic browser Voice fixture against a supplied real Gateway."""
import argparse
import base64
from collections import Counter
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import wave

# Local helper imports must not write caches into the checkout before scratch setup.
sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE.parent))
from fixture import Provider, RATE, write_clip

SPEC = importlib.util.spec_from_file_location("nsh_s2_fixture", HERE.parent / "nsh_s2_shell" / "run.py")
s2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(s2)

CASES = ("success", "permission", "config-only", "limit", "cancel-stt", "cancel-chat", "disconnect")
EXPECTED = {
    "permission": {}, "config-only": {}, "cancel-stt": {"stt": 1},
    "disconnect": {"stt": 1}, "cancel-chat": {"stt": 1, "chat": 1},
    "limit": {"stt": 1, "chat": 1, "tts": 1},
}


def environment(root):
    home, aleph = root / "home", root / "aleph"
    home.mkdir(mode=0o700)
    aleph.mkdir(mode=0o700)
    allow = ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "LANG", "LC_ALL", "LC_CTYPE")
    return {k: os.environ[k] for k in allow if k in os.environ} | {
        "HOME": str(home), "USERPROFILE": str(home), "ALEPH_HOME": str(aleph),
        "APPDATA": str(home), "LOCALAPPDATA": str(home), "XDG_CONFIG_HOME": str(home),
        "XDG_DATA_HOME": str(home), "XDG_CACHE_HOME": str(home), "XDG_RUNTIME_DIR": str(root),
        "TMPDIR": str(root), "TEMP": str(root), "TMP": str(root),
        "NO_PROXY": "127.0.0.1,localhost", "RUST_LOG": "error",
    }


def write_config(aleph, port, panel, provider, configured_stt):
    s2.write_config(aleph, port, panel)
    base = f"http://127.0.0.1:{provider.server_port}"
    stt = 'default_transcription_provider = "qa-stt"\n' if configured_stt else ""
    with (aleph / "config.toml").open("a", encoding="utf-8") as config:
        config.write(f'''\n[general]
default_provider = "phase9-qa"
[providers.phase9-qa]
enabled = true
verified = true
protocol = "anthropic"
base_url = "{base}"
api_key = "{provider.token}"
models = ["phase9-qa"]
timeout_seconds = 20
[generation]
default_speech_provider = "qa-tts"
{stt}[generation.speech_providers.qa-tts]
provider_type = "openai_tts"
enabled = true
base_url = "{base}"
api_key = "{provider.token}"
models = ["tts-1"]
[voice.streaming]
enabled = false
''')
        if configured_stt:
            config.write(f'''[generation.transcription_providers.qa-stt]
provider_type = "openai_whisper"
enabled = true
base_url = "{base}"
api_key = "{provider.token}"
models = ["whisper-1"]
''')


def json_write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def browser_cache():
    # Resolve the installed engine before replacing HOME; pass no other Playwright settings.
    if configured := os.environ.get("PLAYWRIGHT_BROWSERS_PATH"):
        if configured != "0":
            return str(Path(configured).resolve())
        return configured
    if sys.platform == "darwin":
        base = Path.home() / "Library/Caches"
    elif sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData/Local"))
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return str(base / "ms-playwright")


def browser_run(argv, root, env):
    with (root / "browser.log").open("wb") as log:
        child = subprocess.Popen(argv, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
                                 start_new_session=os.name != "nt")
        try:
            return child.wait(timeout=240)
        finally:
            if child.poll() is None:
                if os.name == "nt":
                    subprocess.run([str(Path(env["SystemRoot"]) / "System32/taskkill.exe"), "/PID",
                                    str(child.pid), "/T", "/F"], env=env, timeout=10, capture_output=True)
                    if child.poll() is None:
                        child.kill()
                else:
                    os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)


def execute(args, root, report):
    env = environment(root)
    aleph = Path(env["ALEPH_HOME"])
    clip, audio = root / "input.wav", root / "reply.mp3"
    write_clip(clip, args.clip)
    with wave.open(str(clip)) as source:
        samples = source.getnframes()
    subprocess.run([args.ffmpeg, "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                    "sine=frequency=440:sample_rate=16000:duration=2", "-c:a", "libmp3lame",
                    "-map_metadata", "-1", str(audio)], cwd=root, env=env,
                   check=True, capture_output=True, timeout=15)
    report["media"] = {"profile": args.clip, "input_sha256": s2.sha256(clip), "input_samples": samples,
                       "sample_rate_hz": RATE, "input_bytes": clip.stat().st_size,
                       "synthetic_reply_sha256": s2.sha256(audio), "synthetic_reply_bytes": audio.stat().st_size}
    delayed = {"cancel-stt": "stt", "disconnect": "stt", "cancel-chat": "chat"}.get(args.case)
    reply_audio = audio.read_bytes()
    reply_fingerprint = hashlib.sha256(base64.b64encode(reply_audio)).hexdigest()
    with Provider(reply_audio, delayed, root) as provider, (root / "gateway.log").open("wb") as log:
        if provider.server_port == 18790:
            raise RuntimeError("fixture claimed the default port")
        port = s2.fixture_port()  # Provider already bound, so the OS cannot select its port.
        write_config(aleph, port, args.panel_web, provider, args.case != "config-only")
        thread = threading.Thread(target=provider.serve_forever, daemon=True)
        thread.start()
        try:
            gateway = s2.Gateway(args.server, root, env, port, log)
            try:
                report["stage"] = "start_gateway"
                gateway.start()
                cert = aleph / "data/tls/cert.pem"
                deadline = time.monotonic() + 10
                while not cert.is_file() and gateway.child.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.1)
                s2.require(gateway.child.poll() is None and cert.is_file(), "owned TLS certificate unavailable")
                ticket = None
                if args.mount == "tsx":
                    report["stage"] = "mint_pairing_ticket"
                    ticket = s2.pairing_ticket(args.server, root, env)
                report["stage"] = "browser"
                request, observation = root / "browser.json", root / "observation.json"
                json_write(request, {"origin": f"https://127.0.0.1:{port}", "certificate": cert.read_text(encoding="utf-8"),
                                     "ticket": ticket, "clip": str(clip), "chrome": str(args.chrome) if args.chrome else None,
                                     "mount": args.mount, "reply_fingerprint": reply_fingerprint,
                                     "locale": args.locale, "case": args.case, "warm": args.warm,
                                     "provider_inflight": str(root / f"in-flight-{delayed}") if delayed else None,
                                     "observation": str(observation)})
                code = browser_run([args.node, str(HERE / "web.cjs"), str(request)], root,
                                   env | {"NODE_PATH": str(args.node_modules), "PLAYWRIGHT_BROWSERS_PATH": args.browser_cache})
                if observation.is_file():
                    report["browser"] = json.loads(observation.read_text(encoding="utf-8"))
                s2.require(code == 0 and report.get("browser", {}).get("passed") is True, "browser stage failed")
            finally:
                if gateway.child is not None:
                    report["gateway_alive_before_shutdown"] = gateway.child.poll() is None
                    report["gateway_exit"] = gateway.stop()
        finally:
            provider.done.set()
            provider.shutdown()
            thread.join(timeout=5)
            with provider.lock:
                report["provider_attempts"] = [row.copy() for row in provider.attempts]
        report["stage"] = "check_gateway_shutdown"
        s2.require(report["gateway_alive_before_shutdown"] and report["gateway_exit"] != "killed" and
                   (os.name == "nt" or report["gateway_exit"] == 0), "Gateway did not stop as expected")
        report["stage"] = "check_provider_attempts"
        actual = Counter(r["kind"] for r in report["provider_attempts"])
        turns = 1 + args.warm
        expected = ({"stt": turns, "chat": turns + (args.mount == "tsx"), "tts": turns}
                    if args.case == "success" else EXPECTED[args.case].copy())
        # The browser checks exact Chat sends and terminals. Core can make
        # auxiliary model calls on that same route; count them without assuming
        # each accepted Chat run corresponds to exactly one HTTP request.
        minimum_chat = expected.pop("chat", 0)
        actual_chat = actual.pop("chat", 0)
        report["expected_provider_attempts"] = {"exact": expected, "minimum_chat": minimum_chat}
        chat_matches = actual_chat >= minimum_chat if minimum_chat else actual_chat == 0
        s2.require(actual == expected and chat_matches and not provider.overflow, "provider attempts differed")
        s2.require(all(row["kind"] == delayed or row["response_body_bytes_written"] > 0
                       for row in report["provider_attempts"]), "fixture response not written")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True, type=Path)
    parser.add_argument("--panel-web", required=True, type=Path)
    parser.add_argument("--nsh", type=Path, help="hash a supplied native asset; automated browser phase does not launch it")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--node-modules", type=Path, default=REPO / "node_modules")
    parser.add_argument("--chrome", type=Path, default=os.environ.get("ALEPH_BROWSER_LIVE_CHROME_PATH"),
                        help="Chromium or Chrome executable (default: Playwright-installed Chromium)")
    parser.add_argument("--node", default="node")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mount", choices=("tsx", "classic"), default="tsx",
                        help="pair the TSX Web v1 panel or use the supplied server's embedded Classic Panel on loopback")
    parser.add_argument("--case", choices=CASES, default="success")
    parser.add_argument("--clip", choices=("short", "long", "limit"), default="short")
    parser.add_argument("--warm", type=int, choices=range(6), default=0, help="repeat successful turns in the same browser (0–5)")
    parser.add_argument("--locale", default="en-US")
    args = parser.parse_args()
    args.browser_cache = browser_cache()
    if sys.platform == "win32" and sys.version_info < (3, 13):
        parser.error("Windows requires Python 3.13+ for private directory ACLs")
    if args.mount == "classic" and (args.case != "success" or args.locale != "en-US"):
        parser.error("Classic comparison requires --case success and --locale en-US")
    if (args.warm and args.case != "success") or (args.case == "limit") != (args.clip == "limit"):
        parser.error("--warm is for success only; --case limit requires --clip limit and vice versa")
    for name in ("server", "panel_web", "nsh", "output", "node_modules", "chrome"):
        if getattr(args, name) is not None:
            setattr(args, name, getattr(args, name).resolve())
    for name in ("node", "ffmpeg"):
        executable = shutil.which(getattr(args, name))
        if not executable:
            parser.error(f"--{name} executable not found")
        setattr(args, name, str(Path(executable).resolve()))
    assets = {k: v for k, v in {"server": args.server, "panel_index": args.panel_web / "index.html",
                                "chrome": args.chrome, "nsh": args.nsh}.items() if v is not None}
    if not all(p.is_file() for p in assets.values()) or not args.node_modules.is_dir():
        parser.error("one or more supplied assets do not exist")
    if args.output.is_relative_to(REPO):
        parser.error("--output must be outside the checkout")
    if os.name != "nt":
        os.umask(0o077)
    args.output.mkdir(mode=0o700)  # Must be new; never overwrite evidence.
    report = {"schema_version": 1, "passed": False, "case": args.case, "stage": "fixture_setup",
              "evidence_kind": "instrumented_chromium_synthetic_audio_real_gateway", "mount": args.mount,
              "platform": {"system": platform.system(), "machine": platform.machine(), "release": platform.release()},
              "locale": args.locale, "native_packaged": "unavailable_not_exercised",
              "cross_mount_speedup": "unavailable_not_established",
              "unavailable_measurements": ["first_captured_sample", "end_of_speech_to_stt", "final_to_first_audible_audio",
                                           "end_of_speech_to_first_audible_audio", "peak_buffering", "actual_copies"]}
    try:
        report["harness_revision"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
        harness_sources = [HERE / name for name in ("run.py", "fixture.py", "web.cjs", "wire.cjs", "classic.cjs")]
        harness_sources.extend([HERE.parent / "nsh_panel_provider.py", Path(SPEC.origin)])
        report["harness_sha256"] = {path.relative_to(REPO).as_posix(): s2.sha256(path) for path in harness_sources}
        report["assets_sha256"] = {key: s2.sha256(path) for key, path in assets.items()}
        directory = None
        try:
            with tempfile.TemporaryDirectory(prefix="nsh-voice-") as directory:
                execute(args, Path(directory), report)
        finally:
            if directory is not None:
                report["scratch_removed"] = not Path(directory).exists()
        s2.require(report["scratch_removed"], "scratch directory was not removed")
        report.update(passed=True, stage="complete")
    except Exception as exc:
        # Never export raw exceptions, subprocess output, configs, URLs or credentials.
        report["failure_type"] = type(exc).__name__
    finally:
        json_write(args.output / "receipt.json", report)
    print(f"NSH Voice {report['stage']}: {'PASS' if report['passed'] else 'FAIL'} (receipt.json)")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
