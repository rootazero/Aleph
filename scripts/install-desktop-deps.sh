#!/usr/bin/env bash
# Runtime dependency bootstrap for the Linux NSH desktop candidate only.

usage() {
  printf 'Usage: %s [--check|--install|--help]\n' "${0##*/}"
  printf '%s\n' 'Check GTK4 and WebKitGTK 6 runtime libraries (default), or request installation through the distro package manager.'
}

probe() {
  local cache soname flags path i line loader=ldconfig
  if ! command -v "$loader" >/dev/null; then
    # Some distributions omit sbin from non-root users' PATH.
    loader=/sbin/ldconfig
  fi
  if ! cache=$(LC_ALL=C "$loader" -p); then
    printf 'Cannot inspect the runtime linker cache (ldconfig -p failed).\n' >&2
    return 2
  fi

  missing=(libgtk-4.so.1 libwebkitgtk-6.0.so.4)
  local row='^[[:space:]]*([^[:space:]]+)[[:space:]]+\(([^)]*)\)[[:space:]]+=>[[:space:]]+(.+)$'
  local -a abi_flags
  while IFS= read -r line; do
    [[ $line =~ $row ]] || continue
    soname=${BASH_REMATCH[1]} flags=${BASH_REMATCH[2]} path=${BASH_REMATCH[3]}
    IFS=, read -r -a abi_flags <<< "$flags"
    [[ ${abi_flags[0]} == libc6 && ${abi_flags[1]:-} == "$abi" && -f $path && -r $path ]] || continue
    for i in "${!missing[@]}"; do
      if [[ $soname == "${missing[i]}" ]]; then
        unset 'missing[i]'
      fi
    done
  done <<< "$cache"
}

main() {
  local os_release=$1
  shift
  if (( $# > 1 )); then
    usage >&2
    return 2
  fi
  local mode=${1:---check}
  case "$mode" in
    --help) usage; return 0 ;;
    --check|--install) ;;
    *) usage >&2; return 2 ;;
  esac

  local arch abi
  arch=$(uname -m) || return 2
  case "$arch" in
    x86_64) abi=x86-64 ;;
    aarch64) abi=AArch64 ;;
    *) printf 'Unsupported Linux architecture: %s\n' "$arch" >&2; return 2 ;;
  esac
  local -a missing
  probe || return "$?"
  if (( ${#missing[@]} == 0 )); then
    printf 'GTK4 and WebKitGTK 6 runtime libraries found.\n'
    return 0
  fi
  printf 'Missing runtime libraries: %s\n' "${missing[*]}" >&2
  if [[ $mode == --check ]]; then
    printf 'To request installation from your distro package manager, run: %s --install\n' "$0" >&2
    return 1
  fi
  if [[ ! -t 0 ]]; then
    printf '%s\n' '--install requires an interactive terminal.' >&2
    return 2
  fi

  local distro= line id_line="^ID=([a-z0-9._-]+|\"[a-z0-9._-]+\"|'[a-z0-9._-]+')$"
  if [[ -r $os_release ]]; then
    # This script may be run as root; parse the data without sourcing it.
    while IFS= read -r line || [[ -n $line ]]; do
      [[ $line =~ $id_line ]] || continue
      distro=${BASH_REMATCH[1]//[\"\']/}
      break
    done < "$os_release"
  fi
  local -a manager privilege=()
  case "$distro" in
    arch) manager=(pacman -S --needed gtk4 webkitgtk-6.0) ;;
    debian|ubuntu) manager=(apt-get install libgtk-4-1 libwebkitgtk-6.0-4) ;;
    fedora) manager=(dnf install gtk4 webkitgtk6.0) ;;
    *) printf 'Unsupported distribution (%s); install GTK4 and WebKitGTK 6 with your distro tools.\n' "${distro:-unknown}" >&2; return 2 ;;
  esac
  if ! command -v "${manager[0]}" >/dev/null; then
    printf 'Package manager not found: %s\n' "${manager[0]}" >&2
    return 2
  fi
  if (( EUID != 0 )); then
    if ! command -v sudo >/dev/null; then
      printf '%s\n' 'sudo is unavailable; run --install as an administrator.' >&2
      return 2
    fi
    privilege=(sudo)
  fi
  printf 'Requesting installation: %s\n' "${manager[*]}"
  local status=0
  "${privilege[@]}" "${manager[@]}" || status=$?
  if (( status != 0 )); then
    printf 'Package-manager invocation failed (exit %s). Check its output; no repository or trust settings were changed by this script.\n' "$status" >&2
    return "$status"
  fi
  probe || return "$?"
  if (( ${#missing[@]} > 0 )); then
    printf 'Runtime libraries still missing after installation: %s\n' "${missing[*]}" >&2
    return 1
  fi
  printf 'GTK4 and WebKitGTK 6 runtime libraries found after installation.\n'
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  os_release=/etc/os-release
  [[ -e $os_release ]] || os_release=/usr/lib/os-release
  main "$os_release" "$@"
fi
