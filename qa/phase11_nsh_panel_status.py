"""Functional proof of the real Panel component, not Phase11 acceptance."""
import argparse
from http.client import HTTPConnection, HTTPException
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid
from urllib.parse import urlsplit

EFFECTS = ["native_marker", "credential_load_empty", "connect_authorized_without_credential",
           "component_visible", "status_text_correct", "readonly_text_correct", "toggle_absent",
           "document_bound"]


def evaluate(events, returncode, cleanup, *, gateway_survival=None):
    valid = (isinstance(events, list) and all(isinstance(e, dict) for e in events)
             and [e.get("kind") for e in events] == ["panel-status-fixture", "close-requested", "closed"])
    if not valid or not isinstance(cleanup, dict):
        return {"accepted": False, "reason": "missing_duplicate_or_unordered_effects"}
    fixture = events[0]
    effects = fixture.get("effects")
    children = cleanup.get("children")
    accepted = (
        type(returncode) is int and returncode == 0
        and isinstance(fixture.get("document"), str)
        and re.fullmatch(r"[0-9a-f]{32}", fixture["document"]) is not None
        and fixture.get("route") == "/settings/general"
        and fixture.get("native_status") == {"status": "unavailable", "reason": "not_app_bundle"}
        and isinstance(effects, dict) and set(effects) == set(EFFECTS)
        and all(effects.get(name) is True for name in EFFECTS)
        and isinstance(events[-1].get("payload"), dict)
        and events[-1]["payload"].get("owned_drained") is True
        and cleanup.get("status") == "READY"
        and cleanup.get("panel_assets_unchanged") is True
        and cleanup.get("fixture_removed") is True
        and isinstance(children, list) and all(isinstance(c, dict) for c in children)
        and [c.get("name") for c in children] == ["model", "gateway"]
        and all(c.get("forced") is False and type(c.get("exit_code")) is int
                and c["exit_code"] in [0, -15] for c in children))
    if accepted and gateway_survival is not None:
        accepted = same_ready_gateway(gateway_survival, children[-1].get("pid"))
        return {"accepted": accepted, "reason": "same_ready_gateway_after_native_quit" if accepted
                else "missing_or_changed_gateway_after_native_quit"}
    return {"accepted": bool(accepted), "reason": "production_panel_native_status_and_cleanup" if accepted
            else "effect_asset_native_exit_or_cleanup_failed"}


def same_ready_gateway(samples, expected_pid):
    if (type(expected_pid) is not int or expected_pid <= 0
            or not isinstance(samples, list) or len(samples) != 2):
        return False
    for sample in samples:
        if not isinstance(sample, dict):
            return False
        pids = sample.get("listener_pids")
        if (not isinstance(pids, list) or len(pids) != 1 or type(pids[0]) is not int
                or pids[0] != expected_pid or sample.get("ready") is not True
                or type(sample.get("started_at_unix")) is not int
                or sample["started_at_unix"] <= 0
                or any(not isinstance(sample.get(key), str) or not sample[key].strip()
                       for key in ["instance_id", "version"])):
            return False
    return samples[0] == samples[1]


def gateway_sample(url):
    origin = urlsplit(url)
    port = origin.port
    if (origin.scheme != "http" or port is None or port == 18790
            or origin.netloc != f"127.0.0.1:{port}" or origin.path != "/"
            or origin.query or origin.fragment):
        raise ValueError("private_nondefault_gateway_required")

    def get(path):
        conn = HTTPConnection("127.0.0.1", port, timeout=2)
        try:
            conn.request("GET", path)
            response = conn.getresponse()
            body = response.read(4097)
            return json.loads(body) if response.status == 200 and len(body) <= 4096 else None
        finally:
            conn.close()

    try:
        health, ready = get("/health"), get("/ready")
        if (not isinstance(health, dict) or not isinstance(ready, dict)
                or health.get("status") != "ok" or ready.get("ready") is not True
                or ready.get("phase") != "complete" or health.get("version") != ready.get("version")):
            return None
        lsof = subprocess.run(["/usr/sbin/lsof", "-nP", f"-iTCP@127.0.0.1:{port}",
                               "-sTCP:LISTEN", "-t"], capture_output=True, text=True, timeout=3)
        if lsof.returncode != 0:
            return None
        return {"listener_pids": [int(line) for line in lsof.stdout.splitlines()],
                "instance_id": health.get("instance_id"), "started_at_unix": health.get("started_at_unix"),
                "version": health.get("version"), "ready": True}
    except (OSError, HTTPException, ValueError, subprocess.TimeoutExpired):
        return None


def native_environment(base, profile, gateway_url, nonce, run_id, labels):
    env = {k: v for k, v in base.items()
           if not k.startswith(("ALEPH_NSH_QA_", "ALEPH_PANEL_QA_"))}
    # macOS OS-keyring access needs the real login context. Namespace and
    # native/app data isolation are explicit; a fake HOME is not a sandbox.
    env.update(ALEPH_HOME=str(profile / "aleph"),
               ALEPH_NSH_QA_URL=gateway_url, ALEPH_NSH_QA_NONCE=nonce,
               ALEPH_NSH_QA_WINDOW_DATA_DIR=str(profile), ALEPH_PANEL_QA_RUN_ID=run_id,
               ALEPH_PANEL_QA_STATUS="1", ALEPH_PANEL_QA_STATUS_LABELS=json.dumps(labels),
               ALEPH_SHELL_HOTKEY="disabled")
    return env


def run(exe, gateway_binary, receipt, timeout, *, verify_gateway_survival=False):
    if sys.platform != "darwin":
        raise RuntimeError("macOS native runner required")
    from nsh_panel_gateway import isolated_gateway
    from nsh_panel_launcher import file_sha256

    labels = []
    for language in ["en", "zh"]:
        locale = Path(__file__).resolve().parent.parent / "interfaces/webchat/locales" / f"{language}.json"
        autostart = json.loads(locale.read_text())["settings"]["general"]["autostart"]
        labels.append({key: autostart[key] for key in ["status_unavailable", "read_only"]})
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.touch(exist_ok=False)
    profile = receipt.with_suffix(".profile")
    profile.mkdir(exist_ok=False)
    (profile / "aleph").mkdir()
    nonce, run_id = os.urandom(32).hex(), uuid.uuid4().hex
    gateway_output = receipt.parent / (receipt.stem + "-gateway")
    gateway_output.mkdir(exist_ok=False)
    events, returncode, timed_out = [], None, False
    gateway_survival = [] if verify_gateway_survival else None
    with isolated_gateway(gateway_binary, gateway_output) as gateway_receipt:
        gateway = json.loads(gateway_receipt.read_text())
        if gateway_survival is not None:
            gateway_survival.append(gateway_sample(gateway["url"]))
        env = native_environment(os.environ, profile, gateway["url"], nonce, run_id, labels)
        with receipt.with_suffix(".stdout").open("wb") as out, receipt.with_suffix(".stderr").open("wb") as err:
            try:
                child = subprocess.run([str(exe)], cwd=profile, env=env, stdout=out,
                                       stderr=err, timeout=timeout, check=False)
                returncode = child.returncode
            except subprocess.TimeoutExpired:
                timed_out = True  # Forced termination is never a native-engine return.
        if gateway_survival is not None:
            gateway_survival.append(gateway_sample(gateway["url"]))
    for line in receipt.with_suffix(".stdout").read_text(errors="replace").splitlines():
        if not line.startswith("ALEPH_PANEL_QA "):
            continue
        try:
            event = json.loads(line.removeprefix("ALEPH_PANEL_QA "))
        except ValueError:
            continue
        if isinstance(event, dict) and event.get("nonce") == nonce and event.get("qa_run_id") == run_id:
            events.append({k: v for k, v in event.items() if k not in ["nonce", "qa_run_id"]})
    cleanup = json.loads((gateway_output / "gateway-cleanup.json").read_text())
    verdict = evaluate(events, returncode, cleanup, gateway_survival=gateway_survival)
    pass_status = ("PASS_SCOPED_EXTERNAL_GATEWAY_SURVIVES_QUIT" if verify_gateway_survival
                   else "PASS_SCOPED_PRODUCTION_PANEL_STATUS")
    result = {"status": pass_status if verdict["accepted"] else "FAIL",
              "acceptance": verdict, "events": events, "returncode": returncode, "timed_out": timed_out,
              "binary_sha256": file_sha256(exe), "gateway": gateway, "gateway_cleanup": cleanup,
              "limitations": ["not_performance_acceptance", "not_pairing_or_protected_identity",
                              "not_all_native_helpers_census", "not_platform_parity_or_phase11_acceptance"]}
    if gateway_survival is not None:
        result["gateway_survival"] = gateway_survival
        result["limitations"].append("qa_external_target_and_native_quit_not_local_admission_or_tray_click")
    receipt.write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--gateway-binary", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=float, default=60)
    parser.add_argument("--verify-gateway-survival", action="store_true")
    args = parser.parse_args()
    result = run(args.exe.resolve(), args.gateway_binary.resolve(), args.receipt.resolve(), args.timeout_seconds,
                 verify_gateway_survival=args.verify_gateway_survival)
    print(json.dumps({"status": result["status"], "acceptance": result["acceptance"]}))
    return 0 if result["acceptance"]["accepted"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
