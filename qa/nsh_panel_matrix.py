#!/usr/bin/env python3
"""Collect isolated Tauri-first/NSH-second debug parity runs.

Each run owns a fresh production Gateway, provider process and session fixture.
The existing parity reducer owns warmup removal and all acceptance thresholds.
"""
from pathlib import Path
import argparse
import json
import signal

from nsh_panel_gateway import isolated_gateway
from nsh_panel_launcher import export_parity, launch_native_surfaces


def run_matrix(repo_root, target_dir, gateway_binary, receipt, runs=40, timeout=60):
    output = receipt.parent / (receipt.stem + "-output")
    output.mkdir(parents=True, exist_ok=True)
    results = []
    for surface in ("tauri", "nsh"):
        for index in range(runs):
            prefix = output / f"{surface}-{index:02d}"
            run_receipt = prefix.with_suffix(".json")
            try:
                with isolated_gateway(gateway_binary, Path(str(prefix) + "-gateway")) as gateway:
                    result = launch_native_surfaces(repo_root, target_dir, repo_root / "qa/fixtures/nsh-panel-parity",
                                                    run_receipt, gateway, timeout, 1, surface)
            except (ValueError, OSError, RuntimeError) as error:
                result = {"status": "FAIL", "reason": str(error)}
            cleanup_path = Path(str(prefix) + "-gateway") / "gateway-cleanup.json"
            cleanup = json.loads(cleanup_path.read_text()) if cleanup_path.is_file() else {}
            result["gateway_cleanup"] = cleanup
            if not cleanup.get("fixture_removed") or not cleanup.get("panel_assets_unchanged") or any(c["forced"] for c in cleanup.get("children", [])):
                result["status"] = "FAIL"
                result["cleanup_verification_failed"] = True
                result.setdefault("reason", "isolated_gateway_cleanup_failed")
            run_receipt.write_text(json.dumps(result, indent=2) + "\n")
            results.append({"surface": surface, "index": index, "receipt": str(run_receipt), "result": result})
            checkpoint = {"status": "RUNNING", "runs_per_surface": runs, "completed": len(results), "runs": results}
            receipt.write_text(json.dumps(checkpoint, indent=2) + "\n")
            print(json.dumps({"surface": surface, "index": index, "status": result["status"]}), flush=True)
            if result["status"] != "PASS":
                checkpoint["status"] = "INVALID"
                receipt.write_text(json.dumps(checkpoint, indent=2) + "\n")
                return checkpoint
    collected = [entry["result"]["runs"][0] for entry in results]
    parity = export_parity(collected[:40], collected[40:], output, receipt.stem) if runs == 40 else {"status": "SMOKE_ONLY"}
    accepted = runs == 1 or (parity.get("exit_code") == 0 and parity["result"].get("threshold_status") == "ACCEPTED")
    result = {"status": "PASS" if accepted else "NOT_ACCEPTED", "runs_per_surface": runs,
              "completed": len(results), "runs": results, "performance": parity}
    receipt.write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--target-dir", type=Path, default=Path(".phase9-target"))
    parser.add_argument("--gateway-binary", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--runs", type=int, choices=[1, 40], default=40)
    parser.add_argument("--timeout-seconds", type=float, default=60)
    args = parser.parse_args()
    def terminate(signum, _frame):
        raise SystemExit(128 + signum)
    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        result = run_matrix(args.repo_root.resolve(), args.target_dir.resolve(), args.gateway_binary.resolve(),
                            args.receipt.resolve(), args.runs, args.timeout_seconds)
    finally:
        signal.signal(signal.SIGTERM, previous)
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
