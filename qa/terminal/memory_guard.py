#!/usr/bin/env python3
"""Admit a build only when at least 4 GiB of physical memory is available.

Usage: python memory_guard.py -- <command> [args...]
Unknown probes fail closed; low-memory probes are repeated every five seconds.
This is build admission, not a reservation or a limit on child memory usage.
"""

import ctypes
from pathlib import Path
import re
import subprocess
import sys
import time

MIN_AVAILABLE_BYTES = 4_294_967_296
POLL_SECONDS = 5


def parse_meminfo(text: str) -> int | None:
    """Read Linux MemAvailable only; MemFree is not an equivalent probe."""
    entries = [line for line in text.splitlines() if line.startswith("MemAvailable:")]
    if len(entries) != 1:
        return None
    match = re.fullmatch(r"MemAvailable:\s+([0-9]+)\s+kB\s*", entries[0])
    if match is None:
        return None
    try:
        return int(match.group(1)) * 1024
    except ValueError:
        return None


class _MemoryStatusEx(ctypes.Structure):
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


def _available_memory_windows() -> int | None:
    status = _MemoryStatusEx()
    status.dwLength = ctypes.sizeof(status)
    try:
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        probe = kernel.GlobalMemoryStatusEx
        probe.argtypes = [ctypes.POINTER(_MemoryStatusEx)]
        probe.restype = ctypes.c_int32
        if not probe(ctypes.byref(status)):
            return None
    except (AttributeError, OSError):
        return None
    if status.ullTotalPhys == 0 or status.ullAvailPhys > status.ullTotalPhys:
        return None
    return status.ullAvailPhys


def available_memory_bytes() -> int | None:
    """Return available physical bytes, or None when evidence is unavailable."""
    if sys.platform == "win32":
        return _available_memory_windows()
    if sys.platform.startswith("linux"):
        try:
            return parse_meminfo(Path("/proc/meminfo").read_text(encoding="ascii"))
        except (OSError, UnicodeError):
            return None
    return None


def run_guarded(command: list[str]) -> int:
    """Wait for admission, execute once, and preserve the child's exit status."""
    if not command:
        print("memory guard: command is required", file=sys.stderr)
        return 2
    try:
        while True:
            available = available_memory_bytes()
            if type(available) is not int or available < 0:
                print(
                    "memory guard: available-memory probe unavailable; "
                    "refusing to launch child",
                    file=sys.stderr,
                )
                return 2
            if available >= MIN_AVAILABLE_BYTES:
                print(
                    f"memory guard: admitted (available={available}, "
                    f"minimum={MIN_AVAILABLE_BYTES})",
                    file=sys.stderr,
                    flush=True,
                )
                result = subprocess.run(command, check=False)
                return result.returncode if result.returncode >= 0 else 128 - result.returncode
            print(
                f"memory guard: waiting (available={available}, "
                f"minimum={MIN_AVAILABLE_BYTES}); recheck in {POLL_SECONDS}s",
                file=sys.stderr,
                flush=True,
            )
            time.sleep(POLL_SECONDS)
    except KeyboardInterrupt:
        print("memory guard: interrupted", file=sys.stderr)
        return 130
    except OSError as error:
        print(f"memory guard: child launch failed: {error}", file=sys.stderr)
        return 127


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) < 2 or arguments[0] != "--":
        print("usage: memory_guard.py -- <command> [args...]", file=sys.stderr)
        return 2
    return run_guarded(arguments[1:])


if __name__ == "__main__":
    raise SystemExit(main())
