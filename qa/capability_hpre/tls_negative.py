#!/usr/bin/env python3
"""TLS LAN identity controls for capability_projection_diagnostics.

Sub-commands (orchestrated by run.sh, always against a disposable profile):

  check-ip      the chosen LAN address is a real, assigned, non-loopback IPv4
  prepare       loopback phase: create a member and an admin user, mint bound tickets
  patch-config  switch the temporary profile to native TLS on exactly one LAN IP
  probe         redeem tickets over a real non-loopback socket and probe the tool

Verdicts follow the rest of this fixture: exit 0 PASS, 1 FAIL, 3 UNVERIFIED.
Nothing here trusts a forwarding header, a spoofed identity argument, or a
disabled certificate check: the client trusts only the profile's own cert.
"""
from __future__ import annotations

import argparse
import asyncio
import ipaddress
import json
import os
import re
import socket
import ssl
import sys
from pathlib import Path

import websockets

sys.path.insert(0, str(Path(__file__).resolve().parent))
import drive  # noqa: E402
from drive import DIAG, Conn, QA, provider_bodies, status_body, ws_connect  # noqa: E402


# ----------------------------------------------------------------- pure helpers

def lan_ip_problem(text, assigned=None):
    """Why ``text`` is not an acceptable single LAN bind address, else None."""
    try:
        ip = ipaddress.ip_address(text)
    except ValueError:
        return f"{text!r} is not an IP address"
    if ip.version != 4:
        return "only an IPv4 LAN address is supported"
    if ip.is_unspecified:
        return "wildcard bind is forbidden"
    if ip.is_loopback:
        return "loopback is not a LAN address"
    if assigned is not None and text not in assigned:
        return "address is not assigned to this host"
    return None


def assigned_ipv4():
    """IPv4 addresses actually assigned to this host (ifconfig, then bind test)."""
    import subprocess

    out = subprocess.run(["ifconfig"], capture_output=True, text=True).stdout
    return set(re.findall(r"\binet (\d+\.\d+\.\d+\.\d+)\b", out))


def bindable(text):
    """True when the kernel lets us bind this address (assignment cross-check)."""
    with socket.socket() as s:
        try:
            s.bind((text, 0))
            return True
        except OSError:
            return False


def patch_tls_config(text, host):
    """Set ``[gateway] host`` and ``[gateway.tls] enabled`` with insecure remote off.

    Existing ``[gateway.tls]`` tables are dropped first so there is no duplicate
    table; ``allow_insecure_remote`` is forced to false.  A wildcard host is
    refused here as a second line of defence.
    """
    problem = lan_ip_problem(host)
    if problem:
        raise ValueError(problem)
    out, skip, cur = [], False, None
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1)
            skip = cur == "gateway.tls"
        if skip:
            continue
        if cur == "gateway" and re.match(r"^\s*(host|allow_insecure_remote)\s*=", line):
            continue
        out.append(line)
        if m and cur == "gateway":
            out.append(f'host = "{host}"')
            out.append("allow_insecure_remote = false")
    body = "\n".join(out) + "\n"
    if f'host = "{host}"' not in body:
        body += f'\n[gateway]\nhost = "{host}"\nallow_insecure_remote = false\n'
    return body + "\n[gateway.tls]\nenabled = true\n"


_NOT_A_DENIAL = ("not found", "unknown tool", "unknown field", "invalid", "missing field", "unknown variant", "parse")


def denial_text(msg):
    return json.dumps(msg, sort_keys=True).lower() if isinstance(msg, dict) else ""


def is_gate_denial(msg, needle):
    """A real rejection whose text names the intended gate, not a parse/lookup error."""
    if not drive.is_rejection(msg):
        return False
    text = denial_text(msg)
    return needle in text and not any(bad in text for bad in _NOT_A_DENIAL)


def tool_results(bodies, tool_name):
    """(tool_use_seen, [tool_result text]) for ``tool_name`` across provider bodies."""
    ids, results, seen = set(), [], False
    for body in bodies:
        for message in body.get("messages", []) if isinstance(body, dict) else []:
            content = message.get("content") if isinstance(message, dict) else None
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use" and block.get("name") == tool_name:
                    seen = True
                    ids.add(block.get("id"))
                if block.get("type") == "tool_result" and block.get("tool_use_id") in ids:
                    inner = block.get("content")
                    text = inner if isinstance(inner, str) else json.dumps(inner)
                    results.append((bool(block.get("is_error")), text))
    return seen, results


def ambient_verdict(seen, results):
    """Classify the ordinary-agent probe: pass / unverified / fail plus detail."""
    if not seen:
        return "unverified", "the provider never saw a diagnostic tool_use in the run"
    if not results:
        return "unverified", "no tool_result for the diagnostic call reached the provider"
    is_error, text = results[0]
    low = text.lower()
    if any(bad in low for bad in _NOT_A_DENIAL):
        return "fail", f"tool_result is a parse/lookup error, not a gate denial: {text[:200]}"
    if "got none" in low and "operator" in low:
        return "pass", text[:200]
    if is_error or "not" in low and ("operator" in low or "loopback" in low):
        return "unverified", f"ambient context was present on the agent route (not missing): {text[:200]}"
    return "fail", f"diagnostic call was NOT denied on the ordinary agent route: {text[:200]}"


def snapshot_key(status):
    """Fields that must not move when a denied control call is attempted."""
    return {
        "lifecycle": status.get("lifecycle"),
        "registry_cursor": status.get("registry_cursor"),
        "applied_tool_ids": sorted(status.get("applied_tool_ids", [])),
        "applied_owner_generations": sorted(map(tuple, status.get("applied_owner_generations", []))),
    }


# ------------------------------------------------------------------- transports

def ssl_context(cafile):
    ctx = ssl.create_default_context(cafile=cafile)  # verification stays on
    return ctx


def connect_remote(host, port, cafile, **kw):
    return websockets.connect(f"wss://{host}:{port}/ws", ssl=ssl_context(cafile), max_size=None, ping_interval=None, **kw)


def peers(ws):
    local, remote = ws.local_address, ws.remote_address
    return str(local[0]), str(remote[0])


# --------------------------------------------------------------------- commands

async def cmd_prepare(args):
    q = QA()
    async with ws_connect(args.ws) as ws:
        conn = Conn(ws)
        hello = await conn.connect()
        q.receipt("loopback preparation connection is the local operator", (hello.get("result") or {}).get("role") == "operator", json.dumps(hello.get("result", {}))[:160])
        state = {}
        for label, role in (("member", "member"), ("admin", "admin")):
            made = await conn.call("users.create", {"display_name": f"qa-hpre-{label}-{args.run_id}", "role": role})
            user = (made.get("result") or {}).get("user") or {}
            q.receipt(f"{label} user created with account role {role}", user.get("user_id") and user.get("role") == role, json.dumps(user)[:200])
            if not user.get("user_id"):
                continue
            ticket = await conn.call("gateway.ticket.create", {"user_id": user["user_id"], "ttl_seconds": 900})
            value = (ticket.get("result") or {}).get("ticket")
            q.receipt(f"{label} user-bound bootstrap ticket minted", bool(value), "ticket redacted")
            state[label] = {"user_id": user["user_id"], "account_role": user.get("role"), "ticket": value, "device_id": f"qa-hpre-{label}-{args.run_id}"}
    if q.failures or len(state) != 2:
        print("VERDICT: FAIL (preparation)")
        return 1
    fd = os.open(args.state, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(state, fh)
    print("VERDICT: PASS (preparation)")
    return 0


async def redeem(q, label, host, port, cafile, entry, expected_role):
    ws = await connect_remote(host, port, cafile)
    try:
        local, remote = peers(ws)
        q.receipt(
            f"{label} socket is a real non-loopback LAN peer on both ends",
            local == host and remote == host and not ipaddress.ip_address(local).is_loopback,
            f"local={local} remote={remote}",
        )
        conn = Conn(ws)
        reply = await conn.call(
            "connect",
            {"client_type": "panel", "bootstrap_ticket": entry["ticket"], "device_id": entry["device_id"], "device_name": f"QA {label}"},
        )
        result = reply.get("result") or {}
        q.receipt(f"{label} ticket redeemed: device token returned", bool(result.get("device_token")), "token redacted; keys=" + ",".join(sorted(result)))
        q.receipt(f"{label} resolved gateway role is {expected_role}", result.get("role") == expected_role, f"role={result.get('role')}")
        return ws, conn, bool(result.get("device_token")) and result.get("role") == expected_role
    except BaseException:
        await ws.close()
        raise


async def cmd_probe(args):
    q = QA()
    state = json.loads(Path(args.state).read_text())
    host, port, cafile = args.host, args.port, args.cafile
    sockets = []
    try:
        member_ws, member, member_ok = await redeem(q, "member", host, port, cafile, state["member"], "member")
        sockets.append(member_ws)
        admin_ws, admin, admin_ok = await redeem(q, "admin", host, port, cafile, state["admin"], "operator")
        sockets.append(admin_ws)

        # Experiment, not assumption: a TLS client explicitly source-bound to the
        # real loopback address, connecting to the LAN-bound endpoint.
        local_conn, local_ok, local_why = None, False, ""
        try:
            local_ws = await asyncio.wait_for(connect_remote(host, port, cafile, local_addr=("127.0.0.1", 0)), timeout=10)
            sockets.append(local_ws)
            local_conn = Conn(local_ws)
            hello = await local_conn.connect()
            src, dst = peers(local_ws)
            local_ok = src == "127.0.0.1" and (hello.get("result") or {}).get("role") == "operator"
            local_why = f"src={src} dst={dst} role={(hello.get('result') or {}).get('role')}"
            q.receipt("loopback-source TLS client is a genuine local operator peer", local_ok, local_why)
        except Exception as exc:  # noqa: BLE001
            local_why = repr(exc)[:200]
        if not local_ok:
            q.gap("same-incarnation no-mutation receipt", f"loopback-source client to the LAN endpoint unavailable on this host: {local_why}")

        before = {}
        tool = ""
        if local_ok:
            ok, raw = await local_conn.invoke(DIAG, {"operation": "status"})
            before = status_body(raw)
            q.receipt("loopback-source operator reaches the strict diagnostic handler", ok and "registry_cursor" in before, json.dumps(before)[:200])
            name = args.prefix + "_tls"
            created = await local_conn.call("mcp_config.create", {"name": name, "config": drive.fixture_config(args, 0)})
            hits, _ = await drive.wait_for_catalog(local_conn, drive.server_id(name))
            q.receipt("loopback-source operator mounted a fixture tool for the no-mutation oracle", "error" not in created and bool(hits), str(hits))
            if hits:
                tool = hits[0]
                deadline = asyncio.get_event_loop().time() + 20
                while asyncio.get_event_loop().time() < deadline:
                    _, raw = await local_conn.invoke(DIAG, {"operation": "status"})
                    before = status_body(raw)
                    if tool in before.get("applied_tool_ids", []):
                        break
                    await asyncio.sleep(0.25)
                q.receipt("fixture tool applied before the denied calls", tool in before.get("applied_tool_ids", []), json.dumps(before)[:200])

        # Legitimate mutating control, the only thing that differs from the happy path is the caller.
        mutate = {"operation": "bump_runtime"}
        if member_ok:
            msg = await member.call("tools.invoke", {"tool_name": DIAG, "arguments": mutate})
            q.receipt("remote member is refused by the operator floor", is_gate_denial(msg, "operator"), json.dumps(msg)[:300])
        if admin_ok:
            msg = await admin.call("tools.invoke", {"tool_name": DIAG, "arguments": mutate})
            q.receipt("remote operator is refused by the physical-loopback floor", is_gate_denial(msg, "loopback"), json.dumps(msg)[:300])

        # Missing ambient context: a real agent run, user-text marker only.
        marker = f"QA_HPRE_AMBIENT_{args.run_id}_{os.getpid()}"
        chat = await admin.call("chat.send", {"message": marker, "stream": True})
        run_id = (chat.get("result") or {}).get("run_id")
        q.receipt("ordinary chat.send reached the real AgentLoop", bool(run_id), json.dumps(chat)[:200])
        if run_id:
            terminal = await admin.terminal(run_id)
            q.receipt("agent run reached a terminal frame", terminal.get("method") in ("stream.run_complete", "stream.run_error"), json.dumps(terminal)[:200])
            await drive._recorded_provider_bodies(args.provider_log, marker, timeout=20)
            await asyncio.sleep(1)
            bodies = provider_bodies(args.provider_log, marker)
            seen, results = tool_results(bodies, DIAG)
            verdict, detail = ambient_verdict(seen, results)
            if verdict == "unverified":
                q.gap("missing ambient context denial", detail)
            else:
                q.receipt("ordinary agent route is denied for a missing ambient role", verdict == "pass", detail)

        if local_ok and tool:
            await asyncio.sleep(1)
            ok, raw = await local_conn.invoke(DIAG, {"operation": "status"})
            after = status_body(raw)
            q.receipt(
                "same incarnation: denied calls left status/applied/generations unchanged",
                ok and snapshot_key(after) == snapshot_key(before),
                json.dumps(snapshot_key(after))[:300],
            )
    except Exception as exc:  # noqa: BLE001
        q.check("tls probe completed without transport/runtime error", False, repr(exc))
    finally:
        for ws in sockets:
            await drive._safe_close_ws(ws)
    print(f"RECEIPT_COUNT: {q.receipts}", flush=True)
    for gap in q.unverified:
        print(f"  UNVERIFIED: {gap}", flush=True)
    if q.failures:
        print(f"VERDICT: FAIL ({len(q.failures)} claim(s))", flush=True)
        return 1
    if q.unverified:
        print("VERDICT: UNVERIFIED (no PASS claimed)", flush=True)
        return 3
    print("VERDICT: PASS", flush=True)
    return 0


def cmd_check_ip(args):
    problem = lan_ip_problem(args.ip, assigned_ipv4())
    if problem is None and not bindable(args.ip):
        problem = "kernel refuses to bind the address"
    if problem:
        print(f"LAN_IP_GUARD: FAIL {problem}")
        return 1
    print(f"LAN_IP_GUARD: OK {args.ip}")
    return 0


def cmd_patch(args):
    path = Path(args.config)
    path.write_text(patch_tls_config(path.read_text(), args.host))
    return 0


def parse_args(argv=None):
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check-ip"); c.add_argument("ip")
    c = sub.add_parser("patch-config"); c.add_argument("--config", required=True); c.add_argument("--host", required=True)
    c = sub.add_parser("prepare"); c.add_argument("--ws", required=True); c.add_argument("--state", required=True); c.add_argument("--run-id", required=True)
    c = sub.add_parser("probe")
    c.add_argument("--host", required=True); c.add_argument("--port", type=int, required=True); c.add_argument("--cafile", required=True)
    c.add_argument("--state", required=True); c.add_argument("--provider-log", type=Path, required=True)
    c.add_argument("--effect-log", type=Path, required=True); c.add_argument("--fixture-script", type=Path, required=True)
    c.add_argument("--python", default=sys.executable); c.add_argument("--prefix", required=True); c.add_argument("--run-id", required=True)
    return p.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    if args.cmd == "check-ip":
        return cmd_check_ip(args)
    if args.cmd == "patch-config":
        return cmd_patch(args)
    if args.cmd == "prepare":
        return asyncio.run(cmd_prepare(args))
    return asyncio.run(cmd_probe(args))


if __name__ == "__main__":
    raise SystemExit(main())
