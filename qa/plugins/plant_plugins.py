#!/usr/bin/env python3
"""Plant plugin trees into a scratch `$ALEPH_HOME/plugins/installed/`.

The shapes live here rather than inline in the driver because the shapes ARE
the claim. Round 1 widened Claude Code's five component fields from
`Option<String>` to a path | array | inline-object union, and widened a
marketplace `source` from a bare string to a six-arm union. Both were fixed
with unit tests that feed a fixture straight to the parser. Neither had ever
been through `ExtensionManager::load_all` on a real daemon.

`serde` does not degrade field-by-field: one field whose type is too narrow
fails the *whole* document. So the discriminating question a unit test cannot
answer is what the registry does with a plugin whose manifest it could not
parse -- and the answer is a row with `PluginStatus::Error` and zero
capabilities, which a `plugins.list` caller cannot tell from a plugin that
simply ships nothing.
"""
import json
import shutil
import sys
from pathlib import Path


def w(path: Path, content: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def plant_inline(root: Path):
    """CC manifest using the INLINE object form for mcpServers + hooks.

    This is the shape two of Anthropic's own plugin manifests use. Before the
    round-1 fix this manifest did not parse at all, so the plugin loaded with
    zero capabilities.
    """
    d = root / "qa-inline"
    if d.exists():
        shutil.rmtree(d)
    w(d / ".claude-plugin" / "plugin.json", json.dumps({
        "name": "qa-inline",
        "version": "1.0.0",
        "description": "inline mcpServers + hooks object form",
        # Inline object, NOT a path string.
        "mcpServers": {
            "qa-echo": {
                # `command` must exist on PATH or the server logs a spawn
                # failure; the claim is about parsing + registration, so use
                # something universally present.
                "command": "echo",
                "args": ["qa-inline-server"],
            }
        },
        # The hook command carries a `${CLAUDE_PLUGIN_ROOT}` reference because
        # the hook registry is the one surface that serves an expanded body
        # back over the wire. `commands.list` returns a name/description tree
        # and never the body, so asserting "no unexpanded variable survives"
        # there passes for a plugin whose expansion is completely broken.
        #
        # `m.sh` is short for a reason: the hook inventory elides action labels
        # at 80 characters, and the expanded absolute path plus a longer script
        # name pushes the plugin id past the cut -- leaving a payload that
        # proves only "some path", not "this plugin's path".
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "sh ${CLAUDE_PLUGIN_ROOT}/m.sh",
                        }
                    ],
                }
            ]
        },
        # Array-of-paths form for a directory-shaped field.
        "commands": ["./commands"],
    }, indent=2))
    w(d / "commands" / "qa-inline-cmd.md", """---
description: QA inline command
---
This command exists to prove the array-of-paths arm reaches a real parser.
""")
    w(d / "m.sh", "echo qa-inline-hook\n")
    return "qa-inline"


def plant_path_form(root: Path):
    """The shape that already worked: every component field a plain path string.

    A widening that traded one accepted shape for another would look identical
    to a fix, if the inline plugin were the only one planted. This is the
    control.
    """
    d = root / "qa-pathform"
    if d.exists():
        shutil.rmtree(d)
    w(d / ".claude-plugin" / "plugin.json", json.dumps({
        "name": "qa-pathform",
        "version": "2.0.0",
        "description": "classic path-string component fields",
        "commands": "./commands",
        "agents": "./agents",
    }, indent=2))
    w(d / "commands" / "qa-path-cmd.md", """---
description: QA path-form command
---
Path-string arm.
""")
    w(d / "agents" / "qa-path-agent.md", """---
name: qa-path-agent
description: QA path-form agent
---
Path-string agent body.
""")
    return "qa-pathform"


def plant_var_plugin(root: Path):
    """A command body referencing `${CLAUDE_PLUGIN_ROOT}`.

    Round 1 gave the four documented plugin variables one expander. Whether an
    expanded path is *absolute and points into this plugin's own directory* is
    a property of the installed tree, so it is only observable on a real
    daemon with a real install root.
    """
    d = root / "qa-vars"
    if d.exists():
        shutil.rmtree(d)
    w(d / ".claude-plugin" / "plugin.json", json.dumps({
        "name": "qa-vars",
        "version": "1.0.0",
        "description": "plugin root variable expansion",
        "commands": "./commands",
    }, indent=2))
    w(d / "commands" / "qa-var-cmd.md", """---
description: QA variable expansion command
---
Run ${CLAUDE_PLUGIN_ROOT}/scripts/run.py and read ${CLAUDE_PLUGIN_DATA}/state.json
""")
    w(d / "scripts" / "run.py", "print('qa')\n")
    return "qa-vars"


def plant_marketplace(marketplaces: Path):
    """A marketplace whose entries use FOUR different `source` shapes.

    `source` was a bare `String`. Upstream allows a six-arm union, so a single
    `{"source": {"source": "github", ...}}` entry made the *entire* marketplace
    fail to deserialize -- every plugin in it invisible, not just that one.
    The bare-string entry is the control: it must still resolve.
    """
    d = marketplaces / "qa-market"
    if d.exists():
        shutil.rmtree(d)
    # A real, installable plugin tree for the two entries whose source resolves
    # to a local path. `install_to_scope` runs `validate_plugin` before copying,
    # so a stub directory would fail for a reason unrelated to the claim.
    w(d / "local-copy" / ".claude-plugin" / "plugin.json", json.dumps({
        "name": "qa-mk-string",
        "version": "1.0.0",
        "description": "installed out of a marketplace whose siblings use object sources",
    }, indent=2))
    w(d / ".claude-plugin" / "marketplace.json", json.dumps({
        "name": "qa-market",
        "owner": {"name": "qa"},
        "plugins": [
            {
                "name": "qa-mk-string",
                "description": "bare string source (the control)",
                "source": "./local-copy",
            },
            {
                "name": "qa-mk-github",
                "description": "object source, github kind",
                "source": {"source": "github", "repo": "rootazero/does-not-exist"},
            },
            {
                "name": "qa-mk-npm",
                "description": "object source, npm kind",
                "source": {"source": "npm", "package": "@qa/nothing"},
            },
            {
                # An arm Claude Code has not defined. `Unknown` exists so a
                # newer marketplace never bricks an older Aleph; the only way
                # to see that is to feed it something from the future.
                "name": "qa-mk-future",
                "description": "object source, an arm this build has never heard of",
                "source": {"source": "quantum", "entangle": "yes"},
            },
        ],
    }, indent=2))
    return d


def main():
    installed = Path(sys.argv[1])
    marketplaces = Path(sys.argv[2])
    installed.mkdir(parents=True, exist_ok=True)
    marketplaces.mkdir(parents=True, exist_ok=True)
    planted = [
        plant_inline(installed),
        plant_path_form(installed),
        plant_var_plugin(installed),
    ]
    market = plant_marketplace(marketplaces)
    print(json.dumps({"planted": planted, "marketplace": str(market)}))


if __name__ == "__main__":
    main()
