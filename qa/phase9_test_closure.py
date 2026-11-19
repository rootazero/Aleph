#!/usr/bin/env python3
"""Run one owned Cargo test invocation and retain an honest terminal receipt."""
import argparse
import datetime
import hashlib
import json
import os
import re
import signal
import subprocess
import tempfile
import time
from pathlib import Path


def process_group_members(pgid):
    result = subprocess.run(["ps", "-axo", "pid=,pgid="], capture_output=True, text=True, check=True, timeout=5)
    return [int(fields[0]) for line in result.stdout.splitlines()
            if len(fields := line.split()) == 2 and int(fields[1]) == pgid]


def run_command(argv, cwd, env, prefix, timeout):
    started = time.monotonic()
    prefix.parent.mkdir(parents=True, exist_ok=True)
    stdout = Path(str(prefix) + ".stdout.log")
    stderr = Path(str(prefix) + ".stderr.log")
    timed_out = False
    unjoined_descendants = False
    cleanup_error = None
    group_probe = None
    with stdout.open("wb") as out, stderr.open("wb") as err:
        process = subprocess.Popen(argv, cwd=cwd, env=os.environ | env, stdout=out, stderr=err, start_new_session=True)

        def terminate(signum, _frame):
            raise SystemExit(128 + signum)

        previous_termination = signal.signal(signal.SIGTERM, terminate)
        try:
            process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, SystemExit, KeyboardInterrupt) as error:
            timed_out = isinstance(error, subprocess.TimeoutExpired)
            # Give an inner owner time to drain its independently owned group.
            # Killing the wrapper first would orphan that group's Cargo child.
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            if not timed_out:
                raise
        finally:
            # Cargo can exit before a descendant; never leave the owned group behind.
            try:
                os.killpg(process.pid, 0)
                unjoined_descendants = not timed_out
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError as error:
                cleanup_error = str(error)
                # A denied signal probe alone cannot distinguish an empty group.
                # Accept cleanup only when an independent census observes no PID.
                try:
                    members = process_group_members(process.pid)
                    group_probe = {"signal_probe_error": str(error), "remaining_pids": members}
                    if not members:
                        cleanup_error = None
                except (OSError, subprocess.SubprocessError) as census_error:
                    group_probe = {"signal_probe_error": str(error), "census_error": str(census_error)}
            finally:
                process.wait()
                signal.signal(signal.SIGTERM, previous_termination)
    return {
        "argv": argv, "cwd": str(cwd), "env_overrides": env,
        "exit_code": process.returncode, "timed_out": timed_out,
        "pid": process.pid, "cleanup_error": cleanup_error, "group_probe": group_probe,
        "unjoined_descendants": unjoined_descendants,
        "elapsed_seconds": time.monotonic() - started,
        "stdout": str(stdout), "stderr": str(stderr),
        "output": stdout.read_text(errors="replace") + stderr.read_text(errors="replace"),
    }


def classify(result):
    output = result["output"]
    if result["unjoined_descendants"] or result.get("cleanup_error"):
        return "FAIL"
    if re.search(r"panicked at|AssertionError|test result: FAILED|^test .* \.\.\. FAILED", output, re.MULTILINE):
        return "FAIL"
    if result["timed_out"]:
        return "UNRESOLVED_TIMEOUT"
    if result["exit_code"] != 0:
        if any(text in output for text in ["no matching package named", "attempting to make an HTTP request, but --offline was specified", "failed to download", "No space left on device"]):
            return "BLOCKED"
        return "FAIL"
    summaries = re.findall(r"test result: ok\. (\d+) passed; 0 failed;", output)
    return "PASS" if summaries and sum(map(int, summaries)) > 0 else "FAIL"


def fixture_environment(fixture, inherited, home=None):
    home = home if home is not None else fixture / "home"
    home.mkdir(parents=True, exist_ok=True)
    (fixture / "aleph").mkdir()
    account = Path(inherited.get("HOME", inherited.get("USERPROFILE", str(Path.home()))))
    return {
        "HOME": str(home), "ALEPH_HOME": str(fixture / "aleph"),
        **({"USERPROFILE": str(home)} if os.name == "nt" else {}),
        "CARGO_HOME": inherited.get("CARGO_HOME", str(account / ".cargo")),
        "RUSTUP_HOME": inherited.get("RUSTUP_HOME", str(account / ".rustup")),
        "TMPDIR": str(fixture),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=float, default=600)
    parser.add_argument("--test-filter")
    args = parser.parse_args()
    root = args.repo_root.resolve()
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    receipt = root / args.receipt
    prefix = receipt.parent / "phase9-test-output" / stamp
    gate = run_command(["python3", "qa/browser_live/check_memory.py", "--required-bytes", "4294967296"], root, {}, prefix.with_name(stamp + "-memory"), 30)
    record = {"schema_version": 1, "timestamp": stamp, "memory_gate": {k: v for k, v in gate.items() if k != "output"}, "test_filter": args.test_filter,
              "lock_sha256": hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest(),
              "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    if gate["exit_code"] != 0 or "ALLOW" not in gate["output"]:
        record.update(status="BLOCKED", reason="4GiB memory gate did not allow Cargo")
    else:
        argv = ["cargo", "test", "-p", "alephcore", "--lib", "--offline", "--locked", "--jobs", "1"]
        if args.test_filter:
            argv.append(args.test_filter)
        argv += ["--", "--test-threads=1"]
        # Seatbelt witnesses require HOME outside the temp tree; Unix sockets need a short TMPDIR.
        with tempfile.TemporaryDirectory(prefix="p9-", dir="/tmp") as fixture, tempfile.TemporaryDirectory(prefix=".p9-home-", dir=Path.home()) as account_fixture:
            env = fixture_environment(Path(fixture), os.environ, home=Path(account_fixture)) | {"CARGO_TARGET_DIR": str((root / args.target_dir).resolve()), "RUST_BACKTRACE": "1"}
            result = run_command(argv, root, env, prefix, args.timeout_seconds)
            record.update(status=classify(result), command={k: v for k, v in result.items() if k != "output"}, fixture={"path": fixture, "home": account_fixture, "cleanup": "pending"})
        record["fixture"]["cleanup"] = "removed" if not Path(fixture).exists() and not Path(account_fixture).exists() else "FAILED"
        if record["fixture"]["cleanup"] != "removed":
            record["status"] = "FAIL"
    record["lock_unchanged"] = hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest() == record["lock_sha256"]
    if not record["lock_unchanged"]:
        record["status"] = "FAIL"
    receipt.parent.mkdir(parents=True, exist_ok=True)
    json_path = Path(str(prefix) + ".json")
    json_path.write_text(json.dumps(record, indent=2) + "\n")
    with receipt.open("a") as output:
        output.write(f"\n- {stamp}: **{record['status']}**, filter `{args.test_filter or 'FULL LIB'}`; receipt `{json_path.relative_to(root)}`. / 原始输出与退出码见 JSON 回执。\n")
    print(json.dumps(record, indent=2))
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2, "UNRESOLVED_TIMEOUT": 3}[record["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
