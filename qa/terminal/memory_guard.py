#!/usr/bin/env python3
"""Fail-closed physical-memory admission gate (stdlib only).

Usage:
    python3 qa/terminal/memory_guard.py -- <command> [args...]
    python3 qa/terminal/memory_guard.py --check-only
    python3 qa/terminal/memory_guard.py            # same as --check-only

Semantics (deliberately narrow):
  * ONE probe of currently available physical memory, taken right before the
    command would start. This is an admission check, NOT a reservation and NOT
    a watchdog: nothing protects the command once it is running.
  * Linux: /proc/meminfo MemAvailable (kB * 1024).
    Windows: kernel32 GlobalMemoryStatusEx ullAvailPhys.
    Any other platform, unreadable source, parse error or API error => DENIED.
    No placeholder "large" number is ever substituted for an unknown value.
  * Admitted iff available_bytes >= THRESHOLD_BYTES (exactly 4 GiB).
  * Admitted: run the command (inherited environment) and pass through its
    exit code. Denied: exit 3 and the command is never spawned.

Argument rules (no silent no-ops): `--check-only` and a command are mutually
exclusive; an explicit `--` separator must be followed by a command. Either
violation is a usage error (exit 2) detected BEFORE any probe or spawn.

Exit codes: 0 admitted (check-only) / command's own code; 2 usage error;
3 admission denied; 127 command not found; 126 command not executable or
any other OSError while spawning.

Exit code 3 is ambiguous: the exit code alone cannot distinguish "admission denied" (3) from
"admitted command that itself exited 3" (command exit codes are passed through
unchanged). Read the machine-readable stderr lines instead:
  memory_guard: result=ADMITTED | result=DENIED | result=USAGE_ERROR
  memory_guard: command_exit=<N>      (only when a command actually ran)
A denied run never prints command_exit= and never spawns the command.
"""

import ctypes
import subprocess
import sys
from typing import Callable, List, NamedTuple, Optional

THRESHOLD_BYTES = 4_294_967_296  # 4 GiB, approved value; do not change here.
EXIT_USAGE = 2
EXIT_DENIED = 3


class Probe(NamedTuple):
    ok: bool
    source: str
    available_bytes: Optional[int]
    reason: Optional[str]


def admit(p: Probe) -> bool:
    """Admit only a successful probe carrying a real byte count >= threshold."""
    return bool(p.ok) and p.available_bytes is not None and p.available_bytes >= THRESHOLD_BYTES


def parse_meminfo(text: str) -> int:
    """Return MemAvailable in bytes; raise ValueError on missing/bad data."""
    for line in text.splitlines():
        if not line.startswith("MemAvailable:"):
            continue
        parts = line.split(":", 1)[1].split()
        if len(parts) != 2 or parts[1] != "kB" or not parts[0].isascii() or not parts[0].isdigit():
            raise ValueError("malformed MemAvailable line: %r" % line)
        return int(parts[0]) * 1024
    raise ValueError("MemAvailable not found")


def probe_linux(path: str = "/proc/meminfo") -> Probe:
    source = "linux:%s:MemAvailable" % path
    try:
        with open(path, "r", encoding="ascii", errors="strict") as f:
            return Probe(True, source, parse_meminfo(f.read()), None)
    except (OSError, ValueError) as e:
        return Probe(False, source, None, "linux probe failed: %s" % e)


class MEMORYSTATUSEX(ctypes.Structure):
    _fields_ = [
        ("dwLength", ctypes.c_uint32),
        ("dwMemoryLoad", ctypes.c_uint32),
        ("ullTotalPhys", ctypes.c_uint64),
        ("ullAvailPhys", ctypes.c_uint64),
        ("ullTotalPageFile", ctypes.c_uint64),
        ("ullAvailPageFile", ctypes.c_uint64),
        ("ullTotalVirtual", ctypes.c_uint64),
        ("ullAvailVirtual", ctypes.c_uint64),
        ("ullAvailExtendedVirtual", ctypes.c_uint64),
    ]


def probe_windows(kernel32=None) -> Probe:
    source = "windows:kernel32.GlobalMemoryStatusEx:ullAvailPhys"
    try:
        if kernel32 is None:
            kernel32 = ctypes.windll.kernel32  # AttributeError off Windows -> denied
        status = MEMORYSTATUSEX()
        status.dwLength = ctypes.sizeof(MEMORYSTATUSEX)
        if not kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            return Probe(False, source, None, "GlobalMemoryStatusEx returned failure")
        return Probe(True, source, int(status.ullAvailPhys), None)
    except Exception as e:  # fail closed on anything
        return Probe(False, source, None, "windows probe failed: %s" % e)


def probe(platform: Optional[str] = None,
          linux_probe: Callable[[], Probe] = probe_linux,
          windows_probe: Callable[[], Probe] = probe_windows) -> Probe:
    plat = sys.platform if platform is None else platform
    try:
        if plat.startswith("linux"):
            return linux_probe()
        if plat == "win32":
            return windows_probe()
    except Exception as e:
        return Probe(False, "probe:%s" % plat, None, "probe raised: %s" % e)
    return Probe(False, "probe:%s" % (plat or "unknown"), None,
                 "platform unsupported: %r" % plat)


def default_call(cmd: List[str]) -> int:
    try:
        rc = subprocess.call(cmd)  # inherits environment
    except FileNotFoundError as e:
        print("memory_guard: command not found: %s" % e, file=sys.stderr)
        return 127
    except PermissionError as e:
        print("memory_guard: command not executable: %s" % e, file=sys.stderr)
        return 126
    except OSError as e:  # e.g. ENOEXEC; KeyboardInterrupt is NOT caught
        print("memory_guard: command could not be started: %s" % e, file=sys.stderr)
        return 126
    if rc < 0:  # killed by signal -> shell convention
        return 128 - rc
    return rc


def _report(p: Probe, admitted: bool) -> str:
    return (
        "memory_guard: single-shot probe (NOT a reservation, NOT a watchdog: "
        "nothing guards memory once the command runs)\n"
        "memory_guard: source=%s\n"
        "memory_guard: available_bytes=%s\n"
        "memory_guard: threshold_bytes=%d\n"
        "memory_guard: %s%s\n"
        "memory_guard: result=%s\n"
        % (p.source,
           "unknown" if p.available_bytes is None else p.available_bytes,
           THRESHOLD_BYTES,
           "ADMITTED" if admitted else "DENIED",
           "" if admitted or not p.reason else " (%s)" % p.reason,
           "ADMITTED" if admitted else "DENIED")
    )


def _usage(msg: str) -> int:
    sys.stderr.write("memory_guard: usage error: %s "
                     "(use: memory_guard.py [--check-only | -- <command> args...])\n"
                     "memory_guard: result=USAGE_ERROR\n" % msg)
    return EXIT_USAGE


def main(argv: List[str],
         probe_fn: Callable[[], Probe] = probe,
         call_fn: Callable[[List[str]], int] = default_call) -> int:
    has_sep = "--" in argv
    if has_sep:
        i = argv.index("--")
        opts, command = argv[:i], argv[i + 1:]
    else:
        opts, command = argv, []
    check_only = False
    for o in opts:
        if o == "--check-only":
            check_only = True
        else:
            return _usage("unexpected argument %r" % o)
    if has_sep and not command:
        return _usage("`--` given without a command")
    if check_only and command:
        return _usage("--check-only cannot be combined with a command")

    p = probe_fn()
    admitted = admit(p)
    sys.stderr.write(_report(p, admitted))  # stderr: keep command stdout clean
    if not admitted:
        return EXIT_DENIED
    if not command:  # plain check (no argv or --check-only)
        return 0
    rc = call_fn(command)
    sys.stderr.write("memory_guard: command_exit=%d\n" % rc)
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
