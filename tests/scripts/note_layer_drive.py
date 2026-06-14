#!/usr/bin/env python3
"""note_layer_drive.py — orchestrate the note-layer E2E validation dialog.

For each JSONL row in --dialog, send agent.run with the row's prompt to the
gateway WebSocket. Captures the full response stream per turn into
${out_dir}/turn_NNN_<phase>_<intent>.jsonl, plus a transcript summary.

Optionally invokes tools/note_layer_probe.sh between phases (when the phase
prefix changes — e.g. transitioning A→B→C2→R2→final).

Usage:
    note_layer_drive.py \\
        --token aleph-... \\
        --agent-id test-note-layer-2026-05-04 \\
        --dialog tests/scripts/note_layer_e2e_dialog.jsonl \\
        --out /tmp/note-layer-e2e \\
        [--probe-between-phases] [--turn-timeout 45]
"""
import argparse
import asyncio
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

import websockets


async def one_turn(uri, token, agent_id, session_key, prompt, out_path, timeout, max_retries=3):
    """Send one agent.run, stream frames into out_path, return summary.

    Retries on rate-limit errors (-32002) up to max_retries, sleeping
    retry_after_ms before each retry.
    """
    frames = []
    final_text = ""
    error = None
    started = time.time()
    rate_limit_retries = 0

    try:
      while True:
        async with websockets.connect(uri, max_size=8 * 1024 * 1024) as ws:
            # 1. connect
            rpc_id = str(uuid.uuid4())
            await ws.send(json.dumps({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "method": "connect",
                "params": {
                    "shared_token": token,
                    "device_name": "note-layer-validator",
                    "device_id": "note-layer-validator",
                },
            }))
            ack = await asyncio.wait_for(ws.recv(), timeout=10)
            frames.append(ack)

            # 2. subscribe to events so we see tool calls
            await ws.send(json.dumps({
                "jsonrpc": "2.0",
                "id": str(uuid.uuid4()),
                "method": "events.subscribe",
                "params": {"topics": ["*"]},
            }))
            sub_ack = await asyncio.wait_for(ws.recv(), timeout=5)
            frames.append(sub_ack)

            # 3. agent.run — store rpc-id and the run_id assigned by gateway separately.
            request_rpc_id = str(uuid.uuid4())
            await ws.send(json.dumps({
                "jsonrpc": "2.0",
                "id": request_rpc_id,
                "method": "agent.run",
                "params": {
                    "agent_id": agent_id,
                    "session_key": session_key,
                    "input": prompt,
                },
            }))

            deadline = asyncio.get_event_loop().time() + timeout
            assigned_run_id = None
            request_failed = None
            terminal_seen = False
            while True:
                remaining = deadline - asyncio.get_event_loop().time()
                if remaining <= 0:
                    break
                try:
                    frame = await asyncio.wait_for(ws.recv(), timeout=remaining)
                except asyncio.TimeoutError:
                    break
                except websockets.ConnectionClosed:
                    break
                frames.append(frame)
                try:
                    parsed = json.loads(frame)
                except Exception:
                    continue

                # Immediate JSON-RPC response to our agent.run: NOT terminal
                # unless it's an error (rate-limit etc).
                if parsed.get("id") == request_rpc_id:
                    if "error" in parsed:
                        request_failed = parsed["error"]
                        break
                    if "result" in parsed and isinstance(parsed["result"], dict):
                        assigned_run_id = parsed["result"].get("run_id")
                    continue

                # Streamed terminal events from the assigned run
                method = parsed.get("method", "")
                params = parsed.get("params", {}) if isinstance(parsed.get("params"), dict) else {}
                if method in ("stream.run_complete", "stream.run_error") and \
                        assigned_run_id and params.get("run_id") == assigned_run_id:
                    terminal_seen = True
                    if method == "stream.run_complete":
                        s = params.get("summary", {})
                        if isinstance(s, dict):
                            final_text = s.get("final_response", "") or final_text
                    elif method == "stream.run_error":
                        error = params.get("error", "stream.run_error")
                    await asyncio.sleep(0.4)
                    break

            # Rate-limit retry handling
            if request_failed and isinstance(request_failed, dict) and request_failed.get("code") == -32002:
                if rate_limit_retries < max_retries:
                    msg = request_failed.get("message", "rate limited")
                    # Parse "retry after Xms" from message; fallback 12s
                    delay_ms = 12000
                    import re as _re
                    m = _re.search(r"retry after (\d+)ms", msg)
                    if m:
                        delay_ms = max(int(m.group(1)) + 250, 1000)
                    print(f"            rate-limited, sleeping {delay_ms}ms then retry "
                          f"(attempt {rate_limit_retries + 1}/{max_retries})", flush=True)
                    rate_limit_retries += 1
                    await asyncio.sleep(delay_ms / 1000.0)
                    continue  # retry the whole turn
                else:
                    error = f"rate-limit-exhausted: {request_failed}"
                    break

            if request_failed and not terminal_seen:
                error = request_failed
            break  # success or non-retryable error
    except Exception as e:
        error = repr(e)

    elapsed = time.time() - started
    with open(out_path, "w") as f:
        for fr in frames:
            f.write(fr if fr.endswith("\n") else fr + "\n")

    return {
        "elapsed_s": round(elapsed, 2),
        "frames_n": len(frames),
        "final_text_excerpt": final_text[:500],
        "error": error,
    }


def run_probe(probe_script, agent_id, out_dir, label):
    proc = subprocess.run(
        [probe_script, agent_id, str(out_dir / "probes"), label],
        capture_output=True, text=True
    )
    return {
        "label": label,
        "exit": proc.returncode,
        "stdout": proc.stdout.strip(),
        "stderr": proc.stderr.strip()[:500] if proc.stderr else "",
    }


async def main_async(args):
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "probes").mkdir(parents=True, exist_ok=True)
    (out_dir / "turns").mkdir(parents=True, exist_ok=True)

    transcript = []
    probe_log = []

    with open(args.dialog) as f:
        rows = [json.loads(line) for line in f if line.strip()]

    last_phase_prefix = None

    # Initial baseline probe
    if args.probe_between_phases:
        probe_log.append(run_probe(args.probe_script, args.agent_id, out_dir, "00_baseline"))

    session_key = f"agent:{args.agent_id}:dm:operator"

    for idx, row in enumerate(rows, start=1):
        phase = row.get("phase", "X")
        intent = row.get("intent", f"step{idx}")
        prompt = row["prompt"]

        # Detect phase boundary for probe
        phase_prefix = phase.split("-")[0]
        if args.probe_between_phases and last_phase_prefix and phase_prefix != last_phase_prefix:
            probe_log.append(run_probe(
                args.probe_script, args.agent_id, out_dir,
                f"{idx-1:02d}_after_{last_phase_prefix}"
            ))

        out_path = out_dir / "turns" / f"turn_{idx:02d}_{phase}_{intent}.jsonl"
        print(f"[turn {idx:02d}] phase={phase} intent={intent} → {out_path.name}", flush=True)

        result = await one_turn(
            args.uri, args.token, args.agent_id, session_key,
            prompt, out_path, args.turn_timeout,
        )
        transcript.append({
            "turn": idx, "phase": phase, "intent": intent,
            **result,
        })
        print(f"            elapsed={result['elapsed_s']}s frames={result['frames_n']} "
              f"err={result['error']}", flush=True)

        last_phase_prefix = phase_prefix

        # Brief pause between turns to let async writes settle
        await asyncio.sleep(args.inter_turn_pause)

    # Final probe
    if args.probe_between_phases:
        probe_log.append(run_probe(args.probe_script, args.agent_id, out_dir, "99_final"))

    # Write transcript
    with open(out_dir / "transcript.json", "w") as f:
        json.dump({"transcript": transcript, "probes": probe_log}, f, indent=2, ensure_ascii=False)

    print(f"\nTranscript: {out_dir / 'transcript.json'}", flush=True)
    print(f"Per-turn frames: {out_dir / 'turns'}", flush=True)
    print(f"Probes: {out_dir / 'probes'}", flush=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--token", required=True)
    p.add_argument("--agent-id", required=True)
    p.add_argument("--dialog", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--uri", default="ws://127.0.0.1:18790/ws")
    p.add_argument("--turn-timeout", type=float, default=45.0)
    p.add_argument("--inter-turn-pause", type=float, default=1.0)
    p.add_argument("--probe-between-phases", action="store_true")
    p.add_argument("--probe-script", default="tools/note_layer_probe.sh")
    args = p.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    sys.exit(main() or 0)
