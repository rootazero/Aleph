#!/usr/bin/env python3
"""note_layer_dream_window.py — toggle ~/.aleph/config.toml dream window
between an all-day test profile and the original profile.

We can't safely use a TOML library here because Aleph's config.toml is large
and uses formats we don't want to lose to round-tripping. Instead this does
*minimal* line-based edits inside the [memory.dreaming] table only.

Usage:
    note_layer_dream_window.py --apply-test
    note_layer_dream_window.py --restore
"""
import argparse
import os
import re
import shutil
import sys
from pathlib import Path

CONFIG = Path(os.path.expanduser("~/.aleph/config.toml"))
BACKUP = Path(os.path.expanduser("~/.aleph/config.toml.note-layer-test.bak"))

TEST_PROFILE = {
    "window_start_local": '"00:00"',
    "window_end_local":   '"23:59"',
    "idle_threshold_seconds": "30",
    "weekly_enabled": "false",
}


def replace_in_section(text: str, section: str, key: str, new_value: str) -> str:
    """Replace `key = ...` only inside the `[section]` block (until next [..] header)."""
    section_re = re.compile(
        rf"(\[{re.escape(section)}\][^\[]*?)(^\s*{re.escape(key)}\s*=\s*)([^\n]*)",
        re.MULTILINE | re.DOTALL,
    )
    def sub(m):
        return f"{m.group(1)}{m.group(2)}{new_value}"
    new_text, count = section_re.subn(sub, text, count=1)
    if count == 0:
        # Append to section if missing
        section_header = f"[{section}]"
        idx = new_text.find(section_header)
        if idx == -1:
            raise SystemExit(f"section [{section}] not found in {CONFIG}")
        # Insert after section header line
        eol = new_text.find("\n", idx)
        new_text = new_text[:eol+1] + f"{key} = {new_value}\n" + new_text[eol+1:]
    return new_text


def apply_test():
    if not BACKUP.exists():
        shutil.copy(CONFIG, BACKUP)
        print(f"Backed up {CONFIG} → {BACKUP}")
    text = CONFIG.read_text()
    for k, v in TEST_PROFILE.items():
        text = replace_in_section(text, "memory.dreaming", k, v)
    CONFIG.write_text(text)
    print(f"Applied test dream profile to {CONFIG}")
    for k, v in TEST_PROFILE.items():
        print(f"  {k} = {v}")


def restore():
    if not BACKUP.exists():
        print(f"No backup found at {BACKUP}; nothing to restore", file=sys.stderr)
        sys.exit(1)
    shutil.copy(BACKUP, CONFIG)
    BACKUP.unlink()
    print(f"Restored {CONFIG} from {BACKUP}")


def main():
    p = argparse.ArgumentParser()
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--apply-test", action="store_true")
    g.add_argument("--restore", action="store_true")
    args = p.parse_args()
    if args.apply_test:
        apply_test()
    elif args.restore:
        restore()


if __name__ == "__main__":
    main()
