#!/usr/bin/env python3
"""Validate runtime observations before exporting value-only parity metrics.

Run with: uv run --offline --no-project --with jsonschema==4.25.1 python ...
A protocol fixture validates this boundary; it never proves a live Gateway.
"""
from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import math
import os
import plistlib
import shutil
import re
import selectors
import select
import signal
import statistics
import subprocess
import sys
import time
import tempfile
import uuid
import traceback
from urllib.parse import urlsplit
from pathlib import Path

from jsonschema import Draft202012Validator

import nsh_panel_parity

SCHEMA = Path(__file__).with_name("nsh_panel_runtime.schema.json")
ORDER = ("ready", "hello", "accepted", "streamed", "complete", "artifact-refreshed", "reconnected", "closed")
CHALLENGES = {"old_epoch", "wrong_session", "malformed_topic", "duplicate"}


def evaluate_runtime(document, require_live=True):
    errors = sorted(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(document), key=lambda e: e.json_path)
    if errors:
        raise ValueError(f"schema:{errors[0].json_path}:{errors[0].message}")
    events = document["events"]
    if any(e["kind"] in {"route-error", "qa-error", "projection-violation"} for e in events):
        raise ValueError("runtime_reported_violation")
    if any(not math.isfinite(e["at_ms"]) for e in events):
        raise ValueError("nonfinite_event_clock")
    if any(a["at_ms"] > b["at_ms"] for a, b in zip(events, events[1:])):
        raise ValueError("event_clock_regressed")
    first = {}
    for index, event in enumerate(events):
        first.setdefault(event["kind"], index)
    missing = set(ORDER) - first.keys()
    if missing or [first[k] for k in ORDER] != sorted(first[k] for k in ORDER):
        raise ValueError(f"runtime_order_or_missing:{sorted(missing)}")

    epoch = 0
    session = None
    run = None
    snapshot_epoch = 0
    queued = None
    challenges = set()
    for event in events:
        kind = event["kind"]
        if event["qa_run_id"] != document["qa_run_id"]:
            raise ValueError("foreign_qa_run")
        if kind == "hello":
            if event["connection_epoch"] != epoch + 1:
                raise ValueError("hello_epoch_not_fresh")
            epoch += 1
        if event["connection_epoch"] != epoch:
            raise ValueError("old_or_foreign_epoch_applied")
        if kind == "queued":
            if run is not None or not event["session_key"] or not event["run_id"]:
                raise ValueError("invalid_queued_run")
            queued = (event["session_key"], event["run_id"])
        if kind == "accepted":
            if queued is not None and queued != (event["session_key"], event["run_id"]):
                raise ValueError("queued_acceptance_identity_changed")
            if not event["session_key"] or not event["run_id"]:
                raise ValueError("accepted_missing_canonical_session_or_run")
            session, run = event["session_key"], event["run_id"]
        if kind in {"streamed", "complete", "artifact-refreshed", "snapshot", "reconnected", "closed"}:
            if event["session_key"] != session:
                raise ValueError("foreign_session_applied")
            if event["run_id"] != run:
                raise ValueError("foreign_run_applied")
        if kind == "snapshot":
            if not all(event["payload"].get(k) is True for k in ["sessions", "history", "artifacts"]):
                raise ValueError("incomplete_snapshot")
            snapshot_epoch = epoch
        if kind == "artifact-refreshed" and event["payload"].get("refetched") is not True:
            raise ValueError("artifact_notification_not_refetched")
        if kind == "disconnected" and (
            event["payload"].get("production_state") is not True
            or event["payload"].get("marked_epoch") != epoch
        ):
            raise ValueError("reconnect_start_not_production_state")
        if kind == "reconnected" and (epoch < 2 or snapshot_epoch != epoch):
            raise ValueError("reconnect_missing_fresh_snapshot")
        if kind == "rejected":
            payload = event["payload"]
            if not payload.get("before") or payload.get("before") != payload.get("after"):
                raise ValueError("rejected_event_changed_projection")
            if payload.get("reason") == "old_epoch" and (
                payload.get("boundary") != "panel_ingress"
                or payload.get("handler_invoked") is not True
                or payload.get("current_control_accepted") is not True
                or payload.get("stale_frame_accepted") is not False
            ):
                raise ValueError("old_epoch_receiver_lifecycle_unproven")
            challenges.add(payload.get("reason"))
        if kind == "reconnected" and document["origin"] == "live_gateway" and (
            event["payload"].get("rendered") is not True
            or event["payload"].get("applied_epoch") != epoch
            or not event["payload"].get("projection")
        ):
            raise ValueError("reconnect_projection_not_applied_and_rendered")
        if kind == "closed" and event["payload"].get("owned_drained") is not True:
            raise ValueError("owned_drain_unproven")
    for kind in ("route-requested", "route-ready", "input", "send-received", "close-requested"):
        if kind not in first:
            raise ValueError(f"missing_metric_event:{kind}")
    receipt = events[first["send-received"]]
    if (receipt["session_key"], receipt["run_id"]) != (session, run) or (
        receipt["payload"].get("request_session") not in (None, session) or receipt["payload"].get("request_id") is None
    ):
        raise ValueError("send_receipt_identity")
    if first["send-received"] < first["input"]:
        raise ValueError("send_receipt_before_input")
    if not CHALLENGES.issubset(challenges):
        raise ValueError("missing_adversarial_projection_challenge")
    if document["direct_child"] != {"exit_code": 0, "forced": False}:
        raise ValueError("direct_child_failed_or_forced")

    resources = document["resources"]
    if any(not math.isfinite(s[k]) for s in resources for k in ["at_ms", "rss_mb", "threads"]):
        raise ValueError("nonfinite_resource_sample")
    if any(not 50 <= b["at_ms"] - a["at_ms"] <= 175 for a, b in zip(resources, resources[1:])):
        raise ValueError("resource_sampling_not_100ms")
    settled_at_ms = document.get("resource_scope", {}).get("settled_at_ms", events[-1]["at_ms"])
    if resources[0]["at_ms"] > 175 or max(events[-1]["at_ms"], settled_at_ms) - resources[-1]["at_ms"] > 175:
        raise ValueError("resource_sampling_does_not_cover_lifecycle")

    native_clock = all("native_at_ms" in event for event in events)
    if native_clock and (any(not math.isfinite(event["native_at_ms"]) for event in events)
                         or any(a["native_at_ms"] > b["native_at_ms"] for a, b in zip(events, events[1:]))):
        raise ValueError("invalid_native_event_clock")

    def at(kind):
        if kind not in first:
            raise ValueError(f"missing_metric_event:{kind}")
        return events[first[kind]]["native_at_ms" if native_clock else "at_ms"]

    browser_clock = any("browser_at_ms" in event for event in events)
    if browser_clock and (any(not isinstance(event.get("browser_at_ms"), (int, float))
                              or not math.isfinite(event["browser_at_ms"])
                              or event.get("browser_time_origin_ms") != events[0].get("browser_time_origin_ms")
                              or not isinstance(event.get("browser_time_origin_ms"), (int, float))
                              or not math.isfinite(event["browser_time_origin_ms"])
                              or event["browser_time_origin_ms"] <= 0 for event in events)
                          or any(a["browser_at_ms"] > b["browser_at_ms"] for a, b in zip(events, events[1:]))):
        raise ValueError("invalid_browser_event_clock")

    def browser_at(kind):
        if kind not in first:
            raise ValueError(f"missing_metric_event:{kind}")
        return events[first[kind]]["browser_at_ms"] if browser_clock else at(kind)

    metrics = {
        "first_frame_ms": at("rendered") if "rendered" in first else at("ready"),
        "route_ms": browser_at("route-ready") - browser_at("route-requested"),
        "input_ms": browser_at("send-received") - browser_at("input"),
        "reconnect_ms": browser_at("reconnected") - browser_at("disconnected"),
        "close_ms": max(at("closed"), document.get("resource_scope", {}).get("settled_at_ms", at("closed"))) - at("close-requested"),
        "rss_mb": max(s["rss_mb"] for s in resources),
        "threads": max(s["threads"] for s in resources),
    }
    if any(value < 0 for value in metrics.values()):
        raise ValueError("negative_derived_metric")
    gateway = document["gateway"]
    live = (document["origin"] == "live_gateway" and isinstance(gateway, dict)
            and gateway.get("kind") == "production_gateway" and verified_capture(document))
    return {
        "status": "PASS" if live and require_live else "BLOCKED" if require_live else "PROTOCOL_VALID",
        "reason": None if live else "missing_verified_live_gateway_capture",
        "resource_complete": document.get("resource_scope", {}).get("complete") is True,
        "evidence_scope": "schema_and_marker_correlations_plus_direct_child_exit",
        "qa_run_id": document["qa_run_id"],
        "surface": document["surface"],
        "queued_transition_observed": queued is not None,
        "metrics": metrics,
        "schema_sha256": hashlib.sha256(SCHEMA.read_bytes()).hexdigest(),
    }


def file_sha256(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def parse_launchservices_records(listing):
    records = []
    for block in re.split(r"(?m)^\s*\d+\)", listing):
        pid_match = re.search(r"(?m)^\s*pid = (\d+)", block)
        if not pid_match:
            continue
        name_match = re.search(r'^\s*"([^"]+)"', block)
        bundle_match = re.search(r'(?m)^\s*bundleID="([^"]+)"', block)
        coalition_match = re.search(r"(?m)^\s*coalition:\s*(\d+)(?:[ \t]*\{[ \t]*([^}]*)\})?[ \t]*$", block)
        # lsappinfo omits braces for a singleton; the controlled fork/join
        # experiment pins this public output shape before WebKit is created.
        member_values = re.findall(r"\d+", coalition_match.group(2)) if coalition_match and coalition_match.group(2) is not None else [pid_match.group(1)] if coalition_match else []
        members = tuple(sorted({int(pid) for pid in member_values}))
        if len(members) != len(member_values):
            raise ValueError("duplicate_pid_in_coalition")
        records.append({"pid": int(pid_match.group(1)), "name": name_match.group(1) if name_match else None,
                        "bundle_id": bundle_match.group(1) if bundle_match else None,
                        "coalition_id": int(coalition_match.group(1)) if coalition_match else None,
                        "pids": members})

    if len({record["pid"] for record in records}) != len(records):
        raise ValueError("duplicate_launchservices_pid")
    return records


def parse_launchservices_coalitions(listing, host_pids, require_webkit=True, allow_departed_host=False, departed_pids=()):
    webkit_bundles = {"com.apple.WebKit.GPU", "com.apple.WebKit.Networking", "com.apple.WebKit.WebContent"}
    webkit_suffixes = (" Graphics and Media", " Networking", " Web Content")
    if not host_pids:
        raise ValueError("empty_launchservices_host_set")
    records = parse_launchservices_records(listing)
    by_pid = {record["pid"]: record for record in records}
    missing_hosts = set(host_pids) - by_pid.keys()
    if missing_hosts:
        raise ValueError(f"missing_launchservices_hosts:{sorted(missing_hosts)}")
    hosts = {pid: by_pid[pid] for pid in sorted(host_pids)}
    coalitions = {}
    for pid, host in hosts.items():
        if host["name"] is None:
            raise ValueError(f"missing_launchservices_host_name:{pid}")
        if host["coalition_id"] is None or not host["pids"]:
            raise ValueError(f"incomplete_host_coalition:{pid}")
        if pid not in host["pids"] and not allow_departed_host:
            raise ValueError(f"host_left_coalition:{pid}")
        coalitions[pid] = set(host["pids"])
    coalition_ids = [host["coalition_id"] for host in hosts.values()]
    if len(coalition_ids) != len(set(coalition_ids)):
        raise ValueError("duplicate_instance_coalition_id")
    coalition_owners = {}
    for owner_pid, members in coalitions.items():
        for member_pid in members:
            coalition_owners.setdefault(member_pid, []).append(owner_pid)
    if any(len(owners) != 1 for owners in coalition_owners.values()):
        raise ValueError("overlapping_coalition_members")

    instance_names = {host["name"] for host in hosts.values()}
    webkit = {pid: {bundle: [] for bundle in sorted(webkit_bundles)} for pid in hosts}
    for record in records:
        if record["bundle_id"] not in webkit_bundles or not any(
            record["name"] == name + suffix for name in instance_names for suffix in webkit_suffixes
        ):
            continue
        owners = coalition_owners.get(record["pid"], [])
        if len(owners) != 1:
            if record["pid"] in departed_pids and record["coalition_id"] is None:
                continue
            raise ValueError(f"webkit_process_not_in_unique_coalition: unowned WebKit pid={record['pid']}")
        webkit[owners[0]][record["bundle_id"]].append(record["pid"])
    for pid, roles in webkit.items():
        if require_webkit and any(not process_ids for process_ids in roles.values()):
            raise ValueError(f"incomplete_webkit_process_set:{pid}")
        for bundle in roles:
            roles[bundle] = tuple(sorted(roles[bundle]))
    return {pid: {"coalition_id": hosts[pid]["coalition_id"], "pids": tuple(hosts[pid]["pids"]),
                  "owner_name": hosts[pid]["name"], "webkit": webkit[pid]} for pid in hosts}


class _ProcTaskInfo(ctypes.Structure):
    # Public Darwin sys/proc_info.h: PROC_PIDTASKINFO uses this fixed-width ABI.
    _fields_ = [(name, ctypes.c_uint64) for name in (
        "pti_virtual_size", "pti_resident_size", "pti_total_user", "pti_total_system",
        "pti_threads_user", "pti_threads_system",
    )] + [(name, ctypes.c_int32) for name in (
        "pti_policy", "pti_faults", "pti_pageins", "pti_cow_faults", "pti_messages_sent",
        "pti_messages_received", "pti_syscalls_mach", "pti_syscalls_unix", "pti_csw",
        "pti_threadnum", "pti_numrunning", "pti_priority",
    )]


class _ProcBsdInfo(ctypes.Structure):
    # Public Darwin proc_bsdinfo; start time distinguishes successive PID owners.
    _fields_ = [(name, ctypes.c_uint32) for name in (
        "pbi_flags", "pbi_status", "pbi_xstatus", "pbi_pid", "pbi_ppid", "pbi_uid",
        "pbi_gid", "pbi_ruid", "pbi_rgid", "pbi_svuid", "pbi_svgid", "rfu_1",
    )] + [("pbi_comm", ctypes.c_char * 16), ("pbi_name", ctypes.c_char * 32)] + [
        (name, ctypes.c_uint32) for name in (
            "pbi_nfiles", "pbi_pgid", "pbi_pjobc", "e_tdev", "e_tpgid", "pbi_nice",
        )
    ] + [("pbi_start_tvsec", ctypes.c_uint64), ("pbi_start_tvusec", ctypes.c_uint64)]


def native_process_identity(pid):
    info = _ProcBsdInfo()
    proc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pidinfo
    proc.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    proc.restype = ctypes.c_int
    ctypes.set_errno(0)
    size = proc(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
    if size == 0 and ctypes.get_errno() == errno.ESRCH:
        return None
    if size != ctypes.sizeof(info) or info.pbi_pid != pid:
        raise ValueError(f"native_process_identity_unproven:pid={pid}:bytes={size}:errno={ctypes.get_errno()}")
    return {"pid": pid, "start_sec": info.pbi_start_tvsec, "start_usec": info.pbi_start_tvusec}


def parse_terminal_coalition(listing, owner_pid, coalition_id):
    records = parse_launchservices_records(listing)
    owner = next((record for record in records if record["pid"] == owner_pid), None)
    if owner is not None and owner["coalition_id"] != coalition_id:
        raise ValueError("ownership_changed:coalition_id")
    matching = [record for record in records if record["coalition_id"] == coalition_id]
    if any(not record["pids"] for record in matching):
        raise ValueError("incomplete_terminal_coalition")
    members = {pid for record in matching for pid in record["pids"]}
    if any(members.intersection(record["pids"]) or record["pid"] in members
           for record in records if record["coalition_id"] not in (None, coalition_id)):
        raise ValueError("overlapping_coalition_members")
    return {"coalition_id": coalition_id, "pids": tuple(sorted(members))}


def validate_terminal_catalog(listing, instance, known_members, absent_members):
    parsed = parse_terminal_coalition(listing, instance["owner_pid"], instance["coalition_id"])
    if not set(absent_members).issubset(known_members):
        raise ValueError("unproven_terminal_departures")
    for record in parse_launchservices_records(listing):
        if record["bundle_id"] not in {"com.apple.WebKit.GPU", "com.apple.WebKit.Networking", "com.apple.WebKit.WebContent"}:
            continue
        if record["name"] not in {instance["owner_name"] + suffix for suffix in (" Graphics and Media", " Networking", " Web Content")}:
            continue
        if record["pid"] in parsed["pids"]:
            continue
        if record["pid"] in absent_members and record["coalition_id"] is None:
            continue
        raise ValueError(f"webkit_process_not_in_unique_coalition: unowned WebKit pid={record['pid']}")
    return parsed


def confirm_terminal_departures(listing, instance, known_members):
    # Only a previously observed PID with a proven absent identity can explain a
    # stale WebKit catalog record. Reparse the entire listing to reject any
    # additional unknown/live member or contradictory coalition assignment.
    absent = sorted(pid for pid in known_members if native_process_identity(pid) is None)
    parsed = parse_launchservices_coalitions(
        listing, {instance["owner_pid"]}, require_webkit=False,
        allow_departed_host=True, departed_pids=absent)[instance["owner_pid"]]
    if parsed["coalition_id"] != instance["coalition_id"]:
        raise ValueError("ownership_changed:coalition_id")
    return absent


def sample_launchservices_members(pids):
    pids = tuple(pids)
    if not pids:
        raise ValueError("empty_coalition_process_set")
    if len(pids) != len(set(pids)):
        raise ValueError("duplicate_process_ids")
    if any(not isinstance(pid, int) or pid <= 0 for pid in pids):
        raise ValueError("invalid_process_ids")
    pids = tuple(sorted(pids))
    # Ownership remains the OS coalition's exact PID set. Query both values in
    # one public syscall per member instead of spawning two ps processes.
    proc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pidinfo
    proc.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    proc.restype = ctypes.c_int
    processes = []
    for pid in pids:
        info = _ProcTaskInfo()
        ctypes.set_errno(0)
        size = proc(pid, 4, 0, ctypes.byref(info), ctypes.sizeof(info))
        if size != ctypes.sizeof(info):
            error = ctypes.get_errno()
            if size == 0 and error == errno.ESRCH:
                raise ValueError(f"missing_processes:{[pid]}")
            raise ValueError(f"coalition_task_query_failed:pid={pid}:bytes={size}:errno={error}")
        # A complete kernel record can report zero RSS while exec replaces the
        # address space. Preserve that observation rather than inventing data.
        if info.pti_threadnum <= 0:
            raise ValueError(f"invalid_coalition_task_info:{pid}:resident_bytes={info.pti_resident_size}:threads={info.pti_threadnum}:virtual_bytes={info.pti_virtual_size}")
        processes.append({"pid": pid, "rss_mb": info.pti_resident_size / (1024 * 1024),
                          "threads": info.pti_threadnum})
    return {"pids": pids, "rss_mb": sum(process["rss_mb"] for process in processes),
            "threads": sum(process["threads"] for process in processes),
            "per_process": tuple(processes)}


def validate_launchservices_samples(samples, owner_pid, instance):
    if len(samples) < 2:
        raise ValueError("resource_sampling_does_not_cover_lifecycle")
    coalition_id = instance["coalition_id"]
    for sample in samples:
        pids = tuple(sample["pids"])
        if (sample["owner_pid"] != owner_pid or sample["coalition_id"] != coalition_id
                or owner_pid not in pids or len(pids) != len(set(pids))):
            raise ValueError("ownership_changed:coalition_identity_or_members")
        if tuple(process["pid"] for process in sample["per_process"]) != pids:
            raise ValueError("ownership_changed:sampled_process_set")
    if any(not 50 <= b["at_ms"] - a["at_ms"] <= 175 for a, b in zip(samples, samples[1:])):
        raise ValueError("resource_sampling_not_100ms")
    return samples


def verified_capture(document):
    """A live label alone cannot replace raw owned-process provenance."""
    capture = document.get("capture", {})
    gateway = document.get("gateway") or {}
    if capture.get("cleanup_error") or not capture.get("argv"):
        return False
    paths = [(capture.get("stdout"), capture.get("stdout_sha256")),
              (capture.get("stderr"), capture.get("stderr_sha256")),
             (capture.get("clock_file"), capture.get("clock_sha256")),
             (capture.get("terminal_file"), capture.get("terminal_sha256")),
             (capture.get("pid_file"), capture.get("pid_sha256")),
             (capture["argv"][0], capture.get("binary_sha256")),
             (gateway.get("binary"), gateway.get("binary_sha256"))]
    scope = document.get("resource_scope", {})
    if scope.get("complete") is not True or scope.get("kind") != "launchservices_coalition":
        return False
    paths += [(capture.get("ownership_file"), capture.get("ownership_sha256")),
              (capture.get("launcher_stdout"), capture.get("launcher_stdout_sha256")),
              (capture.get("launcher_stderr"), capture.get("launcher_stderr_sha256"))]
    if capture.get("bundle_provenance"):
        paths += [(capture.get("bundle_provenance"), capture.get("bundle_provenance_sha256")),
                  (capture.get("source_binary"), capture.get("source_binary_sha256"))]
    assets = gateway.get("panel_assets", {})
    if set(assets) != {"index.html", "aleph_panel.js", "aleph_panel_bg.wasm", "tailwind.css"}:
        return False
    paths += [(item.get("capture"), item.get("sha256")) for item in assets.values()]
    if not all(path and digest and Path(path).is_file()
               and file_sha256(path) == digest
               for path, digest in paths):
        return False
    if any(not isinstance(event.get("browser_at_ms"), (int, float))
           or not math.isfinite(event["browser_at_ms"]) or event["browser_at_ms"] < 0
           or not isinstance(event.get("browser_time_origin_ms"), (int, float))
           or not math.isfinite(event["browser_time_origin_ms"]) or event["browser_time_origin_ms"] <= 0
           or event["browser_time_origin_ms"] != document["events"][0]["browser_time_origin_ms"]
           for event in document["events"]):
        return False
    origin_ns = int(Path(capture["clock_file"]).read_text())
    pid_record = json.loads(Path(capture["pid_file"]).read_text())
    if pid_record != {"pid": scope["owner_pid"], "nonce": capture["nonce"]}:
        return False
    if any(event.get("native_clock_origin_ns") != origin_ns
           or not isinstance(event.get("native_clock_ns"), int)
           or not math.isclose(event.get("native_at_ms", -1), (event["native_clock_ns"] - origin_ns) / 1_000_000, abs_tol=0.001)
           for event in document["events"]):
        return False
    if any(not isinstance(sample.get("sample_monotonic_ns"), int)
           or not math.isclose(sample["at_ms"], (sample["sample_monotonic_ns"] - origin_ns) / 1_000_000, abs_tol=0.001)
           for sample in document["resources"]):
        return False
    # Reconstruct the exact marker sequence; binary/log hashes alone cannot bind
    # a separately edited event array to the owned process's observations.
    markers = []
    for line in Path(capture["stdout"]).read_bytes().splitlines():
        if not line.startswith(b"ALEPH_PANEL_QA "):
            continue
        event = json.loads(line[len(b"ALEPH_PANEL_QA "):])
        if event.pop("nonce", None) != capture.get("nonce") or event.get("qa_run_id") != document["qa_run_id"]:
            continue
        markers.append(event)
    if markers != [{key: value for key, value in event.items() if key != "at_ms"}
                   for event in document["events"]]:
        return False
    owner_pid = capture.get("pid")
    if scope.get("owner_pid") != owner_pid or scope.get("coalition_id") is None:
        return False
    try:
        if capture.get("bundle_provenance"):
            provenance = json.loads(Path(capture["bundle_provenance"]).read_text())
            if provenance["source_sha256"] != capture["source_binary_sha256"] or provenance["executable_sha256"] != capture["binary_sha256"]:
                return False
        ownership = [json.loads(line) for line in Path(capture["ownership_file"]).read_text().splitlines()]
        if not ownership:
            return False
        initial = parse_launchservices_coalitions(ownership[0]["listing"], {owner_pid}, require_webkit=False)[owner_pid]
        catalog_instance = {"owner_pid": owner_pid, "coalition_id": scope["coalition_id"], "owner_name": initial["owner_name"]}
        terminal = json.loads(Path(capture["terminal_file"]).read_text())
        native = terminal["native_exit"]
        settlement = terminal["settlement"]
        expected_members = {pid for sample in document["resources"] for pid in sample["pids"]}
        if not terminal.get("tail_ownership"):
            return False
        for snapshot in terminal["tail_ownership"]:
            if (hashlib.sha256(snapshot["listing"].encode()).hexdigest() != snapshot["listing_sha256"]
                    or not origin_ns <= snapshot["observed_at_ns"] <= settlement["snapshots"][-1]["at_monotonic_ns"]):
                return False
            tail = validate_terminal_catalog(snapshot["listing"], catalog_instance, expected_members,
                                             snapshot.get("confirmed_absent_members", ()))
            expected_members.update(tail["pids"])
        if (native["pid"] != owner_pid or not native["fflags"] & 0x04000000
                or os.waitstatus_to_exitcode(native["wait_status"]) != native["exit_code"]
                or native["exit_code"] != 0 or document["direct_child"]["exit_code"] != native["exit_code"]
                or capture.get("helper_exit_code") != 0 or settlement["survivors"]
                or not expected_members.issubset(set(settlement["members"]))
                or not settlement["snapshots"] or settlement["snapshots"][-1]["survivors"]):
            return False
        settled_ns = settlement["snapshots"][-1]["at_monotonic_ns"]
        if scope.get("settled_at_ns") != settled_ns or not math.isclose(
                scope.get("settled_at_ms", -1), (settled_ns - origin_ns) / 1_000_000, abs_tol=0.001):
            return False
        helper_records = [json.loads(line) for line in Path(capture["launcher_stdout"]).read_text().splitlines()]
        launch, helper = helper_records
        if launch.get("pid") != owner_pid or helper.get("pid") != owner_pid or helper.get("terminated") is not True \
                or helper.get("closed_marker_seen") is not True or helper.get("timed_out") is not False \
                or helper.get("forced") is not False:
            return False
        if len(ownership) != scope.get("ownership_sample_count") or len(ownership) != len(document["resources"]):
            return False
        validate_launchservices_samples(document["resources"], owner_pid, {"coalition_id": scope["coalition_id"]})
        for snapshot, sample in zip(ownership, document["resources"]):
            if (hashlib.sha256(snapshot["listing"].encode()).hexdigest() != snapshot["listing_sha256"]
                    or snapshot["at_ms"] != sample["at_ms"] or snapshot.get("resource") != sample
                    or not math.isclose(sample["rss_mb"], sum(p["rss_mb"] for p in sample["per_process"]), rel_tol=1e-12)
                    or sample["threads"] != sum(p["threads"] for p in sample["per_process"])):
                return False
            instance = parse_launchservices_coalitions(snapshot["listing"], {owner_pid}, require_webkit=False)[owner_pid]
            if instance["coalition_id"] != sample["coalition_id"] or instance["pids"] != tuple(sample["pids"]):
                return False
    except (KeyError, OSError, ValueError, TypeError, json.JSONDecodeError):
        return False
    return True


def collect_process(argv, cwd, env, qa_run_id, nonce, surface, gateway, prefix, timeout):
    """Collect an owned native process; external daemons are never its children.

    POSIX descendants cannot establish ownership of WebKit's XPC processes.
    Keep that resource limit explicit until the host supplies owned PID evidence.
    """
    prefix.parent.mkdir(parents=True, exist_ok=True)
    stdout = Path(str(prefix) + ".stdout.log")
    stderr = Path(str(prefix) + ".stderr.log")
    events, resources = [], []
    started = time.monotonic()
    forced = False
    pending = b""
    cleanup_error = None
    with stdout.open("wb", buffering=0) as out, stderr.open("wb") as err:
        process = subprocess.Popen(argv, cwd=cwd, env=os.environ | env, stdout=subprocess.PIPE, stderr=err, start_new_session=True)
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        previous = signal.getsignal(signal.SIGTERM)

        def terminate(signum, _frame):
            raise SystemExit(128 + signum)

        signal.signal(signal.SIGTERM, terminate)
        next_sample = started
        try:
            while process.poll() is None or selector.get_map():
                now = time.monotonic()
                if now - started >= timeout:
                    forced = True
                    break
                if now >= next_sample and process.poll() is None:
                    rss = subprocess.run(["ps", "-p", str(process.pid), "-o", "rss="], capture_output=True, text=True)
                    threads = subprocess.run(["ps", "-M", "-p", str(process.pid), "-o", "pid="], capture_output=True, text=True)
                    if rss.returncode == 0 and rss.stdout.strip() and threads.returncode == 0:
                        resources.append({"at_ms": (now - started) * 1000, "rss_mb": int(rss.stdout.strip()) / 1024, "threads": len(threads.stdout.splitlines())})
                    next_sample += 0.1
                for key, _ in selector.select(max(0, min(0.1, next_sample - time.monotonic()))):
                    chunk = os.read(key.fd, 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    out.write(chunk)
                    pending += chunk
                    if len(pending) > 1048576:
                        raise ValueError("unterminated_marker_exceeds_1MiB")
                    while b"\n" in pending:
                        line, pending = pending.split(b"\n", 1)
                        if not line.startswith(b"ALEPH_PANEL_QA "):
                            continue
                        event = json.loads(line[len(b"ALEPH_PANEL_QA "):])
                        if event.pop("nonce", None) != nonce or event.get("qa_run_id") != qa_run_id:
                            continue
                        event["at_ms"] = (time.monotonic() - started) * 1000
                        events.append(event)
        finally:
            selector.close()
            if process.poll() is None:
                forced = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            process.stdout.close()
            try:
                os.killpg(process.pid, 0)
                forced = True
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError as error:
                cleanup_error = str(error)
            signal.signal(signal.SIGTERM, previous)
    return {
        "version": 1, "surface": surface, "origin": "live_gateway" if gateway else "protocol_fixture",
        "qa_run_id": qa_run_id, "gateway": gateway, "events": events, "resources": resources,
        "direct_child": {"exit_code": process.returncode, "forced": forced},
        "resource_scope": {"complete": False, "kind": "direct_child_only", "reason": "WebKit XPC process ownership is not inferred from POSIX parentage"},
        "capture": {"argv": argv, "cwd": str(cwd), "pid": process.pid, "nonce": nonce, "cleanup_error": cleanup_error,
                    "binary_sha256": file_sha256(argv[0]),
                    "stdout": str(stdout), "stderr": str(stderr),
                    "stdout_sha256": hashlib.sha256(stdout.read_bytes()).hexdigest(),
                    "stderr_sha256": hashlib.sha256(stderr.read_bytes()).hexdigest()},
    }


def ensure_launchservices_bundle(repo_root, surface, target=None):
    launcher_root = Path.home() / ".phase9-native-launchers"
    bundle = launcher_root / "phase9-nsh-probe.app"
    executable = bundle / "Contents" / "MacOS" / "Phase9Launcher"
    executable.parent.mkdir(parents=True, exist_ok=True)
    compiled = subprocess.run(["/usr/bin/clang", "-O2", "-Wall", "-Wextra", "-Werror",
                              str(repo_root / "qa/phase9_app_launch_wrapper.c"), "-o", str(executable)],
                             capture_output=True, text=True, timeout=30)
    if compiled.returncode:
        raise ValueError(f"phase9_wrapper_compile_failed:{compiled.stderr.strip()}")
    contents = bundle / "Contents"
    if target is None:
        raise ValueError("nsh_bundle_requires_target")
    shutil.copy2(target, contents / "MacOS" / "NativeShell")
    contents.mkdir(parents=True, exist_ok=True)
    name = "Aleph NSH Phase9 Probe"
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump({"CFBundleExecutable": executable.name, "CFBundleIdentifier": f"org.aleph.phase9.{surface}.probe",
                       "CFBundleName": name, "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "0.1",
                       "CFBundleVersion": "0.1", "LSMinimumSystemVersion": "13.3", "NSHighResolutionCapable": True,
                       "NSMicrophoneUsageDescription": "Controlled Phase9 Panel runtime probe.",
                       "NSLocalNetworkUsageDescription": "Controlled Phase9 Panel runtime probe.",
                       "NSAppTransportSecurity": {"NSAllowsArbitraryLoads": True}}, stream, sort_keys=True)
    signed = subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(bundle)],
                            capture_output=True, text=True, timeout=30)
    if signed.returncode:
        raise ValueError(f"phase9_wrapper_sign_failed:{signed.stderr.strip()}")
    provenance = {"source": str(target), "source_sha256": file_sha256(target),
                  "executable": str(bundle / "Contents/MacOS" / "NativeShell"),
                  "signing": "codesign --force --deep --sign -"}
    provenance["executable_sha256"] = file_sha256(provenance["executable"])
    bundle.with_suffix(".source.json").write_text(json.dumps(provenance, indent=2) + "\n")
    return bundle


def watch_native_exit(pid):
    # Public sys/event.h permits NOTE_EXITSTATUS for a same-user signalable PID.
    watcher = select.kqueue()
    watcher.control([select.kevent(pid, filter=select.KQ_FILTER_PROC,
                                  flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                  fflags=select.KQ_NOTE_EXIT | 0x04000000)], 0, 0)
    return watcher


def native_exit_status(watcher, pid, timeout):
    events = watcher.control(None, 1, timeout)
    if len(events) != 1 or events[0].ident != pid or not events[0].fflags & 0x04000000:
        raise ValueError("native_exit_status_unproven")
    event = events[0]
    return {"pid": pid, "fflags": event.fflags, "wait_status": event.data,
            "exit_code": os.waitstatus_to_exitcode(event.data)}


def settle_native_members(pids, timeout=5):
    deadline = time.monotonic() + timeout
    snapshots = []
    while True:
        survivors = []
        for pid in sorted(pids):
            try:
                os.kill(pid, 0)
                survivors.append(pid)
            except ProcessLookupError:
                pass
        snapshots.append({"at_monotonic_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC), "survivors": survivors})
        if not survivors or time.monotonic() >= deadline:
            return {"members": sorted(pids), "survivors": survivors, "snapshots": snapshots}
        time.sleep(0.01)


def cleanup_owned_members(identities, timeout=2):
    """Failure cleanup rechecks recorded process identity before every signal."""
    actions, snapshots, errors = [], [], []

    def observe():
        survivors, changed = [], []
        for pid, expected in sorted(identities.items()):
            try:
                actual = native_process_identity(pid)
            except (OSError, ValueError) as error:
                errors.append({"pid": pid, "reason": str(error)})
                survivors.append(pid)
                continue
            if actual == expected:
                survivors.append(pid)
            elif actual is not None:
                changed.append({"expected": expected, "actual": actual})
        snapshot = {"at_monotonic_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC),
                    "survivors": survivors, "identity_changed": changed}
        snapshots.append(snapshot)
        return snapshot

    snapshot = observe()
    for requested_signal in (signal.SIGTERM, signal.SIGKILL):
        if not snapshot["survivors"]:
            break
        for pid in snapshot["survivors"]:
            try:
                actual = native_process_identity(pid)
                if actual != identities[pid]:
                    actions.append({"pid": pid, "signal": requested_signal, "sent": False, "reason": "identity_changed_or_absent"})
                    continue
                os.kill(pid, requested_signal)
                actions.append({"pid": pid, "signal": requested_signal, "sent": True, "identity": actual})
            except ProcessLookupError:
                actions.append({"pid": pid, "signal": requested_signal, "sent": False, "reason": "absent"})
            except (OSError, ValueError) as error:
                errors.append({"pid": pid, "reason": str(error)})
        deadline = time.monotonic() + timeout
        while True:
            snapshot = observe()
            if not snapshot["survivors"] or time.monotonic() >= deadline:
                break
            time.sleep(0.01)
    return {"members": list(identities.values()), "actions": actions, "snapshots": snapshots,
            "survivors": snapshot["survivors"], "errors": errors,
            "complete": not snapshot["survivors"] and not errors}


def native_capture_closed(stdout, owner_pid, nonce, qa_run_id):
    if not stdout.is_file() or owner_pid <= 0:
        return False
    markers = [json.loads(line[15:]) for line in stdout.read_bytes().split(b"\n")[:-1]
               if line.startswith(b"ALEPH_PANEL_QA ")]
    if not any(event.get("nonce") == nonce and event.get("qa_run_id") == qa_run_id
               and event.get("kind") == "closed" and event.get("payload", {}).get("owned_drained") is True
               for event in markers):
        return False
    try:
        os.kill(owner_pid, 0)
    except ProcessLookupError:
        return True
    return False


def await_native_capture_closed(stdout, owner_pid, nonce, qa_run_id, deadline):
    # A listed XPC process can disappear before ps while its host is exiting.
    # Resolve that boundary only inside the last real sample's coverage budget.
    while time.monotonic() < deadline:
        if native_capture_closed(stdout, owner_pid, nonce, qa_run_id):
            return True
        time.sleep(min(0.002, max(0, deadline - time.monotonic())))
    return False


def accept_owned_coalition(parsed, instance, owner_identity, members, identities, *, departing=False, identity_observations=None):
    """Validate the pinned owner before extending authority to clean up members."""
    if parsed["coalition_id"] != instance["coalition_id"]:
        raise ValueError("ownership_changed:coalition_id")
    actual_owner = native_process_identity(instance["owner_pid"])
    if actual_owner is None and not departing:
        raise ValueError(f"missing_processes:{[instance['owner_pid']]}")
    if actual_owner != owner_identity and not (departing and actual_owner is None):
        raise ValueError("ownership_changed:owner_identity")
    members.update(parsed["pids"])
    for pid in parsed["pids"]:
        if pid not in identities:
            for attempt in range(10):
                try:
                    identity = native_process_identity(pid)
                except ValueError as error:
                    if identity_observations is not None:
                        identity_observations.append({"pid": pid, "at_monotonic_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC), "reason": str(error)})
                    if str(error) != f"native_process_identity_unproven:pid={pid}:bytes=0:errno=1" or attempt == 9:
                        raise
                    time.sleep(0.002)
                    continue
                if attempt and identity_observations is not None:
                    identity_observations.append({"pid": pid, "at_monotonic_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC), "identity": identity})
                if identity is not None:
                    identities[pid] = identity
                break


def collect_launchservices_process(argv, cwd, env, qa_run_id, nonce, surface, gateway, prefix, timeout, bundle):
    if sys.platform != "darwin":
        raise ValueError("launchservices_resource_sampling_requires_macos")
    prefix.parent.mkdir(parents=True, exist_ok=True)
    # LaunchServices children write locally before evidence is archived.
    native_capture = Path(tempfile.mkdtemp(prefix="aleph-phase9-capture-"))
    stdout = native_capture / "stdout.log"
    stderr = native_capture / "stderr.log"
    helper_stderr = Path(str(prefix) + ".launcher.stderr.log")
    helper_stdout = Path(str(prefix) + ".launcher.stdout.jsonl")
    ownership_path = native_capture / "launchservices.jsonl"
    helper = Path(__file__).with_name("phase9_nsworkspace_launcher.swift")
    launched_binary = bundle / "Contents/MacOS/NativeShell" if surface == "nsh" else bundle / "Contents/MacOS/Aleph"
    provenance = json.loads(bundle.with_suffix(".source.json").read_text())
    if provenance["source_sha256"] != file_sha256(argv[0]) or provenance["executable_sha256"] != file_sha256(launched_binary):
        raise ValueError("native_bundle_binary_mismatch")
    clock_path = native_capture / "clock-origin-ns"
    gate_path = native_capture / "start-gate"
    pid_path = native_capture / "native-pid.json"
    env = env | {"ALEPH_PANEL_QA_START_GATE": str(gate_path), "ALEPH_PANEL_QA_GATE_NONCE": nonce,
                 "ALEPH_PANEL_QA_CLOCK": str(clock_path), "ALEPH_PANEL_QA_PID": str(pid_path)}
    command = ["/usr/bin/swift", str(helper), str(bundle), str(launched_binary), str(stdout), str(stderr),
               json.dumps(env), json.dumps(argv[1:]), str(timeout)]
    ownership_samples, resources, events, roles_seen = [], [], [], set()
    instance, owner_pid = None, None
    exit_watcher = None
    owned_members = set()
    owned_identities = {}
    tail_ownership = []
    terminal_path = native_capture / "terminal.json"
    terminal = None
    started = None
    offset, pending = 0, b""
    helper_tail = ""
    stage = "launch"
    latest_listing_snapshot = None

    def drain_markers():
        nonlocal offset, pending
        if stdout.exists():
            with stdout.open("rb") as marker_file:
                marker_file.seek(offset)
                pending += marker_file.read()
                offset = marker_file.tell()
        while b"\n" in pending:
            line, pending = pending.split(b"\n", 1)
            if not line.startswith(b"ALEPH_PANEL_QA "):
                continue
            event = json.loads(line[15:])
            if event.pop("nonce", None) != nonce or event.get("qa_run_id") != qa_run_id:
                continue
            origin_ns = int(clock_path.read_text())
            event["at_ms"] = (time.clock_gettime_ns(time.CLOCK_MONOTONIC) - origin_ns) / 1_000_000
            events.append(event)

    def retain_terminal_snapshot(snapshot):
        snapshot["confirmed_absent_members"] = sorted(
            pid for pid in owned_members if native_process_identity(pid) is None)
        parsed = validate_terminal_catalog(snapshot["listing"], instance, owned_members,
                                           snapshot["confirmed_absent_members"])
        accept_owned_coalition(parsed, instance, owned_identities[owner_pid],
                               owned_members, owned_identities, departing=True,
                               identity_observations=snapshot.setdefault("identity_observations", []))
        tail_ownership.append(snapshot | {"observed_at_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC)})

    def resolve_terminal(error, snapshot):
        disappeared = str(error).startswith(("missing_launchservices_hosts:", "missing_processes:", "missing_thread_records:", "host_left_coalition:"))
        remnant = str(error).startswith("webkit_process_not_in_unique_coalition: unowned WebKit pid=")
        if not resources or not (disappeared or remnant):
            return False
        if remnant:
            snapshot["confirmed_absent_members"] = confirm_terminal_departures(
                snapshot["listing"], instance, owned_members)
        deadline = started + resources[-1]["at_ms"] / 1000 + 0.175
        wait_started = time.monotonic()
        resolved = await_native_capture_closed(stdout, owner_pid, nonce, qa_run_id, deadline)
        with (native_capture / "terminal-race.jsonl").open("a") as race:
            race.write(json.dumps({"reason": str(error), "failed_snapshot": snapshot,
                                   "wait_started": wait_started, "wait_finished": time.monotonic(),
                                   "deadline": deadline, "resolved": resolved}) + "\n")
        if resolved:
            retain_terminal_snapshot(snapshot)
        return resolved

    with helper_stderr.open("wb") as helper_err:
        process = subprocess.Popen(command, cwd=cwd, stdout=subprocess.PIPE, stderr=helper_err, text=True,
                                   start_new_session=True, bufsize=1)
        try:
            pid_deadline = time.monotonic() + 45
            while not pid_path.exists() and process.poll() is None and time.monotonic() < pid_deadline:
                time.sleep(0.002)
            if not pid_path.exists():
                raise ValueError("native_pid_rendezvous_failed")
            launch = json.loads(pid_path.read_text())
            owner_pid = launch.get("pid")
            if launch.get("nonce") != nonce or not isinstance(owner_pid, int) or owner_pid <= 0:
                raise ValueError("launchservices_missing_host_pid")
            exit_watcher = watch_native_exit(owner_pid)
            owned_members.add(owner_pid)
            identity = native_process_identity(owner_pid)
            if identity is not None:
                owned_identities[owner_pid] = identity
            started = time.monotonic()
            next_sample = started
            with ownership_path.open("w", encoding="utf-8") as ownership_file:
                while process.poll() is None:
                    drain_markers()
                    if resources and any(event["kind"] == "closed" for event in events) and native_capture_closed(stdout, owner_pid, nonce, qa_run_id):
                        break
                    now = time.monotonic()
                    if now - started >= timeout + 70:
                        raise ValueError("launchservices_helper_timeout")
                    if now < next_sample:
                        time.sleep(min(0.01, next_sample - now))
                        continue
                    drain_markers()
                    listing_started_ns = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                    listing = subprocess.run(["/usr/bin/lsappinfo", "list"], capture_output=True,
                                             text=True, timeout=10)
                    listing_finished_ns = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                    if listing.returncode:
                        raise ValueError(f"launchservices_list_failed:{listing.stderr.strip()}")
                    snapshot = {"at_ms": (time.monotonic() - started) * 1000,
                                "listing_sha256": hashlib.sha256(listing.stdout.encode()).hexdigest(),
                                "listing": listing.stdout}
                    latest_listing_snapshot = snapshot
                    stage = "resource_sample"
                    try:
                        parsed = parse_launchservices_coalitions(listing.stdout, {owner_pid}, require_webkit=False)[owner_pid]
                    except ValueError as error:
                        with (native_capture / "startup.jsonl").open("a") as startup:
                            startup.write(json.dumps(snapshot, allow_nan=False) + "\n")
                        if resolve_terminal(error, snapshot):
                            drain_markers()
                            break
                        if not resources and time.monotonic() - started < 0.175:
                            next_sample += 0.05
                            continue
                        raise
                    if instance is None:
                        instance = {"owner_pid": owner_pid, "coalition_id": parsed["coalition_id"],
                                    "owner_name": parsed["owner_name"]}
                    try:
                        accept_owned_coalition(parsed, instance, owned_identities[owner_pid],
                                              owned_members, owned_identities,
                                              identity_observations=snapshot.setdefault("identity_observations", []))
                    except ValueError as error:
                        if resolve_terminal(error, snapshot):
                            drain_markers()
                            break
                        raise
                    sample_time = time.monotonic()
                    sample_ns = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                    try:
                        sampled = sample_launchservices_members(parsed["pids"])
                    except ValueError as error:
                        if resolve_terminal(error, snapshot):
                            drain_markers()
                            break
                        raise
                    at_ms = (sample_time - started) * 1000
                    sample = {"at_ms": at_ms, "sample_monotonic_ns": sample_ns, "owner_pid": owner_pid, "coalition_id": parsed["coalition_id"],
                              "pids": list(parsed["pids"]), "rss_mb": sampled["rss_mb"],
                              "threads": sampled["threads"], "per_process": list(sampled["per_process"])}
                    resources.append(sample)
                    snapshot["at_ms"] = at_ms
                    snapshot["resource"] = sample
                    snapshot["timing"] = {"listing_started_ns": listing_started_ns,
                                          "listing_finished_ns": listing_finished_ns,
                                          "sampling_finished_ns": time.clock_gettime_ns(time.CLOCK_MONOTONIC)}
                    ownership_samples.append(snapshot)
                    ownership_file.write(json.dumps(snapshot, allow_nan=False) + "\n")
                    ownership_file.flush()
                    if not gate_path.exists():
                        gate_path.write_text(nonce)
                    current_roles = {bundle_id for bundle_id, pids in parsed["webkit"].items() if pids}
                    roles_seen.update(current_roles)

                    drain_markers()
                    # Keep a delayed query from triggering a catch-up sample within 50 ms.
                    next_sample = max(next_sample + 0.1, time.monotonic() + 0.05)
                stage = "owned_settlement"
                if not native_capture_closed(stdout, owner_pid, nonce, qa_run_id):
                    raise ValueError("native_close_not_confirmed")
                final_listing = subprocess.run(["/usr/bin/lsappinfo", "list"], capture_output=True,
                                               text=True, timeout=10)
                if final_listing.returncode:
                    raise ValueError(f"launchservices_list_failed:{final_listing.stderr.strip()}")
                latest_listing_snapshot = {"listing": final_listing.stdout,
                                           "listing_sha256": hashlib.sha256(final_listing.stdout.encode()).hexdigest()}
                retain_terminal_snapshot(latest_listing_snapshot)
                terminal = {"native_exit": native_exit_status(exit_watcher, owner_pid, 5),
                            "tail_ownership": tail_ownership,
                            "settlement": settle_native_members(owned_members)}
                terminal_path.write_text(json.dumps(terminal, allow_nan=False) + "\n")
                if terminal["settlement"]["survivors"]:
                    raise ValueError("owned_coalition_members_survived")
                process.wait(timeout=10)
                helper_tail = process.stdout.read()
                launch_line, separator, helper_tail = helper_tail.partition("\n")
                launch_line += separator
                if json.loads(launch_line).get("pid") != owner_pid:
                    raise ValueError("launchservices_native_pid_mismatch")
            drain_markers()
            if pending.strip():
                raise ValueError("unterminated_native_marker")
            if not roles_seen == {"com.apple.WebKit.GPU", "com.apple.WebKit.Networking", "com.apple.WebKit.WebContent"}:
                raise ValueError("incomplete_webkit_process_set")
            origin_ns = int(clock_path.read_text())
            for snapshot, sample in zip(ownership_samples, resources):
                sample["at_ms"] = (sample["sample_monotonic_ns"] - origin_ns) / 1_000_000
                snapshot["at_ms"] = sample["at_ms"]
            ownership_path.write_text("".join(json.dumps(snapshot, allow_nan=False) + "\n" for snapshot in ownership_samples))
            validate_launchservices_samples(resources, owner_pid, instance)
        except BaseException as error:
            (native_capture / "error.json").write_text(json.dumps({"exception": type(error).__name__, "reason": str(error),
                                                                   "stage": stage, "traceback": traceback.format_exc(),
                                                                   "latest_listing": latest_listing_snapshot}) + "\n")
            try:
                cleanup = cleanup_owned_members(owned_identities)
                cleanup["unregistered_members"] = [
                    {"pid": pid, "current_identity": native_process_identity(pid)}
                    for pid in sorted(owned_members - owned_identities.keys())
                ]
                cleanup["complete"] = cleanup["complete"] and not any(
                    member["current_identity"] is not None for member in cleanup["unregistered_members"]
                )
                (native_capture / "failure-cleanup.json").write_text(json.dumps(cleanup, allow_nan=False) + "\n")
            except Exception as cleanup_error:
                (native_capture / "failure-cleanup.json").write_text(json.dumps({
                    "complete": False, "reason": str(cleanup_error), "traceback": traceback.format_exc(),
                    "members": list(owned_identities.values()),
                }) + "\n")
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            raise
        finally:
            if exit_watcher is not None:
                exit_watcher.close()
            if not helper_tail and process.poll() is not None:
                helper_tail = process.stdout.read()
            helper_stdout.write_text((launch_line if 'launch_line' in locals() else '') + helper_tail, encoding="utf-8")
            process.stdout.close()
            for source, suffix in [(stdout, ".stdout.log"), (stderr, ".stderr.log"), (ownership_path, ".launchservices.jsonl"),
                                   (native_capture / "startup.jsonl", ".startup.jsonl"),
                                   (native_capture / "error.json", ".error.json"),
                                   (native_capture / "failure-cleanup.json", ".failure-cleanup.json"),
                                   (terminal_path, ".terminal.json"),
                                   (clock_path, ".clock-origin-ns"),
                                   (pid_path, ".native-pid.json"),
                                   (native_capture / "terminal-race.jsonl", ".terminal-race.jsonl")]:
                Path(str(prefix) + suffix).write_bytes(source.read_bytes() if source.exists() else b"")
            shutil.rmtree(native_capture)
    stdout = Path(str(prefix) + ".stdout.log")
    stderr = Path(str(prefix) + ".stderr.log")
    ownership_path = Path(str(prefix) + ".launchservices.jsonl")
    raw_stdout = stdout.read_bytes() if stdout.is_file() else b""
    raw_stderr = stderr.read_bytes() if stderr.is_file() else b""
    helper_stdout.write_text(launch_line + helper_tail, encoding="utf-8")
    helper_records = [json.loads(line) for line in helper_tail.splitlines() if line.startswith("{")]
    helper_record = helper_records[-1] if helper_records else {}
    return {
        "version": 1, "surface": surface, "origin": "live_gateway", "qa_run_id": qa_run_id,
        "gateway": gateway, "events": events, "resources": resources,
        "direct_child": {"exit_code": terminal["native_exit"]["exit_code"], "forced": helper_record.get("forced", True)},
        "resource_scope": {"complete": process.returncode == 0 and terminal["native_exit"]["exit_code"] == 0
                           and not terminal["settlement"]["survivors"] and helper_record.get("closed_marker_seen") is True
                           and helper_record.get("terminated") is True and helper_record.get("timed_out") is False
                           and helper_record.get("forced") is False, "kind": "launchservices_coalition",
                           "owner_pid": owner_pid, "coalition_id": instance["coalition_id"],
                           "settled_at_ns": terminal["settlement"]["snapshots"][-1]["at_monotonic_ns"],
                           "settled_at_ms": (terminal["settlement"]["snapshots"][-1]["at_monotonic_ns"] - origin_ns) / 1_000_000,
                           "ownership_sample_count": len(ownership_samples)},
        "capture": {"argv": [str(launched_binary)] + argv[1:], "cwd": str(cwd), "pid": owner_pid, "nonce": nonce, "cleanup_error": None,
                    "clock_file": str(Path(str(prefix) + ".clock-origin-ns")),
                    "pid_file": str(Path(str(prefix) + ".native-pid.json")),
                    "pid_sha256": file_sha256(Path(str(prefix) + ".native-pid.json")),
                    "clock_sha256": file_sha256(Path(str(prefix) + ".clock-origin-ns")),
                    "terminal_file": str(Path(str(prefix) + ".terminal.json")),
                    "terminal_sha256": file_sha256(Path(str(prefix) + ".terminal.json")),
                    "helper_exit_code": process.returncode,
                    "binary_sha256": file_sha256(launched_binary), "source_binary": argv[0],
                    "source_binary_sha256": file_sha256(argv[0]),
                    "bundle_provenance": str(bundle.with_suffix(".source.json")),
                    "bundle_provenance_sha256": file_sha256(bundle.with_suffix(".source.json")), "stdout": str(stdout), "stderr": str(stderr),
                    "stdout_sha256": hashlib.sha256(raw_stdout).hexdigest(),
                    "stderr_sha256": hashlib.sha256(raw_stderr).hexdigest(),
                    "ownership_file": str(ownership_path), "ownership_sha256": file_sha256(ownership_path),
                    "launcher_stdout": str(helper_stdout), "launcher_stdout_sha256": file_sha256(helper_stdout),
                    "launcher_stderr": str(helper_stderr), "launcher_stderr_sha256": file_sha256(helper_stderr)},
    }


def metric_document(results, allow_protocol_fixture=False):
    if len(results) != 40:
        raise ValueError("requires_10_warmups_and_30_measured_runs")
    fixture_only = allow_protocol_fixture and all(r["status"] == "PROTOCOL_VALID" for r in results)
    if not fixture_only and any(r.get("status") != "PASS" or not r.get("resource_complete") for r in results):
        raise ValueError("incomplete_live_resource_scope")
    return {"metrics": {name: [r["metrics"][name] for r in results] for name in nsh_panel_parity.METRICS}}


def export_parity(tauri, nsh, output, run_id, allow_protocol_fixture=False):
    """Connect validated runtime samples to the existing threshold reducer."""
    output.mkdir(parents=True, exist_ok=True)
    paths = [output / "tauri-metrics.json", output / "nsh-metrics.json"]
    for path, results in zip(paths, [tauri, nsh]):
        path.write_text(json.dumps(metric_document(results, allow_protocol_fixture), allow_nan=False) + "\n")
    receipt = output / "parity-receipt.json"
    result = subprocess.run([sys.executable, str(Path(__file__).with_name("nsh_panel_parity.py")),
                             "--baseline", str(paths[0]), "--candidate", str(paths[1]),
                             "--receipt", str(receipt), "--run-id", run_id], capture_output=True, text=True, timeout=30)
    return {"exit_code": result.returncode, "receipt": str(receipt), "result": json.loads(receipt.read_text())}


def launch_native_surfaces(repo_root, target_dir, fixture_root, receipt, gateway_receipt, timeout, runs):
    """Launch NSH against an explicitly isolated Gateway."""
    gateway_path = gateway_receipt or fixture_root / "gateway.json"
    if not gateway_path.is_file():
        return {"status": "BLOCKED", "reason": "isolated_gateway_receipt_missing", "path": str(gateway_path)}
    gateway = json.loads(gateway_path.read_text())
    endpoint = urlsplit(gateway.get("url", ""))
    if (endpoint.scheme != "http" or endpoint.hostname != "127.0.0.1" or not endpoint.port
            or endpoint.path not in ("", "/") or endpoint.query or endpoint.username
            or gateway.get("isolated") is not True):
        raise ValueError("gateway_must_be_an_explicitly_isolated_loopback_origin")
    if not gateway.get("binary") or file_sha256(gateway["binary"]) != gateway.get("binary_sha256"):
        raise ValueError("gateway_binary_provenance_mismatch")
    scenario = json.loads((fixture_root / "scenario.json").read_text())
    results = []
    output = receipt.parent / (receipt.stem + "-output")
    output.mkdir(parents=True, exist_ok=True)
    surface = "nsh"
    executable = target_dir / "debug" / "aleph-nsh-shell"
    if not executable.is_file():
        return {"status": "BLOCKED", "reason": "native_binary_missing", "path": str(executable), "runs": results}
    bundle = ensure_launchservices_bundle(repo_root, surface, executable)
    for index in range(runs):
        qa_id, nonce = uuid.uuid4().hex, os.urandom(32).hex()
        with tempfile.TemporaryDirectory(prefix="aleph-shell-qa-", dir=Path(tempfile.gettempdir()).resolve()) as directory:
            root = Path(directory).resolve()
            (root / ".qa-owned").write_text(nonce + "\n")
            env = {"ALEPH_PANEL_QA_RUN_ID": qa_id, "ALEPH_PANEL_QA_MESSAGE": scenario["message"]}
            argv = [str(executable)]
            env |= {"ALEPH_NSH_QA_URL": gateway["url"], "ALEPH_NSH_QA_NONCE": nonce}
            document = collect_launchservices_process(argv, repo_root / "desktop/nsh-shell",
                                                      env, qa_id, nonce, surface, gateway,
                                                      output / f"{surface}-{index}", timeout, bundle)
        document["fixture_cleanup"] = not root.exists()
        document_path = output / f"{surface}-{index}.json"
        document_path.write_text(json.dumps(document, indent=2) + "\n")
        try:
            result = evaluate_runtime(document)
        except ValueError as error:
            result = {"status": "FAIL", "reason": str(error)}
        result["runtime_document"] = str(document_path)
        results.append(result)
        if result["status"] != "PASS":
            break
    status = "PASS" if len(results) == runs and all(r["status"] == "PASS" for r in results) else "NOT_CLOSED"
    return {"status": status, "runs": results, "performance": {"status": "OPEN"}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime-json", type=Path)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--target-dir", type=Path, default=Path(".phase9-target"))
    parser.add_argument("--fixture-root", type=Path, default=Path("qa/fixtures/nsh-panel-parity"))
    parser.add_argument("--gateway-receipt", type=Path)
    parser.add_argument("--gateway-binary", type=Path, help="Own an isolated production Gateway for this invocation")
    parser.add_argument("--timeout-seconds", type=float, default=60)
    parser.add_argument("--surface", choices=["nsh"], help="Collect one surface against a fresh isolated Gateway")
    parser.add_argument("--runs", type=int, choices=[1, 40], default=1)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    def terminate(signum, _frame):
        raise SystemExit(128 + signum)
    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        if args.gateway_binary and args.runtime_json:
            raise ValueError("gateway_binary_cannot_be_used_with_a_saved_runtime_document")
        if args.gateway_binary:
            from nsh_panel_gateway import isolated_gateway
            gateway_output = (args.receipt.parent / (args.receipt.stem + "-gateway")).resolve()
            with isolated_gateway(args.gateway_binary.resolve(), gateway_output) as gateway_receipt:
                result = launch_native_surfaces(args.repo_root.resolve(), args.target_dir.resolve(), args.fixture_root.resolve(), args.receipt,
                                               gateway_receipt, args.timeout_seconds, args.runs)
            cleanup = json.loads((gateway_output / "gateway-cleanup.json").read_text())
            result["gateway_cleanup"] = cleanup
            if not cleanup["fixture_removed"] or any(child["forced"] for child in cleanup["children"]):
                result["status"] = "FAIL"
        else:
            result = evaluate_runtime(json.loads(args.runtime_json.read_text())) if args.runtime_json else launch_native_surfaces(
                args.repo_root.resolve(), args.target_dir.resolve(), args.fixture_root.resolve(), args.receipt,
                args.gateway_receipt, args.timeout_seconds, args.runs)
    except (ValueError, OSError, RuntimeError) as error:
        result = {"status": "FAIL", "reason": str(error)}
    finally:
        signal.signal(signal.SIGTERM, previous)
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result, allow_nan=False))
    return 0 if result["status"] == "PASS" else 3 if result["status"] == "BLOCKED" else 1


if __name__ == "__main__":
    raise SystemExit(main())
