"""Read-only, nonce-isolated macOS login-item diagnostic. Never registers an item.

Build --features qa-service and pass --exe plus a NEW --receipt path. Retains an
ad-hoc signed bundle in the evidence directory, never installs/launch-registers
it. Not a test of login enable/disable, Panel IPC, server service or other OSes.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys


CASES = ["unbundled", "isolated_first", "wrong_identity", "isolated_second"]


def evaluate(observations):
    valid = (isinstance(observations, list) and len(observations) == len(CASES)
             and all(isinstance(item, dict) for item in observations))
    if not valid:
        return {"accepted": False, "reason": "malformed_observations"}
    if ([item.get("case") for item in observations] != CASES
            or not all(type(item.get("returncode")) is int and item["returncode"] == 0
                       and item.get("stderr_empty") is True for item in observations)):
        return {"accepted": False, "reason": "controls_or_process_failed"}
    values = [item.get("value") for item in observations]
    accepted = (values[0] == {"status": "unavailable", "reason": "not_app_bundle"}
                and values[2] == {"status": "unavailable", "reason": "identity_mismatch"}
                and values[1] in [{"status": "disabled"},
                                  {"status": "unavailable", "reason": "not_found"}]
                and values[1] == values[3])
    return {"accepted": accepted, "reason": "isolated_os_status_and_refusal_controls" if accepted
            else "status_identity_or_stability_failed"}


def run(exe, receipt):
    if sys.platform != "darwin":
        raise RuntimeError("macOS native runner required; no cross-platform acceptance")
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.touch(exist_ok=False)
    # Exclusive ownership of this bundle; do not overwrite another invocation.
    bundle = receipt.with_suffix(".app")
    (bundle / "Contents/MacOS").mkdir(parents=True, exist_ok=False)
    binary = bundle / "Contents/MacOS/aleph-nsh-shell"
    shutil.copy2(exe, binary)
    nonce = os.urandom(32).hex()
    identity = f"com.aleph.nsh-shell.qa.{nonce}"
    (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": identity, "CFBundleExecutable": binary.name,
        "CFBundleName": "Aleph NSH Isolated Status QA", "CFBundlePackageType": "APPL",
        "CFBundleVersion": "1", "CFBundleShortVersionString": "0.1.0",
        "LSMinimumSystemVersion": "13.0", "LSUIElement": True,
    }))
    for args in [["codesign", "--sign", "-", str(bundle)],
                 ["codesign", "--verify", "--strict", str(bundle)]]:
        result = subprocess.run(args, capture_output=True, timeout=30)
        if result.returncode != 0:
            raise RuntimeError("isolated ad-hoc signature creation/verification failed")
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    observations = []
    for case in CASES:
        env = os.environ.copy()
        env.pop("ALEPH_NSH_QA_URL", None)
        env["ALEPH_NSH_QA_NONCE"] = os.urandom(32).hex() if case == "wrong_identity" else nonce
        result = subprocess.run([str(exe if case == "unbundled" else binary),
                                 "--autostart-status"], env=env, capture_output=True, timeout=15)
        try:
            value = json.loads(result.stdout)
        except (ValueError, UnicodeError):
            value = None
        observations.append({"case": case, "returncode": result.returncode,
                             "stderr_empty": not result.stderr, "value": value})
    outcome = evaluate(observations)
    unchanged = hashlib.sha256(binary.read_bytes()).hexdigest() == binary_hash
    value = {"status": "PASS_SCOPED_READ_ONLY_STATUS" if outcome["accepted"] and unchanged else "FAIL",
             "scope": "macOS isolated bundle read only; not enable/disable or Panel/platform acceptance",
             "application_id": identity, "source_binary_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
             "signed_binary_sha256": binary_hash, "binary_unchanged": unchanged,
             "observations": observations, "evaluation": outcome,
             "not_executed": ["register", "unregister", "installed login-item writes", "server service commands"]}
    receipt.write_text(json.dumps(value, indent=2) + "\n")
    return value["status"] == "PASS_SCOPED_READ_ONLY_STATUS"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--receipt", required=True, type=Path)
    args = parser.parse_args()
    ok = run(args.exe.resolve(strict=True), args.receipt.resolve())
    print("PASS_SCOPED_READ_ONLY_STATUS" if ok else "FAIL")
    raise SystemExit(0 if ok else 1)
