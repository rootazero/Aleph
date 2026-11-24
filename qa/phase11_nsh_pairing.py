"""Protected real Panel pairing/reload proof, not full production-server acceptance."""
import argparse
import json
import os
from pathlib import Path
import re
import select
import subprocess
import sys
import tempfile
import time
import uuid

EFFECTS = ["initial_empty", "ticket_submitted", "pair_authorized", "save_ack",
           "reload_stored", "reload_protected", "no_reload_ticket", "cleanup_deleted", "cleanup_empty"]


def evaluate(events, returncode, helper):
    if (not isinstance(events, list) or not all(isinstance(e, dict) for e in events)
            or [e.get("kind") for e in events] != ["protected-panel-fixture", "close-requested", "closed"]
            or not isinstance(helper, dict)):
        return {"accepted": False, "reason": "missing_or_unordered_protected_effects"}
    value = events[0]
    docs, effects = value.get("documents"), value.get("effects")
    accepted = (type(returncode) is int and returncode == 0 and value.get("user") == "alice"
                and isinstance(docs, list) and len(docs) == 2 and len(set(str(d) for d in docs)) == 2
                and all(isinstance(d, str) and re.fullmatch(r"[0-9a-f]{32}", d) for d in docs)
                and isinstance(effects, dict) and set(effects) == set(EFFECTS)
                and all(effects[k] is True for k in EFFECTS)
                and isinstance(events[-1].get("payload"), dict)
                and events[-1]["payload"].get("owned_drained") is True
                and all(helper.get(k) is True for k in ["confirmed", "listener_closed", "root_removed", "assets_unchanged"])
                and helper.get("forced") is False
                and type(helper.get("returncode")) is int and helper["returncode"] == 0)
    return {"accepted": bool(accepted), "reason": "protected_pair_reload_and_cleanup" if accepted else "protected_effect_or_cleanup_failed"}


def helper_stopped(value):
    return (isinstance(value, dict) and value.get("type") == "stopped"
            and value.get("listenerClosed") is True and value.get("fixtureClosed") is True)


class Helper:
    """Own the existing helper's control pipe; never persist its ticket output."""
    def __init__(self, child, nonce):
        self.child, self.nonce, self.buffer, self.sequence = child, nonce, b"", 0

    def receive(self):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                value = json.loads(line)
                if value.get("run") == self.nonce:
                    return value
                continue
            ready, _, _ = select.select([self.child.stdout], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                break
            data = os.read(self.child.stdout.fileno(), 8192)
            if not data or len(self.buffer) + len(data) > 65536:
                break
            self.buffer += data
        raise RuntimeError("protected_helper_control_failed")

    def command(self, command, **fields):
        self.sequence += 1
        value = dict(v=1, type="command", run=self.nonce, id=self.sequence, command=command, **fields)
        self.child.stdin.write((json.dumps(value) + "\n").encode())
        self.child.stdin.flush()
        return self.receive()


def run(exe, helper_binary, receipt, timeout):
    if sys.platform != "darwin":
        raise RuntimeError("macOS native runner required")
    from nsh_panel_gateway import capture_panel_assets, panel_assets_unchanged
    from nsh_panel_launcher import file_sha256
    from phase11_nsh_panel_status import native_environment

    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.touch(exist_ok=False)
    profile = receipt.with_suffix(".profile")
    profile.mkdir(exist_ok=False)
    (profile / "aleph").mkdir()
    nonce, run_id = os.urandom(32).hex(), uuid.uuid4().hex
    fixture = tempfile.TemporaryDirectory(prefix="aleph-shell-qa-", dir=str(Path(tempfile.gettempdir()).resolve()))
    root = Path(fixture.name)
    (root / ".qa-owned").write_text(nonce)
    (root / ".aleph").mkdir()
    events, rc, before, error = [], None, None, None
    cleanup = {"confirmed": False, "listener_closed": False, "forced": False,
               "returncode": None, "root_removed": False, "assets_unchanged": False}
    with receipt.with_suffix(".helper-stderr").open("wb") as stderr:
        child = subprocess.Popen([str(helper_binary), "--owned-root", str(root), "--nonce", nonce,
                                  "--panel-root", str(Path("interfaces/webchat/dist").resolve())],
                                 cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr)
        helper = Helper(child, nonce)
        try:
            ready = helper.receive()
            if ready.get("type") != "ready":
                raise RuntimeError("protected_helper_not_ready")
            origin = ready["origin"] + "/"
            # Routing metadata only; no ticket/token. Preserve recovery identity even if the UI fails.
            fd = os.open(receipt.with_suffix(".cleanup-context.json"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as context:
                json.dump({"nonce": nonce, "origin": origin}, context)
            before = capture_panel_assets(origin)
            pairing = helper.command("issue_pairing", user="alice", device="device-a")
            if pairing.get("type") != "pairing" or not isinstance(pairing.get("ticket"), str):
                raise RuntimeError("protected_pairing_not_issued")
            env = native_environment(os.environ, profile, origin, nonce, run_id, [])
            env["ALEPH_PANEL_QA_MESSAGE"] = "phase11-pairing:" + json.dumps({"ticket": pairing["ticket"]})
            del pairing
            with receipt.with_suffix(".stdout").open("wb") as out, receipt.with_suffix(".stderr").open("wb") as err:
                try:
                    native = subprocess.run([str(exe)], cwd=profile, env=env, stdout=out, stderr=err,
                                            timeout=timeout, check=False)
                    rc = native.returncode
                except subprocess.TimeoutExpired:
                    error = "native_timeout"
            del env
            response = helper.command("confirm_pairing", device="device-a")
            cleanup["confirmed"] = response.get("type") == "ack" and response.get("status") == "ok"
            cleanup["assets_unchanged"] = panel_assets_unchanged(origin, before)
        except (OSError, RuntimeError, ValueError, KeyError) as failure:
            error = type(failure).__name__
        finally:
            if child.poll() is None:
                try:
                    stopped = helper.command("shutdown")
                    cleanup["listener_closed"] = helper_stopped(stopped)
                except (OSError, RuntimeError, ValueError):
                    pass
                child.stdin.close()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    cleanup["forced"] = True
                    child.kill()
                    child.wait()
            cleanup["returncode"] = child.returncode
            if not child.stdin.closed:
                child.stdin.close()
            child.stdout.close()
            fixture.cleanup()
            cleanup["root_removed"] = not root.exists()
    stdout = receipt.with_suffix(".stdout")
    if stdout.exists():
        for line in stdout.read_text(errors="replace").splitlines():
            if not line.startswith("ALEPH_PANEL_QA "):
                continue
            try:
                value = json.loads(line.removeprefix("ALEPH_PANEL_QA "))
            except ValueError:
                continue
            if value.get("nonce") == nonce and value.get("qa_run_id") == run_id:
                events.append({k: v for k, v in value.items() if k not in ["nonce", "qa_run_id"]})
    verdict = evaluate(events, rc, cleanup)
    result = dict(status="PASS_SCOPED_PROTECTED_PANEL" if verdict["accepted"] else "FAIL",
                  acceptance=verdict, events=events, returncode=rc, helper=cleanup, error=error,
                  binary_sha256=file_sha256(exe), helper_sha256=file_sha256(helper_binary), panel_assets=before,
                  limitations=["canonical_protected_auth_fixture_not_full_production_server", "not_TLS_or_platform_parity",
                               "cleanup_is_QA_bridge_action_not_Panel_delete_UI", "not_performance_or_Phase11_acceptance"])
    receipt.write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--helper-binary", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=float, default=75)
    args = parser.parse_args()
    result = run(args.exe.resolve(), args.helper_binary.resolve(), args.receipt.resolve(), args.timeout_seconds)
    print(json.dumps({"status": result["status"], "acceptance": result["acceptance"]}))
    return 0 if result["acceptance"]["accepted"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
