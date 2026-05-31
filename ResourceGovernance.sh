# 资源自检脚本 (Resource Governance)
# 只统计真正在编译/测试的 cargo 进程（排除 rust-analyzer 等长期驻留进程）
_count_active_cargo() {
  # 统计属于当前用户的、命令行包含 build/check/test/run 的 cargo 进程
  pgrep -a cargo 2>/dev/null | grep -E 'cargo\s+(build|check|test|run)' | wc -l | tr -d ' '
}

check_and_run_cargo() {
  while true; do
    local count=$(_count_active_cargo)
    if [ "$count" -lt 3 ]; then
      echo "当前活跃 cargo 实例数为 $count，负载正常，执行: cargo $*"
      cargo "$@"
      break
    else
      echo "检测到 $count 个活跃 cargo 实例，资源争抢，等待 10s..."
      sleep 10
    fi
  done
}

# 如果直接执行此脚本，将所有参数传递给 cargo
if [ $# -ge 1 ]; then
  check_and_run_cargo "$@"
fi
