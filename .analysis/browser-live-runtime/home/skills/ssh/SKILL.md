---
name: ssh
description: "SSH management — connections, port forwarding, file transfer, jump hosts, and key management. Use when connecting to remote servers, setting up SSH tunnels, transferring files via scp/rsync, configuring jump hosts (bastion), managing SSH keys, or troubleshooting SSH connection issues."
---

# SSH Management

## Quick Reference

```bash
# Connect
ssh user@host
ssh -i ~/.ssh/id_ed25519 user@host
ssh prod                               # using config alias

# Local port forward (access remote DB locally)
ssh -L 15432:localhost:5432 prod

# File transfer
scp file.txt user@host:/remote/path/
rsync -avz --progress ./dir/ user@host:/remote/dir/

# Generate key
ssh-keygen -t ed25519 -C "your@email.com" -f ~/.ssh/id_ed25519_purpose

# Jump host
ssh -J bastion@jump.example.com admin@internal-server
```

## Detailed Cookbook

For SSH config best practices, port forwarding patterns, file transfer details, key management, remote execution, and troubleshooting, see [references/ssh-cookbook.md](references/ssh-cookbook.md).

## Aleph Integration

- Synergy with `deploy` (W4): SSH for deployment to remote servers
- Synergy with `shell` (F6): local shell -> remote shell via SSH
- Synergy with `security` (W5): key management, audit SSH config
