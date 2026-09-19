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

* **`user_data_dir`** is where the browser writes `DevToolsActivePort`, which
  makes it the oracle for "the profile Aleph generated actually reached the
  browser" as opposed to merely "a browser came up". Read out of the directory
  rather than out of `playwright-cli list`: under `attach --cdp` the CLI does
  not own the profile dir and has nothing to echo, and under `--driver cdp`
  there is no CLI session at all.

* **`--driver`** picks which of Aleph's two Aleph-launched drivers the default
  profile uses, and is REQUIRED. See its help text: a default here would make
  the fixture measure whatever the product currently does.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--cli-binary", required=True)
p.add_argument("--user-data-dir", required=True)
p.add_argument("--headless", default="true", choices=["true", "false"])
p.add_argument(
    "--driver",
    required=True,
    choices=["managed", "cdp"],
    help="the default profile's driver. `managed` is playwright-cli; `cdp` is "
    "Aleph's own CDP client. The same scenarios run under both, which is the "
    "only way to tell a claim about the BROWSER from a claim about the CLI. "
    "REQUIRED rather than defaulted: a later task flips the product's default "
    "profile to engine=obscura/driver=cdp, and a fixture that inherited the "
    "default would quietly stop testing the driver its own name promises.",
)
p.add_argument(
    "--engine",
    default="",
    choices=["", "chromium", "obscura"],
    help="the default profile's engine; only meaningful with --driver cdp",
)
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
p.add_argument(
    "--runtime-binary-path",
    default="",
    help="[browser.runtime] binary_path — pins the browser Aleph launches. The "
    "attach scenario pins it so the run never depends on which browsers this "
    "machine happens to have, and the RED control renames it.",
)
p.add_argument(
    "--prefer-system-browser",
    default="",
    choices=["", "true", "false"],
    help="[browser.runtime] prefer_system_browser",
)
p.add_argument(
    "--obscura-binary-path",
    default="",
    help="[general.browser.obscura] binary_path — pins the obscura the engine "
    "launches. Every qa/browser_dual stage but `provision` sets it, so those "
    "stages never depend on a network install; `provision` is the one stage "
    "that must NOT set it, or it would test nothing.",
)
p.add_argument(
    "--obscura-download-host",
    default="",
    help="[general.browser.obscura] download_host — where the ASSET BYTES come "
    "from. NOT [general.browser.runtime] download_host, which is the Playwright "
    "CDN mirror and serves no GitHub release tree (github_release.rs:530-534). "
    "The release METADATA never follows this key (ReleaseSource::for_runtime "
    "pins it at api.github.com), and a non-https value here is ignored with a "
    "warning — both facts are what qa/browser_dual/run.sh provision asserts.",
)
p.add_argument(
    "--cdp-command-timeout-secs",
    type=int,
    default=0,
    help="[general.browser] cdp_command_timeout_secs — Aleph's per-CDP-command "
    "budget. Must be 1..=59 (profile.rs:626-644); the `stall` stage sets it "
    "below the window a spinning page blocks the connection for.",
)
p.add_argument(
    "--default-extra-args",
    default=None,
    metavar="'ARG ARG'",
    help="the default profile's extra_args, space-separated; omit to keep the "
    "historical `--disable-gpu`, pass '' for none. It has to be settable "
    "because extra_args reaches the ENGINE's own argv and the engines do not "
    "share a flag vocabulary: obscura refuses an unknown argument outright "
    "(`unexpected argument '--disable-gpu' found`, exit 2, measured), so the "
    "Chromium-era default is fatal on an obscura profile.",
)
p.add_argument(
    "--profile-engine",
    action="append",
    default=[],
    metavar="NAME=ENGINE",
    help="append a SECOND profile on the named engine, repeatable. `driver` is "
    "written as `cdp` explicitly rather than inherited: the global default is "
    "`cdp` today and a fixture that relied on that would break the day it moves "
    "again (判据 §5).",
)
p.add_argument(
    "--profile-user-data-dir",
    action="append",
    default=[],
    metavar="NAME=DIR",
    help="that profile's user_data_dir, repeatable. Separate from "
    "--profile-engine because a QA run that must `pgrep` for the process needs "
    "to KNOW the directory, and the derived default "
    "(`browser_state_dir(engine.data_subdir())/<profile>`, manager.rs:531-535) "
    "is not a path the fixture chose.",
)
args = p.parse_args()


def pairs(specs, flag):
    """`NAME=VALUE` occurrences as a dict, refusing anything else BY NAME.

    Fail-closed rather than skipped: a typo'd `--profile-engine escape:chromium`
    that silently wrote no section would leave the fixture asserting about a
    profile that does not exist, and `browser_open` would answer
    `ProfileNotFound` — which reads like a product defect (判据 §8).
    """
    out = {}
    for spec in specs:
        name, sep, value = spec.partition("=")
        if not sep or not name or not value:
            raise SystemExit(f"{flag} needs NAME=VALUE, got {spec!r}")
        out[name] = value
    return out


profile_engines = pairs(args.profile_engine, "--profile-engine")
profile_udds = pairs(args.profile_user_data_dir, "--profile-user-data-dir")
# A udd for a profile nobody declares is written nowhere, so it would be a
# setting that reports success and does nothing (判据 §11).
for name in profile_udds:
    if name not in profile_engines:
        raise SystemExit(
            f"--profile-user-data-dir names {name!r}, which no --profile-engine declares"
        )

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

if args.runtime_binary_path:
    src = set_key(src, "general.browser.runtime", "binary_path", f'"{args.runtime_binary_path}"')
if args.prefer_system_browser:
    src = set_key(src, "general.browser.runtime", "prefer_system_browser", args.prefer_system_browser)
if args.obscura_binary_path:
    src = set_key(src, "general.browser.obscura", "binary_path", f'"{args.obscura_binary_path}"')
if args.obscura_download_host:
    src = set_key(src, "general.browser.obscura", "download_host", f'"{args.obscura_download_host}"')
if args.cdp_command_timeout_secs:
    # Bare integer, no quotes — it is a `u64`, and a quoted value is a TOML
    # string that serde rejects at load with a type error, which reads like a
    # corrupt config rather than a fixture bug.
    src = set_key(
        src, "general.browser", "cdp_command_timeout_secs", str(args.cdp_command_timeout_secs)
    )

# A sub-table of the already-declared (empty) `[general.browser.profiles]`.
# Declaring a child of a defined table is valid TOML; re-declaring the parent
# would not be.
extra = ["--disable-gpu"] if args.default_extra_args is None else args.default_extra_args.split()
default_lines = [
    "[general.browser.profiles.default]",
    f'driver = "{args.driver}"',
    f'user_data_dir = "{args.user_data_dir}"',
    "extra_args = [" + ", ".join(f'"{x}"' for x in extra) + "]",
]
if args.engine:
    default_lines.append(f'engine = "{args.engine}"')
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
        # Deliberately NOT `args.driver`: this profile's job is to be the one
        # the sweep must not close, and keeping it on the driver whose reaper
        # behaviour is already known is what makes it a control.
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

for name, engine in profile_engines.items():
    extra_lines = [
        f"[general.browser.profiles.{name}]",
        f'engine = "{engine}"',
        # Stated, not inherited — see --profile-engine's help.
        'driver = "cdp"',
        # Per ENGINE, not per fixture: `extra_args` is prepended to that
        # engine's own argv, and obscura exits 2 on an unrecognised one.
        'extra_args = ["--disable-gpu"]' if engine == "chromium" else "extra_args = []",
        # Far enough out that no QA run reaches it: a profile added by this flag
        # exists to still be there later in the stage, never to be reaped
        # mid-run.
        "idle_timeout_secs = 99999",
        "tab_idle_timeout_secs = 99999",
    ]
    if name in profile_udds:
        extra_lines.append(f'user_data_dir = "{profile_udds[name]}"')
    src += "\n" + "\n".join(extra_lines) + "\n"

open(args.path, "w").write(src)
print(f"patched [general.browser] in {args.path}: cli={args.cli_binary} headless={args.headless}")
