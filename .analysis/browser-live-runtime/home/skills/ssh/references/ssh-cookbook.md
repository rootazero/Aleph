# SSH Cookbook

## SSH Config Best Practices

Always use `~/.ssh/config` instead of long command-line flags:

```
# ~/.ssh/config

# Default settings for all hosts
Host *
    AddKeysToAgent yes
    IdentitiesOnly yes
    ServerAliveInterval 60
    ServerAliveCountMax 3

# Production server
Host prod
    HostName 10.0.1.100
    User deploy
    IdentityFile ~/.ssh/id_ed25519_prod
    Port 22

# Jump through bastion
Host internal-db
    HostName 10.0.2.50
    User dbadmin
    ProxyJump bastion

Host bastion
    HostName bastion.example.com
    User jump
    IdentityFile ~/.ssh/id_ed25519_bastion

# Connection multiplexing (reuse connections)
Host *.example.com
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist 600
```

```bash
mkdir -p ~/.ssh/sockets && chmod 700 ~/.ssh/sockets
```

## Port Forwarding

```bash
# Local forward: access remote service on local port
# "I want to access remote DB (port 5432) at localhost:15432"
ssh -L 15432:localhost:5432 prod
# Now: psql -h localhost -p 15432

# Remote forward: expose local service to remote
# "Let the remote server access my local dev server (port 3000)"
ssh -R 8080:localhost:3000 prod

# Dynamic forward (SOCKS proxy)
ssh -D 1080 prod
# Configure browser: SOCKS5 proxy -> localhost:1080

# Background tunnel (no interactive shell)
ssh -fNL 15432:localhost:5432 prod
# Kill later: kill $(lsof -ti:15432)
```

## File Transfer

```bash
# SCP: simple file copy
scp file.txt user@host:/remote/path/
scp user@host:/remote/file.txt ./local/
scp -r ./local-dir/ user@host:/remote/dir/

# Rsync: incremental sync (preferred for large transfers)
rsync -avz --progress ./local-dir/ user@host:/remote/dir/
rsync -avz --progress user@host:/remote/dir/ ./local-dir/

# Rsync with exclusions
rsync -avz --exclude='node_modules' --exclude='.git' ./project/ user@host:/deploy/

# Rsync dry run (preview changes)
rsync -avzn ./local/ user@host:/remote/
```

| Tool | Best For |
|------|----------|
| `scp` | Quick single file copy |
| `rsync` | Large directories, incremental sync, bandwidth efficiency |
| `sftp` | Interactive file browsing |

## Jump Hosts (Bastion)

```bash
# ProxyJump (modern, preferred)
ssh -J bastion@jump.example.com admin@internal-server

# Multi-hop
ssh -J bastion1,bastion2 admin@deep-internal

# Via config (see SSH Config section above)
ssh internal-db
```

## Key Management

```bash
# Generate key (ed25519 recommended)
ssh-keygen -t ed25519 -C "your@email.com" -f ~/.ssh/id_ed25519_purpose

# Copy public key to server
ssh-copy-id -i ~/.ssh/id_ed25519_purpose.pub user@host

# Add key to agent
ssh-add ~/.ssh/id_ed25519_purpose

# List loaded keys
ssh-add -l
```

## Remote Command Execution

```bash
# Run single command
ssh prod "uname -a"

# Run script remotely
ssh prod "bash -s" < local-script.sh

# Pipe data through SSH
cat local.sql | ssh prod "psql -U postgres mydb"
tar czf - ./project | ssh prod "tar xzf - -C /deploy/"
```

## Troubleshooting

```bash
# Verbose connection debugging
ssh -v user@host    # level 1
ssh -vvv user@host  # level 3

# Test connection
ssh -o ConnectTimeout=5 user@host echo "Connected"

# Fix permissions (SSH is strict about this)
chmod 700 ~/.ssh
chmod 600 ~/.ssh/id_ed25519
chmod 644 ~/.ssh/id_ed25519.pub
chmod 600 ~/.ssh/config
chmod 600 ~/.ssh/authorized_keys

# Clear known_hosts entry (after server rebuild)
ssh-keygen -R hostname
```

| Problem | Solution |
|---------|----------|
| Permission denied (publickey) | Check key permissions (600), verify key is in authorized_keys |
| Connection refused | Verify sshd running, check firewall, verify port |
| Connection timed out | Check network, verify host is reachable, check security groups |
| Host key verification failed | Server was rebuilt -- `ssh-keygen -R host` to clear old key |
| Broken pipe | Add `ServerAliveInterval 60` to config |
