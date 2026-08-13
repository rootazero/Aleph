#!/usr/bin/env python3
"""Apply the browser settings this scenario needs to a generated QA config.

Runs AFTER `qa/busy_input/patch_config.py`, which already made the daemon inert
and set the gateway port — this file adds only what is browser-specific, rather
than growing a second copy of the config-shaping logic.

⚠️ The sections are `[general.browser.*]`, NOT `[browser.*]`.
`config/types/general.rs` has a doc test that deserializes `[browser.policy]`
straight into `GeneralConfig`, but a *generated* config nests the whole thing
under `[general]`. The first version of this fixture copied the section name off
that unit test, so every setting below landed in a table nothing reads: the
policy stayed at its default and the browser was refused the fixture page on
127.0.0.1. Read the generated file, not the unit test's fixture.

Settings are edited **in place** for the same reason a second `[general.browser.
policy]` table would be a TOML duplicate-key error — the generator already
writes these sections.

Three settings, each load-bearing:

* **`binary_path`** pins the already-installed `playwright-cli`. Without it the
  driver's slow path runs `ensure_capability`, which *installs the runtime over
  the network* into the scratch HOME: minutes of fetching, and a QA verdict
  that depends on the network.

* **`block_private = false`** lets the browser reach the fixture page on
  127.0.0.1. That is what makes the scenario hermetic — no public site is
  involved — and it is a policy knob, not a hole punched for the test.

* **`user_data_dir`** is the one launch setting the CLI echoes back out of band
  (`playwright-cli list` prints `user-data-dir:`), which makes it the oracle for
  "the `--config` file Aleph generated actually reached the browser" as opposed
  to merely "a browser came up".
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--cli-binary", required=True)
p.add_argument("--user-data-dir", required=True)
p.add_argument("--headless", default="true", choices=["true", "false"])
args = p.parse_args()

src = open(args.path).read()


def set_key(text, section, key, value):
    """Set `key = value` inside `[section]`, creating the section if absent."""
    out, cur, inserted = [], None, False
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1)
            out.append(line)
            if cur == section:
                out.append(f"{key} = {value}")
                inserted = True
            continue
        if cur == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            continue  # replaced by the line inserted at the header
        out.append(line)
    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[{section}]\n{key} = {value}\n"
    return text


for section, key, value in [
    ("general.browser.playwright_cli", "binary_path", f'"{args.cli_binary}"'),
    ("general.browser.playwright_cli", "headless", args.headless),
    ("general.browser.playwright_cli", "nav_timeout_secs", "120"),
    ("general.browser.playwright_cli", "action_timeout_secs", "60"),
    ("general.browser.policy", "block_private", "false"),
    ("general.browser.policy", "block_secrets_in_url", "false"),
    ("general.browser.policy", "block_secrets_in_input", "false"),
    ("general.browser.policy", "redact_secrets_in_content", "false"),
]:
    src = set_key(src, section, key, value)

# A sub-table of the already-declared (empty) `[general.browser.profiles]`.
# Declaring a child of a defined table is valid TOML; re-declaring the parent
# would not be.
src += f"""
[general.browser.profiles.default]
driver = "managed"
user_data_dir = "{args.user_data_dir}"
extra_args = ["--disable-gpu"]
"""

open(args.path, "w").write(src)
print(f"patched [general.browser] in {args.path}: cli={args.cli_binary} headless={args.headless}")
