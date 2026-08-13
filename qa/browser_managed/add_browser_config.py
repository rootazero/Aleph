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
p.add_argument(
    "--idle-timeout-secs",
    type=int,
    default=None,
    help="default profile's idle_timeout_secs; the `reap` scenario sets it to "
    "a handful of seconds so the reaper is observable inside a QA run rather "
    "than in half an hour",
)
p.add_argument("--tab-idle-timeout-secs", type=int, default=None)
p.add_argument("--max-tabs", type=int, default=None)
p.add_argument(
    "--control-profile",
    default="",
    help="name of a SECOND managed profile that must survive the same sweep "
    "(idle_timeout_secs far in the future). Without it, `every session closed` "
    "and `the idle one closed` look identical.",
)
p.add_argument("--control-user-data-dir", default="")
p.add_argument(
    "--existing-session-profile",
    default="",
    help="name of a profile with driver=existing_session (the Chrome DevTools "
    "MCP driver), so the OTHER driver gets real-machine coverage too",
)
p.add_argument(
    "--chrome-mcp-command",
    default="",
    help="pin the MCP server command. The default is `npx -y "
    "chrome-devtools-mcp@latest`, which under the scenario's scratch HOME has "
    "no npx cache and would fetch from the network mid-run.",
)
p.add_argument("--chrome-mcp-arg", action="append", default=[])
p.add_argument(
    "--control-max-tabs",
    type=int,
    default=None,
    help="max_tabs_per_profile for the control profile — the LRU cap is the one "
    "reaper behaviour that does NOT need an idle wait, so the control profile "
    "carries it instead of costing a second sweep",
)
args = p.parse_args()

src = open(args.path).read()


def set_key(text, section, key, value):
    """Set `key = value` inside `[section]`, creating the section if absent.

    Replaces multi-line array values too. The generated config writes
    `args = [` … `]` across several lines; a line-at-a-time replacement removed
    only the first of them and left the rest orphaned, which the daemon rejects
    at boot with a parse error pointing at the *continuation* line — a good
    thirty lines away from anything this script wrote.
    """
    out, cur, inserted = [], None, False
    skipping_array = False
    for line in text.splitlines():
        if skipping_array:
            if line.rstrip().endswith("]"):
                skipping_array = False
            continue
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1)
            out.append(line)
            if cur == section:
                out.append(f"{key} = {value}")
                inserted = True
            continue
        if cur == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            rhs = line.split("=", 1)[1].strip()
            # An array that opens but does not close on this line continues.
            if rhs.startswith("[") and not rhs.endswith("]"):
                skipping_array = True
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
default_lines = [
    "[general.browser.profiles.default]",
    'driver = "managed"',
    f'user_data_dir = "{args.user_data_dir}"',
    'extra_args = ["--disable-gpu"]',
]
if args.idle_timeout_secs is not None:
    default_lines.append(f"idle_timeout_secs = {args.idle_timeout_secs}")
if args.tab_idle_timeout_secs is not None:
    default_lines.append(f"tab_idle_timeout_secs = {args.tab_idle_timeout_secs}")
if args.max_tabs is not None:
    default_lines.append(f"max_tabs_per_profile = {args.max_tabs}")
src += "\n" + "\n".join(default_lines) + "\n"

if args.control_profile:
    control_lines = [
        f"[general.browser.profiles.{args.control_profile}]",
        'driver = "managed"',
        'extra_args = ["--disable-gpu"]',
        # Far enough out that no QA run reaches it: this profile's job is to be
        # the one the sweep must NOT close.
        "idle_timeout_secs = 99999",
        "tab_idle_timeout_secs = 99999",
    ]
    if args.control_user_data_dir:
        control_lines.append(f'user_data_dir = "{args.control_user_data_dir}"')
    if args.control_max_tabs is not None:
        control_lines.append(f"max_tabs_per_profile = {args.control_max_tabs}")
    src += "\n" + "\n".join(control_lines) + "\n"

if args.existing_session_profile:
    src += "\n".join(
        [
            "",
            f"[general.browser.profiles.{args.existing_session_profile}]",
            'driver = "existing_session"',
            "idle_timeout_secs = 99999",
            "",
        ]
    )

if args.chrome_mcp_command:
    # Edited in place, not appended: the generator already writes
    # `[general.browser.chrome_mcp]`, and a second table header is a TOML
    # duplicate-key error that stops the daemon booting (after it has printed
    # its banner, so the failure reads like a port problem).
    arg_list = ", ".join(f'"{a}"' for a in args.chrome_mcp_arg)
    src = set_key(src, "general.browser.chrome_mcp", "command", f'"{args.chrome_mcp_command}"')
    src = set_key(src, "general.browser.chrome_mcp", "args", f"[{arg_list}]")

open(args.path, "w").write(src)
print(f"patched [general.browser] in {args.path}: cli={args.cli_binary} headless={args.headless}")
