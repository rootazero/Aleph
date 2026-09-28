#!/bin/zsh
cd /Volumes/TBU4/Workspace/Aleph
PAT='"(plugin|plugins|extensions|hooks|skills|mcp_config|mcp|services)\.[a-zA-Z_.]+"'
rg --no-filename --no-line-number -o -e "(register|reg!)\\($PAT" src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers/ src/bin/aleph-server/commands/start/builder/ | rg --no-filename -o -e '"[^"]+"' | sort -u > /tmp/srv.txt
rg --no-filename --no-line-number -o -e "$PAT" interfaces/webchat/src interfaces/tui/src interfaces/cli/src qa shared | sort -u > /tmp/cli.txt
echo "server-registered: $(wc -l < /tmp/srv.txt | tr -d ' ')"; tr '\n' ' ' < /tmp/srv.txt; echo
echo "ZERO-CLIENT:"; comm -23 /tmp/srv.txt /tmp/cli.txt | tr '\n' ' '; echo
echo "CLIENT-ONLY (unregistered):"; comm -13 /tmp/srv.txt /tmp/cli.txt | grep -v 'extensions.cat\.\|plugin\.\(zip\|toml\|js\)' | tr '\n' ' '; echo
