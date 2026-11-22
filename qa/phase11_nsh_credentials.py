"""Opt-in real NSH WebView/credential fixture, NOT a Panel or Phase11 acceptance.

Build the debug host with --features qa-service, then pass --exe and a NEW
--receipt path. Uses only a random loopback origin, isolated native profile and
nonce-derived QA keyring service. No Gateway, installed service or release path
is used. A timeout is a failure, never evidence of natural native shutdown.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STEPS = ["initial_empty", "save_ack", "read_after_save", "superseded_lease_rejected",
         "read_after_stale", "navigation_lease_rejected", "read_after_navigation",
         "wrong_document_no_effect", "delete_existed", "final_empty"]


def evaluate(events, returncode):
    """Require every effect once, two real document identities and native return."""
    valid = isinstance(events, list) and all(isinstance(e, dict) for e in events)
    if not valid:
        return {"accepted": False, "reason": "malformed_observations"}
    observed = [e for e in events if e.get("kind") in
                ["credential-fixture", "close-requested", "closed"]]
    if [e["kind"] for e in observed] != ["credential-fixture", "close-requested", "closed"]:
        return {"accepted": False, "reason": "missing_duplicate_or_unordered_terminal_events"}
    fixture = observed[0]
    documents = fixture.get("documents")
    steps = fixture.get("steps")
    documents_ok = (isinstance(documents, list) and len(documents) == 2
                    and all(isinstance(d, str) and re.fullmatch(r"[0-9a-f]{32}", d) for d in documents)
                    and documents[0] != documents[1])
    steps_ok = (isinstance(steps, list) and all(isinstance(s, dict) for s in steps)
                and [s.get("name") for s in steps] == STEPS
                and all(s.get("ok") is True for s in steps))
    closed = observed[-1].get("payload")
    accepted = (returncode == 0 and documents_ok and steps_ok and fixture.get("cleanup") is True
                and isinstance(closed, dict) and closed.get("owned_drained") is True)
    return {"accepted": bool(accepted), "reason": "observed_effects_and_native_return" if accepted
            else "effect_document_cleanup_or_native_exit_failed"}


def evaluate_autostart(events, returncode):
    base = evaluate(events, returncode)
    if not base["accepted"]:
        return base
    fixture = next(event for event in events if event.get("kind") == "credential-fixture")
    expected = [{"document": document, "value": {"status": "unavailable", "reason": "not_app_bundle"}}
                for document in fixture["documents"]]
    accepted = fixture.get("autostart") == expected
    return {"accepted": accepted, "reason": "native_status_on_two_documents" if accepted
            else "missing_wrong_or_collapsed_native_status"}


def run(exe, receipt, timeout, observe_autostart=False):
    if observe_autostart and (not hasattr(os, "uname") or os.uname().sysname != "Darwin"):
        raise RuntimeError("status extension requires the macOS native runner")
    receipt.parent.mkdir(parents=True, exist_ok=True)
    # Do not overwrite evidence or reuse another invocation's native profile.
    receipt.touch(exist_ok=False)
    profile = receipt.with_suffix(".profile")
    profile.mkdir(exist_ok=False)
    nonce, run_id = os.urandom(32).hex(), uuid.uuid4().hex
    page = Path(__file__).with_name("phase11_nsh_credentials.html").read_text()
    settings = json.dumps({"nonce": nonce, "run_id": run_id, "observe_autostart": observe_autostart})
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path not in ["/", "/after-navigation"]:
                self.send_error(404)
                return
            requests.append(self.path)
            body = page.replace("/*SETTINGS*/", f"const settings = {settings};").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass  # Never log request URLs or fixture credentials.

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    env = dict(os.environ)
    for key in ["ALEPH_PANEL_QA_START_GATE", "ALEPH_PANEL_QA_GATE_NONCE", "ALEPH_PANEL_QA_CLOCK",
                "ALEPH_PANEL_QA_PID"]:
        env.pop(key, None)
    env.update({"ALEPH_NSH_QA_URL": f"http://127.0.0.1:{server.server_port}/",
                "ALEPH_NSH_QA_NONCE": nonce, "ALEPH_NSH_QA_WINDOW_DATA_DIR": str(profile.resolve()),
                "ALEPH_PANEL_QA_RUN_ID": run_id, "ALEPH_SHELL_HOTKEY": "disabled"})
    result = {"artifact": "phase11-native-credential-fixture", "status": "FAILED",
              "scope": "real NSH WebView to isolated OS credential store; NOT Panel WASM or Phase11 acceptance",
              "binary_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
              "platform": os.uname().sysname if hasattr(os, "uname") else os.name,
              "profile": str(profile), "run_id": run_id, "events": [],
              "limitations": ["no production keyring, Gateway or service operation",
                              "not full pairing, platform parity, performance or dual-track evidence",
                              "on failed/timeout runs, isolated QA store cleanup may be unknown"]}
    try:
        process = subprocess.run([str(exe.resolve())], env=env, capture_output=True,
                                 timeout=timeout, check=False)
        for line in process.stdout.decode(errors="replace").splitlines():
            if not line.startswith("ALEPH_PANEL_QA "):
                continue
            event = json.loads(line.removeprefix("ALEPH_PANEL_QA "))
            if event.pop("nonce", None) != nonce or event.pop("qa_run_id", None) != run_id:
                continue
            if event.get("kind") not in ["credential-fixture", "close-requested", "closed"]:
                continue
            # The fixture emits only names/booleans/document IDs, never credentials.
            result["events"].append(event)
        result["returncode"] = process.returncode
        result["evaluation"] = (evaluate_autostart if observe_autostart else evaluate)(result["events"], process.returncode)
        if result["evaluation"]["accepted"] and requests == ["/", "/after-navigation"]:
            result["status"] = "PASS_SCOPED_NATIVE_CREDENTIAL_AND_STATUS_EFFECTS" if observe_autostart else "PASS_SCOPED_NATIVE_CREDENTIAL_EFFECTS"
    except subprocess.TimeoutExpired:
        result["evaluation"] = {"accepted": False, "reason": "owned_host_timeout_not_natural_exit"}
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
        result["documents_served"] = requests
        receipt.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "receipt": str(receipt)}))
    return result["status"] in ["PASS_SCOPED_NATIVE_CREDENTIAL_EFFECTS", "PASS_SCOPED_NATIVE_CREDENTIAL_AND_STATUS_EFFECTS"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--receipt", required=True, type=Path)
    parser.add_argument("--observe-autostart", action="store_true",
                        help="also require real read-only status replies on both macOS documents")
    parser.add_argument("--timeout-seconds", type=float, default=45)
    args = parser.parse_args()
    if not args.exe.is_file() or args.timeout_seconds <= 0:
        parser.error("an existing debug qa-service binary and positive timeout are required")
    return 0 if run(args.exe, args.receipt, args.timeout_seconds, args.observe_autostart) else 1


if __name__ == "__main__":
    raise SystemExit(main())
