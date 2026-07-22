#!/usr/bin/env bash
# probe_concurrent.sh <label> <N> <task>
#   并发起 N 个 bg_runner 跑同一 task(竞态探测),末行打印各进程退出码数组。
set -uo pipefail
LABEL="${1:?label required}"; N="${2:?N required}"; TASK="${3:?task required}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-probe}"
SELF="$(cd "$(dirname "$0")" && pwd)/bg_runner.sh"
pids=(); rcs=()
for i in $(seq 1 "$N"); do
  CODECODER_ROOT="$LAB" bash "$SELF" "${LABEL}-p${i}" "$TASK" &
  pids+=($!)
done
for i in "${!pids[@]}"; do
  if wait "${pids[$i]}"; then rcs[$i]=0; else rcs[$i]=$?; fi
done
echo "=== concurrent $LABEL: exit codes [${rcs[*]}] ==="
