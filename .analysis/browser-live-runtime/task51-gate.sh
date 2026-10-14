#!/usr/bin/env bash
free=$(powershell -NoProfile -Command "[int64](Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory*1024")
echo "free_bytes=$free"
[ "$free" -ge 4294967296 ] || { echo "MEMORY GATE FAIL"; exit 99; }
export CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_NET_OFFLINE=true
"$@"
