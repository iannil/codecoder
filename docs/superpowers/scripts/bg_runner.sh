#!/usr/bin/env bash
# bg_runner.sh <label> <task>  — CODECODER_BG_TASK headless one-shot,tee 日志,传播退出码。
#   运行期间进度写到 $LOG.body(可 tail 实时观察);结束后并入 $LOG 并删 .body。
set -uo pipefail
LABEL="${1:?label required}"; TASK="${2:?task required}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-lab}"
BG_BIN="${BG_BIN:-/Users/rong.zhu/Code/codecoder/target/debug/codecoder}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$LAB/logs/${TS}-bg-${LABEL}.log"
mkdir -p "$LAB/logs"
{ echo "=== bg $TS | $LABEL ==="; echo "TASK: $TASK"; } > "$LOG"
CODECODER_ROOT="$LAB" CODECODER_BG_TASK="$TASK" "$BG_BIN" > "$LOG.body" 2>&1
RC=$?
cat "$LOG.body" | tee -a "$LOG"
echo "EXIT=$RC" | tee -a "$LOG"
rm -f "$LOG.body"
exit $RC
