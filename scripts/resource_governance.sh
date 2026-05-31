#!/usr/bin/env bash
# 资源自检脚本 (Resource Governance)
# Wait for cargo load to drop below a threshold before running, to avoid
# contention on a shared CARGO_TARGET_DIR across concurrent sessions/worktrees.
check_and_run_cargo() {
  local cmd=$1
  while true; do
    # 获取当前运行的 cargo 实例数 (macOS BSD pgrep has no -c, use wc -l)
    local count=$(pgrep -x cargo | wc -l | tr -d ' ')
    if [ "$count" -lt 3 ]; then
      echo "当前 cargo 实例数为 $count，负载正常，执行: cargo $cmd"
      cargo "$cmd"
      break
    else
      echo "检测到 $count 个 cargo 实例，资源争抢，等待 10s..."
      sleep 10
    fi
  done
}
