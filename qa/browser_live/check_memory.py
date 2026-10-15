#!/usr/bin/env python3
"""Memory gate for browser_live QA (fail-closed, stdlib only).

Usage:
  python check_memory.py --self-test          # pure fixtures, no external process
  python check_memory.py [--required-bytes N] # real probe; exit 0 ALLOW, 1 BLOCK, 2 error

The gate answers one narrow question: is available RAM >= required bytes
(default 4 GiB)? It is NOT a guarantee that a large cargo link succeeds;
10 GiB free is the recommended headroom for heavy linking, but the enforced
gate stays at 4 GiB. Unknown / missing / malformed / negative input is an
error (exit 2), never an implicit "allow" and never a guessed default.
"""
import argparse
import platform
import re
import subprocess
import sys

REQUIRED_BYTES = 4 * 1024 ** 3  # 4294967296
KIB = 1024  # Linux MemAvailable "kB" and Windows FreePhysicalMemory are both KiB
PROBE_TIMEOUT_S = 15


class MemoryProbeError(Exception):
    """Probe or parse failure; callers must treat it as BLOCK-like (exit 2)."""


def _check_bytes(value, name):
    # bool is an int subclass; reject it so True never means 1 byte.
    if isinstance(value, bool) or not isinstance(value, int):
        raise MemoryProbeError(f"{name} must be an int, got {value!r}")
    if value < 0:
        raise MemoryProbeError(f"{name} must be non-negative, got {value}")
    return value


def allowed(free_bytes, required_bytes=REQUIRED_BYTES):
    """Pure gate: True iff free_bytes >= required_bytes (inclusive boundary)."""
    return _check_bytes(free_bytes, "free_bytes") >= _check_bytes(
        required_bytes, "required_bytes")


def _strict_uint(text, what):
    if not re.fullmatch(r"[0-9]+", text or ""):
        raise MemoryProbeError(f"malformed {what}: {text!r}")
    return int(text)


def parse_linux_meminfo(text):
    """/proc/meminfo: 'MemAvailable:  N kB' (kB == KiB) -> bytes."""
    hits = [ln for ln in text.splitlines() if ln.startswith("MemAvailable:")]
    if len(hits) != 1:
        raise MemoryProbeError(f"expected exactly one MemAvailable line, got {len(hits)}")
    m = re.fullmatch(r"MemAvailable:\s+(\S+)\s+kB\s*", hits[0])
    if not m:
        raise MemoryProbeError(f"malformed MemAvailable line: {hits[0]!r}")
    return _strict_uint(m.group(1), "MemAvailable") * KIB


def parse_windows_free_kib(text):
    """PowerShell (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory:
    a single integer in KiB -> bytes."""
    return _strict_uint((text or "").strip(), "FreePhysicalMemory") * KIB


def parse_macos_vm_stat(text):
    """vm_stat -> bytes, CONSERVATIVE lower bound: 'Pages free' * page size only.

    Inactive/speculative/purgeable pages are deliberately NOT added: there is
    no verified Apple definition here proving they are reclaimable without
    double counting, so the result may under-report (the fail-closed
    direction). The real macOS probe is unsupported (see probe_free_bytes);
    this parser exists for fixtures only.
    """
    lines = text.splitlines()
    if not lines:
        raise MemoryProbeError("empty vm_stat output")
    m = re.fullmatch(
        r"Mach Virtual Memory Statistics: \(page size of ([0-9]+) bytes\)\s*", lines[0])
    if not m:
        raise MemoryProbeError(f"malformed vm_stat header: {lines[0]!r}")
    page_size = int(m.group(1))
    if page_size <= 0:
        raise MemoryProbeError(f"invalid page size: {page_size}")
    hits = [ln for ln in lines[1:] if ln.startswith("Pages free:")]
    if len(hits) != 1:
        raise MemoryProbeError(f"expected exactly one 'Pages free' line, got {len(hits)}")
    m = re.fullmatch(r"Pages free:\s+([0-9]+)\.\s*", hits[0])
    if not m:
        raise MemoryProbeError(f"malformed 'Pages free' line: {hits[0]!r}")
    return int(m.group(1)) * page_size


def _run(argv):
    # argv list, no shell; non-zero exit or timeout is an error.
    try:
        proc = subprocess.run(argv, capture_output=True, text=True,
                              timeout=PROBE_TIMEOUT_S, shell=False)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise MemoryProbeError(f"probe {argv[0]} failed: {e}")
    if proc.returncode != 0:
        raise MemoryProbeError(
            f"probe {argv[0]} exit {proc.returncode}: {proc.stderr.strip()}")
    return proc.stdout


def probe_free_bytes(system=None):
    """Real probe. Only Linux and Windows are supported; everything else errors."""
    system = system or platform.system()
    if system == "Linux":
        try:
            with open("/proc/meminfo", encoding="ascii") as f:
                return parse_linux_meminfo(f.read())
        except OSError as e:
            raise MemoryProbeError(f"cannot read /proc/meminfo: {e}")
    if system == "Windows":
        out = _run(["powershell", "-NoProfile", "-NonInteractive", "-Command",
                    "(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory"])
        return parse_windows_free_kib(out)
    # macOS (Darwin) is intentionally unsupported until vm_stat reclaimable
    # accounting is verified; never guess.
    raise MemoryProbeError(f"unsupported platform for real probe: {system!r}")


def self_test():
    """Pure fixtures only: no subprocess, no file, no network."""
    req = 4294967296
    assert req == REQUIRED_BYTES
    assert allowed(4294967296)
    assert not allowed(4294967295)
    assert allowed(4294967296, 4294967296) and not allowed(0, 1) and allowed(0, 0)

    def fails(fn, *args):
        try:
            fn(*args)
        except MemoryProbeError:
            return True
        return False

    # Gate input validation.
    for bad in (-1, 1.5, "1", None, True):
        assert fails(allowed, bad), bad
    assert fails(allowed, 1, -1) and fails(allowed, 1, "x")

    # Linux: kB are KiB; 4 GiB == 4194304 kB.
    lin = ("MemTotal:       16000000 kB\nMemFree:  1 kB\n"
           "MemAvailable:   {} kB\nBuffers: 5 kB\n")
    assert parse_linux_meminfo(lin.format(4194304)) == req
    assert allowed(parse_linux_meminfo(lin.format(4194304)))
    assert not allowed(parse_linux_meminfo(lin.format(4194303)))
    assert parse_linux_meminfo(lin.format(0)) == 0
    for bad in ("MemTotal: 1 kB\n", lin.format("-5"), lin.format("abc"),
                lin.format(1) + "MemAvailable: 2 kB\n", "MemAvailable: 10 MB\n",
                "MemAvailable: 10\n", ""):
        assert fails(parse_linux_meminfo, bad), bad

    # Windows: FreePhysicalMemory is KiB.
    assert parse_windows_free_kib("4194304\r\n") == req
    assert allowed(parse_windows_free_kib("4194304"))
    assert not allowed(parse_windows_free_kib("4194303\r\n"))
    assert parse_windows_free_kib("0") == 0
    for bad in ("", "  \n", "-1", "1.5", "4194304 KB", "a\nb", "1\n2"):
        assert fails(parse_windows_free_kib, bad), bad

    # macOS fixture: conservative 'Pages free' * page size.
    mac = ("Mach Virtual Memory Statistics: (page size of {ps} bytes)\n"
           "Pages free:                               {n}.\n"
           "Pages active:                             999.\n"
           "Pages inactive:                           888.\n")
    assert parse_macos_vm_stat(mac.format(ps=16384, n=262144)) == req
    assert allowed(parse_macos_vm_stat(mac.format(ps=16384, n=262144)))
    assert not allowed(parse_macos_vm_stat(mac.format(ps=16384, n=262143)))
    assert parse_macos_vm_stat(mac.format(ps=4096, n=1048576)) == req
    for bad in ("", mac.format(ps=0, n=1), mac.format(ps="x", n=1),
                mac.format(ps=4096, n="-1"), mac.format(ps=4096, n="1x"),
                "Pages free: 1.\n", mac.format(ps=4096, n=1) + "Pages free: 2.\n",
                mac.format(ps=4096, n=1).replace("Pages free:", "Pages freed:")):
        assert fails(parse_macos_vm_stat, bad), bad

    # Unsupported platforms / macOS real probe fail closed (no process is run).
    assert fails(probe_free_bytes, "Darwin")
    assert fails(probe_free_bytes, "Plan9")
    print("self-test OK")


def _nonneg_int(s):
    if not re.fullmatch(r"[0-9]+", s):
        raise argparse.ArgumentTypeError(f"must be a non-negative integer: {s!r}")
    return int(s)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--self-test", action="store_true",
                    help="run pure fixture tests (no external processes)")
    ap.add_argument("--required-bytes", type=_nonneg_int, default=REQUIRED_BYTES,
                    help="required available bytes (default 4294967296)")
    args = ap.parse_args(argv)
    try:
        if args.self_test:
            self_test()
            return 0
        free = probe_free_bytes()
        ok = allowed(free, args.required_bytes)
    except MemoryProbeError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2
    except AssertionError as e:
        print(f"SELF-TEST FAILED: {e!r}", file=sys.stderr)
        return 2
    print(f"available_bytes={free} required_bytes={args.required_bytes} "
          f"{'ALLOW' if ok else 'BLOCK'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
