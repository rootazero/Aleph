#!/usr/bin/env python3
"""Evaluate the bounded NSH Panel parity performance receipt.

The launcher/runtime marker is intentionally separate from this reducer. This
script consumes two value-only metric files, applies the approved sampling and
threshold protocol, and emits a fixed-shape receipt.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from pathlib import Path

WARMUPS = 10
MEASURED = 30
P95_RANK = math.ceil(0.95 * MEASURED)
LATENCY_METRICS = ("first_frame_ms", "route_ms", "input_ms", "reconnect_ms", "close_ms")
RESOURCE_METRICS = ("rss_mb", "threads")
METRICS = LATENCY_METRICS + RESOURCE_METRICS
DEFAULT_THRESHOLDS = {
    "latency_median_ratio": 1.10,
    "latency_p95_ratio": 1.20,
    "rss_median_ratio": 1.10,
    "threads_median_delta": 2.0,
}


def fail(reason: str) -> int:
    print(json.dumps({"status": "INVALID", "invalid_reason": reason}, sort_keys=True))
    return 2


def read_json(path: Path) -> object:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot_read_json:{path}:{error}") from error


def samples(document: object, label: str) -> dict[str, list[float]]:
    if not isinstance(document, dict):
        raise ValueError(f"{label}_not_object")
    raw_metrics = document.get("metrics")
    if not isinstance(raw_metrics, dict):
        raise ValueError(f"{label}_metrics_missing")
    result: dict[str, list[float]] = {}
    for metric in METRICS:
        raw = raw_metrics.get(metric)
        if not isinstance(raw, list) or len(raw) != WARMUPS + MEASURED:
            raise ValueError(f"{label}_{metric}_requires_40_samples")
        if any(isinstance(value, bool) or not isinstance(value, (int, float)) or value < 0 for value in raw):
            raise ValueError(f"{label}_{metric}_contains_invalid_sample")
        result[metric] = [float(value) for value in raw[WARMUPS:]]
    return result


def summary(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)
    return {
        "median": statistics.median(ordered),
        "p95": ordered[P95_RANK - 1],
        "sample_count": float(len(ordered)),
        "warmup_count": float(WARMUPS),
    }


def ratio(candidate: float, baseline: float) -> float:
    if baseline == 0:
        return 1.0 if candidate == 0 else float("inf")
    return candidate / baseline


def evaluate(baseline: dict[str, list[float]], candidate: dict[str, list[float]]) -> tuple[dict, list[str]]:
    baseline_summary = {metric: summary(values) for metric, values in baseline.items()}
    candidate_summary = {metric: summary(values) for metric, values in candidate.items()}
    failures: list[str] = []
    comparisons: dict[str, dict[str, float | bool]] = {}
    for metric in LATENCY_METRICS:
        base = baseline_summary[metric]
        current = candidate_summary[metric]
        median_ratio = ratio(current["median"], base["median"])
        p95_ratio = ratio(current["p95"], base["p95"])
        comparisons[metric] = {
            "median_ratio": median_ratio,
            "p95_ratio": p95_ratio,
            "median_accepted": median_ratio <= DEFAULT_THRESHOLDS["latency_median_ratio"],
            "p95_accepted": p95_ratio <= DEFAULT_THRESHOLDS["latency_p95_ratio"],
        }
        if median_ratio > DEFAULT_THRESHOLDS["latency_median_ratio"]:
            failures.append(f"{metric}:median")
        if p95_ratio > DEFAULT_THRESHOLDS["latency_p95_ratio"]:
            failures.append(f"{metric}:p95")

    rss_ratio = ratio(candidate_summary["rss_mb"]["median"], baseline_summary["rss_mb"]["median"])
    threads_delta = candidate_summary["threads"]["median"] - baseline_summary["threads"]["median"]
    comparisons["rss_mb"] = {
        "median_ratio": rss_ratio,
        "median_accepted": rss_ratio <= DEFAULT_THRESHOLDS["rss_median_ratio"],
    }
    comparisons["threads"] = {
        "median_delta": threads_delta,
        "median_accepted": threads_delta <= DEFAULT_THRESHOLDS["threads_median_delta"],
    }
    if rss_ratio > DEFAULT_THRESHOLDS["rss_median_ratio"]:
        failures.append("rss_mb:median")
    if threads_delta > DEFAULT_THRESHOLDS["threads_median_delta"]:
        failures.append("threads:median")

    return {
        "baseline": baseline_summary,
        "candidate": candidate_summary,
        "comparisons": comparisons,
    }, failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    try:
        baseline = samples(read_json(args.baseline), "baseline")
        candidate = samples(read_json(args.candidate), "candidate")
        comparison, failures = evaluate(baseline, candidate)
    except ValueError as error:
        args.receipt.write_text(json.dumps({"status": "INVALID", "invalid_reason": str(error)}, indent=2) + "\n")
        print(json.dumps({"status": "INVALID", "invalid_reason": str(error)}, sort_keys=True))
        return 2
    threshold_status = "ACCEPTED" if not failures else "NOT_ACCEPTED"
    receipt = {
        "version": 1,
        "run_id": args.run_id,
        "status": "VALID",
        "threshold_status": threshold_status,
        "baseline": comparison["baseline"],
        "candidate": comparison["candidate"],
        "comparisons": comparison["comparisons"],
        "thresholds": DEFAULT_THRESHOLDS,
        "acceptance_reason": "all_thresholds_met" if not failures else None,
        "invalid_reason": None,
        "failure_metrics": failures,
        "protocol": {
            "warmups": WARMUPS,
            "measured": MEASURED,
            "p95_rank_one_based": P95_RANK,
            "order": "tauri_then_nsh",
        },
    }
    args.receipt.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"status": "VALID", "threshold_status": threshold_status, "failure_metrics": failures}, sort_keys=True))
    return 0 if not failures else 3


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except OSError as error:
        print(json.dumps({"status": "LAUNCHER_FAILURE", "invalid_reason": str(error)}, sort_keys=True))
        raise SystemExit(4)
