# 资源自检脚本 (Resource Governance)
check_and_run_cargo() {
  local cmd=$1
  while true; do
    # 获取当前运行的 cargo 实例数 (排除自身)
    local count=$(pgrep cargo | wc -l)
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

# 如果直接执行此脚本，检查参数
if [ $# -ge 1 ]; then
  check_and_run_cargo "$1"
fi
