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
import os
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
    try:
        return int(text)
    except ValueError as e:
        raise MemoryProbeError(f"cannot convert {what} to an integer") from e


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


def _parse_macos_vm_stat(text, counters):
    """Parse the header and exactly one unsigned count per requested label."""
    lines = text.splitlines()
    if not lines:
        raise MemoryProbeError("empty vm_stat output")
    m = re.fullmatch(
        r"Mach Virtual Memory Statistics: \(page size of ([0-9]+) bytes\)\s*", lines[0])
    if not m:
        raise MemoryProbeError(f"malformed vm_stat header: {lines[0]!r}")
    page_size = _strict_uint(m.group(1), "page size")
    if page_size <= 0:
        raise MemoryProbeError(f"invalid page size: {page_size}")
    counts = {}
    for name in counters:
        hits = [ln for ln in lines[1:] if ln.startswith(name + ":")]
        if len(hits) != 1:
            raise MemoryProbeError(
                f"expected exactly one {name!r} line, got {len(hits)}")
        m = re.fullmatch(re.escape(name) + r":\s+([0-9]+)\.\s*", hits[0])
        if not m:
            raise MemoryProbeError(f"malformed {name!r} line: {hits[0]!r}")
        counts[name] = _strict_uint(m.group(1), name)
    return page_size, counts


def parse_macos_vm_stat(text):
    """Compatibility lower bound: textual 'Pages free' * page size only."""
    page_size, counts = _parse_macos_vm_stat(text, ("Pages free",))
    return page_size * counts["Pages free"]


def parse_macos_available_bytes(text):
    """psutil-style estimate, not a guarantee of immediately allocatable RAM.

    Apple's textual free excludes speculative; adding it reconstructs raw
    free_count. Available is raw free + inactive, with no other counters.
    """
    page_size, counts = _parse_macos_vm_stat(
        text, ("Pages free", "Pages speculative", "Pages inactive"))
    if sum(ln.startswith("Mach Virtual Memory Statistics:")
           for ln in text.splitlines()) != 1:
        raise MemoryProbeError("expected exactly one vm_stat header")
    return page_size * sum(counts.values())


def _run(argv, env=None):
    # argv list, no shell; non-zero exit or timeout is an error.
    try:
        proc = subprocess.run(argv, capture_output=True, text=True,
                              timeout=PROBE_TIMEOUT_S, shell=False, env=env)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise MemoryProbeError(f"probe {argv[0]} failed: {e}")
    if proc.returncode != 0:
        raise MemoryProbeError(
            f"probe {argv[0]} exit {proc.returncode}: {proc.stderr.strip()}")
    return proc.stdout


def probe_free_bytes(system=None):
    """Real available-memory probe; unknown platforms and errors fail closed."""
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
    if system == "Darwin":
        env = os.environ.copy()
        env["LC_ALL"] = "C"
        return parse_macos_available_bytes(_run(["/usr/bin/vm_stat"], env=env))
    raise MemoryProbeError(f"unsupported platform for real probe: {system!r}")


def self_test():
    """Pure fixtures and subprocess mocks: no real process, file, or network."""
    from unittest.mock import patch

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

    # Available reconstructs raw free (textual free + speculative), then adds
    # inactive. Other counters must not affect the estimate.
    available_mac = (
        "Mach Virtual Memory Statistics: (page size of {ps} bytes)\n"
        "Pages free:        {free}.\n"
        "Pages speculative: {speculative}.\n"
        "Pages inactive:    {inactive}.\n"
        "Pages active:      999999999.\n"
        "Pages purgeable:   999999999.\n"
        "Pages occupied by compressor: 999999999.\n")
    for ps in (4096, 16384):
        for delta in (-1, 0, 1):
            fixture = available_mac.format(
                ps=ps, free=1, speculative=2, inactive=req // ps - 3 + delta)
            value = parse_macos_available_bytes(fixture)
            assert value == req + ps * delta
            assert allowed(value) == (delta >= 0)
            assert parse_macos_vm_stat(fixture) == ps
        assert parse_macos_available_bytes(available_mac.format(
            ps=ps, free=0, speculative=0, inactive=0)) == 0
        for field in ("free", "speculative", "inactive"):
            values = dict(ps=ps, free=0, speculative=0, inactive=0)
            values[field] = 1
            assert parse_macos_available_bytes(available_mac.format(**values)) == ps
    assert allowed(req + 1)

    valid_mac = available_mac.format(ps=16384, free=1, speculative=2, inactive=3)
    for field in ("free", "speculative", "inactive"):
        label = "Pages " + field + ":"
        line = next(ln for ln in valid_mac.splitlines() if ln.startswith(label))
        # Missing, duplicate (even if malformed), and malformed counter tokens.
        for bad in (valid_mac.replace(line + "\n", ""),
                    valid_mac + line + "\n", valid_mac + label + " bad.\n",
                    valid_mac.replace(line, label + " 1"),
                    valid_mac.replace(line, label + " 1. kB")):
            assert fails(parse_macos_available_bytes, bad), bad
        for token in ("-1", "+1", "1.5", "x", "1x", "1 kB", "", "١"):
            values = dict(ps=16384, free=1, speculative=2, inactive=3)
            values[field] = token
            bad = available_mac.format(**values)
            assert fails(parse_macos_available_bytes, bad), bad
    header, body = valid_mac.split("\n", 1)
    for bad in ("", body, "\n" + valid_mac, valid_mac + header + "\n",
                valid_mac.replace(header, header[:-1]),
                valid_mac.replace("Mach Virtual", "Invalid Virtual")):
        assert fails(parse_macos_available_bytes, bad), bad
    for ps in (0, -1, "x", "1.5", "+4096", "4096 kB", ""):
        bad = available_mac.format(ps=ps, free=1, speculative=2, inactive=3)
        assert fails(parse_macos_available_bytes, bad), bad

    # Python may reject very long decimal strings before integer conversion.
    # That parse error must still use the documented exit-2 path.
    digit_limit = getattr(sys, "get_int_max_str_digits", lambda: 0)()
    if digit_limit:
        huge = "9" * (digit_limit + 1)
        for field in ("ps", "free", "speculative", "inactive"):
            values = dict(ps=16384, free=1, speculative=2, inactive=3)
            values[field] = huge
            bad = available_mac.format(**values)
            assert fails(parse_macos_available_bytes, bad)
            with patch(__name__ + ".probe_free_bytes", side_effect=lambda: (
                    parse_macos_available_bytes(bad))):
                with patch("sys.stderr"):
                    assert main([]) == 2

    # Mock the actual subprocess boundary: Darwin must use one absolute argv,
    # a fresh C-locale environment, a bounded timeout, and never a shell.
    argv = ["/usr/bin/vm_stat"]
    original_env = os.environ.copy()
    expected_env = dict(original_env, LC_ALL="C")
    result = subprocess.CompletedProcess(argv, 0, valid_mac, "")
    with patch.object(subprocess, "run", return_value=result) as run:
        with patch.object(platform, "system", return_value="Darwin"):
            assert probe_free_bytes() == 16384 * 6
        run.assert_called_once_with(
            argv, capture_output=True, text=True, timeout=15,
            shell=False, env=expected_env)
        first_env = run.call_args.kwargs["env"]
        assert first_env is not os.environ
        assert probe_free_bytes("Darwin") == 16384 * 6
        assert run.call_count == 2
        assert run.call_args.kwargs["env"] is not first_env
    assert dict(os.environ) == original_env

    # Existing Windows invocation keeps inherited env and KiB units.
    win_argv = ["powershell", "-NoProfile", "-NonInteractive", "-Command",
                "(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory"]
    with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess(
            win_argv, 0, "4194304\r\n", "")) as run:
        assert probe_free_bytes("Windows") == req
        run.assert_called_once_with(
            win_argv, capture_output=True, text=True, timeout=15,
            shell=False, env=None)

    for result in (subprocess.CompletedProcess(argv, 1, valid_mac, "denied"),
                   subprocess.CompletedProcess(argv, 0, "malformed", "")):
        with patch.object(subprocess, "run", return_value=result) as run:
            assert fails(probe_free_bytes, "Darwin")
            run.assert_called_once()
    for error in (subprocess.TimeoutExpired(argv, 15),
                  FileNotFoundError("missing vm_stat"), PermissionError("denied")):
        with patch.object(subprocess, "run", side_effect=error) as run:
            assert fails(probe_free_bytes, "Darwin")
            run.assert_called_once()
    with patch.object(subprocess, "run") as run:
        assert fails(probe_free_bytes, "Plan9")
        run.assert_not_called()
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
