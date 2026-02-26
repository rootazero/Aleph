---
name: shell
description: Scene-driven shell commands — system diagnostics, Docker operations, process management
scope: standalone
---

# Shell & Containers

## When to Use

Invoke this skill for system administration tasks: checking ports, managing processes, Docker containers, disk cleanup, and network debugging.

## Port & Process Diagnostics

### "What's using this port?"
```bash
lsof -i :3000                      # macOS/Linux
ss -tlnp | grep :3000              # Linux (faster)
```

### "What's this process doing?"
```bash
ps aux | grep <name>               # find process
kill <pid>                         # graceful stop
kill -9 <pid>                      # force kill (last resort)
top -l 1 -n 10                    # top 10 by CPU (macOS)
htop                               # interactive (if installed)
```

### "Something is hanging"
```bash
# Find zombie processes
ps aux | awk '$8 ~ /Z/'

# Find processes holding a file
lsof <filepath>

# Kill all processes matching a pattern
pkill -f <pattern>
```

## Disk & Filesystem

### "Disk is full"
```bash
df -h                              # check disk usage
du -sh * | sort -rh | head -20     # largest items in current dir
du -sh ~/.cache                    # check common cache dirs

# Safe cleanup (check before deleting)
docker system prune -f             # Docker: remove unused data
npm cache clean --force            # npm cache
cargo clean                        # Rust build artifacts
rm -rf node_modules && npm install # reset node_modules
```

### "Find large files"
```bash
find / -type f -size +100M 2>/dev/null | head -20
```

## Docker

### Container Lifecycle
```bash
docker ps                          # running containers
docker ps -a                       # all containers (including stopped)
docker run -d --name <n> <image>   # start detached
docker stop <container>            # graceful stop
docker rm <container>              # remove stopped container
docker logs -f <container>         # follow logs
docker exec -it <container> sh     # interactive shell
```

### Images
```bash
docker images                      # list images
docker pull <image>:<tag>          # pull from registry
docker rmi <image>                 # remove image
docker image prune -a              # remove unused images
```

### Docker Compose
```bash
docker compose up -d               # start all services
docker compose down                # stop and remove
docker compose logs -f <service>   # follow service logs
docker compose restart <service>   # restart one service
docker compose ps                  # service status
docker compose exec <svc> sh       # shell into service
```

### System Cleanup
```bash
docker system df                   # disk usage breakdown
docker system prune -af            # remove ALL unused (careful!)
docker volume prune                # remove unused volumes
```

## Network Debugging

### "Can I reach this host?"
```bash
ping -c 3 <host>                   # basic connectivity
curl -v <url>                      # HTTP request with headers
curl -o /dev/null -w "%{http_code}" <url>  # just status code
```

### "DNS issues"
```bash
dig <domain>                       # DNS lookup
nslookup <domain>                  # alternative DNS lookup
host <domain>                      # simple DNS check
```

### "What's listening?"
```bash
lsof -i -P -n | grep LISTEN       # all listening ports (macOS)
ss -tlnp                           # all listening ports (Linux)
```

## Environment & Config

### "Check environment"
```bash
env | grep <PATTERN>               # search env vars
echo $PATH | tr ':' '\n'           # readable PATH
which <command>                    # find command location
<command> --version                # check installed version
```

### "Manage background jobs"
```bash
<command> &                        # run in background
jobs                               # list background jobs
fg %1                              # bring job 1 to foreground
nohup <command> &                  # survive terminal close
```

## Safety Rules

- **Always preview before bulk delete**: `ls` before `rm`, `docker ps` before `docker rm`
- **Use `-i` flag for interactive rm** when deleting in unfamiliar directories
- **Never `rm -rf /`** or `rm -rf ~` — triple-check paths with variables
- **Docker prune**: Understand what `-a` removes (ALL unused, not just dangling)
- Aleph exec safety: Dangerous commands require explicit user confirmation
